#![no_std]
#![no_main]

macro_rules! linker_symbol_addr {
    ($symbol:path) => {
        ($symbol as *const ()).addr()
    };
}

//use crate::drivers::{GPU_DEVICE, KEYBOARD_DEVICE, MOUSE_DEVICE, INPUT_CONDVAR};
use crate::drivers::{GPU_DEVICE, KEYBOARD_DEVICE, MOUSE_DEVICE};
extern crate alloc;

#[macro_use]
extern crate bitflags;

use log::*;

#[path = "boards/qemu.rs"]
mod board;

#[macro_use]
mod console;
mod config;
mod device_tree;
mod drivers;
mod fs;
mod lang_items;
mod logging;
mod mm;
mod net;
mod sbi;
mod sync;
mod syscall;
mod task;
mod timer;
mod trap;

use crate::drivers::chardev::CharDevice;
use crate::drivers::chardev::UART;

core::arch::global_asm!(include_str!("entry.asm"));

fn clear_bss() {
    unsafe extern "C" {
        safe fn sbss();
        safe fn ebss();
    }
    unsafe {
        core::slice::from_raw_parts_mut(
            linker_symbol_addr!(sbss) as *mut u8,
            linker_symbol_addr!(ebss) - linker_symbol_addr!(sbss),
        )
        .fill(0);
    }
}

use lazy_static::*;
use sync::UPIntrFreeCell;

lazy_static! {
    pub static ref DEV_NON_BLOCKING_ACCESS: UPIntrFreeCell<bool> =
        unsafe { UPIntrFreeCell::new(false) };
}

#[unsafe(no_mangle)]
pub fn rust_main(_hart_id: usize, dtb_pa: usize) -> ! {
    clear_bss();
    mm::init();
    UART.init();
    logging::init();
    device_tree::init(dtb_pa);
    info!("KERN: init gpu");
    let _gpu = GPU_DEVICE.clone();
    info!("KERN: init keyboard");
    let _keyboard = KEYBOARD_DEVICE.clone();
    info!("KERN: init mouse");
    let _mouse = MOUSE_DEVICE.clone();
    info!("KERN: init trap");
    trap::init();
    trap::enable_timer_interrupt();
    timer::set_next_trigger();
    board::device_init();
    fs::list_apps();
    println!("[kernel] scheduler: {}", task::scheduler_name());
    task::add_initproc();
    *DEV_NON_BLOCKING_ACCESS.exclusive_access() = true;
    task::run_tasks();
    panic!("Unreachable in rust_main!");
}
