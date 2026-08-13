mod context;

use crate::config::TRAMPOLINE;
use crate::mm::VirtAddr;
use crate::syscall::syscall;
use crate::task::{
    SignalFlags, check_signals_of_current, current_add_signal, current_process, current_trap_cx,
    current_trap_cx_user_va, current_user_token, exit_current_and_run_next,
    on_current_task_timer_tick, should_preempt_current_task, suspend_current_and_run_next,
};
use crate::timer::{check_timer, set_next_trigger};
use core::arch::{asm, global_asm};
use riscv::interrupt::{Exception, Interrupt, Trap};
use riscv::register::{mtvec::TrapMode, scause, sie, sscratch, sstatus, stval, stvec};

global_asm!(include_str!("trap.S"));

pub fn init() {
    set_kernel_trap_entry();
}

fn set_kernel_trap_entry() {
    unsafe extern "C" {
        unsafe fn __alltraps();
        unsafe fn __alltraps_k();
    }
    let __alltraps_k_va =
        linker_symbol_addr!(__alltraps_k) - linker_symbol_addr!(__alltraps) + TRAMPOLINE;
    unsafe {
        stvec::write(stvec::Stvec::new(__alltraps_k_va, TrapMode::Direct));
        sscratch::write(linker_symbol_addr!(trap_from_kernel));
    }
}

fn set_user_trap_entry() {
    unsafe {
        stvec::write(stvec::Stvec::new(TRAMPOLINE as usize, TrapMode::Direct));
    }
}

pub fn enable_timer_interrupt() {
    unsafe {
        sie::set_stimer();
    }
}

fn enable_supervisor_interrupt() {
    unsafe {
        sstatus::set_sie();
    }
}

fn disable_supervisor_interrupt() {
    unsafe {
        sstatus::clear_sie();
    }
}

#[unsafe(no_mangle)]
pub fn trap_handler() -> ! {
    set_kernel_trap_entry();
    let scause = scause::read();
    let stval = stval::read();
    // println!("into {:?}", scause.cause());
    let trap: Trap<Interrupt, Exception> = match scause.cause().try_into() {
        Ok(trap) => trap,
        Err(_) => panic!(
            "Unsupported trap {:?}, stval = {:#x}!",
            scause.cause(),
            stval
        ),
    };
    match trap {
        Trap::Exception(Exception::UserEnvCall) => {
            // jump to next instruction anyway
            let mut cx = current_trap_cx();
            cx.sepc += 4;

            enable_supervisor_interrupt();

            // get system call return value
            let result = syscall(cx.x[17], [cx.x[10], cx.x[11], cx.x[12]]);
            // cx is changed during sys_exec, so we have to call it again
            cx = current_trap_cx();
            cx.x[10] = result as usize;
            if should_preempt_current_task() {
                suspend_current_and_run_next();
            }
        }
        Trap::Exception(Exception::StorePageFault) => {
            let handled = {
                let process = current_process();
                let mut process_inner = process.inner_exclusive_access();
                process_inner
                    .memory_set
                    .handle_cow_fault(VirtAddr::from(stval))
            };
            if !handled {
                current_add_signal(SignalFlags::SIGSEGV);
            }
        }
        Trap::Exception(Exception::StoreFault)
        | Trap::Exception(Exception::InstructionFault)
        | Trap::Exception(Exception::InstructionPageFault)
        | Trap::Exception(Exception::LoadFault)
        | Trap::Exception(Exception::LoadPageFault) => {
            /*
            println!(
                "[kernel] {:?} in application, bad addr = {:#x}, bad instruction = {:#x}, kernel killed it.",
                scause.cause(),
                stval,
                current_trap_cx().sepc,
            );
            */
            current_add_signal(SignalFlags::SIGSEGV);
        }
        Trap::Exception(Exception::IllegalInstruction) => {
            current_add_signal(SignalFlags::SIGILL);
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_trigger();
            check_timer();
            if on_current_task_timer_tick() {
                suspend_current_and_run_next();
            }
        }
        Trap::Interrupt(Interrupt::SupervisorExternal) => {
            crate::board::irq_handler();
        }
        _ => {
            panic!(
                "Unsupported trap {:?}, stval = {:#x}!",
                scause.cause(),
                stval
            );
        }
    }
    // check signals
    if let Some((errno, msg)) = check_signals_of_current() {
        println!("[kernel] {}", msg);
        exit_current_and_run_next(errno);
    }
    trap_return();
}

#[unsafe(no_mangle)]
/// set the new addr of __restore asm function in TRAMPOLINE page,
/// set the reg a0 = trap_cx_ptr, reg a1 = phy addr of usr page table,
/// finally, jump to new addr of __restore asm function
pub fn trap_return() -> ! {
    disable_supervisor_interrupt();
    set_user_trap_entry();
    let trap_cx_user_va = current_trap_cx_user_va();
    let user_satp = current_user_token();
    unsafe extern "C" {
        unsafe fn __alltraps();
        unsafe fn __restore();
    }
    let restore_va = linker_symbol_addr!(__restore) - linker_symbol_addr!(__alltraps) + TRAMPOLINE;
    //println!("before return");
    unsafe {
        asm!(
            "fence.i",
            "jr {restore_va}",
            restore_va = in(reg) restore_va,
            in("a0") trap_cx_user_va,
            in("a1") user_satp,
            options(noreturn)
        );
    }
}

#[unsafe(no_mangle)]
pub fn trap_from_kernel(_trap_cx: &TrapContext) {
    let scause = scause::read();
    let stval = stval::read();
    let trap: Trap<Interrupt, Exception> = match scause.cause().try_into() {
        Ok(trap) => trap,
        Err(_) => {
            panic!(
                "Unsupported trap from kernel: {:?}, stval = {:#x}!",
                scause.cause(),
                stval
            );
        }
    };
    match trap {
        Trap::Interrupt(Interrupt::SupervisorExternal) => {
            crate::board::irq_handler();
        }
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            set_next_trigger();
            check_timer();
            // The kernel is executing on behalf of the current user task.
            // Account this tick even though context switching is deferred
            // until execution returns to user mode.
            let _ = on_current_task_timer_tick();
            // do not schedule now
        }
        _ => {
            panic!(
                "Unsupported trap from kernel: {:?}, stval = {:#x}!",
                scause.cause(),
                stval
            );
        }
    }
}

pub use context::TrapContext;
