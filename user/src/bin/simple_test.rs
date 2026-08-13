#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[simple_test] start");

    let left = 20;
    let right = 22;
    let result = left + right;

    println!("{} + {} = {}", left, right, result);
    assert_eq!(result, 42);
    println!("[simple_test] passed");

    0
}
