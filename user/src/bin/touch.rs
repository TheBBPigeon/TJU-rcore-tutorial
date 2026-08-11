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
    println!("[touch] path='{}'", path);

    let fd = open(path, OpenFlags::CREATE | OpenFlags::WRONLY);
    if fd < 0 {
        println!("touch: cannot touch '{}': Is a directory", path);
        return -1;
    }
    println!("[touch] created={}", fd >= 0);
    user_lib::close(fd as usize);
    0
}
