@phao90016-jpg

# Chapter 4：地址空间与虚拟内存

## 1. 实验目标

本章在第三章分时多任务系统的基础上引入 Sv39 虚拟内存，实现并理解：

- 物理地址、虚拟地址、物理页号和虚拟页号；
- 物理页帧的分配与自动回收；
- Sv39 三级页表；
- 页表项及页面访问权限；
- 内核地址空间和用户地址空间；
- 从 ELF 创建用户地址空间；
- 用户栈、Guard Page 和用户堆；
- `sbrk` 动态扩展与收缩用户堆；
- Trampoline 和 TrapContext；
- 用户页表与内核页表之间的切换。

第三章中，各个应用使用不同的固定物理地址；第四章中，每个应用拥有独立页表，不同应用可以使用相同虚拟地址，但映射到不同物理页面。

## 2. 实验环境

- 操作系统：WSL2 Ubuntu
- Rust：rustc 1.97.1
- QEMU：8.2.2
- 目标架构：RISC-V 64
- 官方基线：`upstream/ch4`
- 个人分支：`learning/ch4`

运行命令：

```bash
cd os
make run
```

## 3. 第四章新增模块

第四章新增的内存管理模块如下：

```text
os/src/mm/address.rs
os/src/mm/frame_allocator.rs
os/src/mm/heap_allocator.rs
os/src/mm/page_table.rs
os/src/mm/memory_set.rs
os/src/mm/mod.rs
```

| 模块 | 作用 |
|---|---|
| `address.rs` | 定义物理地址、虚拟地址和页号 |
| `frame_allocator.rs` | 分配和回收物理页帧 |
| `heap_allocator.rs` | 提供内核动态内存分配 |
| `page_table.rs` | 实现 Sv39 三级页表 |
| `memory_set.rs` | 管理完整地址空间和连续映射区域 |
| `mod.rs` | 初始化并导出内存管理功能 |

新增依赖：

| 依赖 | 作用 |
|---|---|
| `xmas-elf` | 解析用户程序 ELF |
| `buddy_system_allocator` | 内核堆分配 |
| `bitflags` | 表示页表项及映射权限 |
| `spin` | 裸机环境中的同步支持 |

## 4. 实验运行结果

内核成功输出各段地址：

```text
.text [0x80200000, 0x8020e000)
.rodata [0x8020e000, 0x80212000)
.data [0x80212000, 0x80240000)
.bss [0x80240000, 0x80551000)
```

随后完成内核地址空间映射：

```text
mapping .text section
mapping .rodata section
mapping .data section
mapping .bss section
mapping physical memory
mapping memory-mapped registers
[kernel] back to world!
remap_test passed!
```

这说明内核页表已经成功启用，并且代码段、只读数据段和可写数据段的权限符合预期。

用户程序测试结果包括：

```text
Test power_3 OK!
Test power_5 OK!
Test power_7 OK!
Test sleep OK!
All applications completed!
```

非法地址读取和写入均触发 PageFault：

```text
[kernel] PageFault in application, bad addr = 0x0, ...
```

错误应用被内核终止，其他任务仍能继续运行。

## 5. Sv39 地址结构

本章定义四种地址类型：

```rust
pub struct PhysAddr(pub usize);
pub struct VirtAddr(pub usize);
pub struct PhysPageNum(pub usize);
pub struct VirtPageNum(pub usize);
```

| 类型 | 含义 |
|---|---|
| `PhysAddr` | 物理地址 PA |
| `VirtAddr` | 虚拟地址 VA |
| `PhysPageNum` | 物理页号 PPN |
| `VirtPageNum` | 虚拟页号 VPN |

项目采用：

```text
虚拟地址有效位：39 位
物理地址有效位：56 位
页面大小：4096 字节
页内偏移：12 位
VPN：27 位
PPN：44 位
```

Sv39 虚拟地址结构：

```text
63                    39 38        30 29       21 20       12 11       0
+-----------------------+------------+-----------+-----------+----------+
|       符号扩展         |   VPN[2]   |  VPN[1]   |  VPN[0]   | 页内偏移 |
+-----------------------+------------+-----------+-----------+----------+
                              9 位        9 位       9 位       12 位
```

如果虚拟地址第 38 位为 1，则高位全部扩展为 1；否则高位全部为 0。这保证地址满足 Sv39 canonical address 的要求。

## 6. 地址与页号转换

向下取整：

```rust
pub fn floor(&self) -> VirtPageNum
```

用于得到某地址所在的虚拟页。

向上取整：

```rust
pub fn ceil(&self) -> VirtPageNum
```

用于计算覆盖地址区间所需的页面终点。

页内偏移：

```rust
self.0 & (PAGE_SIZE - 1)
```

相当于取地址的低 12 位。

页号转地址：

```text
地址 = 页号 << 12
```

## 7. 三级页表索引

```rust
pub fn indexes(&self) -> [usize; 3]
```

将 27 位 VPN 拆为三个 9 位索引：

```text
[VPN[2], VPN[1], VPN[0]]
```

每级页表有：

```text
2^9 = 512
```

个页表项。

一个页表页面大小为 4096 字节，一个 PTE 为 8 字节：

```text
4096 / 8 = 512
```

页表遍历过程：

```text
根页表
→ VPN[2]
→ 第二级页表
→ VPN[1]
→ 第三级页表
→ VPN[0]
→ 最终页表项
```

## 8. 物理页帧分配

物理内存被划分为 4 KiB 的页帧，每个页帧由 `PhysPageNum` 表示。

可分配物理内存范围：

```text
[ekernel 向上页对齐, MEMORY_END 向下页对齐)
```

因此内核自身占用的物理页不会进入分配器。

页帧分配器：

```rust
pub struct StackFrameAllocator {
    current: usize,
    end: usize,
    recycled: Vec<usize>,
}
```

分配策略：

```text
优先从 recycled 弹出回收页
→ 如果没有回收页，则从 current 顺序分配
→ current == end 时返回 None
```

回收使用 `Vec` 的 `push` 和 `pop`，因此表现为栈式回收。

## 9. `FrameTracker` 与自动回收

```rust
pub struct FrameTracker {
    pub ppn: PhysPageNum,
}
```

分配页面时会清零整个物理页：

```rust
let bytes_array = ppn.get_bytes_array();
for byte in bytes_array {
    *byte = 0;
}
```

清零可以：

- 避免旧数据污染新页；
- 防止新进程读取其他进程的历史数据；
- 保证新建页表项初始为无效状态。

自动回收：

```rust
impl Drop for FrameTracker {
    fn drop(&mut self) {
        frame_dealloc(self.ppn);
    }
}
```

因此：

```text
FrameTracker 生命周期
=
对应物理页被占用的生命周期
```

## 10. 页表项

页表项标志：

| 标志 | 含义 |
|---|---|
| `V` | 有效 |
| `R` | 可读 |
| `W` | 可写 |
| `X` | 可执行 |
| `U` | 用户态可访问 |
| `G` | 全局映射 |
| `A` | 已访问 |
| `D` | 已写入 |

页表项创建：

```rust
bits: ppn.0 << 10 | flags.bits as usize
```

低 10 位保存标志，PPN 从第 10 位开始。

用户代码页通常具有：

```text
V | R | X | U
```

用户数据页通常具有：

```text
V | R | W | U
```

内核页不包含 `U`，因此用户态不能访问。

## 11. `PageTable`

```rust
pub struct PageTable {
    root_ppn: PhysPageNum,
    frames: Vec<FrameTracker>,
}
```

- `root_ppn` 是根页表物理页号；
- `frames` 保存所有页表页面的所有权。

创建页表时先分配根页表页面：

```rust
let frame = frame_alloc().unwrap();
```

中间级页表不存在时：

```text
分配新的物理页
→ 设置中间 PTE 的 V 标志
→ 将 FrameTracker 加入 frames
```

当 `PageTable` 被销毁时，`frames` 中的物理页会被自动回收。

## 12. 建立和取消映射

建立映射：

```rust
pub fn map(vpn, ppn, flags)
```

最终生成：

```text
VPN → PPN + 权限
```

如果 VPN 已经存在有效映射，则触发断言，防止重复映射。

取消映射：

```rust
pub fn unmap(vpn)
```

将最终 PTE 清零。

`unmap` 只删除页表项，数据页的实际回收由 `MapArea.data_frames` 中的 `FrameTracker` 管理。

## 13. `satp` Token

```rust
pub fn token(&self) -> usize {
    8usize << 60 | self.root_ppn.0
}
```

`satp` 包含：

```text
MODE | ASID | 根页表 PPN
```

这里：

```text
MODE = 8，即 Sv39
ASID = 0
PPN = root_ppn
```

将 Token 写入 `satp` 后，CPU 开始使用对应根页表进行虚拟地址翻译。

## 14. 跨页用户缓冲区翻译

用户缓冲区可能跨越多个虚拟页面，而对应的物理页不一定连续。

```rust
translated_byte_buffer(token, ptr, len)
```

处理过程：

```text
用户虚拟地址
→ 得到 VPN 和页内偏移
→ 使用用户页表得到 PPN
→ 取得对应物理页切片
→ 继续处理下一页
```

因此返回：

```rust
Vec<&'static mut [u8]>
```

而不是单一连续切片。

第二章中内核可以直接访问用户指针；第四章中必须先通过用户页表进行地址翻译。

## 15. `MemorySet`

```rust
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}
```

它表示一个完整地址空间：

```text
MemorySet
├── PageTable
├── MapArea(.text)
├── MapArea(.rodata)
├── MapArea(.data)
├── MapArea(.bss)
├── MapArea(user stack)
├── MapArea(user heap)
└── MapArea(TrapContext)
```

`new_bare()` 创建一个只有空页表的地址空间。

## 16. `MapArea`

```rust
pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
}
```

它描述：

```text
连续 VPN 区间
+ 映射方式
+ 页面权限
+ 所拥有的物理页
```

两种映射方式：

```rust
pub enum MapType {
    Identical,
    Framed,
}
```

### 16.1 `Identical`

```text
VPN = PPN
VA = PA
```

主要用于内核地址空间。

### 16.2 `Framed`

为每个 VPN 分配一个新物理页：

```text
VPN → 新分配的 PPN
```

用于用户程序、用户栈、用户堆和 TrapContext。

## 17. 内核地址空间

内核地址空间采用恒等映射。

权限如下：

| 区域 | 权限 |
|---|---|
| `.text` | `R | X` |
| `.rodata` | `R` |
| `.data` | `R | W` |
| `.bss` | `R | W` |
| 剩余物理内存 | `R | W` |
| MMIO | `R | W` |

`remap_test()` 检查：

```text
.text 不可写
.rodata 不可写
.data 不可执行
```

运行输出：

```text
remap_test passed!
```

证明权限配置正确。

## 18. 从 ELF 创建用户地址空间

```rust
pub fn from_elf(elf_data: &[u8])
    -> (Self, usize, usize)
```

返回：

```text
用户地址空间
用户栈顶
ELF 入口地址
```

首先检查 ELF Magic：

```text
0x7F 'E' 'L' 'F'
```

然后遍历 ELF Program Header，只加载 `Type::Load` 段。

权限根据 ELF 标志转换：

```text
Read    → R
Write   → W
Execute → X
```

用户程序段还必须添加 `U`：

```text
用户 .text   → R | X | U
用户 .rodata → R | U
用户 .data   → R | W | U
用户 .bss    → R | W | U
```

## 19. 用户栈和 Guard Page

用户栈放置在 ELF 最高地址之后。

ELF 与用户栈之间留出一个未映射页面：

```text
ELF 最高地址
→ Guard Page
→ 用户栈
```

如果用户栈向下越界，将访问 Guard Page 并触发 PageFault，而不是破坏程序数据。

用户栈权限：

```text
R | W | U
```

## 20. 用户堆和 `sbrk`

在用户栈顶部创建一个初始为空的映射区域，作为用户堆：

```text
heap_bottom = user_stack_top
program_brk = user_stack_top
```

扩大堆：

```text
sbrk(正数)
→ MemorySet::append_to
→ 分配物理页
→ 建立页表映射
```

缩小堆：

```text
sbrk(负数)
→ MemorySet::shrink_to
→ 删除页表映射
→ FrameTracker::drop
→ 回收物理页
```

实验输出：

```text
one page allocated
write ok
11 page DEALLOCATED
```

撤销映射后再次访问：

```text
bad addr = 0x15000
PageFault
```

说明页面回收和内存保护均已生效。

## 21. TrapContext 映射

每个用户地址空间都在固定虚拟地址：

```text
TRAP_CONTEXT
```

映射独立物理页。

权限为：

```text
R | W
```

没有 `U`，因此用户态不能访问 TrapContext。

不同进程的映射关系：

```text
进程 A：TRAP_CONTEXT → PPN A
进程 B：TRAP_CONTEXT → PPN B
进程 C：TRAP_CONTEXT → PPN C
```

内核通过任务控制块保存的 `trap_cx_ppn` 直接访问对应物理页。

## 22. Trampoline

Trampoline 被映射到每个用户页表和内核页表中的相同高虚拟地址：

```text
用户页表：TRAMPOLINE → strampoline 物理页
内核页表：TRAMPOLINE → strampoline 物理页
```

权限：

```text
R | X
```

切换 `satp` 后地址空间会立即改变。Trampoline 在两个地址空间中具有相同虚拟地址和物理映射，因此页表切换前后 CPU 都能继续执行 Trap 汇编代码。

## 23. 第四章任务控制块

```rust
pub struct TaskControlBlock {
    pub task_status: TaskStatus,
    pub task_cx: TaskContext,
    pub memory_set: MemorySet,
    pub trap_cx_ppn: PhysPageNum,
    pub base_size: usize,
    pub heap_bottom: usize,
    pub program_brk: usize,
}
```

与第三章相比，任务新增：

- 独立地址空间；
- TrapContext 物理页号；
- 用户堆起点；
- 当前 program break。

任务已经从单纯的 CPU 执行状态扩展为拥有独立内存资源的进程雏形。

## 24. 第四章 TrapContext

```rust
pub struct TrapContext {
    pub x: [usize; 32],
    pub sstatus: Sstatus,
    pub sepc: usize,
    pub kernel_satp: usize,
    pub kernel_sp: usize,
    pub trap_handler: usize,
}
```

新增字段：

| 字段 | 作用 |
|---|---|
| `kernel_satp` | 内核页表 Token |
| `kernel_sp` | 当前任务内核栈 |
| `trap_handler` | Rust Trap 处理入口 |

它们对应 `trap.S` 中的固定偏移：

```asm
ld t0, 34*8(sp)
ld sp, 35*8(sp)
ld t1, 36*8(sp)
```

## 25. 用户态进入内核态

用户程序运行时：

```text
satp = 用户页表
sp = 用户栈
sscratch = TRAP_CONTEXT
```

发生 Trap 后：

```text
CPU 跳转到 TRAMPOLINE::__alltraps
→ 交换 sp 与 sscratch
→ sp 指向 TrapContext
→ 保存用户寄存器、sstatus 和 sepc
→ 读取 kernel_satp
→ 读取 kernel_sp
→ 读取 trap_handler
→ 写入内核 satp
→ sfence.vma
→ 切换到内核栈
→ 跳转 trap_handler
```

## 26. `stvec` 的切换

用户程序运行时：

```text
stvec = TRAMPOLINE
```

用户态 Trap 进入 `__alltraps`。

内核运行时：

```text
stvec = trap_from_kernel
```

当前章节尚未实现完整的内核态 Trap 处理，因此内核态发生 Trap 会 panic。

## 27. 从内核返回用户态

`trap_return()` 准备：

```text
a0 = TRAP_CONTEXT 虚拟地址
a1 = 当前用户页表 Token
```

然后计算 `__restore` 在 Trampoline 中的虚拟地址：

```rust
restore_va =
    __restore地址
    - __alltraps地址
    + TRAMPOLINE;
```

跳入 `__restore` 后：

```text
写入用户 satp
→ sfence.vma
→ sp 指向用户 TrapContext
→ 恢复 sstatus 和 sepc
→ 恢复用户寄存器
→ 恢复用户栈
→ sret
→ 返回用户程序
```

## 28. `TaskContext` 的变化

第三章中新任务的初始 `ra` 是 `__restore`。第四章改为：

```rust
ra: linker_symbol_addr!(trap_return)
```

原因是第四章的 `__restore` 需要：

```text
a0 = TRAP_CONTEXT
a1 = 用户页表 Token
```

因此新任务启动路径为：

```text
__switch
→ ret
→ trap_return
→ 准备 a0 和 a1
→ 跳转 TRAMPOLINE::__restore
→ 切换用户页表
→ sret
```

## 29. 任务之间的地址空间切换

假设从任务 A 切换到任务 B：

```text
任务 A 用户态
│ satp = A 用户页表
│
├─ 时钟中断
↓
TRAMPOLINE::__alltraps
│ 保存 A 的 TrapContext
│
├─ satp = 内核页表
├─ sp = A 的内核栈
↓
trap_handler
↓
suspend_current_and_run_next
↓
__switch
│ 保存 A 的 TaskContext
│ 恢复 B 的 TaskContext
↓
trap_return
│ a0 = B 的 TRAP_CONTEXT
│ a1 = B 的用户页表 Token
↓
TRAMPOLINE::__restore
│ satp = B 用户页表
│ 恢复 B 的 TrapContext
↓
sret
↓
任务 B 用户态
```

`__switch` 切换内核任务上下文，`__restore` 切换用户页表并恢复用户现场。

## 30. 用户地址空间布局

一个用户地址空间大致如下：

```text
低地址
┌──────────────────────┐
│ 用户 ELF 代码和数据   │ R/W/X/U
├──────────────────────┤
│ Guard Page（未映射）  │
├──────────────────────┤
│ 用户栈                │ R/W/U
├──────────────────────┤
│ 用户堆（动态扩展）     │ R/W/U
├──────────────────────┤
│ 未映射空间            │
├──────────────────────┤
│ TrapContext           │ R/W，无 U
├──────────────────────┤
│ Trampoline            │ R/X，无 U
└──────────────────────┘
高地址
```

## 31. 第四章完整启动流程

```text
rust_main
→ clear_bss
→ logging::init
→ device_tree::init
→ mm::init
→ 初始化内核堆
→ 初始化页帧分配器
→ 创建 KERNEL_SPACE
→ 激活内核页表
→ remap_test
→ trap::init
→ 加载用户 ELF
→ 为每个任务创建独立 MemorySet
→ 初始化 TrapContext 和 TaskContext
→ 启动第一个任务
```

## 32. 第四章完整 Trap 流程

```text
用户程序
用户页表 + 用户栈
        ↓ Trap
TRAMPOLINE::__alltraps
        ↓
保存 TrapContext
        ↓
读取 kernel_satp / kernel_sp / trap_handler
        ↓
切换内核页表
        ↓
trap_handler
        ↓
可能执行 __switch 切换任务
        ↓
trap_return
        ↓
TRAMPOLINE::__restore
        ↓
切换目标任务用户页表
        ↓
恢复 TrapContext
        ↓
sret
        ↓
目标用户程序
```

## 33. 当前实现的限制

本章实现仍然是教学版本：

- 用户指针权限检查还不完整；
- 某些无效用户地址可能导致内核 `unwrap()`；
- 没有使用 ASID；
- 页表更新主要依赖 `sfence.vma` 全量刷新 TLB；
- 每个任务仍然由内核启动时静态加载；
- 内核态 Trap 处理尚不完整；
- 尚未实现 `fork`、`exec` 和进程树。

这些功能将在后续章节逐步完善。

## 34. 本章总结

第四章为每个用户任务建立了独立的 Sv39 虚拟地址空间。

本章的关键机制是：

1. 使用强类型区分物理地址、虚拟地址和页号；
2. 使用 `FrameTracker` 管理物理页生命周期；
3. 使用 Sv39 三级页表建立 VPN 到 PPN 的映射；
4. 使用 PTE 权限保护代码、数据和内核内存；
5. 使用 `MemorySet` 和 `MapArea` 管理完整地址空间；
6. 从 ELF 创建用户程序代码、数据、栈和堆；
7. 使用 Guard Page 检测栈越界；
8. 使用 `sbrk` 动态映射和回收用户堆页面；
9. 使用 TrapContext 保存用户现场和内核入口信息；
10. 使用 Trampoline 在用户页表与内核页表之间安全切换；
11. 在任务切换时恢复目标任务的用户页表。

第三章与第四章的核心区别：

```text
第三章：多个任务共享同一个地址空间
第四章：每个任务拥有独立的虚拟地址空间
```

第四章核心执行路径：

```text
用户页表
→ Trampoline
→ 内核页表
→ trap_handler
→ 任务调度
→ Trampoline
→ 目标用户页表
```
