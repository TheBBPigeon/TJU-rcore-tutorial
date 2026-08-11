#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, getpgrp, getpid, setpgid, wait};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let pid = getpid();
    let pgrp = getpgrp();
    println!("[pgid_test] pid={} pgrp={}", pid, pgrp);
    if fork() == 0 {
        let child_pid = getpid();
        setpgid(0, 0);
        println!(
            "[pgid_test] child pid={} pgrp={} (expect {} == {})",
            child_pid,
            getpgrp(),
            getpgrp(),
            child_pid
        );
        exit(0);
    }
    let mut exit_code = 0;
    wait(&mut exit_code);
    println!("[pgid_test] child reaped, exit_code={}", exit_code);
    0
}
