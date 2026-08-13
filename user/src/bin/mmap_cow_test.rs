#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::mem::size_of;
use core::ptr::{read_volatile, write_volatile};
use user_lib::{PROT_READ, PROT_WRITE, exit, fork, mmap, munmap, waitpid};

const PAGE_SIZE: usize = 4096;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("mmap_cow_test: start");

    let address = mmap(0, 2 * PAGE_SIZE, PROT_READ | PROT_WRITE);

    assert!(address > 0);

    let first_page = address as *mut usize;

    let second_page = unsafe { first_page.add(PAGE_SIZE / size_of::<usize>()) };

    // Parent initializes both mapped pages before fork.
    unsafe {
        write_volatile(first_page, 100);
        write_volatile(second_page, 200);
    }

    println!("mmap_cow_test: parent mapped at {:#x}", address);

    let pid = fork();

    if pid == 0 {
        println!("mmap_cow_test: child checks inherited data");

        unsafe {
            assert_eq!(read_volatile(first_page), 100);

            assert_eq!(read_volatile(second_page), 200);
        }

        /*
         * These writes should cause two StorePageFault exceptions.
         * Each fault allocates a private physical page for the child.
         */
        unsafe {
            write_volatile(first_page, 300);
            write_volatile(second_page, 400);

            assert_eq!(read_volatile(first_page), 300);

            assert_eq!(read_volatile(second_page), 400);
        }

        println!("mmap_cow_test: child private writes passed");

        assert_eq!(munmap(address as usize, 2 * PAGE_SIZE,), 0);

        exit(0);
    }

    assert!(pid > 0);

    let mut exit_code = -1;

    assert_eq!(waitpid(pid as usize, &mut exit_code,), pid);

    assert_eq!(exit_code, 0);

    /*
     * The child modified its private copies, so the parent must still
     * observe the original values.
     */
    unsafe {
        assert_eq!(read_volatile(first_page), 100);

        assert_eq!(read_volatile(second_page), 200);
    }

    println!("mmap_cow_test: parent isolation passed");

    assert_eq!(munmap(address as usize, 2 * PAGE_SIZE,), 0);

    println!("mmap_cow_test passed!");
    0
}
