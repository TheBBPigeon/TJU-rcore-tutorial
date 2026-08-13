use super::File;
use crate::drivers::BLOCK_DEVICE;
use crate::mm::UserBuffer;
use crate::sync::{Mutex, MutexBlocking, UPIntrFreeCell};
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
        FILE_SYSTEM_LOCK.lock();

        let result = {
            let mut inner = self.inner.exclusive_access();
            let mut buffer = [0u8; 512];
            let mut data = Vec::new();

            loop {
                let len = inner.inode.read_at(inner.offset, &mut buffer);
                if len == 0 {
                    break;
                }
                inner.offset += len;
                data.extend_from_slice(&buffer[..len]);
            }

            data
        };

        FILE_SYSTEM_LOCK.unlock();
        result
    }
}

lazy_static! {
    /// easy-fs 内部使用同步自旋锁，而 VirtIO 块设备 I/O 可能阻塞并切换任务。
    ///
    /// 如果两个进程并发访问 easy-fs，第一个任务可能持有 easy-fs 锁等待设备，
    /// 第二个任务则在单核 CPU 上自旋等待同一把锁，使第一个任务无法恢复运行。
    ///
    /// 这里使用任务级阻塞互斥锁，把所有 easy-fs 操作串行化。等待锁的任务会
    /// 进入 Blocked 状态，而不是在单核 CPU 上持续自旋。
    static ref FILE_SYSTEM_LOCK: MutexBlocking = MutexBlocking::new();

    pub static ref ROOT_INODE: Arc<Inode> = {
        let efs = EasyFileSystem::open(BLOCK_DEVICE.clone());
        Arc::new(EasyFileSystem::root_inode(&efs))
    };
}

pub fn list_apps() {
    FILE_SYSTEM_LOCK.lock();

    let apps = ROOT_INODE.ls();

    FILE_SYSTEM_LOCK.unlock();

    println!("/**** APPS ****");
    for app in apps {
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
    /// Return (readable, writable).
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

pub fn open_file(name: &str, flags: OpenFlags) -> Option<Arc<OSInode>> {
    FILE_SYSTEM_LOCK.lock();

    let (readable, writable) = flags.read_write();

    let result = if flags.contains(OpenFlags::CREATE) {
        if let Some(inode) = ROOT_INODE.find(name) {
            inode.clear();
            Some(Arc::new(OSInode::new(readable, writable, inode)))
        } else {
            ROOT_INODE
                .create(name)
                .map(|inode| Arc::new(OSInode::new(readable, writable, inode)))
        }
    } else {
        ROOT_INODE.find(name).map(|inode| {
            if flags.contains(OpenFlags::TRUNC) {
                inode.clear();
            }
            Arc::new(OSInode::new(readable, writable, inode))
        })
    };

    FILE_SYSTEM_LOCK.unlock();
    result
}

impl File for OSInode {
    fn readable(&self) -> bool {
        self.readable
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn read(&self, mut buf: UserBuffer) -> usize {
        FILE_SYSTEM_LOCK.lock();

        let result = {
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
        };

        FILE_SYSTEM_LOCK.unlock();
        result
    }

    fn write(&self, buf: UserBuffer) -> usize {
        FILE_SYSTEM_LOCK.lock();

        let result = {
            let mut inner = self.inner.exclusive_access();
            let mut total_write_size = 0usize;

            for slice in buf.buffers.iter() {
                let write_size = inner.inode.write_at(inner.offset, *slice);
                assert_eq!(write_size, slice.len());
                inner.offset += write_size;
                total_write_size += write_size;
            }

            total_write_size
        };

        FILE_SYSTEM_LOCK.unlock();
        result
    }
}
