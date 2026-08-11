#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::mkdir;

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    assert!(argc >= 2, "Usage: mkdir <directory>");
    let path = argv[1];
    let result = mkdir(path);
    if result < 0 {
        println!("mkdir: cannot create directory '{}': File exists", path);
        return -1;
    }
    println!("created directory {}", path);
    0
}
