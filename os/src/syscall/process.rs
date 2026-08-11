use crate::DEV_NON_BLOCKING_ACCESS;
use crate::fs::{OpenFlags, open_file};
use crate::mm::{translated_ref, translated_refmut, translated_str};
use crate::task::{
    SignalFlags, add_signal_to_process, current_process, current_task, current_user_token,
    exit_current_and_run_next, pid2process, signal_process_group, suspend_current_and_run_next,
};
use crate::timer::get_time_ms;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub fn sys_exit(exit_code: i32) -> ! {
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}

pub fn sys_get_time() -> isize {
    get_time_ms() as isize
}

pub fn sys_getpid() -> isize {
    current_task().unwrap().process.upgrade().unwrap().getpid() as isize
}

pub fn sys_fork() -> isize {
    let current_process = current_process();
    let new_process = current_process.fork();
    let new_pid = new_process.getpid();
    // modify trap context of new_task, because it returns immediately after switching
    let new_process_inner = new_process.inner_exclusive_access();
    let task = new_process_inner.tasks[0].as_ref().unwrap();
    let trap_cx = task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    new_pid as isize
}

pub fn sys_exec(path: *const u8, mut args: *const usize) -> isize {
    let token = current_user_token();
    let path = translated_str(token, path);
    let mut args_vec: Vec<String> = Vec::new();
    loop {
        let arg_str_ptr = *translated_ref(token, args);
        if arg_str_ptr == 0 {
            break;
        }
        args_vec.push(translated_str(token, arg_str_ptr as *const u8));
        unsafe {
            args = args.add(1);
        }
    }
    // Load the ELF with synchronous block reads. The easy-fs layer holds
    // spin locks across blocking I/O, so an asynchronous read here can
    // deadlock when two pipeline children call exec concurrently.
    *DEV_NON_BLOCKING_ACCESS.exclusive_access() = false;
    let app_inode = open_file(path.as_str(), OpenFlags::RDONLY);
    let all_data = app_inode.as_ref().map(|inode| inode.read_all());
    *DEV_NON_BLOCKING_ACCESS.exclusive_access() = true;
    if let Some(all_data) = all_data {
        let process = current_process();
        let argc = args_vec.len();
        process.exec(all_data.as_slice(), args_vec);
        // return argc because cx.x[10] will be covered with it later
        argc as isize
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    let process = current_process();
    // find a child process

    let mut inner = process.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

pub fn sys_kill(pid: isize, signal: u32) -> isize {
    let flag = match SignalFlags::from_bits(signal) {
        Some(flag) => flag,
        None => return -1,
    };
    if pid > 0 {
        if let Some(process) = pid2process(pid as usize) {
            add_signal_to_process(&process, flag);
            0
        } else {
            -1
        }
    } else if pid < 0 {
        signal_process_group((-pid) as usize, flag);
        0
    } else {
        let pgrp = current_process().inner_exclusive_access().pgid;
        signal_process_group(pgrp, flag);
        0
    }
}

/// Set the process group of `pid` (0 means the calling process).
/// `pgid` 0 means "make `pid` the group leader".
pub fn sys_setpgid(pid: usize, pgid: usize) -> isize {
    let current = current_process();
    let target_pid = if pid == 0 { current.getpid() } else { pid };
    if let Some(process) = pid2process(target_pid) {
        let mut inner = process.inner_exclusive_access();
        inner.pgid = if pgid == 0 { target_pid } else { pgid };
        0
    } else {
        -1
    }
}

pub fn sys_getpgrp() -> isize {
    let process = current_process();
    process.inner_exclusive_access().pgid as isize
}

pub fn sys_tcsetpgrp(_fd: usize, pgrp: usize) -> isize {
    crate::tty::TTY.set_fg_pgrp(pgrp);
    0
}

pub fn sys_tcgetpgrp(_fd: usize) -> isize {
    crate::tty::TTY.get_fg_pgrp() as isize
}

/// Minimal sigaction: act 0 = SIG_DFL, act 1 = SIG_IGN.
/// `signal` is a SignalFlags bit (e.g. 1 << 2 for SIGINT).
pub fn sys_sigaction(signal: u32, act: usize) -> isize {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let flag = match SignalFlags::from_bits(signal) {
        Some(flag) => flag,
        None => return -1,
    };
    match act {
        0 => {
            inner.sig_ignored.remove(flag);
            0
        }
        1 => {
            inner.sig_ignored.insert(flag);
            0
        }
        _ => -1,
    }
}
