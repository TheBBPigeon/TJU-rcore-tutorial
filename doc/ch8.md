# Chapter 8 并发：线程与同步机制

## 1. 本章目标

第八章在进程、文件系统、管道和信号机制的基础上，为 rCore 增加线程与同步机制，主要包括：

1. 用户线程创建、调度、退出和回收；
2. 进程控制块与线程控制块拆分；
3. 每线程独立用户栈、内核栈和 TrapContext；
4. 数据竞争及其产生原因；
5. 自旋互斥锁；
6. 阻塞互斥锁；
7. 信号量；
8. 条件变量；
9. Barrier、生产者消费者和哲学家就餐问题。

学习分支：

```text
learning/ch8
```

代码基线：

```text
upstream/ch8
```

## 2. ch8 总体变化

ch7 中，一个 `TaskControlBlock` 基本对应一个进程。ch8 将进程共享资源与线程私有资源分离：

```text
ProcessControlBlock
├── 地址空间
├── 文件描述符表
├── 信号状态
├── 子进程
├── 互斥锁表
├── 信号量表
├── 条件变量表
└── 多个 TaskControlBlock
```

一个进程可以包含多个线程。线程共享进程资源，但拥有独立执行现场。

关键文件：

```text
os/src/task/process.rs
os/src/task/task.rs
os/src/task/id.rs
os/src/syscall/thread.rs
os/src/syscall/sync.rs
os/src/sync/mutex.rs
os/src/sync/semaphore.rs
os/src/sync/condvar.rs
```

## 3. ProcessControlBlock

进程控制块保存进程级共享资源：

```rust
pub struct ProcessControlBlock {
    pub pid: PidHandle,
    inner: UPSafeCell<ProcessControlBlockInner>,
}
```

共享资源包括：

```rust
memory_set
fd_table
signals
children
tasks
mutex_list
semaphore_list
condvar_list
```

同一进程中的线程共享：

* 代码段和数据段；
* 全局变量和堆；
* 页表和地址空间；
* 文件描述符；
* 信号状态；
* 内核同步对象。

## 4. TaskControlBlock

线程控制块保存线程私有资源：

```rust
pub struct TaskControlBlock {
    pub process: Weak<ProcessControlBlock>,
    pub kstack: KernelStack,
    inner: UPSafeCell<TaskControlBlockInner>,
}
```

私有资源包括：

```rust
res
trap_cx_ppn
task_cx
task_status
exit_code
```

每个线程独立拥有：

* TID；
* 用户栈；
* 内核栈；
* TrapContext；
* TaskContext；
* 调度状态；
* 线程退出码。

线程使用 `Weak<ProcessControlBlock>` 指向所属进程，避免 PCB 与 TCB 形成强引用环。

## 5. PID 与 TID

PID 在系统范围内唯一，TID 只需在所属进程内部唯一。

例如：

```text
进程A：pid=2，tid=0、1、2
进程B：pid=3，tid=0、1、2
```

编号由 `RecycleAllocator` 分配。分配器优先复用已释放编号，没有可复用编号时才分配新编号。

主线程通常获得：

```text
tid = 0
```

随后创建的线程依次获得：

```text
tid = 1, 2, 3, ...
```

## 6. 每线程用户栈

线程共享地址空间，但必须拥有独立用户栈。

用户栈底地址：

```rust
ustack_base
    + tid * (PAGE_SIZE + USER_STACK_SIZE)
```

当前配置：

```text
PAGE_SIZE       = 4096字节
USER_STACK_SIZE = 8192字节
```

不同线程用户栈之间保留一个未映射页面，可用于检测栈越界。

每线程用户栈映射权限：

```text
R | W | U
```

用户态可读写，但不能执行。

## 7. 每线程 TrapContext

TrapContext 地址按 TID 从高地址向低地址排列：

```rust
TRAP_CONTEXT_BASE - tid * PAGE_SIZE
```

每个线程必须分别保存：

* `sepc`；
* 用户栈指针；
* 通用寄存器；
* 内核栈指针；
* Trap处理入口。

TrapContext 页面没有用户权限 `U`，用户程序不能直接修改内核返回现场。

## 8. 每线程内核栈

每个线程进入内核后，都在自己的内核栈中执行：

* 系统调用；
* 中断和异常处理；
* 调度；
* 内核函数调用。

不同内核栈之间也保留一个保护页。

`KernelStack` 销毁时自动：

1. 解除内核地址空间映射；
2. 回收内核栈编号。

## 9. thread_create

用户接口：

```rust
thread_create(entry, arg)
```

参数：

* `entry`：线程入口函数地址；
* `arg`：传给入口函数的参数。

系统调用过程：

```text
thread_create
→ sys_thread_create
→ 创建新TaskControlBlock
→ 分配TID
→ 映射用户栈
→ 映射TrapContext
→ 分配内核栈
→ 设置sepc=entry
→ 设置a0=arg
→ 加入进程线程表
→ 加入调度器
```

RISC-V 中：

```text
a0 = x10
```

因此新线程第一次返回用户态时，相当于执行：

```rust
entry(arg)
```

## 10. gettid 与 waittid

`gettid()` 返回当前线程在所属进程中的编号。

`waittid()` 的内核返回约定：

```text
-1：线程不存在，或者线程等待自己
-2：线程存在但尚未退出
其他值：目标线程退出码
```

用户库在返回 `-2` 时调用 `yield_()`，主动让出 CPU：

```text
waittid
→ 目标未退出
→ yield
→ 重新检查
```

目标线程退出后，`waittid()` 从进程线程表移除 TCB，并返回退出码。

## 11. 线程退出与资源回收

普通线程调用 `exit(code)` 后：

```rust
exit_code = Some(code)
res = None
```

销毁 `TaskUserRes` 会回收：

* TID；
* 用户栈；
* TrapContext。

线程退出时不能立刻销毁整个 TCB，因为退出流程仍在当前线程内核栈上执行。

主线程调用 `waittid()` 后，从线程表移除 TCB，最后释放内核栈。

如果退出线程的 TID 为0，当前实现会终止整个进程，并清理其他线程、地址空间、文件描述符和同步对象。

## 12. 数据竞争

多个线程执行普通累加时，一次：

```rust
A += 1
```

实际包含：

```text
读取A
→ 计算A+1
→ 写回A
```

两个线程可能同时读取相同旧值，随后分别写回相同新值，导致更新丢失。

`adder` 预期结果：

```text
160000
```

实际得到更小结果并以 `SIGABRT=-6` 退出，正确展示了数据竞争。

## 13. 普通变量不能实现可靠锁

`adder_simple_spin` 和 `adder_simple_yield` 使用普通变量进行检查和加锁。

以下过程不是原子的：

```text
检查lock==0
→ 设置lock=1
```

多个线程可能同时观察到 `lock==0`，并同时进入临界区。

调用 `yield_()` 只能改变调度时机，不能让检查与赋值成为原子操作，因此两项测试按预期失败。

## 14. 原子操作

`adder_atomic` 使用原子操作更新共享计数器。

原子指令保证读、改、写不可被其他线程观察为中间状态，因此最终结果正确。

测试正常返回 Shell，说明断言通过。

## 15. Mutex trait

内核通过统一接口支持不同锁：

```rust
pub trait Mutex: Sync + Send {
    fn lock(&self);
    fn unlock(&self);
}
```

进程保存：

```rust
Vec<Option<Arc<dyn Mutex>>>
```

线程通过进程内 `mutex_id` 访问锁。

## 16. 自旋互斥锁

`MutexSpin` 保存：

```rust
locked: UPSafeCell<bool>
```

加锁时：

```text
locked=false
→ 设置为true
→ 获得锁
```

锁已被占用时：

```text
释放UPSafeCell访问
→ suspend_current_and_run_next
→ 重新竞争
```

当前教程中的自旋锁会主动让出 CPU，但没有专用等待队列。线程仍保持可调度状态，每次被调度后重新检查锁。

解锁只执行：

```rust
locked = false
```

不保证等待线程的严格公平顺序。

## 17. 阻塞互斥锁

`MutexBlocking` 保存：

```rust
locked: bool
wait_queue: VecDeque<Arc<TaskControlBlock>>
```

锁已占用时：

```text
当前线程加入等待队列
→ 状态变为Blocked
→ 切换到其他线程
```

解锁时：

* 有等待线程：唤醒队首线程，并将锁直接交给它；
* 没有等待线程：设置 `locked=false`。

有等待线程时不先清除 `locked`，可以避免其他线程在被唤醒线程运行前抢占锁。

## 18. suspend 与 block

```text
suspend：Running → Ready
block：Running → Blocked
wakeup：Blocked → Ready
```

自旋锁竞争失败时使用 `suspend`，阻塞锁竞争失败时使用 `block`。

## 19. 信号量

信号量包含：

```rust
count: isize
wait_queue: VecDeque<Arc<TaskControlBlock>>
```

`down()`：

```text
count -= 1
若count<0：
    加入等待队列
    阻塞当前线程
```

`up()`：

```text
count += 1
若count<=0：
    唤醒一个等待线程
```

含义：

```text
count>0：可用资源数量
count=0：无空闲资源
count<0：存在等待线程
```

信号量可以保存提前发生的通知。没有等待者时执行 `up()`，会留下一个可供未来 `down()` 消费的许可。

## 20. 条件变量

条件变量只保存等待队列，不保存历史通知：

```rust
wait_queue: VecDeque<Arc<TaskControlBlock>>
```

`wait(mutex)` 执行：

```text
释放mutex
→ 加入条件变量等待队列
→ 阻塞
→ 被唤醒
→ 重新获得mutex
```

`signal()` 唤醒一个等待线程。

条件变量必须与：

* 共享条件；
* 互斥锁；

共同使用。

标准使用方式：

```rust
mutex_lock(mutex);

while !condition {
    condvar_wait(condvar, mutex);
}

使用共享状态;
mutex_unlock(mutex);
```

使用 `while` 是因为线程被唤醒后必须重新检查条件是否仍然成立。

## 21. Barrier

Barrier 保证所有线程到达某个阶段后才能共同继续。

本章测试中三个线程依次输出：

```text
a阶段
→ BARRIER_AB
→ b阶段
→ BARRIER_BC
→ c阶段
```

最后一个到达的线程调用 `signal()`，被唤醒线程继续 `signal()` 下一个等待者，形成链式唤醒。

无 Barrier 时，输出中出现 `b` 后仍可能出现 `a`。

使用条件变量 Barrier 后，输出严格形成：

```text
所有a
→ 所有b
→ 所有c
```

## 22. 生产者消费者

`mpsc_sem` 实现四个生产者和一个消费者，缓冲区大小为8。

使用三个信号量：

```text
SEM_MUTEX=1：保护缓冲区
SEM_EMPTY=8：空槽数量
SEM_AVAIL=0：可读数据数量
```

生产者：

```text
down(EMPTY)
down(MUTEX)
写入缓冲区
up(MUTEX)
up(AVAIL)
```

消费者：

```text
down(AVAIL)
down(MUTEX)
读取缓冲区
up(MUTEX)
up(EMPTY)
```

测试完成400次数据传递并输出：

```text
mpsc_sem passed!
```

## 23. 哲学家就餐

五个哲学家共享五把叉子，每把叉子由阻塞互斥锁表示。

所有线程统一按编号从小到大加锁：

```rust
mutex_lock(min);
mutex_lock(max);
```

这打破了循环等待条件，避免五个哲学家各持有一把叉子并永久等待另一把叉子的死锁。

测试耗时：

```text
7285 ms
```

最终正常返回，说明未发生死锁。

## 24. 测试结果

基础线程：

```text
thread#1 exited with code 1
thread#2 exited with code 2
thread#3 exited with code 3
main thread exited.
```

数据竞争：

```text
adder                 → SIGABRT=-6
adder_simple_spin     → SIGABRT=-6
adder_simple_yield    → SIGABRT=-6
```

正确同步：

```text
adder_atomic          → 通过
adder_mutex_spin      → 通过
adder_mutex_blocking  → 通过
```

同步原语：

```text
sync_sem passed!
test_condvar passed!
barrier_condvar → OK!
mpsc_sem passed!
```

总体测试：

```text
32 of sueecssed apps, 9 of failed apps run correctly.
Usertests passed!
```

其中9个失败应用均按照预期退出码运行，说明异常和竞争测试正确。

## 25. 本章总结

第八章把 rCore 从单执行流进程扩展为支持多线程的操作系统。

进程控制块管理共享地址空间和系统资源，线程控制块管理独立的执行现场。每个线程拥有独立用户栈、内核栈、TrapContext 和调度状态。

互斥锁解决临界区互斥问题；信号量同时表达资源数量和线程同步；条件变量允许线程等待由共享状态描述的条件。Barrier、生产者消费者和哲学家就餐展示了这些同步原语的组合使用。

本章还通过错误累加程序说明：线程共享内存虽然提高了协作能力，但也引入数据竞争、死锁和同步顺序等问题。
