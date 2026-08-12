# 虚拟内存扩展实验：COW Fork 与匿名 mmap

## 1. 实验目标

本实验基于 rCore-Tutorial-v3 第九章代码，进一步完善用户虚拟内存管理，主要实现：

- 物理页引用计数；
- Copy-on-Write（COW）fork；
- COW 写缺页处理；
- 内核写用户缓冲区时的 COW 隔离保护；
- 普通 fork 与 COW fork 的物理页分配数量对比；
- 匿名私有 `mmap/munmap`；
- 匿名映射与 COW fork 的协同工作；
- 父子进程内存隔离测试。

开发分支为 `feature/cow-memory`。

## 2. 原有 fork 的问题

原始 `MemorySet::from_existed_user()` 在 fork 时为子进程的每个用户页重新分配物理页，并复制完整页面。即使子进程随后立刻调用 `exec()`，这些复制也会被丢弃。

普通 fork 的主要成本为：

```text
遍历父进程用户页
  -> 为子进程分配物理页
  -> 复制 4 KiB 页面
  -> 建立子进程映射
```

COW fork 改为让父子进程暂时共享物理页。只有其中一方首次写入时，才复制被写入的页面。

## 3. 物理页引用计数

### 3.1 数据结构

物理页分配器增加：

```rust
ref_counts: BTreeMap<usize, usize>
```

新分配物理页的引用计数为 1。`FrameTracker::clone()` 调用 `frame_add_ref()`，引用计数加 1；`FrameTracker::drop()` 调用 `frame_dealloc()`，引用计数减 1。只有计数降为 0 时，物理页才加入 `recycled`。

### 3.2 VirtIO 连续页约束

`frame_alloc_more()` 被 VirtIO DMA 使用。当前 `VirtioHal::dma_alloc()` 将返回向量的最后一个物理页视为连续区域基址，因此必须保留原有的“高 PPN 到低 PPN”返回顺序。改变顺序会导致 VirtIO 初始化报 `BufferTooSmall`。

## 4. COW 页表标志

Sv39 页表项的第 8～9 位为 RSW，由操作系统软件使用。本实验使用第 8 位表示 COW：

```rust
const PTE_COW: usize = 1 << 8;
```

可写用户页进入 COW 状态时：

1. 清除硬件 `W` 权限；
2. 设置软件 `COW` 标志；
3. 父子页表映射同一个物理页；
4. 增加物理页引用计数；
5. 执行 `sfence.vma` 刷新 TLB。

只读代码页可以直接共享，不设置 COW。TrapContext 会被内核分别修改，因此 fork 时仍立即复制。

## 5. COW fork

`MemorySet::from_existed_user_cow()` 遍历父地址空间的所有 `MapArea`：

- 用户可写页：父子都改为只读 COW 映射；
- 用户只读页：直接共享只读物理页；
- 非用户页：为子进程分配新页并复制；
- 页表页：父子各自分配。

fork 输出物理页统计：

```text
[cow] fork pages: eager=21 cow=6 saved=15 shared=15 copied=1
```

字段含义：

| 字段 | 含义 |
|---|---|
| `eager` | 普通 fork 预计新增的物理页数 |
| `cow` | COW fork 当下实际新增的物理页数 |
| `saved` | fork 阶段节省的物理页数 |
| `shared` | 父子共享的用户页数 |
| `copied` | 立即复制的非用户页数 |

示例中的 fork 阶段节省比例为：

```text
15 / 21 ≈ 71.4%
```

## 6. COW 写缺页处理

父子进程写入只读 COW 页时产生 `StorePageFault`。内核使用 `stval` 获得故障虚拟地址，并调用 `MemorySet::handle_cow_fault()`。

处理分为两种情况：

### 6.1 引用计数大于 1

说明物理页仍由多个地址空间共享：

1. 分配新物理页；
2. 复制旧页的 4 KiB 内容；
3. 当前进程改为映射新物理页；
4. 新映射恢复 `W` 权限并清除 COW；
5. 旧页引用计数减 1；
6. 刷新 TLB；
7. 重新执行产生缺页的写指令。

### 6.2 引用计数等于 1

说明当前进程已经是旧页的唯一拥有者，不需要复制，只需恢复写权限、清除 COW 并刷新 TLB。

对于非 COW 的非法写入，内核仍发送 `SIGSEGV`。

## 7. 内核写用户空间的隔离保护

用户态写入会经过页表权限检查，但内核的 `translated_refmut()` 和 `translated_byte_buffer()` 通过物理地址直接访问用户内存，不会自动触发用户页表的 COW 缺页。

因此增加 `MemorySet::ensure_private_range()`，在内核写用户地址前主动检查并拆分 COW 页。已接入：

- `sys_read()` 的用户接收缓冲区；
- `sys_pipe()` 的文件描述符数组；
- `sys_waitpid()` 的退出码指针。

这样可以避免内核直接修改共享物理页，破坏父子进程隔离。

## 8. 匿名 mmap/munmap

### 8.1 系统调用

新增：

```rust
mmap(start, len, prot)
munmap(start, len)
```

系统调用号：

| 系统调用 | 编号 |
|---|---:|
| `munmap` | 215 |
| `mmap` | 222 |

用户权限标志：

```rust
PROT_READ  = 1 << 0
PROT_WRITE = 1 << 1
PROT_EXEC  = 1 << 2
```

RISC-V 不允许有效叶子页使用 `W=1, R=0`，因此 `PROT_WRITE` 会同时添加读权限。

### 8.2 地址范围与选址

匿名映射使用：

```text
0x2000_0000..0x3000_0000
```

`start=0` 时，内核从 `MMAP_BASE` 开始逐页寻找无冲突区间；指定地址时要求页对齐、位于 mmap 区间内且不与现有 `MapArea` 重叠。

两个半开区间重叠的判断为：

```text
new_start < old_end && old_start < new_end
```

### 8.3 munmap

第一版 `munmap` 要求参数精确匹配一个完整的匿名映射区。成功时：

1. 从 `MemorySet.areas` 删除对应 `MapArea`；
2. 删除页表项；
3. `FrameTracker` 析构并减少引用计数；
4. 引用计数为 0 的物理页被回收；
5. 执行 `sfence.vma`。

部分区间解除需要拆分 `MapArea`，留作后续扩展。

## 9. mmap 与 COW 的协同

匿名映射使用 `MapType::Framed` 和用户权限，因此 fork 时会自动进入现有 COW 流程。

测试流程：

```text
父进程 mmap 两页
  -> 写入 100 和 200
  -> fork
  -> 子进程写入 300 和 400
  -> 子进程发生两个 COW 写缺页
  -> 父进程仍读取到 100 和 200
  -> 父子分别 munmap
```

## 10. 测试结果

### 10.1 COW 隔离测试

```text
COW fork isolation test start
child: checking inherited values
child: writing private COW pages
child: private writes passed
parent: checking memory isolation
cow_fork_test passed!
```

覆盖全局数据、两页数组、用户栈和父子内存隔离。

### 10.2 基础 mmap 测试

```text
mmap_test: mapped two pages at 0x20000000
mmap_test: read/write passed
mmap_test: overlap rejection passed
mmap_test: partial munmap rejection passed
mmap_test: full munmap passed
mmap_test: address reuse passed
mmap_test passed!
```

### 10.3 mmap COW 测试

```text
mmap_cow_test: parent mapped at 0x20000000
[cow] fork pages: eager=24 cow=7 saved=17 shared=17 copied=1
mmap_cow_test: child checks inherited data
mmap_cow_test: child private writes passed
mmap_cow_test: parent isolation passed
mmap_cow_test passed!
```

### 10.4 回归测试

已通过：

- `forktest_simple`；
- `forktest`；
- `forktest2`；
- `pipetest`；
- `pipe_large_test`；
- `filetest_simple`；
- `cow_fork_test`；
- `mmap_test`；
- `mmap_cow_test`。

## 11. 当前限制与后续计划

当前 `mmap` 为匿名、私有、立即分配实现，尚不支持：

- 懒分配；
- 部分区间 `munmap`；
- 文件映射；
- `MAP_SHARED`；
- 页面换出；
- 多核并发引用计数。

后续可以让 `mmap()` 只登记虚拟区间，在第一次 Load/Store Page Fault 时分配物理页；也可以实现 `MapArea` 拆分，从而支持任意页对齐子区间的 `munmap`。

## 12. 实验总结

本实验将 rCore 原有的立即复制 fork 改造为按需复制的 COW fork，并以引用计数管理共享物理页生命周期。COW 不仅需要修改 fork，还必须正确处理页表软件标志、TLB 刷新、TrapContext、用户写缺页和内核写用户空间等边界。

匿名 mmap 复用了 `MemorySet`、`MapArea`、页表和 `FrameTracker`，并自然接入 COW fork。测试表明，在典型实验程序中，COW fork 可在 fork 当下节省约 15～17 个物理页，同时保持父子进程数据隔离。
