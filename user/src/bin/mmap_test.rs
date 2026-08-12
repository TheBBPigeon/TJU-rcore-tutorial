#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::ptr::{read_volatile, write_volatile};
use user_lib::{PROT_READ, PROT_WRITE, mmap, munmap};

const PAGE_SIZE: usize = 4096;

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("mmap_test: start");

    // start=0 means that the kernel chooses an available address.
    let address = mmap(0, 2 * PAGE_SIZE, PROT_READ | PROT_WRITE);

    assert!(address > 0);
    assert_eq!(address as usize % PAGE_SIZE, 0);

    println!("mmap_test: mapped two pages at {:#x}", address);

    let pointer = address as *mut u8;

    // Write one byte to each page.
    unsafe {
        write_volatile(pointer, 0x12);

        write_volatile(pointer.add(PAGE_SIZE), 0x34);

        assert_eq!(read_volatile(pointer), 0x12);

        assert_eq!(read_volatile(pointer.add(PAGE_SIZE)), 0x34);
    }

    println!("mmap_test: read/write passed");

    // Mapping an overlapping page at the same address must fail.
    assert_eq!(mmap(address as usize, PAGE_SIZE, PROT_READ,), -1);

    println!("mmap_test: overlap rejection passed");

    /*
     * The first implementation of munmap requires the range to match
     * the complete MapArea, so unmapping only the first page fails.
     */
    assert_eq!(munmap(address as usize, PAGE_SIZE,), -1);

    println!("mmap_test: partial munmap rejection passed");

    assert_eq!(munmap(address as usize, 2 * PAGE_SIZE,), 0);

    println!("mmap_test: full munmap passed");

    /*
     * Once the previous mapping has been removed, the exact virtual
     * address should be available again.
     */
    let reused = mmap(address as usize, 2 * PAGE_SIZE, PROT_READ | PROT_WRITE);

    assert_eq!(reused, address);

    println!("mmap_test: address reuse passed");

    assert_eq!(munmap(reused as usize, 2 * PAGE_SIZE,), 0);

    println!("mmap_test passed!");
    0
}
