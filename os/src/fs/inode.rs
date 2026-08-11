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
pub fn lookup_path_from(root: &Arc<Inode>, path: &str) -> Option<Arc<Inode>> {
    root.lookup_path(path)
}

/// Open a file at a given path relative to a root inode.
pub fn open_file_at(
    root: &Arc<Inode>,
    path: &str,
    flags: OpenFlags,
) -> Option<Arc<OSInode>> {
    let (readable, writable) = flags.read_write();
    if flags.contains(OpenFlags::CREATE) {
        let (parent_path, file_name) = split_path(path);
        let parent = if parent_path.is_empty() || parent_path == "/" {
            root.clone()
        } else {
            lookup_path_from(root, parent_path)?
        };
        if let Some(inode) = parent.find(file_name) {
            if inode.is_dir() {
                return None;
            }
            if flags.contains(OpenFlags::TRUNC) {
                inode.clear();
            }
            Some(Arc::new(OSInode::new(readable, writable, inode)))
        } else {
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

/// Create a directory at a given path relative to a root inode.
/// Returns 0 on success, -1 on failure.
pub fn make_directory_at(root: &Arc<Inode>, path: &str) -> isize {
    let (parent_path, dir_name) = split_path(path);
    let parent = if parent_path.is_empty() || parent_path == "/" {
        root.clone()
    } else {
        match lookup_path_from(root, parent_path) {
            Some(p) => p,
            None => return -1,
        }
    };
    match parent.create_as(dir_name, easy_fs::DiskInodeType::Directory) {
        Some(_) => 0,
        None => -1,
    }
}

/// Remove a file or empty directory at a given path relative to a root inode.
/// Returns 0 on success, -1 on failure.
pub fn unlink_at(root: &Arc<Inode>, path: &str) -> isize {
    let (parent_path, name) = split_path(path);
    let parent = if parent_path.is_empty() || parent_path == "/" {
        root.clone()
    } else {
        match lookup_path_from(root, parent_path) {
            Some(p) => p,
            None => return -1,
        }
    };
    match parent.unlink(name) {
        Some(_) => 0,
        None => -1,
    }
}

/// List the contents of a directory at a given path relative to a root inode.
pub fn list_directory_at(root: &Arc<Inode>, path: &str) -> Option<Vec<String>> {
    let dir = if path.is_empty() || path == "/" {
        root.clone()
    } else {
        lookup_path_from(root, path)?
    };
    if !dir.is_dir() {
        return None;
    }
    Some(dir.ls())
}

/// Open a file using CWD-aware path resolution.
pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<OSInode>> {
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
