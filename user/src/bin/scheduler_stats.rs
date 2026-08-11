#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::hint::black_box;
use user_lib::{SchedStats, get_sched_stats, get_time, set_priority, yield_};

fn burn_until(deadline_ms: isize) {
    let mut value = 1usize;
    while get_time() < deadline_ms {
        for _ in 0..2048 {
            value = value.wrapping_mul(1664525).wrapping_add(1013904223);
        }
        black_box(value);
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    assert_eq!(set_priority(0, 4), 0);
    let mut before = SchedStats::default();
    let mut after = SchedStats::default();
    assert_eq!(get_sched_stats(0, &mut before), 0);

    burn_until(get_time() + 120);
    yield_();
    assert_eq!(get_sched_stats(0, &mut after), 0);

    assert!(after.runtime_ticks > before.runtime_ticks);
    assert!(after.total_wait_ticks >= before.total_wait_ticks);
    assert!(after.scheduled_count > before.scheduled_count);
    println!(
        "[PASS] scheduler stats: runtime={} wait={} dispatches={}",
        after.runtime_ticks, after.total_wait_ticks, after.scheduled_count
    );
    0
}
