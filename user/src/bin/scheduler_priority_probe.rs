#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::get_priority;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    assert_eq!(get_priority(0), 6);
    println!("[PASS] exec preserves process priority");
    0
}
