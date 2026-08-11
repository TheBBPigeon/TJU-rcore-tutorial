use super::File;
use crate::drivers::BLOCK_DEVICE;
use crate::mm::UserBuffer;
use crate::sync::UPIntrFreeCell;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::*;
use easy_fs::{EasyFileSystem, Inode};
use lazy_static::*;

pub struct OSInode {
    readable: bool,
    writable: bool,
    inner: UPIntrFreeCell<OSInodeInner>,
}

pub struct OSInodeInner {
    offset: usize,
    inode: Arc<Inode>,
}

impl OSInode {
    pub fn new(readable: bool, writable: bool, inode: Arc<Inode>) -> Self {
        Self {
            readable,
            writable,
            inner: unsafe { UPIntrFreeCell::new(OSInodeInner { offset: 0, inode }) },
        }
    }
    pub fn read_all(&self) -> Vec<u8> {
        let mut inner = self.inner.exclusive_access();
        let mut buffer = [0u8; 512];
        let mut v: Vec<u8> = Vec::new();
        loop {
            let len = inner.inode.read_at(inner.offset, &mut buffer);
            if len == 0 {
                break;
            }
            inner.offset += len;
            v.extend_from_slice(&buffer[..len]);
        }
        v
    }
}

lazy_static! {
    pub static ref ROOT_INODE: Arc<Inode> = {
        let efs = EasyFileSystem::open(BLOCK_DEVICE.clone());
        Arc::new(EasyFileSystem::root_inode(&efs))
    };
}

/// Get a clone of the root inode.
pub fn get_root_inode() -> Arc<Inode> {
    ROOT_INODE.clone()
}

pub fn list_apps() {
    println!("/**** APPS ****");
    for app in ROOT_INODE.ls() {
        println!("{}", app);
    }
    println!("**************/")
}

bitflags! {
    pub struct OpenFlags: u32 {
        const RDONLY = 0;
        const WRONLY = 1 << 0;
        const RDWR = 1 << 1;
        const CREATE = 1 << 9;
        const TRUNC = 1 << 10;
    }
}

impl OpenFlags {
    /// Do not check validity for simplicity
    /// Return (readable, writable)
    pub fn read_write(&self) -> (bool, bool) {
        if self.is_empty() {
            (true, false)
        } else if self.contains(Self::WRONLY) {
            (false, true)
        } else {
            (true, true)
        }
    }
}

/// Split a path into (parent_directory_path, file_name).
/// "dir/file.txt" -> ("dir", "file.txt")
/// "file.txt" -> ("", "file.txt")
/// "/dir/file.txt" -> ("/dir", "file.txt")
fn split_path(path: &str) -> (&str, &str) {
    let stripped = path.trim_start_matches('/');
    let slash_count = path.len() - stripped.len();
    match stripped.rfind('/') {
        Some(idx) => (&path[..slash_count + idx], &path[slash_count + idx + 1..]),
        None if slash_count > 0 => ("/", stripped),
        None => ("", path),
    }
}

/// Look up a path starting from a given root inode.
/// NOTE: ".." (parent directory) traversal is NOT YET IMPLEMENTED.
/// Path components equal to ".." are treated as literal directory entry
/// names and will fail to resolve. See the corresponding NOTE in
/// easy-fs/src/vfs.rs for the rationale and planned TODO approach.
pub fn lookup_path_from(root: &Arc<Inode>, path: &str) -> Option<Arc<Inode>> {
    println!(
        "[lookup_path_from] path='{}', root_inode_id={}",
        path,
        root.inode_number()
    );
    let result = root.lookup_path(path);
    match &result {
        Some(inode) => println!(
            "[lookup_path_from] SUCCESS: path='{}', found_inode_id={}",
            path,
            inode.inode_number()
        ),
        None => println!("[lookup_path_from] FAILED: path='{}' not found", path),
    }
    result
}

/// Open a file at a given path relative to a root inode.
pub fn open_file_at(
    root: &Arc<Inode>,
    path: &str,
    flags: OpenFlags,
) -> Option<Arc<OSInode>> {
    println!(
        "[open_file_at] path='{}', root_inode_id={}, flags={:?}",
        path,
        root.inode_number(),
        flags
    );
    let (readable, writable) = flags.read_write();
    if flags.contains(OpenFlags::CREATE) {
        let (parent_path, file_name) = split_path(path);
        let parent = if parent_path.is_empty() || parent_path == "/" {
            root.clone()
        } else {
            lookup_path_from(root, parent_path)?
        };
        println!(
            "[open_file_at] CREATE: parent_path='{}', file_name='{}'",
            parent_path, file_name
        );
        if let Some(inode) = parent.find(file_name) {
            // If the existing entry is a directory, refuse to overwrite
            if inode.is_dir() {
                println!(
                    "[open_file_at] FAILED: '{}' is a directory, cannot open as file",
                    file_name
                );
                return None;
            }
            // clear size
            inode.clear();
            Some(Arc::new(OSInode::new(readable, writable, inode)))
        } else {
            // create file
            parent
                .create(file_name)
                .map(|inode| Arc::new(OSInode::new(readable, writable, inode)))
        }
    } else {
        lookup_path_from(root, path).map(|inode| {
            if flags.contains(OpenFlags::TRUNC) {
                inode.clear();
            }
            Arc::new(OSInode::new(readable, writable, inode))
        })
    }
}

/// Create a file at a given path relative to a root inode.
/// Returns the new OSInode, or None if the file already exists or path is invalid.
pub fn create_file_at(root: &Arc<Inode>, path: &str) -> Option<Arc<OSInode>> {
    println!("[create_file_at] path='{}'", path);
    let (parent_path, file_name) = split_path(path);
    let parent = if parent_path.is_empty() || parent_path == "/" {
        root.clone()
    } else {
        lookup_path_from(root, parent_path)?
    };
    parent
        .create(file_name)
        .map(|inode| Arc::new(OSInode::new(true, true, inode)))
}

/// Create a directory at a given path relative to a root inode.
/// Returns 0 on success, -1 on failure.
pub fn make_directory_at(root: &Arc<Inode>, path: &str) -> isize {
    println!("[make_directory_at] path='{}', root_inode_id={}", path, root.inode_number());
    let (parent_path, dir_name) = split_path(path);
    let parent = if parent_path.is_empty() || parent_path == "/" {
        root.clone()
    } else {
        match lookup_path_from(root, parent_path) {
            Some(p) => p,
            None => {
                println!("[make_directory_at] FAILED: parent path '{}' not found", parent_path);
                return -1;
            }
        }
    };
    match parent.create_as(dir_name, easy_fs::DiskInodeType::Directory) {
        Some(_) => {
            println!("[make_directory_at] SUCCESS: path='{}'", path);
            0
        }
        None => {
            println!("[make_directory_at] FAILED: could not create '{}'", dir_name);
            -1
        }
    }
}

/// Remove a file or empty directory at a given path relative to a root inode.
/// Returns 0 on success, -1 on failure.
pub fn unlink_at(root: &Arc<Inode>, path: &str) -> isize {
    println!("[unlink_at] path='{}', root_inode_id={}", path, root.inode_number());
    let (parent_path, name) = split_path(path);
    let parent = if parent_path.is_empty() || parent_path == "/" {
        root.clone()
    } else {
        match lookup_path_from(root, parent_path) {
            Some(p) => p,
            None => {
                println!("[unlink_at] FAILED: parent path '{}' not found", parent_path);
                return -1;
            }
        }
    };
    match parent.unlink(name) {
        Some(_) => {
            println!("[unlink_at] SUCCESS: path='{}'", path);
            0
        }
        None => {
            println!("[unlink_at] FAILED: could not unlink '{}'", name);
            -1
        }
    }
}

/// List the contents of a directory at a given path relative to a root inode.
pub fn list_directory_at(root: &Arc<Inode>, path: &str) -> Option<Vec<String>> {
    println!("[list_directory_at] path='{}'", path);
    let dir = if path.is_empty() || path == "/" {
        root.clone()
    } else {
        lookup_path_from(root, path)?
    };
    let entries = dir.ls();
    println!("[list_directory_at] path='{}', entry_count={}", path, entries.len());
    Some(entries)
}

/// Open a file using CWD-aware path resolution.
/// Kept for backward compatibility — delegates to open_file_at with ROOT_INODE.
pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<OSInode>> {
    // For backward compatibility, use ROOT_INODE as the default root.
    // The syscall layer (sys_open) will determine the correct root
    // based on whether the path is absolute or relative.
    open_file_at(&get_root_inode(), name, flags)
}

impl File for OSInode {
    fn readable(&self) -> bool {
        self.readable
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn read(&self, mut buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_read_size = 0usize;
        for slice in buf.buffers.iter_mut() {
            let read_size = inner.inode.read_at(inner.offset, *slice);
            if read_size == 0 {
                break;
            }
            inner.offset += read_size;
            total_read_size += read_size;
        }
        total_read_size
    }
    fn write(&self, buf: UserBuffer) -> usize {
        let mut inner = self.inner.exclusive_access();
        let mut total_write_size = 0usize;
        for slice in buf.buffers.iter() {
            let write_size = inner.inode.write_at(inner.offset, *slice);
            assert_eq!(write_size, slice.len());
            inner.offset += write_size;
            total_write_size += write_size;
        }
        total_write_size
    }
}
