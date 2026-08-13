#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{getpgrp, getpid, yield_};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!(
        "[sigint_test] pid={} pgrp={}; press Ctrl-C to kill me",
        getpid(),
        getpgrp()
    );
    loop {
        yield_();
    }
}
