#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use user_lib::{SchedStats, fork, get_sched_stats, get_time, set_priority, waitpid};

fn run_until(deadline_ms: isize, priority: usize) -> i32 {
    assert_eq!(set_priority(0, priority), 0);
    let mut value = priority + 11;
    while get_time() < deadline_ms {
        for _ in 0..4096 {
            value = value.wrapping_mul(22695477).wrapping_add(1);
        }
        black_box(value);
    }
    let mut stats = SchedStats::default();
    assert_eq!(get_sched_stats(0, &mut stats), 0);
    println!(
        "compare priority {}: runtime={} wait={} dispatches={}",
        priority, stats.runtime_ticks, stats.total_wait_ticks, stats.scheduled_count
    );
    stats.runtime_ticks.min(i32::MAX as usize) as i32
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let deadline = get_time() + 700;
    let low_pid = fork();
    if low_pid == 0 {
        return run_until(deadline, 0);
    }
    let high_pid = fork();
    if high_pid == 0 {
        return run_until(deadline, 7);
    }

    let mut low_runtime = -1;
    let mut high_runtime = -1;
    assert_eq!(waitpid(low_pid as usize, &mut low_runtime), low_pid);
    assert_eq!(waitpid(high_pid as usize, &mut high_runtime), high_pid);
    println!(
        "[COMPARE] high={} ticks, low={} ticks, delta={}",
        high_runtime,
        low_runtime,
        high_runtime - low_runtime
    );
    0
}
