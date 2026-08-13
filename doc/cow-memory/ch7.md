@phao90016-jpg

# Chapter 7 进程间通信与 I/O 重定向

## 1. 本章目标

第七章在第六章文件系统和文件描述符机制的基础上，实现以下功能：

1. 基于 `File` trait 的匿名管道；
2. 父子进程之间的管道通信；
3. 命令行参数传递；
4. 标准输入、标准输出重定向；
5. Shell 多级管道；
6. 信号的发送、屏蔽和处理；
7. 用户信号处理函数；
8. `sigreturn` 执行现场恢复；
9. 将用户异常转换为 `SIGILL` 和 `SIGSEGV`。

本章的核心思想是复用已有抽象：

* 管道也实现 `File` trait；
* 标准输入、标准输出、普通文件和管道统一使用文件描述符；
* 信号处理通过修改 `TrapContext` 实现；
* `exec` 通过重建地址空间和用户栈传递参数。

## 2. 实验环境与分支

个人仓库：

```text
https://github.com/phao90016-jpg/rCore-Tutorial-v3-Tongji.git
```

官方仓库：

```text
https://github.com/rcore-os/rCore-Tutorial-v3.git
```

学习分支：

```text
learning/ch7
```

当前分支以官方 `upstream/ch7` 为代码基线，同时保留之前的学习文档。

## 3. ch7 总体功能

本章主要增加四组功能：

| 功能        | 关键文件                                                                 |
| --------- | -------------------------------------------------------------------- |
| 管道        | `os/src/fs/pipe.rs`                                                  |
| 管道系统调用    | `os/src/syscall/fs.rs`                                               |
| 命令行参数与重定向 | `user/src/bin/user_shell.rs`、`os/src/task/task.rs`                   |
| 信号        | `os/src/task/signal.rs`、`os/src/task/action.rs`、`os/src/task/mod.rs` |

总体调用关系如下：

```text
用户应用
→ 用户库
→ 系统调用
→ 内核文件描述符/进程管理
→ Pipe、exec 或 signal
```

## 4. 管道抽象

管道是一种具有有限缓冲区的先进先出字节队列，包含：

* 一个只读端；
* 一个只写端；
* 一个由读写端共享的环形缓冲区。

管道实现 `File` trait，因此可以放入进程的文件描述符表。

```rust
pub struct Pipe {
    readable: bool,
    writable: bool,
    buffer: Arc<UPSafeCell<PipeRingBuffer>>,
}
```

读端属性：

```text
readable = true
writable = false
```

写端属性：

```text
readable = false
writable = true
```

两个端点通过 `Arc` 共享同一个 `PipeRingBuffer`。

## 5. PipeRingBuffer

管道缓冲区大小为32字节：

```rust
const RING_BUFFER_SIZE: usize = 32;
```

核心结构：

```rust
pub struct PipeRingBuffer {
    arr: [u8; RING_BUFFER_SIZE],
    head: usize,
    tail: usize,
    status: RingBufferStatus,
    write_end: Option<Weak<Pipe>>,
}
```

字段含义：

| 字段          | 作用           |
| ----------- | ------------ |
| `arr`       | 保存管道数据       |
| `head`      | 下一个读取位置      |
| `tail`      | 下一个写入位置      |
| `status`    | 区分空、满和普通状态   |
| `write_end` | 判断管道写端是否全部关闭 |

环形缓冲区使用取模运算更新位置：

```rust
self.head = (self.head + 1) % RING_BUFFER_SIZE;
self.tail = (self.tail + 1) % RING_BUFFER_SIZE;
```

当 `head == tail` 时，缓冲区既可能为空，也可能已满，因此需要额外的状态：

```rust
enum RingBufferStatus {
    Full,
    Empty,
    Normal,
}
```

## 6. 管道写入

写入一个字节时：

1. 把字节写入 `tail`；
2. `tail` 循环向后移动；
3. 如果 `tail` 追上 `head`，状态变成 `Full`。

如果管道已满：

```rust
drop(ring_buffer);
suspend_current_and_run_next();
```

当前进程释放缓冲区独占访问并让出 CPU，等待读进程取走数据。

## 7. 管道读取与 EOF

读取一个字节时：

1. 从 `head` 位置读取；
2. `head` 循环向后移动；
3. 如果 `head` 追上 `tail`，状态变成 `Empty`。

缓冲区为空时有两种情况：

### 仍有写端

以后可能产生数据，读进程主动让出 CPU：

```rust
suspend_current_and_run_next();
```

### 所有写端均已关闭

以后不可能再有数据，读取返回当前已读取的字节数。若一个字节都没有读取，则返回0，表示 EOF。

## 8. Weak 与写端关闭判断

缓冲区使用：

```rust
write_end: Option<Weak<Pipe>>
```

记录写端。

使用 `Weak` 是为了防止形成引用环：

```text
Pipe写端
→ PipeRingBuffer
→ Pipe写端
```

判断所有写端是否关闭：

```rust
self.write_end
    .as_ref()
    .unwrap()
    .upgrade()
    .is_none()
```

如果 `upgrade()` 返回 `None`，说明不存在写端的强引用。

`fork()` 会克隆文件描述符中的 `Arc<Pipe>`，因此只有父子进程都关闭写端后，管道读端才能检测到 EOF。

## 9. sys_pipe

系统调用号：

```rust
const SYSCALL_PIPE: usize = 59;
```

系统调用过程：

```text
用户 pipe(&mut pipe_fd)
→ sys_pipe
→ make_pipe
→ 创建读端和写端
→ 分配两个文件描述符
→ 写入当前进程 fd_table
→ 把两个 fd 写回用户空间
```

核心代码：

```rust
let (pipe_read, pipe_write) = make_pipe();

let read_fd = inner.alloc_fd();
inner.fd_table[read_fd] = Some(pipe_read);

let write_fd = inner.alloc_fd();
inner.fd_table[write_fd] = Some(pipe_write);
```

新进程初始已经使用 `fd 0`、`fd 1` 和 `fd 2`，因此第一次创建管道通常得到：

```text
pipe_fd[0] = 3
pipe_fd[1] = 4
```

## 10. 文件描述符分配与关闭

文件描述符表类型：

```rust
Vec<Option<Arc<dyn File + Send + Sync>>>
```

`alloc_fd()` 优先使用值为 `None` 的最小位置，没有空位时扩展文件描述符表。

`sys_close()` 使用：

```rust
inner.fd_table[fd].take();
```

清空文件描述符表项，并减少对应 `Arc` 的强引用计数。

## 11. fork 与管道共享

`fork()` 克隆父进程文件描述符表：

```rust
new_fd_table.push(Some(file.clone()));
```

这里克隆的是 `Arc`，不会复制底层文件对象。

因此父子进程中的文件描述符仍然指向同一个管道端点和同一个环形缓冲区。

管道通信时，每个进程必须关闭无用端点，否则多余的写端引用会导致读端无法检测 EOF。

## 12. sys_dup 与重定向

`sys_dup(fd)` 创建新的文件描述符，并让它与原 fd 指向同一个文件对象。

Shell 利用 `close()` 和 `dup()` 实现重定向：

```rust
close(0);
assert_eq!(dup(input_fd), 0);
```

因为 `alloc_fd()` 选择最小空位，所以关闭 `fd 0` 后，`dup()` 返回的新描述符就是0。

类似地：

```rust
close(1);
assert_eq!(dup(output_fd), 1);
```

可以把标准输出重定向到文件或管道。

## 13. Shell 管道

对于命令：

```text
cat filea | count_lines
```

Shell 创建两个子进程和一条管道：

```text
cat 的 fd 1
→ 管道写端
→ PipeRingBuffer
→ 管道读端
→ count_lines 的 fd 0
```

N个命令需要N−1条管道。

Shell 为每个命令调用 `fork()`，在子进程中配置文件描述符，然后执行 `exec()`。

父 Shell 和所有子进程都必须关闭不再使用的原始管道端点，否则可能无法产生 EOF。

## 14. 命令行参数

Shell 把命令拆分成字符串，并在每个字符串末尾添加 `\0`。

参数指针数组末尾添加空指针：

```rust
args_addr.push(core::ptr::null::<u8>());
```

`sys_exec()` 在旧地址空间被替换前，使用旧页表读取所有参数，保存到内核 `Vec<String>` 中。

随后读取应用 ELF，并调用：

```rust
task.exec(elf_data, args_vec);
```

## 15. exec 参数栈

`TaskControlBlock::exec()` 创建新地址空间后，将以下内容写入新用户栈：

1. 参数字符串；
2. 指向各字符串的 `argv[]`；
3. 末尾空指针。

随后设置：

```text
a0 = argc
a1 = argv_base
```

RISC-V 中：

```text
x10 = a0
x11 = a1
```

新应用从：

```rust
_start(argc, argv)
```

开始执行，用户库将 `argv[]` 恢复成 `&[&str]`，再调用应用的 `main()`。

## 16. 信号表示

信号使用32位位图：

```rust
pub struct SignalFlags: u32
```

常用信号包括：

| 信号        | 编号 | 作用       |
| --------- | -: | -------- |
| `SIGILL`  |  4 | 非法指令     |
| `SIGKILL` |  9 | 强制终止     |
| `SIGUSR1` | 10 | 用户自定义信号1 |
| `SIGSEGV` | 11 | 非法内存访问   |
| `SIGCONT` | 18 | 恢复进程     |
| `SIGSTOP` | 19 | 暂停进程     |

## 17. 进程信号状态

`TaskControlBlockInner` 增加：

```rust
signals: SignalFlags,
signal_mask: SignalFlags,
handling_sig: isize,
signal_actions: SignalActions,
killed: bool,
frozen: bool,
trap_ctx_backup: Option<TrapContext>,
```

其中：

* `signals` 保存待处理信号；
* `signal_mask` 保存全局屏蔽集合；
* `handling_sig` 保存当前处理的信号；
* `signal_actions` 保存用户注册的处理动作；
* `trap_ctx_backup` 保存信号发生前的执行现场。

## 18. 信号系统调用

本章实现：

| 系统调用          |  ID | 作用        |
| ------------- | --: | --------- |
| `kill`        | 129 | 向进程发送信号   |
| `sigaction`   | 134 | 注册信号处理函数  |
| `sigprocmask` | 135 | 设置屏蔽信号    |
| `sigreturn`   | 139 | 恢复信号前执行现场 |

`sys_kill()` 只把信号加入目标进程的 pending signals，不立即执行处理函数。

## 19. 信号处理流程

在 Trap 返回用户态前调用：

```rust
handle_signals();
```

处理流程：

```text
检查pending signals
→ 排除被signal_mask屏蔽的信号
→ 判断内核信号或用户信号
→ 执行默认动作或用户handler
```

内核直接处理：

```text
SIGKILL
SIGSTOP
SIGCONT
SIGDEF
```

`SIGSTOP` 设置：

```rust
frozen = true
```

`SIGCONT` 设置：

```rust
frozen = false
```

## 20. 用户信号处理函数

调用用户处理函数前，内核：

1. 保存当前 `TrapContext`；
2. 记录正在处理的信号；
3. 清除 pending bit；
4. 把 `sepc` 改成 handler 地址；
5. 把 `a0` 设置为信号编号。

返回用户态后，程序从用户 handler 开始执行。

## 21. sigreturn

用户 handler 不是通过普通函数调用指令进入的，因此不能使用普通 `return` 恢复原程序。

handler 必须调用：

```rust
sigreturn();
```

内核从：

```rust
trap_ctx_backup
```

恢复完整 TrapContext，使程序从信号发生前的位置继续执行。

## 22. 异常转换为信号

以下用户异常转换成 `SIGSEGV`：

```text
StoreFault
StorePageFault
LoadFault
LoadPageFault
InstructionFault
InstructionPageFault
```

非法指令转换为：

```text
SIGILL
```

没有用户处理函数时，内核根据 `check_error()` 返回对应负退出码：

```text
SIGILL  → -4
SIGKILL → -9
SIGSEGV → -11
```

## 23. 测试结果

### pipetest

```text
Read OK, child process exited!
pipetest passed!
```

基础管道通信通过。

### pipe_large_test

```text
sum = 671653(parent)
sum = 671653(child)
Child process exited!
pipe_large_test passed!
```

3000字节大容量管道传输通过，父子进程校验和一致。

### cmdline_args

```text
argc = 4
argv[0] = cmdline_args
argv[1] = hello
argv[2] = rcore
argv[3] = ch7
```

参数传递通过。

### 信号测试

```text
ALL TESTS PASSED
```

信号注册、发送、屏蔽、恢复、停止和继续均通过。

### 管道与重定向

```text
cat filea | count_lines
1
```

```text
count_lines < filea
1
```

```text
cmdline_args hello rcore ch7 > argsout
cat argsout
```

输出文件内容正确。

### usertests

```text
21 of succeeded apps, 4 of failed apps run correctly.
Usertests passed!
```

其中 `store_fault` 按预期收到 `SIGSEGV`，以 `-11` 退出。

## 24. 本章总结

第七章建立在文件系统、文件描述符、进程和虚拟内存机制之上。

管道通过实现 `File` trait 接入统一文件抽象，使用共享环形缓冲区实现进程间字节流通信。`fork()` 通过克隆 `Arc` 共享文件对象，Shell 使用 `close()` 和 `dup()` 修改标准输入输出，实现管道和文件重定向。

`exec()` 在替换地址空间前复制参数，再将字符串和 `argv[]` 写入新用户栈，通过 `a0/a1` 传递给新程序。

信号机制使用位图保存待处理信号，通过 Trap 返回路径检查信号。用户信号处理通过备份和修改 `TrapContext` 实现，并使用 `sigreturn()` 恢复原执行现场。

本章使 rCore 具备了基本的进程间通信、命令组合、I/O 重定向和异步事件处理能力。
