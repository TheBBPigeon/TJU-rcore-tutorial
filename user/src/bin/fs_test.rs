#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use user_lib::{OpenFlags, chdir, close, getdents, mkdir, open, unlink};

/// Print a heading line for each test step.
fn step(description: &str) {
    println!("");
    println!("--- {} ---", description);
}

/// Run `ls` equivalent: call getdents on the given path and print result.
fn list_directory(path: &str) -> bool {
    let mut buf = [0u8; 4096];
    let result = getdents(path, &mut buf);
    if result < 0 {
        println!("  FAILED: getdents('{}') returned {}", path, result);
        return false;
    }
    let len = result as usize;
    if len == 0 {
        println!("  (empty)");
    } else {
        let listing = core::str::from_utf8(&buf[..len]).unwrap_or("<invalid utf8>");
        // Print each line with indent
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
    println!("");
    println!("========================================");
    println!("  fs_test: File System Test Suite");
    println!("========================================");

    // Step 1: ls root
    step("1. ls / (list root directory)");
    list_directory("/");

    // Step 2: touch hello.txt
    step("2. touch hello.txt");
    let fd = open("hello.txt", OpenFlags::CREATE | OpenFlags::WRONLY);
    if fd >= 0 {
        println!("  ok: created hello.txt, fd={}", fd);
        close(fd as usize);
    } else {
        println!("  FAILED: open returned {}", fd);
    }

    // Step 3: ls (verify hello.txt)
    step("3. ls / (verify hello.txt)");
    list_directory("/");

    // Step 4: mkdir testdir
    step("4. mkdir testdir");
    let result = mkdir("testdir");
    if result == 0 {
        println!("  ok: created testdir");
    } else {
        println!("  FAILED: mkdir returned {}", result);
    }

    // Step 5: ls (verify testdir)
    step("5. ls / (verify testdir)");
    list_directory("/");

    // Step 6: cd testdir
    step("6. cd testdir");
    let result = chdir("testdir");
    if result == 0 {
        println!("  ok: changed to testdir");
    } else {
        println!("  FAILED: chdir returned {}", result);
    }

    // Step 7: ls (empty dir)
    step("7. ls (should be empty)");
    list_directory(".");

    // Step 8: touch inner.txt
    step("8. touch inner.txt");
    let fd = open("inner.txt", OpenFlags::CREATE | OpenFlags::WRONLY);
    if fd >= 0 {
        println!("  ok: created inner.txt, fd={}", fd);
        close(fd as usize);
    } else {
        println!("  FAILED: open returned {}", fd);
    }

    // Step 9: ls (verify inner.txt)
    step("9. ls (verify inner.txt)");
    list_directory(".");

    // Step 10: mkdir nested
    step("10. mkdir nested");
    let result = mkdir("nested");
    if result == 0 {
        println!("  ok: created nested");
    } else {
        println!("  FAILED: mkdir returned {}", result);
    }

    // Step 11: ls (verify nested)
    step("11. ls (verify nested)");
    list_directory(".");

    // Step 12: rm inner.txt
    step("12. rm inner.txt");
    let result = unlink("inner.txt");
    if result == 0 {
        println!("  ok: removed inner.txt");
    } else {
        println!("  FAILED: unlink returned {}", result);
    }

    // Step 13: ls (verify inner.txt gone)
    step("13. ls (verify inner.txt gone)");
    list_directory(".");

    // Step 14: rm nested
    step("14. rm nested");
    let result = unlink("nested");
    if result == 0 {
        println!("  ok: removed nested");
    } else {
        println!("  FAILED: unlink returned {}", result);
    }

    // Step 15: ls (verify nested gone)
    step("15. ls (verify nested gone)");
    list_directory(".");

    // Step 16: cd /
    step("16. cd /");
    let result = chdir("/");
    if result == 0 {
        println!("  ok: changed to /");
    } else {
        println!("  FAILED: chdir returned {}", result);
    }

    // Step 17: ls (verify back to root)
    step("17. ls / (verify back to root)");
    list_directory("/");

    // Step 18: rm hello.txt
    step("18. rm hello.txt");
    let result = unlink("hello.txt");
    if result == 0 {
        println!("  ok: removed hello.txt");
    } else {
        println!("  FAILED: unlink returned {}", result);
    }

    // Step 19: rm testdir
    step("19. rm testdir");
    let result = unlink("testdir");
    if result == 0 {
        println!("  ok: removed testdir");
    } else {
        println!("  FAILED: unlink returned {}", result);
    }

    // Step 20: ls (clean)
    step("20. ls / (should be clean)");
    list_directory("/");

    println!("");
    println!("========================================");
    println!("  fs_test: ALL TESTS COMPLETED");
    println!("========================================");
    0
}
