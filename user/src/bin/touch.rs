#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{OpenFlags, close, open};

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    if argc < 2 {
        println!("Usage: touch <file>");
        return -1;
    }
    let path = argv[1];

    // Check if file already exists
    let fd = open(path, OpenFlags::RDONLY);
    if fd >= 0 {
        close(fd as usize);
        println!("touch: '{}' already exists", path);
        0
    } else {
        // File does not exist, create it
        let fd = open(path, OpenFlags::CREATE | OpenFlags::WRONLY);
        if fd < 0 {
            println!("touch: cannot touch '{}': Is a directory", path);
            return -1;
        }
        close(fd as usize);
        println!("created {}", path);
        0
    }
}
