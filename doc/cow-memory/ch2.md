@phao90016-jpg

# Chapter 2：批处理系统与用户态 Trap

## 1. 实验目标

本章在第一章最小内核的基础上，实现并理解：

- 用户程序的编译与链接；
- 多个应用程序的加载与依次执行；
- 用户态与内核态之间的切换；
- `ecall` 系统调用；
- PageFault 和 IllegalInstruction 异常处理；
- Trap 上下文保存与恢复；
- 应用退出后的批处理调度。

## 2. 实验环境

- 操作系统：WSL2 Ubuntu
- Rust：rustc 1.97.1
- QEMU：8.2.2
- 目标架构：RISC-V 64
- 官方基线：`upstream/ch2`
- 个人分支：`learning/ch2`

运行命令：

```bash
cd os
make run
```

## 3. 第二章相对第一章的变化

与第一章相比，第二章新增了用户程序和批处理系统：

```text
os/build.rs
os/src/batch.rs
os/src/syscall/
os/src/trap/
user/
```

- `user/` 保存用户态程序；
- `os/build.rs` 负责构建并嵌入用户程序；
- `batch.rs` 负责加载和运行应用；
- `syscall/` 负责处理系统调用；
- `trap/` 负责处理异常和系统调用入口。

## 4. 实验运行结果

本章共加载了 5 个用户程序：

```text
app_0: 00hello_world
app_1: 01store_fault
app_2: 02power
app_3: 03priv_inst
app_4: 04priv_csr
```

### 4.1 Hello World

```text
Hello, world!
[kernel] Application exited with code 0
```

用户程序能够正常运行，并通过系统调用向标准输出打印字符串。

### 4.2 Store Fault

```text
Into Test store_fault, we will insert an invalid store operation...
Kernel should kill this application!
[kernel] PageFault in application, kernel killed it.
```

程序执行非法内存写入，触发 PageFault。内核捕获异常并终止当前应用，然后加载下一个应用。

### 4.3 Power

```text
3^10000=5079(MOD 10007)
...
Test power OK!
[kernel] Application exited with code 0
```

普通用户程序能够完成计算，并通过 `sys_exit` 正常退出。

### 4.4 Privileged Instruction

```text
Try to execute privileged instruction in U Mode
Kernel should kill this application!
[kernel] IllegalInstruction in application, kernel killed it.
```

用户态执行特权指令，触发 IllegalInstruction 异常，被内核终止。

### 4.5 Privileged CSR

```text
Try to access privileged CSR in U Mode
Kernel should kill this application!
[kernel] IllegalInstruction in application, kernel killed it.
```

用户态访问特权 CSR，同样触发非法指令异常。

所有应用完成后输出：

```text
All applications completed!
```

## 5. 用户程序结构

用户程序使用：

```rust
#![no_std]
#![no_main]
```

例如：

```rust
#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("Hello, world!");
    0
}
```

用户程序没有使用 Rust 标准库，而是依赖 `user_lib` 提供的运行环境。

用户程序的大致启动路径：

```text
_start
→ 清零用户程序的 .bss
→ 调用 main
→ main 返回退出码
→ 调用 sys_exit
```

## 6. `sys_write` 系统调用

用户态定义：

```rust
const SYSCALL_WRITE: usize = 64;

pub fn sys_write(fd: usize, buffer: &[u8]) -> isize {
    syscall(
        SYSCALL_WRITE,
        [fd, buffer.as_ptr() as usize, buffer.len()]
    )
}
```

系统调用参数通过 RISC-V 寄存器传递：

| 寄存器 | 作用 |
|---|---|
| `a0` | 第一个参数 |
| `a1` | 第二个参数 |
| `a2` | 第三个参数 |
| `a7` | 系统调用编号 |

`sys_write` 的参数为：

```text
a0 = 文件描述符 fd
a1 = 缓冲区地址
a2 = 缓冲区长度
a7 = 64
```

用户态通过汇编执行：

```asm
ecall
```

进入内核。内核根据系统调用编号进行分发：

```rust
match syscall_id {
    SYSCALL_WRITE => sys_write(args[0], args[1] as *const u8, args[2]),
    SYSCALL_EXIT => sys_exit(args[0] as i32),
    _ => panic!("Unsupported syscall_id: {}", syscall_id),
}
```

当前只支持标准输出：

```rust
const FD_STDOUT: usize = 1;
```

系统调用路径为：

```text
println!
→ sys_write
→ syscall(64, ...)
→ ecall
→ UserEnvCall
→ trap_handler
→ sys_write
→ print!
```

本章还没有虚拟内存，用户程序与内核暂时共享同一地址空间，因此内核可以直接访问用户指针。后续建立地址空间后，需要重新设计用户指针检查和数据拷贝。

## 7. `sys_exit` 系统调用

用户态定义：

```rust
const SYSCALL_EXIT: usize = 93;

pub fn sys_exit(exit_code: i32) -> isize {
    syscall(SYSCALL_EXIT, [exit_code as usize, 0, 0])
}
```

内核实现：

```rust
pub fn sys_exit(exit_code: i32) -> ! {
    println!("[kernel] Application exited with code {}", exit_code);
    run_next_app()
}
```

`sys_exit` 的返回类型是 `!`，表示它不会返回到原来的用户程序。

```text
用户程序调用 sys_exit
→ 内核打印退出码
→ run_next_app
→ 加载下一个应用
→ 构造下一个应用的 TrapContext
→ __restore
→ 进入下一个用户程序
```

## 8. Trap 初始化

在 `os/src/trap/mod.rs` 中：

```rust
pub fn init() {
    unsafe extern "C" {
        safe fn __alltraps();
    }

    unsafe {
        stvec::write(stvec::Stvec::new(
            linker_symbol_addr!(__alltraps),
            TrapMode::Direct,
        ));
    }
}
```

`stvec` 是 RISC-V 的 Trap 向量寄存器，所有 Trap 都进入：

```text
__alltraps
```

之后由 `trap_handler` 根据 `scause` 区分具体类型。

`TrapMode::Direct` 表示所有 Trap 跳转到同一个固定入口地址。

## 9. Trap 上下文

```rust
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: Sstatus,
    pub sepc: usize,
}
```

TrapContext 保存：

- 32 个通用寄存器；
- `sstatus`；
- `sepc`。

初始化用户程序上下文时：

```rust
sstatus.set_spp(SPP::User);
sepc: entry;
cx.set_sp(sp);
```

其中：

- `sepc` 是用户程序入口地址；
- `x[2]` 是用户栈指针；
- `sstatus.SPP` 设置为 User，保证 `sret` 后返回用户态。

## 10. `__alltraps`：保存用户现场

用户程序执行 `ecall` 或发生异常后，CPU 跳转到：

```asm
__alltraps:
```

第一条关键指令：

```asm
csrrw sp, sscratch, sp
```

它交换用户栈和内核栈：

```text
进入 Trap 前：
sp       → 用户栈
sscratch → 内核栈

交换之后：
sp       → 内核栈
sscratch → 用户栈
```

然后在内核栈上分配 TrapContext：

```asm
addi sp, sp, -34*8
```

保存通用寄存器、用户栈指针、`sstatus` 和 `sepc`，最后：

```asm
mv a0, sp
call trap_handler
```

将 TrapContext 地址作为参数传给 Rust 函数。

## 11. `trap_handler`

系统调用处理：

```rust
Trap::Exception(Exception::UserEnvCall) => {
    cx.sepc += 4;
    cx.x[10] = syscall(
        cx.x[17],
        [cx.x[10], cx.x[11], cx.x[12]]
    ) as usize;
}
```

其中：

- `cx.x[17]` 是系统调用编号；
- `cx.x[10]`、`cx.x[11]`、`cx.x[12]` 是参数；
- 返回值写回 `cx.x[10]`。

`sepc += 4` 很重要，因为 `ecall` 是一条 4 字节指令。如果不增加 `sepc`，返回用户态后会再次执行同一条 `ecall`。

异常处理：

```rust
Trap::Exception(Exception::StoreFault)
| Trap::Exception(Exception::StorePageFault) => {
    println!("[kernel] PageFault in application, kernel killed it.");
    run_next_app();
}
```

```rust
Trap::Exception(Exception::IllegalInstruction) => {
    println!("[kernel] IllegalInstruction in application, kernel killed it.");
    run_next_app();
}
```

三种情况的区别：

```text
系统调用
→ 处理完成
→ 返回当前用户程序

应用退出
→ 切换到下一个用户程序

应用异常
→ 终止当前用户程序
→ 切换到下一个用户程序
```

## 12. `__restore`：恢复用户现场

`trap_handler` 返回后进入：

```asm
__restore:
```

恢复：

```asm
sstatus
sepc
sscratch
```

然后恢复通用寄存器，释放 TrapContext：

```asm
addi sp, sp, 34*8
```

最后：

```asm
csrrw sp, sscratch, sp
sret
```

执行 `sret` 后：

- 恢复用户态；
- 从 `sepc` 指向的位置继续执行；
- 使用恢复后的用户寄存器和用户栈。

## 13. 应用加载

批处理系统将用户程序加载到：

```rust
const APP_BASE_ADDRESS: usize = 0x80400000;
```

加载过程：

```text
读取应用起始地址
→ 清空应用区域
→ 将应用复制到 0x80400000
→ 执行 fence.i
→ 构造 TrapContext
→ __restore
→ 进入用户程序
```

`fence.i` 用于保证处理器取指时能够看到刚刚写入的应用代码。

本章还没有建立虚拟地址空间，用户程序和内核共享地址空间。第四章引入页表后，用户程序将拥有独立地址空间。

## 14. 第二章完整执行流程

```text
rust_main
→ trap::init
→ batch::init
→ 读取应用数量和起始地址
→ 加载 app_0
→ 构造 TrapContext
→ __restore
→ sret
→ 进入用户态

用户程序执行
→ println!
→ sys_write
→ ecall
→ __alltraps
→ trap_handler
→ sys_write
→ __restore
→ sret
→ 返回用户态

用户程序退出
→ sys_exit
→ run_next_app
→ 加载下一个应用

用户程序发生异常
→ __alltraps
→ trap_handler
→ PageFault/IllegalInstruction
→ 杀死当前应用
→ run_next_app

所有应用完成
→ shutdown(false)
```

## 15. 本章遇到的问题

### 15.1 GitHub 推送认证

GitHub HTTPS 推送不能使用账户密码。通过 GitHub CLI 的设备授权完成登录后，成功推送了 `learning/ch2` 分支。

### 15.2 QEMU 关机阶段返回错误

所有应用完成后输出：

```text
All applications completed!
```

随后出现：

```text
[rustsbi-panic] ...
make: *** Error 255
```

根据内核源码，所有应用完成时调用的是：

```rust
shutdown(false);
```

因此应用执行和异常处理本身是成功的。错误发生在最后的 RustSBI/QEMU 关机阶段，暂时记录为当前 QEMU 8.2.2 与教学代码关机路径的兼容现象。

## 16. 本章总结

第二章实现了一个最小批处理操作系统。

本章的关键机制是：

1. 内核将多个应用嵌入自身；
2. 批处理系统逐个加载应用；
3. `__restore` 将控制权交给用户程序；
4. 用户程序通过 `ecall` 请求内核服务；
5. `__alltraps` 保存用户现场；
6. `trap_handler` 处理系统调用和异常；
7. `__restore` 恢复用户现场；
8. 应用退出或异常后切换到下一个应用。

核心执行路径：

```text
用户程序
→ ecall
→ __alltraps
→ trap_handler
→ syscall
→ __restore
→ sret
→ 用户程序
```
