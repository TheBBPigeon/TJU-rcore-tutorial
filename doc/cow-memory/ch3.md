@phao90016-jpg

# Chapter 3：多任务与分时调度

## 1. 实验目标

本章在第二章批处理系统的基础上，实现并理解：

- 多个用户程序同时驻留内存；
- 任务控制块和任务状态；
- 内核任务上下文；
- `__switch` 汇编上下文切换；
- Supervisor Timer 时钟中断；
- 基于时间片的抢占式调度；
- 用户主动让出 CPU；
- 任务退出和任务回收。

第二章是：

```text
应用 A 完成
→ 应用 B 完成
→ 应用 C 完成
```

第三章变为：

```text
应用 A 运行一段时间
→ 时钟中断
→ 应用 B
→ 时钟中断
→ 应用 C
→ 时钟中断
→ 应用 A
```

## 2. 实验环境

- 操作系统：WSL2 Ubuntu
- Rust：rustc 1.97.1
- QEMU：8.2.2
- 目标架构：RISC-V 64
- 官方基线：`upstream/ch3`
- 个人分支：`learning/ch3`

运行命令：

```bash
cd os
make run
```

## 3. 第三章相对第二章的变化

第三章新增的核心文件包括：

```text
os/src/loader.rs
os/src/task/context.rs
os/src/task/task.rs
os/src/task/mod.rs
os/src/task/switch.rs
os/src/task/switch.S
os/src/timer.rs
os/src/device_tree.rs
```

各部分作用：

| 模块 | 作用 |
|---|---|
| `loader.rs` | 加载所有用户应用并创建初始 TrapContext |
| `task/context.rs` | 定义内核任务上下文 |
| `task/task.rs` | 定义任务控制块和任务状态 |
| `task/mod.rs` | 实现任务管理器和调度 |
| `task/switch.S` | 保存和恢复内核寄存器 |
| `timer.rs` | 读取时钟、设置下一次时钟中断 |
| `device_tree.rs` | 从设备树读取时钟频率 |

## 4. 实验运行结果

本章加载了四个用户程序：

```text
00power_3
01power_5
02power_7
03sleep
```

应用的加载地址为：

```text
00power_3 → 0x80400000
01power_5 → 0x80420000
02power_7 → 0x80440000
03sleep   → 0x80460000
```

程序输出互相交错，例如：

```text
power_3 [50000/200000]
power_5 [10000/140000]
power_7 [10000/160000]
```

这说明三个计算程序并不是顺序执行，而是在单核 CPU 上按照时间片交替运行。

最终结果包括：

```text
Test power_3 OK!
Test power_5 OK!
Test power_7 OK!
Test sleep OK!
[kernel] Application exited with code 0
All applications completed!
```

## 5. 应用的固定加载地址

在 `config.rs` 中配置：

```rust
APP_BASE_ADDRESS = 0x80400000
APP_SIZE_LIMIT = 0x20000
```

加载地址由下式计算：

```rust
fn get_base_i(app_id: usize) -> usize {
    APP_BASE_ADDRESS + app_id * APP_SIZE_LIMIT
}
```

因此每个应用间隔：

```text
0x20000 = 128 KiB
```

这种设计比较简单，但存在限制：

- 每个应用不能超过 128 KiB；
- 应用之间不能动态共享空间；
- 目前还没有虚拟地址空间隔离；
- 内核可以直接访问用户地址。

## 6. 应用加载过程

第三章的用户程序已经被嵌入内核二进制。`loader::load_apps()` 通过链接器生成的 `_num_app` 和应用起始地址表找到各个应用。

对每个应用，内核执行：

```text
读取应用起止地址
→ 清空目标区域
→ 将应用复制到固定运行地址
→ 执行 fence.i
```

核心代码：

```rust
let base_i = get_base_i(i);

(base_i..base_i + APP_SIZE_LIMIT)
    .for_each(|addr| unsafe {
        (addr as *mut u8).write_volatile(0)
    });

dst.copy_from_slice(src);
```

`fence.i` 用于保证处理器取指时能够看到刚刚写入的指令。

## 7. 用户栈、内核栈和初始上下文

系统为每个应用分别分配：

```rust
static KERNEL_STACK: [KernelStack; MAX_APP_NUM]
static USER_STACK: [UserStack; MAX_APP_NUM]
```

每个任务拥有：

- 一个内核栈；
- 一个用户栈；
- 一个 `TrapContext`；
- 一个 `TaskContext`。

初始化应用上下文：

```rust
pub fn init_app_cx(app_id: usize) -> usize {
    KERNEL_STACK[app_id].push_context(
        TrapContext::app_init_context(
            get_base_i(app_id),
            USER_STACK[app_id].get_sp(),
        )
    )
}
```

初始化结果：

```text
sepc = 用户程序入口地址
sp   = 对应用户栈顶部
SPP  = User
```

## 8. `TaskContext`

第三章新增的任务上下文：

```rust
#[repr(C)]
pub struct TaskContext {
    ra: usize,
    sp: usize,
    s: [usize; 12],
}
```

布局如下：

| 偏移 | 内容 |
|---:|---|
| `0` | `ra` |
| `8` | `sp` |
| `16` | `s0` |
| ... | ... |
| `104` | `s11` |

这里的 `TaskContext` 与第二章的 `TrapContext` 不同：

| 上下文 | 用途 |
|---|---|
| `TrapContext` | 用户态与内核态之间的切换 |
| `TaskContext` | 内核任务之间的切换 |

`TaskContext` 只保存内核切换所需的寄存器。用户态寄存器已经由 `TrapContext` 保存。

## 9. 初始任务上下文

```rust
pub fn goto_restore(kstack_ptr: usize) -> Self {
    Self {
        ra: linker_symbol_addr!(__restore),
        sp: kstack_ptr,
        s: [0; 12],
    }
}
```

第一个任务尚未执行过，因此需要手动构造初始上下文：

```text
ra = __restore
sp = 该任务的内核栈
```

第一次切换到任务时：

```text
__switch
→ 恢复 ra = __restore
→ ret
→ __restore
→ sret
→ 进入用户程序
```

## 10. `__switch` 汇编

`__switch` 接收两个参数：

```text
a0 = 当前任务上下文地址
a1 = 下一个任务上下文地址
```

保存当前任务：

```asm
sd sp, 8(a0)
sd ra, 0(a0)
sd s0-s11, ...
```

恢复下一个任务：

```asm
ld ra, 0(a1)
ld s0-s11, ...
ld sp, 8(a1)
ret
```

`ret` 使用恢复后的 `ra`。因此从下一个任务的角度看，它仿佛从之前调用 `__switch` 的地方恢复执行。

切换过程：

```text
任务 A 调用 __switch
→ 保存 A 的 ra、sp、s0-s11
→ 恢复 B 的 ra、sp、s0-s11
→ ret
→ 任务 B 继续执行
```

## 11. 任务控制块和任务状态

```rust
pub struct TaskControlBlock {
    pub task_status: TaskStatus,
    pub task_cx: TaskContext,
}
```

任务状态：

```rust
pub enum TaskStatus {
    UnInit,
    Ready,
    Running,
    Exited,
}
```

状态变化：

```text
UnInit → Ready
Ready  → Running
Running → Ready
Running → Exited
```

时钟中断导致：

```text
Running → Ready
```

用户程序退出导致：

```text
Running → Exited
```

## 12. 全局任务管理器

```rust
pub struct TaskManager {
    num_app: usize,
    inner: UPSafeCell<TaskManagerInner>,
}
```

`TaskManagerInner` 保存：

```rust
pub struct TaskManagerInner {
    tasks: [TaskControlBlock; MAX_APP_NUM],
    current_task: usize,
}
```

初始化时，所有应用被设置为 `Ready`：

```rust
for (i, task) in tasks.iter_mut().enumerate() {
    task.task_cx = TaskContext::goto_restore(init_app_cx(i));
    task.task_status = TaskStatus::Ready;
}
```

## 13. Round-Robin 调度

寻找下一个任务：

```rust
(current + 1..current + self.num_app + 1)
    .map(|id| id % self.num_app)
    .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
```

假设有四个任务，当前任务为 2，搜索顺序为：

```text
3 → 0 → 1 → 2
```

这是一种循环队列式的 Round-Robin 调度。

如果找到下一个 `Ready` 任务：

```rust
inner.tasks[next].task_status = TaskStatus::Running;
inner.current_task = next;
__switch(current_task_cx_ptr, next_task_cx_ptr);
```

如果没有任何 `Ready` 任务：

```rust
println!("All applications completed!");
shutdown(false);
```

## 14. 时钟与时间片

时钟模块中：

```rust
const TICKS_PER_SEC: usize = 100;
```

设置下一次时钟中断：

```rust
set_timer(get_time() + clock_freq() / TICKS_PER_SEC);
```

QEMU 设备树输出：

```text
CLOCK_FREQ from device tree: 10000000
```

表示每秒有 10,000,000 个 `mtime` tick。

由于每秒设置 100 次中断，因此时间片约为：

```text
1 / 100 秒 = 10 ms
```

## 15. 时钟抢占路径

在 `trap_handler` 中：

```rust
Trap::Interrupt(Interrupt::SupervisorTimer) => {
    set_next_trigger();
    suspend_current_and_run_next();
}
```

完整流程：

```text
SupervisorTimer
→ 设置下一次时钟中断
→ 当前任务状态改为 Ready
→ 查找下一个 Ready 任务
→ 更新 current_task
→ __switch
→ 恢复下一个任务
```

## 16. 主动让出 CPU

用户程序可以调用 `sys_yield`：

```rust
pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}
```

这属于协作式调度：

```text
用户主动调用 yield
→ 当前任务暂时让出 CPU
→ 调度下一个 Ready 任务
```

而时钟中断触发的是抢占式调度：

```text
时间片结束
→ SupervisorTimer
→ 当前任务被内核强制切换
```

## 17. 任务退出

```rust
pub fn sys_exit(exit_code: i32) -> ! {
    println!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}
```

退出流程：

```text
用户程序调用 sys_exit
→ 当前任务 Running → Exited
→ 查找下一个 Ready 任务
→ __switch
→ 下一个任务继续运行
```

已经退出的任务不会再次参与调度。

## 18. 第三章完整启动流程

```text
rust_main
→ clear_bss
→ logging::init
→ device_tree::init
→ trap::init
→ loader::load_apps
→ trap::enable_timer_interrupt
→ timer::set_next_trigger
→ task::run_first_task
→ __switch
→ __restore
→ sret
→ 进入第一个用户程序
```

## 19. 第三章完整调度流程

```text
用户程序运行
→ 时钟中断
→ __alltraps
→ trap_handler
→ set_next_trigger
→ suspend_current_and_run_next
→ Running → Ready
→ find_next_task
→ __switch
→ 恢复下一个任务
→ __restore
→ sret
→ 下一个用户程序继续运行
```

任务退出时：

```text
sys_exit
→ Running → Exited
→ find_next_task
→ 切换下一个 Ready 任务
```

## 20. 实验现象解释

`power_3`、`power_5`、`power_7` 的输出互相穿插，是因为单个 CPU 在多个任务之间进行时间片轮转。

输出出现同一行交错的原因是多个任务写入控制台时没有统一的行缓冲，但这并不表示多个 CPU 同时运行，也不是数据损坏。

本实验实际体现的是：

```text
单核 CPU
+ 多个任务
+ 时钟中断
+ TaskContext 保存/恢复
= 抢占式多任务
```

## 21. 本章总结

第三章将第二章的批处理系统升级为支持多任务的操作系统。

本章的关键机制是：

1. 启动时将多个应用加载到不同的固定地址；
2. 为每个应用分配独立的用户栈和内核栈；
3. 为每个应用建立初始 `TrapContext` 和 `TaskContext`；
4. 使用 `TaskManager` 管理任务状态；
5. 使用 `__switch` 保存和恢复内核任务上下文；
6. 使用 Supervisor Timer 产生周期性时钟中断；
7. 通过 Round-Robin 算法轮转调度；
8. 任务退出后标记为 `Exited`，不再被调度。

第二章与第三章的核心区别：

```text
第二章：任务按顺序运行
第三章：任务按时间片交错运行
```

第三章的核心执行路径：

```text
时钟中断
→ trap_handler
→ suspend_current_and_run_next
→ __switch
→ 下一个任务
```