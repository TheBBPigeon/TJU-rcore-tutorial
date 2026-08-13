#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use user_lib::{SchedStats, fork, get_sched_stats, get_time, set_priority, waitpid};

fn run_cpu_child(deadline_ms: isize, child_no: usize) -> i32 {
    assert_eq!(set_priority(0, 4), 0);
    let mut value = child_no + 1;
    while get_time() < deadline_ms {
        for _ in 0..4096 {
            value = value.rotate_left(5) ^ 0x9e3779b9;
        }
        black_box(value);
    }
    let mut stats = SchedStats::default();
    assert_eq!(get_sched_stats(0, &mut stats), 0);
    println!(
        "preempt child {}: runtime={} dispatches={}",
        child_no, stats.runtime_ticks, stats.scheduled_count
    );
    if stats.runtime_ticks > 0 && stats.scheduled_count > 1 {
        0
    } else {
        1
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let deadline = get_time() + 500;
    let first = fork();
    if first == 0 {
        return run_cpu_child(deadline, 1);
    }
    let second = fork();
    if second == 0 {
        return run_cpu_child(deadline, 2);
    }

    let mut first_code = -1;
    let mut second_code = -1;
    assert_eq!(waitpid(first as usize, &mut first_code), first);
    assert_eq!(waitpid(second as usize, &mut second_code), second);
    assert_eq!(first_code, 0);
    assert_eq!(second_code, 0);
    println!("[PASS] timer preempts CPU-bound tasks without yield");
    0
}
