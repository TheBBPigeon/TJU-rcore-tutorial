use crate::fs::{
    OpenFlags, make_directory_at, make_pipe, open_file_at, rename_at, unlink_at,
    lookup_path_from, get_root_inode,
};
use crate::mm::{UserBuffer, translated_byte_buffer, translated_refmut, translated_str};
use crate::task::{current_process, current_user_token};
use alloc::string::String;
use alloc::sync::Arc;

pub fn sys_write(fd: usize, buf: *const u8, len: usize) -> isize {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        if !file.writable() {
            return -1;
        }
        let file = file.clone();
        drop(inner);
        file.write(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

pub fn sys_read(fd: usize, buf: *const u8, len: usize) -> isize {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if let Some(file) = &inner.fd_table[fd] {
        let file = file.clone();
        if !file.readable() {
            return -1;
        }
        drop(inner);
        file.read(UserBuffer::new(translated_byte_buffer(token, buf, len))) as isize
    } else {
        -1
    }
}

/// CWD-aware sys_open. Absolute paths resolve from ROOT_INODE;
/// relative paths resolve from the process's current working directory.
pub fn sys_open(path: *const u8, flags: u32) -> isize {
    let process = current_process();
    let token = current_user_token();
    let path = translated_str(token, path);
    let flags = OpenFlags::from_bits(flags).unwrap();
    let root = if path.starts_with('/') {
        get_root_inode()
    } else {
        process.inner_exclusive_access().get_working_directory()
    };
    if let Some(inode) = open_file_at(&root, path.as_str(), flags) {
        let mut inner = process.inner_exclusive_access();
        let fd = inner.alloc_fd();
        inner.fd_table[fd] = Some(inode);
        fd as isize
    } else {
        -1
    }
}

pub fn sys_close(fd: usize) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    inner.fd_table[fd].take();
    0
}

pub fn sys_pipe(pipe: *mut usize) -> isize {
    let process = current_process();
    let token = current_user_token();
    let mut inner = process.inner_exclusive_access();
    let (pipe_read, pipe_write) = make_pipe();
    let read_fd = inner.alloc_fd();
    inner.fd_table[read_fd] = Some(pipe_read);
    let write_fd = inner.alloc_fd();
    inner.fd_table[write_fd] = Some(pipe_write);
    *translated_refmut(token, pipe) = read_fd;
    *translated_refmut(token, unsafe { pipe.add(1) }) = write_fd;
    0
}

pub fn sys_dup(fd: usize) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    if fd >= inner.fd_table.len() {
        return -1;
    }
    if inner.fd_table[fd].is_none() {
        return -1;
    }
    let new_fd = inner.alloc_fd();
    inner.fd_table[new_fd] = Some(Arc::clone(inner.fd_table[fd].as_ref().unwrap()));
    new_fd as isize
}

/// Create a directory at the given path. CWD-aware.
pub fn sys_mkdir(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let process = current_process();
    let root = if path.starts_with('/') {
        get_root_inode()
    } else {
        process.inner_exclusive_access().get_working_directory()
    };
    make_directory_at(&root, path.as_str())
}

/// Remove a file or empty directory at the given path. CWD-aware.
pub fn sys_unlink(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let process = current_process();
    let root = if path.starts_with('/') {
        get_root_inode()
    } else {
        process.inner_exclusive_access().get_working_directory()
    };
    unlink_at(&root, path.as_str())
}

/// Rename a file or directory. CWD-aware. Both paths must be in the same
/// parent directory (same-directory rename only).
pub fn sys_rename(old_path: *const u8, new_path: *const u8) -> isize {
    let token = current_user_token();
    let old_path = translated_str(token, old_path);
    let new_path = translated_str(token, new_path);
    let process = current_process();
    let root = if old_path.starts_with('/') {
        get_root_inode()
    } else {
        process.inner_exclusive_access().get_working_directory()
    };
    rename_at(&root, old_path.as_str(), new_path.as_str())
}

/// Normalize a path by removing "." and empty components.
fn normalize_path(path: &str) -> String {
    let mut result = String::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            _ => {
                if !result.is_empty() {
                    result.push('/');
                }
                result.push_str(component);
            }
        }
    }
    if result.is_empty() {
        String::from("/")
    } else if path.starts_with('/') {
        String::from("/") + &result
    } else {
        result
    }
}

/// Change the current working directory.
pub fn sys_chdir(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let process = current_process();

    let (old_path, root) = {
        let inner = process.inner_exclusive_access();
        let old_p = inner.get_working_directory_path();
        let root_inode = if path.starts_with('/') {
            get_root_inode()
        } else {
            inner.get_working_directory()
        };
        (old_p, root_inode)
    };

    match lookup_path_from(&root, path.as_str()) {
        Some(new_inode) => {
            if !new_inode.is_dir() {
                return -1;
            }
            let new_path = if path.starts_with('/') {
                let trimmed = path.trim_matches('/');
                if trimmed.is_empty() {
                    String::from("/")
                } else {
                    String::from("/") + trimmed
                }
            } else if path == "." {
                old_path.clone()
            } else if path == ".." {
                if old_path == "/" {
                    String::from("/")
                } else {
                    match old_path.rfind('/') {
                        Some(pos) if pos == 0 => String::from("/"),
                        Some(pos) => String::from(&old_path[..pos]),
                        None => String::from("/"),
                    }
                }
            } else if old_path == "/" {
                String::from("/") + &path
            } else {
                old_path + "/" + &path
            };
            // Normalize: strip "/./" and trailing "/." components
            let new_path = normalize_path(&new_path);
            let mut inner = process.inner_exclusive_access();
            inner.set_working_directory(new_inode);
            inner.set_working_directory_path(new_path);
            0
        }
        None => -1,
    }
}

/// Get the current working directory path.
pub fn sys_getcwd(buf: *mut u8, len: usize) -> isize {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let cwd_path = inner.get_working_directory_path();
    drop(inner);
    let path_bytes = cwd_path.as_bytes();
    let write_len = core::cmp::min(path_bytes.len(), len);
    let mut buffers = translated_byte_buffer(token, buf, write_len);
    let mut offset = 0;
    for slice in buffers.iter_mut() {
        let end = core::cmp::min(offset + slice.len(), write_len);
        slice[..end - offset].copy_from_slice(&path_bytes[offset..end]);
        offset = end;
    }
    write_len as isize
}

/// Get directory entries at a given path. CWD-aware.
/// Writes newline-separated file names into the user buffer.
pub fn sys_getdents(path: *const u8, buf: *mut u8, len: usize) -> isize {
    let token = current_user_token();
    let path_str = translated_str(token, path);
    let process = current_process();
    let root = if path_str.starts_with('/') {
        get_root_inode()
    } else {
        process.inner_exclusive_access().get_working_directory()
    };
    let dir = if path_str.is_empty() || path_str == "/" {
        root.clone()
    } else {
        match lookup_path_from(&root, path_str.as_str()) {
            Some(d) => d,
            None => return -1,
        }
    };
    if !dir.is_dir() {
        return -1;
    }
    let entries = dir.ls();

    let mut listing = alloc::string::String::new();
    listing.push_str("total ");
    listing.push_str(&alloc::string::ToString::to_string(&entries.len()));
    listing.push('\n');
    for name in &entries {
        let entry_type = if let Some(inode) = dir.find(name) {
            if inode.is_dir() { "d " } else { "f " }
        } else {
            "? "
        };
        listing.push_str(entry_type);
        listing.push_str(name);
        listing.push('\n');
    }
    let bytes = listing.as_bytes();
    let write_len = core::cmp::min(bytes.len(), len);

    let mut buffers = translated_byte_buffer(token, buf, write_len);
    let mut offset = 0;
    for slice in buffers.iter_mut() {
        let end = core::cmp::min(offset + slice.len(), write_len);
        let copy_len = end - offset;
        slice[..copy_len].copy_from_slice(&bytes[offset..end]);
        offset = end;
        if offset >= write_len {
            break;
        }
    }
    write_len as isize
}

/// Get the PATH variable for the current process.
pub fn sys_getpath(buf: *mut u8, len: usize) -> isize {
    let token = current_user_token();
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let path = inner.get_path_variable();
    drop(inner);
    let path_bytes = path.as_bytes();
    let write_len = core::cmp::min(path_bytes.len(), len);
    let mut buffers = translated_byte_buffer(token, buf, write_len);
    let mut offset = 0;
    for slice in buffers.iter_mut() {
        let end = core::cmp::min(offset + slice.len(), write_len);
        slice[..end - offset].copy_from_slice(&path_bytes[offset..end]);
        offset = end;
    }
    write_len as isize
}

/// Set the PATH variable for the current process.
pub fn sys_setpath(path: *const u8) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    inner.set_path_variable(path);
    0
}

/// Move the file offset for a given fd.
/// Stub — not yet implemented.
pub fn sys_lseek(_fd: usize, _offset: isize, _whence: u32) -> isize {
    -1
}
