# Chapter 5：进程管理

## 1. 实验目标

本章在第四章独立地址空间的基础上实现进程管理，主要目标包括：

- 动态分配和回收 PID；
- 为每个进程创建独立内核栈；
- 使用进程控制块管理进程资源；
- 建立父子进程关系；
- 实现 `fork`、`exec`、`exit` 和 `waitpid`；
- 实现 Zombie 进程和退出状态保留；
- 将孤儿进程转交 `initproc`；
- 使用 FIFO 就绪队列进行调度；
- 实现 `initproc → user_shell → 用户程序` 的进程模型。

第四章中的任务由内核启动时静态创建；第五章允许进程在运行期间动态创建子进程并替换用户程序。

## 2. 实验环境

- 操作系统：WSL2 Ubuntu
- Rust：rustc 1.97.1
- QEMU：8.2.2
- 目标架构：RISC-V 64
- 官方基线：`upstream/ch5`
- 个人分支：`learning/ch5`

运行命令：

```bash
cd os
make run
```

## 3. 第五章主要变化

第五章相对第四章的代码变化较大：

```text
60 files changed
1600 insertions
583 deletions
```

任务模块被拆分为：

```text
os/src/task/pid.rs
os/src/task/manager.rs
os/src/task/processor.rs
os/src/task/task.rs
os/src/task/mod.rs
```

各模块作用：

| 模块 | 作用 |
|---|---|
| `pid.rs` | PID 和进程内核栈管理 |
| `manager.rs` | FIFO 就绪队列 |
| `processor.rs` | 当前 CPU 和调度循环 |
| `task.rs` | PCB、父子关系、`fork` 和 `exec` |
| `task/mod.rs` | 进程暂停、退出和孤儿进程托管 |
| `syscall/process.rs` | 进程相关系统调用 |

新增用户程序包括：

```text
exit
fantastic_text
forkexec
forktest
forktest2
forktest_simple
forktree
hello_world
initproc
matrix
sleep
sleep_simple
stack_overflow
user_shell
usertests
usertests-simple
yield
```

## 4. 实验运行结果

内核初始化后输出应用列表，并启动用户 Shell：

```text
after initproc!
/**** APPS ****
...
**************/
Rust user shell
>>
```

执行：

```text
hello_world
```

输出：

```text
pid 3: Hello world from user mode program!
Shell: Process 3 exited with code 0
```

这说明 Shell 成功完成：

```text
fork
→ 子进程 exec("hello_world")
→ 用户程序运行
→ 子进程 exit(0)
→ Shell waitpid
→ 回收子进程
```

执行：

```text
exit
```

输出：

```text
I am the parent. Forking the child...
I am parent, fork a child pid 4
I am the parent, waiting now..
I am the child.
waitpid 4 ok.
exit pass.
Shell: Process 3 exited with code 0
```

这验证了父进程创建子进程、等待子进程以及读取退出状态的流程。

## 5. 为什么 `ls` 执行失败

输入：

```text
ls
```

输出：

```text
Error when executing!
Shell: Process 3 exited with code -4
```

第五章尚未实现文件系统，Shell 只能执行内核中嵌入的应用。应用列表中不存在 `ls`，所以：

```text
exec("ls")
→ 内核找不到应用
→ exec 返回 -1
→ 子进程 main 返回 -4
→ Shell 获得退出码 -4
```

真正的文件和 `ls` 命令将在文件系统章节出现。

## 6. 为什么输入 `exit` 不会退出 Shell

第五章中的 `exit` 是一个用户测试程序，而不是 Shell 内置命令。

Shell 对所有输入统一执行：

```text
fork
→ 子进程 exec(命令名称)
→ 父进程 waitpid
```

所以输入 `exit` 实际运行 `user/src/bin/exit.rs`。测试程序结束后，Shell 仍然继续显示：

```text
>>
```

退出 QEMU 需要：

```text
Ctrl+A
松开
x
```

或者使用 `Ctrl+C`。

## 7. PID 分配器

```rust
pub struct PidAllocator {
    current: usize,
    recycled: Vec<usize>,
}
```

初始化：

```rust
current: 1
```

因此第一个 PID 是 1，一般分配给 `initproc`。

分配策略：

```text
优先从 recycled 中取出 PID
→ 没有回收 PID 时使用 current
→ current 递增
```

## 8. `PidHandle` 和 PID 自动回收

```rust
pub struct PidHandle(pub usize);
```

当 `PidHandle` 被销毁时：

```rust
impl Drop for PidHandle {
    fn drop(&mut self) {
        PID_ALLOCATOR.exclusive_access().dealloc(self.0);
    }
}
```

因此：

```text
PCB 被完全销毁
→ PidHandle 被 Drop
→ PID 进入 recycled
→ PID 可以重新使用
```

Shell 多次运行程序时经常看到 PID 3，是因为上一个 PID 3 子进程已经被 `waitpid` 完全回收。

## 9. 进程内核栈

内核栈位置由 PID 决定：

```rust
let top =
    TRAMPOLINE - pid * (KERNEL_STACK_SIZE + PAGE_SIZE);
```

布局大致如下：

```text
TRAMPOLINE
├── Guard Page
├── PID 1 KernelStack
├── Guard Page
├── PID 2 KernelStack
├── Guard Page
└── PID 3 KernelStack
```

相邻内核栈之间保留一个未映射页面。如果内核栈越界，将触发异常，而不是破坏相邻进程的内核栈。

创建内核栈：

```rust
KernelStack::new(&pid_handle)
```

它在内核地址空间建立 `R | W` 的 Framed 映射。

## 10. 内核栈自动回收

```rust
impl Drop for KernelStack {
    fn drop(&mut self) {
        KERNEL_SPACE
            .exclusive_access()
            .remove_area_with_start_vpn(...);
    }
}
```

回收过程：

```text
KernelStack 被 Drop
→ 删除内核页表映射
→ MapArea 被移除
→ FrameTracker 被 Drop
→ 内核栈物理页回收
```

第五章使用两套 RAII：

```text
PidHandle   → 自动回收 PID
KernelStack → 自动回收内核栈
```

## 11. 进程控制块

```rust
pub struct TaskControlBlock {
    pub pid: PidHandle,
    pub kernel_stack: KernelStack,
    inner: UPSafeCell<TaskControlBlockInner>,
}
```

可变部分：

```rust
pub struct TaskControlBlockInner {
    pub trap_cx_ppn: PhysPageNum,
    pub base_size: usize,
    pub task_cx: TaskContext,
    pub task_status: TaskStatus,
    pub memory_set: MemorySet,
    pub parent: Option<Weak<TaskControlBlock>>,
    pub children: Vec<Arc<TaskControlBlock>>,
    pub exit_code: i32,
}
```

PCB 管理：

```text
PID
+ 内核栈
+ 用户地址空间
+ CPU 上下文
+ 进程状态
+ 父子关系
+ 退出码
```

## 12. `Arc` 和 `Weak`

父进程保存对子进程的强引用：

```rust
children: Vec<Arc<TaskControlBlock>>
```

子进程保存对父进程的弱引用：

```rust
parent: Option<Weak<TaskControlBlock>>
```

关系为：

```text
父进程 --Arc--> 子进程
子进程 --Weak--> 父进程
```

如果父子双方都使用 `Arc`，会产生循环引用，导致 PCB 永远无法释放。使用 `Weak` 可以避免这一问题。

## 13. 创建初始进程

```rust
pub fn new(elf_data: &[u8]) -> Self
```

创建过程：

```text
从 ELF 创建 MemorySet
→ 找到 TrapContext 物理页
→ 分配 PID
→ 创建内核栈
→ 初始化 TaskContext
→ 状态设为 Ready
→ 初始化 TrapContext
```

初始进程没有父进程：

```rust
parent: None
```

该接口用于创建 `initproc`。

## 14. `initproc`

`initproc` 首先调用：

```rust
if fork() == 0 {
    exec("user_shell\0");
}
```

进程关系：

```text
PID 1 initproc
└── PID 2 user_shell
```

父进程 `initproc` 随后循环执行：

```rust
loop {
    let pid = wait(&mut exit_code);
    if pid == -1 {
        yield_();
        continue;
    }
}
```

它负责回收被托管的孤儿进程和 Zombie 进程。

## 15. 用户 Shell

Shell 从控制台逐字符读取命令：

```rust
let c = getchar();
```

用户按下回车后：

```rust
let pid = fork();
```

### 子进程

```rust
if pid == 0 {
    if exec(line.as_str()) == -1 {
        println!("Error when executing!");
        return -4;
    }
}
```

### 父进程

```rust
let exit_pid = waitpid(pid as usize, &mut exit_code);
```

完整流程：

```text
读取命令
→ fork
→ 子进程 exec
→ 父进程 waitpid
→ 子进程退出
→ 父进程读取退出码
→ 显示下一个提示符
```

## 16. `fork`

```rust
pub fn fork(self: &Arc<Self>) -> Arc<Self>
```

首先复制父进程用户地址空间：

```rust
MemorySet::from_existed_user(&parent_inner.memory_set)
```

随后为子进程分配：

- 新 PID；
- 新内核栈；
- 新 PCB；
- 新用户物理页；
- 新页表；
- 新 TaskContext。

子进程的父指针：

```rust
parent: Some(Arc::downgrade(self))
```

父进程保存子进程：

```rust
parent_inner.children.push(child.clone());
```

## 17. `fork` 复制地址空间

```rust
pub fn from_existed_user(user_space: &Self) -> Self
```

复制过程：

```text
创建空 MemorySet
→ 映射共享 Trampoline
→ 遍历父进程所有 MapArea
→ 创建相同 VPN 范围和权限
→ 为子进程分配新物理页
→ 将父进程每个页面复制到子进程
```

逐页复制：

```rust
let src_ppn = user_space.translate(vpn).unwrap().ppn();
let dst_ppn = memory_set.translate(vpn).unwrap().ppn();

dst_ppn
    .get_bytes_array()
    .copy_from_slice(src_ppn.get_bytes_array());
```

结果：

```text
父子虚拟地址相同
父子物理页不同
页面初始内容相同
```

这是立即完整复制，不是 Copy-on-Write。

## 18. 为什么复制 TrapContext

TrapContext 也属于用户 `MemorySet`，因此会被一起复制。

子进程最初拥有与父进程相同的：

- 用户寄存器；
- `sepc`；
- 用户栈；
- 用户内存；
- 系统调用现场。

之后内核修改：

```rust
trap_cx.kernel_sp = child_kernel_stack_top;
trap_cx.x[10] = 0;
```

确保子进程使用自己的内核栈，并让子进程中的 `fork()` 返回 0。

## 19. `fork` 的返回值

父进程：

```text
fork() → 子进程 PID
```

子进程：

```text
fork() → 0
```

子进程返回值通过修改 TrapContext 的 `a0/x10` 实现：

```rust
trap_cx.x[10] = 0;
```

父进程由 `sys_fork` 直接返回：

```rust
new_pid as isize
```

## 20. `exec`

```rust
pub fn exec(&self, elf_data: &[u8])
```

执行过程：

```text
从新 ELF 创建 MemorySet
→ 替换原用户地址空间
→ 更新 TrapContext 物理页
→ 更新用户栈
→ 重建 TrapContext
→ 从新 ELF 入口运行
```

`exec` 不会创建新进程，也不会修改：

- PID；
- 内核栈；
- 父进程；
- 子进程关系。

它只替换：

```text
用户地址空间
+ 用户程序
+ 用户栈
+ TrapContext
```

## 21. `sys_exec`

用户传入的路径位于用户地址空间：

```rust
path: *const u8
```

内核先执行：

```rust
translated_str(token, path)
```

再通过：

```rust
get_app_data_by_name(path.as_str())
```

查找内核中嵌入的应用。

找到程序时：

```text
task.exec(data)
→ 返回 0
```

找不到时：

```text
返回 -1
```

## 22. 进程状态

```rust
pub enum TaskStatus {
    Ready,
    Running,
    Zombie,
}
```

状态变化：

```text
Ready → Running
Running → Ready
Running → Zombie
```

Zombie 进程已经停止执行，但必须暂时保留 PCB、PID 和退出码，等待父进程调用 `waitpid`。

## 23. `exit`

```rust
pub fn exit_current_and_run_next(exit_code: i32)
```

退出过程：

```text
从 PROCESSOR 取出当前进程
→ Running → Zombie
→ 保存 exit_code
→ 子进程转交 initproc
→ 回收用户数据页
→ 切换回调度器
```

退出进程不会再次运行，因此无需保存 TaskContext：

```rust
let mut _unused = TaskContext::zero_init();
schedule(&mut _unused as *mut _);
```

## 24. 孤儿进程托管

父进程退出时，它的所有子进程会被转交给 `initproc`：

```rust
for child in inner.children.iter() {
    child.parent = Some(Arc::downgrade(&INITPROC));
    initproc_inner.children.push(child.clone());
}
```

然后：

```rust
inner.children.clear();
```

流程：

```text
父进程退出
→ 子进程成为孤儿
→ parent 改为 initproc
→ initproc 负责 wait 回收
```

## 25. 进程退出时的资源回收

```rust
inner.memory_set.recycle_data_pages();
```

实现：

```rust
self.areas.clear();
```

清空 `areas` 会销毁其中的 `MapArea` 和 `FrameTracker`，从而回收：

- 用户代码页；
- 用户数据页；
- 用户栈；
- 用户堆；
- TrapContext 数据页。

退出时仍保留：

- PCB；
- PID；
-内核栈；
- Zombie 状态；
- 退出码。

## 26. `waitpid`

```rust
pub fn sys_waitpid(
    pid: isize,
    exit_code_ptr: *mut i32
) -> isize
```

三种返回结果：

### 返回 `-1`

没有符合要求的子进程。

### 返回 `-2`

存在符合要求的子进程，但它仍在运行，尚未成为 Zombie。

### 返回子进程 PID

找到 Zombie 子进程：

```text
从 children 移除
→ 读取 exit_code
→ 写入父进程用户地址
→ 返回子进程 PID
```

写入退出码时必须使用父进程页表：

```rust
translated_refmut(
    inner.memory_set.token(),
    exit_code_ptr
)
```

## 27. `waitpid` 完全回收进程

```rust
let child = inner.children.remove(idx);
assert_eq!(Arc::strong_count(&child), 1);
```

从 `children` 移除后，只剩当前局部变量持有 PCB。

函数返回时：

```text
child 被 Drop
→ Arc strong_count 归零
→ PCB 被销毁
→ PidHandle 被 Drop
→ PID 回收
→ KernelStack 被 Drop
→ 内核栈回收
→ 剩余 MemorySet 资源回收
```

因此 `waitpid` 既读取退出状态，也执行最终资源回收。

## 28. 两阶段资源回收

```text
第一阶段：sys_exit
├── 状态变为 Zombie
├── 保存退出码
└── 回收用户数据页

第二阶段：waitpid
├── 从父进程 children 移除
├── 销毁 PCB
├── 回收 PID
└── 回收内核栈
```

## 29. FIFO 就绪队列

```rust
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}
```

加入队尾：

```rust
push_back(task)
```

从队首取出：

```rust
pop_front()
```

示例：

```text
初始：[A, B, C]
取出 A：[B, C]
A 时间片结束后放回：[B, C, A]
```

这实现了 FIFO Round-Robin 调度。

## 30. `Processor`

```rust
pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    idle_task_cx: TaskContext,
}
```

| 字段 | 作用 |
|---|---|
| `current` | 当前 CPU 正在运行的进程 |
| `idle_task_cx` | 调度循环自身的内核上下文 |

`idle_task_cx` 不是用户进程，也不是 PID 1，而是 `run_tasks` 调度循环的执行现场。

## 31. 主调度循环

```rust
pub fn run_tasks() {
    loop {
        if let Some(task) = fetch_task() {
```

执行流程：

```text
从就绪队列取出一个任务
→ 状态改为 Running
→ PROCESSOR.current = task
→ __switch(idle_task_cx, task_cx)
→ 进程运行
```

进程暂停或退出后，通过 `schedule` 切换回 `idle_task_cx`，`run_tasks` 继续选择下一个任务。

## 32. 进程暂停

```rust
pub fn suspend_current_and_run_next()
```

流程：

```text
从 PROCESSOR 取出当前进程
→ Running → Ready
→ 放回 TASK_MANAGER 队尾
→ 保存 TaskContext
→ schedule
→ 返回调度循环
```

暂停进程以后还会继续运行，因此必须保存它的 TaskContext。

## 33. 完整 `fork-exec-wait` 流程

```text
Shell
│
├─ fork
│   ├─ 父进程返回子 PID
│   └─ 子进程返回 0
│
├─ 子进程 exec("hello_world")
│   ├─ PID 不变
│   ├─ 替换用户地址空间
│   └─ 从 hello_world 入口运行
│
├─ 子进程 exit(0)
│   ├─ 状态变为 Zombie
│   └─ 保存 exit_code
│
└─ Shell waitpid
    ├─ 找到 Zombie 子进程
    ├─ 读取退出码
    ├─ 从 children 移除
    └─ 完全释放子进程资源
```

## 34. 完整进程树

正常启动后：

```text
PID 1 initproc
└── PID 2 user_shell
    └── PID 3 用户命令
        └── PID 4 命令创建的子进程
```

如果 PID 3 先于 PID 4 退出：

```text
PID 4
→ 重新挂到 PID 1 initproc
→ 由 initproc 回收
```

## 35. 完整进程生命周期

```text
创建 PCB
→ 分配 PID
→ 创建内核栈
→ 创建或复制用户地址空间
→ Ready
→ 加入 FIFO 就绪队列
→ Running
→ 时间片结束后回到 Ready
→ sys_exit
→ Zombie
→ 回收用户数据页
→ 父进程 waitpid
→ 销毁 PCB
→ 回收 PID 和内核栈
```

## 36. 当前实现的限制

第五章仍然是教学版本：

- `fork` 会立即复制所有用户物理页，没有 Copy-on-Write；
- 用户程序仍然嵌入内核镜像，不来自文件系统；
- Shell 不支持管道、重定向和后台执行；
- `ls` 等文件系统命令尚不可用；
- 单核调度器使用简单 FIFO 队列；
- `waitpid` 采用轮询与 `yield`，没有阻塞等待队列；
- PID 分配器是简单的顺序分配和回收栈。

这些问题将在后续章节逐步完善。

## 37. 本章总结

第五章在独立地址空间和分时调度的基础上实现了完整的进程生命周期。

关键机制包括：

1. 使用 `PidHandle` 自动管理 PID；
2. 使用 `KernelStack` 自动管理内核栈；
3. 使用 PCB 保存地址空间、上下文和父子关系；
4. 使用 `Arc` 和 `Weak` 避免父子进程循环引用；
5. `fork` 完整复制父进程地址空间；
6. `exec` 在保持 PID 不变的情况下替换用户程序；
7. `exit` 将进程变为 Zombie 并保存退出码；
8. `waitpid` 获取退出状态并最终释放子进程；
9. `initproc` 接管并回收孤儿进程；
10. `TaskManager` 使用 FIFO 就绪队列；
11. `Processor` 保存当前运行进程和调度循环上下文。

第四章与第五章的核心区别：

```text
第四章：内核启动时静态创建任务
第五章：进程可以动态 fork、exec、exit 和 wait
```

第五章核心执行路径：

```text
initproc
→ fork
→ user_shell
→ fork
→ exec
→ 用户程序
→ exit
→ Zombie
→ waitpid
→ 资源回收
```
