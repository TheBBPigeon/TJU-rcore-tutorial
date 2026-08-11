use super::*;

bitflags! {
    pub struct OpenFlags: u32 {
        const RDONLY = 0;
        const WRONLY = 1 << 0;
        const RDWR = 1 << 1;
        const CREATE = 1 << 9;
        const TRUNC = 1 << 10;
    }
}

pub fn dup(fd: usize) -> isize {
    sys_dup(fd)
}
pub fn open(path: &str, flags: OpenFlags) -> isize {
    sys_open(path, flags.bits)
}
pub fn close(fd: usize) -> isize {
    sys_close(fd)
}
pub fn pipe(pipe_fd: &mut [usize]) -> isize {
    sys_pipe(pipe_fd)
}
pub fn read(fd: usize, buf: &mut [u8]) -> isize {
    sys_read(fd, buf)
}
pub fn write(fd: usize, buf: &[u8]) -> isize {
    sys_write(fd, buf)
}

// New file system operations
pub fn mkdir(path: &str) -> isize {
    sys_mkdir(path)
}

pub fn unlink(path: &str) -> isize {
    sys_unlink(path)
}

pub fn chdir(path: &str) -> isize {
    sys_chdir(path)
}

pub fn getdents(path: &str, buf: &mut [u8]) -> isize {
    sys_getdents(path, buf)
}

pub fn getcwd(buf: &mut [u8]) -> isize {
    sys_getcwd(buf)
}

pub fn lseek(fd: usize, offset: isize, whence: u32) -> isize {
    sys_lseek(fd, offset, whence)
}
