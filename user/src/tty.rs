use super::syscall::*;

pub const TTY_ECHO: usize = 1;
pub const TTY_ICANON: usize = 2;
pub const TTY_ISIG: usize = 4;

pub const TTY_CTL_GET_FLAGS: usize = 1;
pub const TTY_CTL_SET_FLAGS: usize = 2;

pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

pub fn setpgid(pid: usize, pgid: usize) -> isize {
    sys_setpgid(pid, pgid)
}

pub fn getpgrp() -> isize {
    sys_getpgrp()
}

pub fn tcsetpgrp(fd: usize, pgrp: usize) -> isize {
    sys_tcsetpgrp(fd, pgrp)
}

pub fn tcgetpgrp(fd: usize) -> isize {
    sys_tcgetpgrp(fd)
}

pub fn tty_ctl(fd: usize, cmd: usize, arg: usize) -> isize {
    sys_tty_ctl(fd, cmd, arg)
}

pub fn sigaction(signal: i32, act: usize) -> isize {
    sys_sigaction(signal as u32, act)
}
