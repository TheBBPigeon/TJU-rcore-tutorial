mod inode;
mod pipe;
mod stdio;

use crate::mm::UserBuffer;

pub trait File: Send + Sync {
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn read(&self, buf: UserBuffer) -> usize;
    fn write(&self, buf: UserBuffer) -> usize;
}

pub use inode::{
    OpenFlags, get_root_inode, list_app_names, lookup_path_from, make_directory_at, normalize_path,
    open_file, open_file_at, rename_at, unlink_at,
};
pub use pipe::make_pipe;
pub use stdio::{Stdin, Stdout};
