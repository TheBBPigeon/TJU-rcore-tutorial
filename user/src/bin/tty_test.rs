#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    TTY_CTL_GET_FLAGS, TTY_CTL_SET_FLAGS, TTY_ECHO, TTY_ICANON, TTY_ISIG, read, tty_ctl,
};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let flags = tty_ctl(0, TTY_CTL_GET_FLAGS, 0);
    println!("[tty_test] initial flags = {:#x}", flags);

    // canonical mode: type a line and press Enter
    tty_ctl(0, TTY_CTL_SET_FLAGS, TTY_ECHO | TTY_ICANON | TTY_ISIG);
    println!("[tty_test] canonical: please type a line and press Enter");
    let mut buf = [0u8; 64];
    let n = read(0, &mut buf) as usize;
    println!(
        "[tty_test] got {} bytes: {:?}",
        n,
        core::str::from_utf8(&buf[..n]).unwrap()
    );

    // raw mode without echo: type some bytes (e.g. abc)
    tty_ctl(0, TTY_CTL_SET_FLAGS, TTY_ISIG);
    println!("[tty_test] raw mode: type some bytes");
    let n = read(0, &mut buf) as usize;
    println!("[tty_test] raw got {} bytes: {:?}", n, &buf[..n]);

    // back to canonical and test Ctrl-D (EOF)
    tty_ctl(0, TTY_CTL_SET_FLAGS, TTY_ECHO | TTY_ICANON | TTY_ISIG);
    println!("[tty_test] canonical again: press Ctrl-D for EOF");
    let n = read(0, &mut buf);
    println!("[tty_test] read after Ctrl-D returned {}", n);
    0
}
