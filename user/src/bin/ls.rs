#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::getdents;

#[unsafe(no_mangle)]
pub fn main(argc: usize, argv: &[&str]) -> i32 {
    let path = if argc >= 2 { argv[1] } else { "." };
    let mut buf = [0u8; 4096];
    let result = getdents(path, &mut buf);
    if result < 0 {
        println!("ls: cannot access '{}': No such file or directory", path);
        return -1;
    }
    let len = result as usize;
    if len > 0 {
        let listing = core::str::from_utf8(&buf[..len]).unwrap_or("");
        print!("{}", listing);
    }
    0
}
