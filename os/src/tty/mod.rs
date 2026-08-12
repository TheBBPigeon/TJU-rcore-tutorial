use crate::drivers::chardev::{CharDevice, UART};
use crate::mm::UserBuffer;
use crate::sync::{Condvar, UPIntrFreeCell};
use crate::task::{SignalFlags, current_add_signal, current_pgrp, schedule, signal_process_group};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::*;

pub const TTY_ECHO: usize = 1;
pub const TTY_ICANON: usize = 2;
pub const TTY_ISIG: usize = 4;

pub const TTY_CTL_GET_FLAGS: usize = 1;
pub const TTY_CTL_SET_FLAGS: usize = 2;

const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const LF: u8 = 0x0a;
const CR: u8 = 0x0d;
const BS: u8 = 0x08;
const DEL: u8 = 0x7f;

pub struct TtyInner {
    flags: usize,
    line: Vec<u8>,
    lines: VecDeque<Vec<u8>>,
    raw_buf: VecDeque<u8>,
    eof: bool,
    fg_pgrp: usize,
}

impl TtyInner {
    fn new() -> Self {
        Self {
            flags: TTY_ECHO | TTY_ICANON | TTY_ISIG,
            line: Vec::new(),
            lines: VecDeque::new(),
            raw_buf: VecDeque::new(),
            eof: false,
            fg_pgrp: 0,
        }
    }

    /// Feed one byte into the line discipline. Returns a signal to raise
    /// (e.g. Ctrl-C) when the byte must not be delivered as input.
    fn push_byte(&mut self, ch: u8) -> Option<SignalFlags> {
        if self.flags & TTY_ISIG != 0 && ch == CTRL_C {
            UART.write(b'^');
            UART.write(b'C');
            UART.write(CR);
            UART.write(LF);
            self.line.clear();
            return Some(SignalFlags::SIGINT);
        }
        if self.flags & TTY_ICANON != 0 {
            match ch {
                CR | LF => {
                    if self.flags & TTY_ECHO != 0 {
                        UART.write(LF);
                    }
                    self.lines.push_back(core::mem::take(&mut self.line));
                }
                BS | DEL => {
                    if self.line.pop().is_some() && self.flags & TTY_ECHO != 0 {
                        UART.write(BS);
                        UART.write(b' ');
                        UART.write(BS);
                    }
                }
                CTRL_D => {
                    if self.line.is_empty() {
                        self.eof = true;
                    } else {
                        // POSIX-like: Ctrl-D on a partial line flushes what
                        // has been typed so far.
                        self.lines.push_back(core::mem::take(&mut self.line));
                    }
                }
                _ => {
                    if ch >= 0x20 {
                        if self.flags & TTY_ECHO != 0 {
                            UART.write(ch);
                        }
                        self.line.push(ch);
                    }
                }
            }
        } else {
            self.raw_buf.push_back(ch);
        }
        None
    }

    fn has_data(&self) -> bool {
        !self.lines.is_empty() || !self.raw_buf.is_empty() || self.eof
    }
}

pub struct Tty {
    inner: UPIntrFreeCell<TtyInner>,
    condvar: Condvar,
}

impl Tty {
    fn new() -> Self {
        Self {
            inner: unsafe { UPIntrFreeCell::new(TtyInner::new()) },
            condvar: Condvar::new(),
        }
    }

    /// Blocking read from the terminal.
    ///
    /// In canonical mode a whole line is returned (as much as fits into
    /// `user_buf`). In raw mode buffered bytes are returned immediately.
    pub fn read(&self, user_buf: UserBuffer) -> usize {
        let want = user_buf.len();
        if want == 0 {
            return 0;
        }
        let mut buf_iter = user_buf.into_iter();
        loop {
            let mut inner = self.inner.exclusive_access();
            // Only the foreground process group may read from the terminal;
            // background readers get EOF so they cannot steal interactive input.
            if inner.fg_pgrp != 0 && current_pgrp() != inner.fg_pgrp {
                return 0;
            }
            if inner.flags & TTY_ICANON != 0 {
                if let Some(mut line) = inner.lines.pop_front() {
                    let n = line.len().min(want);
                    for i in 0..n {
                        unsafe { *buf_iter.next().unwrap() = line[i] };
                    }
                    if n < line.len() {
                        inner.lines.push_front(line.split_off(n));
                    }
                    return n;
                }
                if inner.eof {
                    inner.eof = false;
                    return 0;
                }
            } else {
                let n = inner.raw_buf.len().min(want);
                if n > 0 {
                    for _ in 0..n {
                        unsafe { *buf_iter.next().unwrap() = inner.raw_buf.pop_front().unwrap() };
                    }
                    return n;
                }
            }
            let task_cx_ptr = self.condvar.wait_no_sched();
            drop(inner);
            schedule(task_cx_ptr);
        }
    }

    pub fn get_flags(&self) -> usize {
        self.inner.exclusive_access().flags
    }

    pub fn set_flags(&self, flags: usize) {
        self.inner.exclusive_access().flags = flags;
    }

    pub fn set_fg_pgrp(&self, pgrp: usize) {
        self.inner.exclusive_access().fg_pgrp = pgrp;
    }

    pub fn get_fg_pgrp(&self) -> usize {
        self.inner.exclusive_access().fg_pgrp
    }

    fn signal_fg(&self, signal: SignalFlags) {
        let pgrp = self.inner.exclusive_access().fg_pgrp;
        if pgrp == 0 {
            current_add_signal(signal);
        } else {
            signal_process_group(pgrp, signal);
        }
    }
}

lazy_static! {
    pub static ref TTY: Arc<Tty> = Arc::new(Tty::new());
}

/// Called from the UART IRQ path after raw bytes have been queued by the
/// serial driver. Drains the UART buffer into the line discipline and wakes
/// up blocked readers.
pub fn handle_uart_irq() {
    let mut signal: Option<SignalFlags> = None;
    let mut ready = false;
    TTY.inner.exclusive_session(|inner| {
        while let Some(ch) = UART.try_read() {
            if let Some(sig) = inner.push_byte(ch) {
                signal = Some(sig);
            }
        }
        ready = inner.has_data();
    });
    if ready {
        TTY.condvar.signal();
    }
    if let Some(sig) = signal {
        TTY.signal_fg(sig);
    }
}
