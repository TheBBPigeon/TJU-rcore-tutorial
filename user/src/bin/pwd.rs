#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::getcwd;

#[unsafe(no_mangle)]
pub fn main(_argc: usize, _argv: &[&str]) -> i32 {
    let mut buf = [0u8; 256];
    let len = getcwd(&mut buf);
    if len > 0 {
        let path = core::str::from_utf8(&buf[..len as usize]).unwrap_or("/");
        println!("{}", path);
    }
    0
}
