#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exec, fork, wait, yield_};

#[unsafe(no_mangle)]
fn main() -> i32 {
    // Run the file system test suite first
    if fork() == 0 {
        exec("fs_test\0", &[core::ptr::null::<u8>()]);
        panic!("initproc: exec fs_test failed!");
    } else {
        let mut exit_code: i32 = 0;
        loop {
            let pid = wait(&mut exit_code);
            if pid == -1 {
                yield_();
                continue;
            }
            // fs_test has exited; break out to start the shell
            break;
        }
    }

    println!("[initproc] starting user shell...");

    // Start the user shell
    if fork() == 0 {
        exec("user_shell\0", &[core::ptr::null::<u8>()]);
        panic!("initproc: exec user_shell failed!");
    } else {
        loop {
            let mut exit_code: i32 = 0;
            let pid = wait(&mut exit_code);
            if pid == -1 {
                yield_();
                continue;
            }
            /*
            println!(
                "[initproc] Released a zombie process, pid={}, exit_code={}",
                pid,
                exit_code,
            );
            */
        }
    }
}
