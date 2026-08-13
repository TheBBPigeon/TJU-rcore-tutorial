@phao90016-jpg

# 第九章：I/O 设备管理

## 1. 本章目标

第九章在前面进程、文件系统、信号、线程与同步机制的基础上，引入实际设备管理。本章主要完成：

- 从设备树中读取硬件信息；
- 使用 PLIC 管理外部中断；
- 使用中断驱动 UART 输入；
- 支持 VirtIO 块设备的非阻塞访问；
- 支持 VirtIO GPU、键盘和鼠标；
- 将帧缓冲映射到用户地址空间；
- 支持简化的 ARP、UDP 和 TCP 网络通信；
- 使用条件变量阻塞并唤醒等待 I/O 的任务。

本章的核心思想是：任务等待慢速设备时不应持续占用 CPU。任务提交 I/O 请求后进入阻塞状态，设备完成操作并产生中断，再由内核唤醒对应任务。

---

## 2. 系统初始化顺序

内核入口 `rust_main` 按照依赖关系初始化各个模块，主要顺序如下：

1. 清空 `.bss` 段；
2. 初始化内存管理；
3. 初始化 UART 和日志系统；
4. 解析设备树；
5. 初始化 GPU、键盘和鼠标；
6. 初始化异常与中断处理；
7. 设置时钟中断；
8. 初始化 PLIC 和板级设备；
9. 初始化文件系统并加载初始进程；
10. 开启设备的非阻塞访问；
11. 启动任务调度器。

在调度器启动前，块设备必须使用同步访问，因为此时没有其他任务可以调度。调度器启动后，内核将 `DEV_NON_BLOCKING_ACCESS` 设置为 `true`，块设备开始使用异步请求和中断唤醒。

---

## 3. 设备树

QEMU 将设备树二进制 DTB 的地址传递给内核。设备树用于描述当前平台的硬件信息，使内核不必把所有硬件参数写死在代码中。

本章从设备树中读取 `timebase-frequency`，用于得到平台时钟频率：

```text
CLOCK_FREQ = 10_000_000 Hz
```

启动输出中可以看到：

```text
[ INFO] CLOCK_FREQ from device tree: 10000000
```

当前教程只使用设备树获取部分信息，VirtIO MMIO 地址和 PLIC 参数仍主要针对 QEMU 平台固定配置。因此它展示了设备树机制，但还不是完全由设备树驱动的通用内核。

---

## 4. MMIO 与 volatile 访问

设备寄存器通过内存映射 I/O（MMIO）暴露给内核。内核读写特定物理地址，效果相当于访问设备寄存器。

常见设备地址如下：

| 设备 | MMIO 地址 | PLIC IRQ |
|---|---:|---:|
| VirtIO Net | `0x10004000` | 4 |
| VirtIO Keyboard | `0x10005000` | 5 |
| VirtIO Mouse | `0x10006000` | 6 |
| VirtIO GPU | `0x10007000` | 7 |
| VirtIO Block | `0x10008000` | 8 |
| UART | `0x10000000` | 10 |

设备寄存器可能在程序看不到的情况下被硬件改变，因此必须使用 volatile 读写，防止编译器删除或合并看似多余的访问。

---

## 5. PLIC 外部中断控制器

PLIC（Platform-Level Interrupt Controller）负责收集外部设备中断，并将中断转发给处理器。

PLIC 初始化主要包括：

- 为中断源设置优先级；
- 为 Supervisor 上下文启用中断源；
- 设置优先级阈值；
- 开启 RISC-V Supervisor External Interrupt；
- 开启全局 Supervisor 中断。

一次外部中断的处理过程为：

```text
设备产生中断
    -> PLIC 记录中断源
    -> CPU 进入 SupervisorExternal trap
    -> trap_handler 调用板级 irq_handler
    -> PLIC claim 获得 IRQ 编号
    -> 调用对应设备的 handle_irq
    -> PLIC complete 完成中断
    -> 返回用户态
```

`claim` 用于取得当前待处理的最高优先级中断，`complete` 告诉 PLIC 该中断已经处理完成。遗漏 `complete` 可能导致同一中断无法正常再次上报。

---

## 6. 中断安全的内核数据

设备对象可能同时被普通内核代码和中断处理程序访问。如果普通代码持有锁时发生中断，而中断处理程序再次获取同一把锁，就可能死锁。

第九章使用 `UPIntrFreeCell` 保护这类数据。进入独占访问时，它会保存当前中断状态并关闭 Supervisor 中断；退出临界区时恢复原状态。

这样可以保证单核环境下：

- 临界区不会被同核中断处理程序打断；
- 不会出现普通代码与中断处理程序对同一对象的嵌套借用；
- 退出临界区后能够正确恢复原来的中断状态。

该方法适用于当前单核教程。在多核系统中，仅关闭本核中断不能阻止其他 CPU 并发访问，还需要自旋锁等跨核同步机制。

---

## 7. UART 中断驱动输入

UART 使用 NS16550A 兼容接口。初始化时开启接收数据中断：

```rust
let ier = IER::RX_AVAILABLE;
read_end.ier.write(ier);
```

UART 内核对象维护一个字符队列：

```rust
read_buffer: VecDeque<u8>
```

### 7.1 用户读取字符

如果缓冲区中已有字符，`read()` 直接返回。若缓冲区为空，则当前任务加入条件变量等待队列并阻塞：

```rust
let task_cx_ptr = self.condvar.wait_no_sched();
drop(inner);
schedule(task_cx_ptr);
```

### 7.2 UART 中断处理

UART 产生 IRQ 10 后，中断处理函数把硬件寄存器中的所有字符转移到内核队列：

```rust
while let Some(ch) = inner.ns16550a.read() {
    inner.read_buffer.push_back(ch);
}
```

只要读取到字符，就调用：

```rust
self.condvar.signal();
```

唤醒一个正在等待输入的任务。

这使 `getchar()` 在没有输入时真正阻塞，而不是不断轮询 UART 寄存器。

---

## 8. 条件变量与 `wait_no_sched`

设备驱动不能在持有设备独占访问权时直接调度，否则其他任务或中断处理程序可能永远无法访问设备。

因此等待操作被拆成两步：

1. `wait_no_sched()` 将任务加入等待队列，并把任务状态改为阻塞；
2. 离开设备独占区域并释放访问权；
3. 调用 `schedule()` 切换到其他任务。

```rust
let task_cx_ptr = device.exclusive_session(|dev| {
    // 提交请求
    condvar.wait_no_sched()
});
schedule(task_cx_ptr);
```

这个顺序同时避免了丢失唤醒和持锁调度，是本章理解异步设备访问的关键。

---

## 9. VirtIO 块设备

块设备同时提供同步和非阻塞两种访问方式。

### 9.1 同步模式

调度器启动前使用：

```rust
blk.read_block(block_id, buf)
blk.write_block(block_id, buf)
```

调用者一直等待设备完成请求。这种模式适合启动阶段读取文件系统。

### 9.2 非阻塞模式

调度器启动后使用：

```rust
let token = blk.read_block_nb(block_id, buf, &mut resp).unwrap();
```

每个异步请求得到一个 `token`。驱动为 VirtIO 队列的每个 token 建立条件变量：

```rust
condvars: BTreeMap<u16, Condvar>
```

任务提交请求后，等待 token 对应的条件变量。设备完成请求后产生 IRQ 8，中断处理程序从 used ring 中取回完成的 token：

```rust
while let Ok(token) = blk.pop_used() {
    self.condvars.get(&token).unwrap().signal();
}
```

因此即使同时存在多个请求，也能唤醒正确的任务。任务恢复后检查 `BlkResp`，确认操作状态为 `RespStatus::Ok`。

---

## 10. VirtIO 输入设备

键盘和鼠标分别使用两个 VirtIO Input 设备：

```rust
KEYBOARD_DEVICE -> 0x10005000
MOUSE_DEVICE    -> 0x10006000
```

设备中断到来后，驱动确认中断并取出全部待处理事件：

```rust
inner.virtio_input.ack_interrupt();
while let Some(event) = inner.virtio_input.pop_pending_event() {
    // 编码并进入队列
}
```

每个输入事件被编码成 `u64`：

```text
高 16 位：event_type
中 16 位：code
低 32 位：value
```

编码表达式为：

```rust
(event_type as u64) << 48 | (code as u64) << 32 | value as u64
```

鼠标相对位移可能是负数。如果低 32 位按无符号整数打印，例如 `4294967294`，其补码按 `i32` 解释就是 `-2`，属于正常的鼠标移动事件。

`sys_event_get()` 会先检查键盘队列，再检查鼠标队列；没有事件时返回 0，因此用户程序可以使用轮询方式读取输入事件。

---

## 11. VirtIO GPU 与帧缓冲

GPU 位于 `0x10007000`。初始化时调用：

```rust
virtio.setup_framebuffer()
virtio.setup_cursor(...)
```

驱动还将 `mouse.bmp` 转换为四通道像素数据并设置为鼠标光标。

系统调用 `sys_framebuffer()` 将 GPU 帧缓冲映射到当前进程的固定虚拟地址：

```rust
const FB_VADDR: usize = 0x10000000;
```

分辨率为 1280×800，每个像素 4 字节，因此帧缓冲大小为：

```text
1280 * 800 * 4 = 4,096,000 bytes
```

用户程序直接修改映射后的内存，然后调用：

```rust
framebuffer_flush();
```

由内核执行 `GPU_DEVICE.flush()`。共享帧缓冲避免了每绘制一个像素都进行一次系统调用。

---

## 12. VirtIO 网络设备

网络设备位于：

```rust
const VIRTIO8: usize = 0x10004000;
```

底层驱动提供两个基本操作：

```rust
NET_DEVICE.transmit(data)
NET_DEVICE.receive(buf)
```

内核使用 `lose-net-stack` 解析和构造 ARP、UDP、TCP 数据包。

### 12.1 网络对象也是文件

`UDP`、`TCP` 和监听端口对象均实现 `File` trait，并被放入进程的 `fd_table`。因此用户程序继续使用统一接口：

```rust
read(fd, buf)
write(fd, data)
close(fd)
```

这体现了 Unix“一切皆文件”的设计思想。

### 12.2 ARP

收到 ARP 请求后，协议栈根据本机 IP 和 MAC 构造 ARP 回复，再通过 VirtIO Net 发送。

### 12.3 UDP

`connect()` 创建 UDP 对象并返回文件描述符。发送时构造 UDP/IP/以太网帧；接收时根据远端 IP、本地端口和远端端口查找 Socket，并把数据放入对应缓冲队列。

示例程序连接到 QEMU 宿主机地址 `10.0.2.2`，向 UDP 端口 26099 发送消息并等待回复。

### 12.4 TCP

`listen(port)` 在监听表中登记端口，`accept()` 等待 TCP SYN。收到 SYN 后，内核创建 TCP 文件对象并回复 SYN+ACK。HTTP 示例监听客户机的 80 端口，通过 QEMU 端口转发，可以从宿主机访问：

```text
http://localhost:6201/
```

访问 `/close` 可以让示例服务器退出：

```text
http://localhost:6201/close
```

本章网络栈主要用于教学，TCP 状态机、重传、拥塞控制、超时以及异步等待等机制并不完整，不能视为生产级网络实现。

---

## 13. 实验运行与测试

依赖下载完成后，可使用离线模式构建：

```bash
cd os
CARGO_NET_OFFLINE=true make run
```

若需要图形界面：

```bash
CARGO_NET_OFFLINE=true make run GUI=on
```

本次实验启动时成功识别：

- 10 MHz 平台时钟；
- 1280×800 VirtIO GPU；
- VirtIO Keyboard；
- VirtIO Mouse；
- 32 MiB VirtIO Block 设备；
- VirtIO Net 设备。

### 13.1 用户测试

运行 `usertests` 得到：

```text
34 of sueecssed apps, 9 of failed apps run correctly.
Usertests passed!
```

其中 `adder`、`adder_simple_spin` 和 `adder_simple_yield` 等程序故意制造数据竞争并触发断言失败。测试框架期待这些程序失败，因此最终出现 `Usertests passed!` 表示结果正确。

### 13.2 图形与输入测试

成功运行：

```text
gui_shape
gui_tri
gui_move
gui_snake
inputdev_event
```

图形程序能够显示并刷新画面，输入测试能够输出键盘 Press/Release 以及鼠标移动和按键事件。

### 13.3 多线程大文件写入

单线程测试成功：

```text
1MiB written by 1 threads, time cost = 7397ms, write speed = 138KiB/s
```

`huge_write_mt 2` 和 `huge_write_mt 4` 在当前实现中可能长时间不返回。它们不属于官方 `usertests` 的验收集合。该现象可以记录为 easy-fs、同步锁和异步块设备 I/O 在多线程高并发场景下的当前限制，后续可作为死锁分析和文件系统并发改进方向。

---

## 14. 本章总结

第九章将前几章的机制组合成了较完整的 I/O 子系统：

- trap 机制负责接收设备中断；
- PLIC 负责路由和管理外部中断；
- `UPIntrFreeCell` 保护中断上下文共享数据；
- 条件变量负责阻塞和唤醒等待 I/O 的任务；
- VirtIO 队列和 token 区分多个异步请求；
- 文件描述符统一普通文件、管道和网络连接；
- 地址空间映射让用户程序高效访问帧缓冲；
- 设备树提供平台硬件描述。

至此，rCore-Tutorial-v3 已经覆盖从裸机启动、内存管理、进程、文件系统、IPC、信号、线程同步到设备与网络的主要操作系统机制。后续可以在此基础上继续实现新的调度算法、系统调用、设备驱动以及更完整的文件系统和网络协议栈。