use crate::tty::{TTY, TTY_CTL_GET_FLAGS, TTY_CTL_SET_FLAGS};

pub fn sys_tty_ctl(_fd: usize, cmd: usize, arg: usize) -> isize {
    match cmd {
        TTY_CTL_GET_FLAGS => TTY.get_flags() as isize,
        TTY_CTL_SET_FLAGS => {
            TTY.set_flags(arg);
            0
        }
        _ => -1,
    }
}
