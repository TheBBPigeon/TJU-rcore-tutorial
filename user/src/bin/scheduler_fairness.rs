#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use user_lib::{SchedStats, fork, get_sched_stats, get_time, set_priority, sleep, waitpid};

const CHILDREN: usize = 3;

fn run_equal_priority_child(start_ms: isize, deadline_ms: isize, seed: usize) -> i32 {
    assert_eq!(set_priority(0, 4), 0);
    let now = get_time();
    if now < start_ms {
        sleep((start_ms - now) as usize);
    }
    let mut value = seed;
    while get_time() < deadline_ms {
        for _ in 0..4096 {
            value = value.wrapping_mul(1103515245).wrapping_add(12345);
        }
        black_box(value);
    }
    let mut stats = SchedStats::default();
    assert_eq!(get_sched_stats(0, &mut stats), 0);
    stats.runtime_ticks.min(i32::MAX as usize) as i32
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let start = get_time() + 200;
    let deadline = start + 1000;
    let mut pids = [0isize; CHILDREN];
    for (index, pid_slot) in pids.iter_mut().enumerate() {
        let pid = fork();
        if pid == 0 {
            return run_equal_priority_child(start, deadline, index + 1);
        }
        *pid_slot = pid;
    }

    let mut runtimes = [0usize; CHILDREN];
    for (index, pid) in pids.iter().enumerate() {
        let mut exit_code = -1;
        assert_eq!(waitpid(*pid as usize, &mut exit_code), *pid);
        assert!(exit_code > 0);
        runtimes[index] = exit_code as usize;
    }

    let minimum = *runtimes.iter().min().unwrap();
    let maximum = *runtimes.iter().max().unwrap();
    let spread = maximum - minimum;
    println!("fairness raw runtimes: {:?}", runtimes);
    assert!(spread * 100 <= maximum * 20 + 100);
    println!(
        "[PASS] equal-priority fairness: runtimes={:?}, spread={} ticks",
        runtimes, spread
    );
    0
}
