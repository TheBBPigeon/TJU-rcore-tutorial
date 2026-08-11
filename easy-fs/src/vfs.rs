use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ, BLOCK_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};

pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    /// We should not acquire efs lock here.
    pub fn new(
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
    ) -> Self {
        Self {
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
        }
    }

    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }

    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }

    fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        // Only directories have directory entries
        if !disk_inode.is_dir() {
            return None;
        }
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                return Some(dirent.inode_number() as u32);
            }
        }
        None
    }

    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).map(|inode_id| {
                let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
                Arc::new(Self::new(
                    block_id,
                    block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                ))
            })
        })
    }

    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }

    /// Create a new inode of the specified type (File or Directory) in this directory.
    pub fn create_as(&self, name: &str, inode_type: DiskInodeType) -> Option<Arc<Inode>> {
        // Reject reserved names
        if name == "." || name == ".." {
            return None;
        }
        // Compute parent inode number BEFORE acquiring the fs lock,
        // since inode_number() internally acquires self.fs.lock().
        let parent_inode_number = self.inode_number();
        let mut fs = self.fs.lock();
        let op = |root_inode: &mut DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.modify_disk_inode(op).is_some() {
            return None;
        }
        // create a new file/directory
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(inode_type);
                new_inode.parent_inode = parent_inode_number;
            });
        self.modify_disk_inode(|root_inode| {
            // append file in the dirent
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            // increase size
            self.increase_size(new_size as u32, root_inode, &mut fs);
            // write dirent
            let dirent = DirEntry::new(name, new_inode_id);
            root_inode.write_at(
                file_count * DIRENT_SZ,
                dirent.as_bytes(),
                &self.block_device,
            );
        });

        let (block_id, block_offset) = fs.get_disk_inode_pos(new_inode_id);
        block_cache_sync_all();
        // return inode
        Some(Arc::new(Self::new(
            block_id,
            block_offset,
            self.fs.clone(),
            self.block_device.clone(),
        )))
        // release efs lock automatically by compiler
    }

    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        self.create_as(name, DiskInodeType::File)
    }

    /// Get the inode number by computing its position from the filesystem layout.
    /// Inode 0 is the root, inode N is at position N in the inode area.
    pub fn inode_number(&self) -> u32 {
        let fs = self.fs.lock();
        let inode_size = core::mem::size_of::<DiskInode>();
        let inodes_per_block = (BLOCK_SZ / inode_size) as u32;
        let inode_area_start_block = fs.inode_area_start_block;
        let block_offset_in_area = self.block_id as u32 - inode_area_start_block;
        let inode_index_in_block = self.block_offset as u32 / inode_size as u32;
        block_offset_in_area * inodes_per_block + inode_index_in_block
    }

    /// Look up a path from this inode (which must be a directory).
    /// The path is split by '/' and each component is resolved recursively.
    /// Returns None if any component is not found or is not a directory
    /// when it is not the last component.
    pub fn lookup_path(&self, path: &str) -> Option<Arc<Inode>> {
        // Handle empty path or root
        if path.is_empty() || path == "/" {
            return Some(Arc::new(Self::new(
                self.block_id as u32,
                self.block_offset,
                self.fs.clone(),
                self.block_device.clone(),
            )));
        }
        // Handle "."
        if path == "." {
            return Some(Arc::new(Self::new(
                self.block_id as u32,
                self.block_offset,
                self.fs.clone(),
                self.block_device.clone(),
            )));
        }
        // Strip leading '/'
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return Some(Arc::new(Self::new(
                self.block_id as u32,
                self.block_offset,
                self.fs.clone(),
                self.block_device.clone(),
            )));
        }

        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        // Resolve the first component (may be "..").
        // We work on a clone of self since the first ".." goes to our parent.
        let mut current = self.resolve_component(components[0])?;

        for component in components.iter().skip(1) {
            // Before descending, verify current is a directory
            let is_dir = current.read_disk_inode(|disk_inode| disk_inode.is_dir());
            if !is_dir {
                return None;
            }
            current = current.resolve_component(component)?;
        }

        // For the last component, if the path had a trailing '/', verify it's a directory
        if path.ends_with('/') {
            let is_dir = current.read_disk_inode(|disk_inode| disk_inode.is_dir());
            if !is_dir {
                return None;
            }
        }

        Some(current)
    }

    /// Resolve a single path component relative to this inode.
    /// Handles ".", "..", and named entries.
    fn resolve_component(&self, component: &str) -> Option<Arc<Inode>> {
        if component == "." {
            return Some(Arc::new(Self::new(
                self.block_id as u32,
                self.block_offset,
                self.fs.clone(),
                self.block_device.clone(),
            )));
        }
        if component == ".." {
            let parent_id = self.read_disk_inode(|disk_inode| disk_inode.parent_inode);
            // parent_id == 0 means either we are root (self-parent), or our
            // parent is root (inode 0). Check which case by seeing if we
            // ourselves are root.
            if parent_id == 0 && self.is_root() {
                return Some(Arc::new(Self::new(
                    self.block_id as u32,
                    self.block_offset,
                    self.fs.clone(),
                    self.block_device.clone(),
                )));
            }
            let fs = self.fs.lock();
            let (block_id, block_offset) = fs.get_disk_inode_pos(parent_id);
            return Some(Arc::new(Self::new(
                block_id,
                block_offset,
                self.fs.clone(),
                self.block_device.clone(),
            )));
        }
        self.find(component)
    }

    /// Set the parent inode number (used when moving directories).
    pub fn set_parent_inode(&self, parent_id: u32) {
        self.modify_disk_inode(|disk_inode| {
            disk_inode.parent_inode = parent_id;
        });
    }

    /// Check if this is the root inode (inode 0).
    fn is_root(&self) -> bool {
        self.inode_number() == 0
    }

    /// Check if this inode is a directory.
    pub fn is_dir(&self) -> bool {
        self.read_disk_inode(|disk_inode| disk_inode.is_dir())
    }

    /// Check if this directory is empty (has no entries).
    pub fn is_empty_directory(&self) -> bool {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            if !disk_inode.is_dir() {
                return false;
            }
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            file_count == 0
        })
    }

    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                v.push(String::from(dirent.name()));
            }
            v
        })
    }

    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }

    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }

    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }

    /// Remove a directory entry from this directory by name.
    /// This only modifies the directory content; it does NOT deallocate
    /// the target inode or its data blocks. That is done by `unlink()`.
    /// Returns true if the entry was found and removed.
    fn remove_directory_entry(&self, name: &str, disk_inode: &mut DiskInode) -> bool {
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut target_index = None;
        for i in 0..file_count {
            let mut dirent = DirEntry::empty();
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                target_index = Some(i);
                break;
            }
        }
        if target_index.is_none() {
            return false;
        }
        let idx = target_index.unwrap();
        // Shift all subsequent entries forward by DIRENT_SZ
        for i in idx..file_count - 1 {
            let mut next_dirent = DirEntry::empty();
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * (i + 1), next_dirent.as_bytes_mut(), &self.block_device),
                DIRENT_SZ,
            );
            disk_inode.write_at(DIRENT_SZ * i, next_dirent.as_bytes(), &self.block_device);
        }
        // Clear the last entry (now duplicated or stale)
        let empty_dirent = DirEntry::empty();
        disk_inode.write_at(DIRENT_SZ * (file_count - 1), empty_dirent.as_bytes(), &self.block_device);
        // Reduce directory size
        disk_inode.size = ((file_count - 1) * DIRENT_SZ) as u32;
        true
    }

    /// Link an existing inode into this directory with the given name.
    /// Returns false if the name already exists.
    pub fn link(&self, name: &str, target_inode_id: u32) -> bool {
        if name == "." || name == ".." {
            return false;
        }
        let mut fs = self.fs.lock();
        let exists = self.read_disk_inode(|disk_inode| {
            self.find_inode_id(name, disk_inode).is_some()
        });
        if exists {
            return false;
        }
        self.modify_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            self.increase_size(new_size as u32, disk_inode, &mut fs);
            let dirent = DirEntry::new(name, target_inode_id);
            disk_inode.write_at(file_count * DIRENT_SZ, dirent.as_bytes(), &self.block_device);
        });
        block_cache_sync_all();
        true
    }

    /// Rename a directory entry within the same directory.
    /// Returns true on success, false if old_name doesn't exist or new_name already exists.
    pub fn rename(&self, old_name: &str, new_name: &str) -> bool {
        if old_name == "." || old_name == ".." || new_name == "." || new_name == ".." {
            return false;
        }
        let mut fs = self.fs.lock();
        // Find old entry inode
        let old_inode_id = match self.read_disk_inode(|disk_inode| {
            self.find_inode_id(old_name, disk_inode)
        }) {
            Some(id) => id,
            None => return false,
        };
        // Check new_name doesn't exist
        let new_exists = self.read_disk_inode(|disk_inode| {
            self.find_inode_id(new_name, disk_inode).is_some()
        });
        if new_exists {
            return false;
        }
        // Add new dirent
        self.modify_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let new_size = (file_count + 1) * DIRENT_SZ;
            self.increase_size(new_size as u32, disk_inode, &mut fs);
            let dirent = DirEntry::new(new_name, old_inode_id);
            disk_inode.write_at(file_count * DIRENT_SZ, dirent.as_bytes(), &self.block_device);
        });
        // Remove old dirent
        self.modify_disk_inode(|disk_inode| {
            self.remove_directory_entry(old_name, disk_inode);
        });
        block_cache_sync_all();
        true
    }

    /// Remove a directory entry from this directory without deallocating
    /// the target inode. Used by rename/move operations. Returns true on success.
    pub fn detach(&self, name: &str) -> bool {
        if name == "." || name == ".." {
            return false;
        }
        let mut fs = self.fs.lock();
        let result = self.modify_disk_inode(|disk_inode| {
            self.remove_directory_entry(name, disk_inode)
        });
        block_cache_sync_all();
        result
    }

    /// Unlink (remove) a directory entry from this directory and deallocate
    /// the target inode and its data blocks. For files, this is equivalent to
    /// `rm`. For directories, the directory must be empty.
    /// Returns Some(inode_number) of the removed entry, or None if not found.
    pub fn unlink(&self, name: &str) -> Option<u32> {
        // Reject reserved names
        if name == "." || name == ".." {
            return None;
        }
        // Step 1: Find the target inode WITHOUT holding this directory's fs lock.
        // `find()` acquires and releases the lock internally.
        let target_inode = self.find(name)?;

        // Step 2: Check the target's type and emptiness.
        // The target_inode shares the same fs Mutex, so we must ensure the lock
        // from find() is released before checking (which it is — find() returns
        // after releasing).
        let (target_is_dir, target_is_empty) = {
            let _fs_guard = target_inode.fs.lock();
            target_inode.read_disk_inode(|disk_inode| {
                let is_dir = disk_inode.is_dir();
                let file_count = (disk_inode.size as usize) / DIRENT_SZ;
                (is_dir, file_count == 0)
            })
        };
        let target_inode_id = target_inode.inode_number();

        // If it's a directory, it must be empty
        if target_is_dir && !target_is_empty {
            return None;
        }

        // Step 3: Lock this directory's fs and perform the unlink operations.
        let mut fs = self.fs.lock();

        // Clear the target inode's data blocks and deallocate them
        target_inode.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });

        // Deallocate the target inode itself
        fs.dealloc_inode(target_inode_id);

        // Remove the directory entry from this directory
        self.modify_disk_inode(|disk_inode| {
            self.remove_directory_entry(name, disk_inode);
        });

        block_cache_sync_all();
        Some(target_inode_id)
    }
}
