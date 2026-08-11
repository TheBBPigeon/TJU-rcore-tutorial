#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exec, fork, waitpid};

const TESTS: &[&str] = &[
    "scheduler_syscall\0",
    "scheduler_stats\0",
    "scheduler_preempt\0",
    "scheduler_fairness\0",
    "scheduler_priority\0",
    "scheduler_aging\0",
];

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    for test in TESTS {
        println!("Scheduler tests: running {}", test);
        let pid = fork();
        if pid == 0 {
            exec(test, &[core::ptr::null::<u8>()]);
            return 127;
        }
        let mut exit_code = -1;
        assert_eq!(waitpid(pid as usize, &mut exit_code), pid);
        assert_eq!(exit_code, 0, "{} failed", test);
    }
    println!("[PASS] all scheduler tests passed");
    0
}
