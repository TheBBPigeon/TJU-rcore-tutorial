#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::rename;

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    if argc < 3 {
        println!("Usage: mv <source> <target>");
        return -1;
    }
    let result = rename(argv[1], argv[2]);
    if result < 0 {
        println!("mv: cannot move '{}' to '{}'", argv[1], argv[2]);
        return -1;
    }
    0
}
