#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use user_lib::{SchedStats, fork, get_sched_stats, get_time, set_priority, sleep, waitpid, yield_};

fn burn_until(deadline_ms: isize, seed: usize) {
    let mut value = seed;
    while get_time() < deadline_ms {
        for _ in 0..4096 {
            value = value.wrapping_mul(1664525).wrapping_add(1013904223);
        }
        black_box(value);
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    assert_eq!(set_priority(0, 7), 0);
    let deadline = get_time() + 1900;

    let low_pid = fork();
    if low_pid == 0 {
        assert_eq!(set_priority(0, 0), 0);
        burn_until(deadline, 1);
        let mut stats = SchedStats::default();
        assert_eq!(get_sched_stats(0, &mut stats), 0);
        println!(
            "aged task: runtime={} wait={} dispatches={}",
            stats.runtime_ticks, stats.total_wait_ticks, stats.scheduled_count
        );
        return if stats.scheduled_count >= 2 { 0 } else { 1 };
    }

    // Let the low-priority child run once and lower its own priority before
    // introducing sustained priority-7 competition.
    sleep(60);
    let high_pid = fork();
    if high_pid == 0 {
        assert_eq!(set_priority(0, 7), 0);
        burn_until(deadline, 7);
        return 0;
    }

    let mut maximum_effective_priority = 0;
    let mut maximum_wait_ticks = 0;
    while get_time() < deadline {
        let mut stats = SchedStats::default();
        if get_sched_stats(low_pid as usize, &mut stats) == 0 {
            maximum_effective_priority = maximum_effective_priority.max(stats.effective_priority);
            maximum_wait_ticks = maximum_wait_ticks.max(stats.total_wait_ticks);
        }
        yield_();
    }

    let mut low_code = -1;
    let mut high_code = -1;
    assert_eq!(waitpid(low_pid as usize, &mut low_code), low_pid);
    assert_eq!(waitpid(high_pid as usize, &mut high_code), high_pid);
    assert_eq!(low_code, 0);
    assert_eq!(high_code, 0);
    assert_eq!(maximum_effective_priority, 7);
    assert!(maximum_wait_ticks >= 140);
    println!(
        "[PASS] aging prevents starvation: max_priority={} wait={} ticks",
        maximum_effective_priority, maximum_wait_ticks
    );
    0
}
