#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::cell::UnsafeCell;
use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{exit, fork, waitpid};

/// Global writable data.
///
/// The child modifies this value after fork. The parent's value must
/// remain unchanged.
static GLOBAL_VALUE: AtomicUsize = AtomicUsize::new(100);

/// UnsafeCell permits controlled mutation of static test data.
struct CowBuffer(UnsafeCell<[u8; 8192]>);

unsafe impl Sync for CowBuffer {}

/// Two writable pages.
///
/// The child writes one byte in each page, which should trigger two
/// independent COW faults.
static LARGE_BUFFER: CowBuffer = CowBuffer(UnsafeCell::new([1; 8192]));

fn read_buffer(index: usize) -> u8 {
    assert!(index < 8192);

    unsafe {
        let ptr = LARGE_BUFFER.0.get() as *const u8;
        read_volatile(ptr.add(index))
    }
}

fn write_buffer(index: usize, value: u8) {
    assert!(index < 8192);

    unsafe {
        let ptr = LARGE_BUFFER.0.get() as *mut u8;
        write_volatile(ptr.add(index), value);
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("COW fork isolation test start");

    GLOBAL_VALUE.store(100, Ordering::SeqCst);
    write_buffer(0, 1);
    write_buffer(4096, 1);

    let mut stack_value = 200usize;

    let pid = fork();

    if pid == 0 {
        println!("child: checking inherited values");

        assert_eq!(GLOBAL_VALUE.load(Ordering::SeqCst), 100);
        assert_eq!(read_buffer(0), 1);
        assert_eq!(read_buffer(4096), 1);

        unsafe {
            assert_eq!(read_volatile(addr_of_mut!(stack_value)), 200);
        }

        println!("child: writing private COW pages");

        GLOBAL_VALUE.store(300, Ordering::SeqCst);
        write_buffer(0, 2);
        write_buffer(4096, 3);

        unsafe {
            write_volatile(addr_of_mut!(stack_value), 400);
        }

        assert_eq!(GLOBAL_VALUE.load(Ordering::SeqCst), 300);
        assert_eq!(read_buffer(0), 2);
        assert_eq!(read_buffer(4096), 3);

        unsafe {
            assert_eq!(read_volatile(addr_of_mut!(stack_value)), 400);
        }

        println!("child: private writes passed");
        exit(0);
    }

    assert!(pid > 0);

    let mut exit_code = -1;

    let waited_pid = waitpid(pid as usize, &mut exit_code);

    assert_eq!(waited_pid, pid);
    assert_eq!(exit_code, 0);

    println!("parent: checking memory isolation");

    assert_eq!(GLOBAL_VALUE.load(Ordering::SeqCst), 100);
    assert_eq!(read_buffer(0), 1);
    assert_eq!(read_buffer(4096), 1);

    unsafe {
        assert_eq!(read_volatile(addr_of_mut!(stack_value)), 200);
    }

    println!("cow_fork_test passed!");
    0
}
