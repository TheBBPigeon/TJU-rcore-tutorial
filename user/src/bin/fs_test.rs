#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{OpenFlags, chdir, close, getdents, mkdir, open, unlink};

fn list_directory(path: &str) -> bool {
    let mut buf = [0u8; 4096];
    let result = getdents(path, &mut buf);
    if result < 0 {
        println!("  FAILED: cannot list '{}'", path);
        return false;
    }
    let len = result as usize;
    if len == 0 {
        println!("  (empty)");
    } else {
        let listing = core::str::from_utf8(&buf[..len]).unwrap_or("");
        for line in listing.lines() {
            if !line.is_empty() {
                println!("  {}", line);
            }
        }
    }
    true
}

#[unsafe(no_mangle)]
pub fn main(_argc: usize, _argv: &[&str]) -> i32 {
    println!("=== fs_test: File System Test Suite ===");

    // 1. ls /
    println!("--- 1. ls / ---");
    list_directory("/");

    // 2. touch hello.txt
    println!("--- 2. touch hello.txt ---");
    let fd = open("hello.txt", OpenFlags::CREATE | OpenFlags::WRONLY);
    if fd >= 0 {
        println!("  created hello.txt");
        close(fd as usize);
    } else {
        println!("  FAILED: open returned {}", fd);
    }

    // 3. ls /
    println!("--- 3. ls / ---");
    list_directory("/");

    // 4. mkdir testdir
    println!("--- 4. mkdir testdir ---");
    if mkdir("testdir") == 0 {
        println!("  created directory testdir");
    } else {
        println!("  FAILED: mkdir returned -1");
    }

    // 5. ls /
    println!("--- 5. ls / ---");
    list_directory("/");

    // 6. cd testdir
    println!("--- 6. cd testdir ---");
    if chdir("testdir") == 0 {
        println!("  changed to testdir");
    } else {
        println!("  FAILED: chdir returned -1");
    }

    // 7. ls
    println!("--- 7. ls ---");
    list_directory(".");

    // 8. touch inner.txt
    println!("--- 8. touch inner.txt ---");
    let fd = open("inner.txt", OpenFlags::CREATE | OpenFlags::WRONLY);
    if fd >= 0 {
        println!("  created inner.txt");
        close(fd as usize);
    } else {
        println!("  FAILED: open returned {}", fd);
    }

    // 9. ls
    println!("--- 9. ls ---");
    list_directory(".");

    // 10. mkdir nested
    println!("--- 10. mkdir nested ---");
    if mkdir("nested") == 0 {
        println!("  created directory nested");
    } else {
        println!("  FAILED: mkdir returned -1");
    }

    // 11. ls
    println!("--- 11. ls ---");
    list_directory(".");

    // 12. rm inner.txt
    println!("--- 12. rm inner.txt ---");
    if unlink("inner.txt") == 0 {
        println!("  removed inner.txt");
    } else {
        println!("  FAILED: unlink returned -1");
    }

    // 13. ls
    println!("--- 13. ls ---");
    list_directory(".");

    // 14. rm nested
    println!("--- 14. rm nested ---");
    if unlink("nested") == 0 {
        println!("  removed nested");
    } else {
        println!("  FAILED: unlink returned -1");
    }

    // 15. ls
    println!("--- 15. ls ---");
    list_directory(".");

    // 16. cd /
    println!("--- 16. cd / ---");
    if chdir("/") == 0 {
        println!("  changed to /");
    } else {
        println!("  FAILED: chdir returned -1");
    }

    // 17. ls /
    println!("--- 17. ls / ---");
    list_directory("/");

    // 18. rm hello.txt
    println!("--- 18. rm hello.txt ---");
    if unlink("hello.txt") == 0 {
        println!("  removed hello.txt");
    } else {
        println!("  FAILED: unlink returned -1");
    }

    // 19. rm testdir
    println!("--- 19. rm testdir ---");
    if unlink("testdir") == 0 {
        println!("  removed testdir");
    } else {
        println!("  FAILED: unlink returned -1");
    }

    // 20. ls /
    println!("--- 20. ls / ---");
    list_directory("/");

    println!("=== fs_test: ALL TESTS COMPLETED ===");
    0
}
