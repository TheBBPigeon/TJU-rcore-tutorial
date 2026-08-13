#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    SchedStats, exec, fork, get_priority, get_sched_stats, get_sched_stats_raw, getpid,
    set_priority, waitpid,
};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let pid = getpid() as usize;
    assert_eq!(get_priority(0), 4);
    assert_eq!(set_priority(0, 7), 0);
    assert_eq!(get_priority(pid), 7);
    assert_eq!(set_priority(0, 8), -2);
    assert_eq!(set_priority(usize::MAX, 4), -1);
    assert_eq!(get_priority(usize::MAX), -1);

    let mut stats = SchedStats::default();
    assert_eq!(get_sched_stats(usize::MAX, &mut stats), -1);
    assert_eq!(unsafe { get_sched_stats_raw(0, core::ptr::null_mut()) }, -2);
    assert_eq!(get_sched_stats(0, &mut stats), 0);
    assert_eq!(stats.pid, pid);
    assert_eq!(stats.base_priority, 7);

    assert_eq!(set_priority(0, 6), 0);
    let child = fork();
    if child == 0 {
        assert_eq!(get_priority(0), 6);
        exec("scheduler_priority_probe\0", &[core::ptr::null::<u8>()]);
        return 127;
    }
    let mut child_code = -1;
    assert_eq!(waitpid(child as usize, &mut child_code), child);
    assert_eq!(child_code, 0);

    assert_eq!(set_priority(0, 4), 0);
    println!("[PASS] scheduler syscall validation");
    0
}
