#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::unlink;

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    assert!(argc >= 2, "Usage: rm <file>");
    let path = argv[1];
    let result = unlink(path);
    if result < 0 {
        println!("rm: cannot remove '{}': No such file or directory", path);
        return -1;
    }
    println!("removed {}", path);
    0
}
