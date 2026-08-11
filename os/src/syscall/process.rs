use crate::fs::{OpenFlags, open_file};
use crate::mm::{
    PageTable, VirtAddr, translated_byte_buffer, translated_ref, translated_refmut, translated_str,
};
use crate::task::{
    MAX_PRIORITY, MIN_PRIORITY, SchedStats, SignalFlags, current_process, current_task,
    current_user_token, exit_current_and_run_next, pid2process, suspend_current_and_run_next,
};
use crate::timer::get_time_ms;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;

const USER_VA_LIMIT: usize = 1usize << 38;

fn resolve_process(pid: usize) -> Option<Arc<crate::task::ProcessControlBlock>> {
    if pid == 0 {
        Some(current_process())
    } else {
        pid2process(pid)
    }
}

fn user_range_writable(token: usize, start: usize, len: usize) -> bool {
    if start == 0 || len == 0 || start >= USER_VA_LIMIT {
        return false;
    }
    let Some(end) = start.checked_add(len - 1) else {
        return false;
    };
    if end >= USER_VA_LIMIT {
        return false;
    }
    let page_table = PageTable::from_token(token);
    let mut address = start;
    loop {
        let Some(pte) = page_table.translate(VirtAddr::from(address).floor()) else {
            return false;
        };
        if !pte.is_valid() || !pte.writable() {
            return false;
        }
        if address >= end {
            return true;
        }
        let next_page = (address & !0xfff).saturating_add(0x1000);
        if next_page == 0 || next_page > end {
            return true;
        }
        address = next_page;
    }
}

fn write_sched_stats_to_user(stats_ptr: *mut SchedStats, stats: &SchedStats) -> bool {
    let token = current_user_token();
    let len = size_of::<SchedStats>();
    if !user_range_writable(token, stats_ptr as usize, len) {
        return false;
    }
    let source =
        unsafe { core::slice::from_raw_parts((stats as *const SchedStats).cast::<u8>(), len) };
    let mut copied = 0;
    for destination in translated_byte_buffer(token, stats_ptr.cast::<u8>(), len) {
        let count = destination.len();
        destination.copy_from_slice(&source[copied..copied + count]);
        copied += count;
    }
    copied == len
}

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
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
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

pub fn sys_kill(pid: usize, signal: u32) -> isize {
    if let Some(process) = pid2process(pid) {
        if let Some(flag) = SignalFlags::from_bits(signal) {
            process.inner_exclusive_access().signals |= flag;
            0
        } else {
            -1
        }
    } else {
        -1
    }
}

pub fn sys_set_priority(pid: usize, priority: usize) -> isize {
    if !(MIN_PRIORITY..=MAX_PRIORITY).contains(&priority) {
        return -2;
    }
    let Some(process) = resolve_process(pid) else {
        return -1;
    };
    process.set_priority(priority);
    0
}

pub fn sys_get_priority(pid: usize) -> isize {
    let Some(process) = resolve_process(pid) else {
        return -1;
    };
    process.priority().map_or(-1, |priority| priority as isize)
}

pub fn sys_get_sched_stats(pid: usize, stats_ptr: *mut SchedStats) -> isize {
    let Some(process) = resolve_process(pid) else {
        return -1;
    };
    let stats = process.sched_stats();
    if write_sched_stats_to_user(stats_ptr, &stats) {
        0
    } else {
        -2
    }
}
