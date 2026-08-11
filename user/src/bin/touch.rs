#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{OpenFlags, open};

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    assert!(argc >= 2, "Usage: touch <file>");
    let path = argv[1];
    let fd = open(path, OpenFlags::CREATE | OpenFlags::WRONLY);
    if fd < 0 {
        println!("touch: cannot touch '{}': Is a directory", path);
        return -1;
    }
    user_lib::close(fd as usize);
    println!("created {}", path);
    0
}
