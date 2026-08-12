# Chapter 6 文件系统：easy-fs 与文件描述符

## 1. 本章目标

第六章在第五章进程管理和虚拟内存的基础上加入文件系统，使内核能够：

- 通过 VirtIO 块设备访问 QEMU 虚拟磁盘；
- 使用 easy-fs 管理磁盘上的 inode、目录和数据块；
- 使用块缓存减少设备访问并延迟写回；
- 向进程提供文件描述符表；
- 实现 `open`、`close`、`read`、`write` 系统调用；
- 从文件系统中读取用户程序 ELF，并通过 `exec` 执行；
- 支持普通文件、标准输入和标准输出的统一抽象。

本章完成后，用户程序不再由构建脚本直接嵌入内核，而是存放在 `fs.img` 文件系统镜像中。

---

## 2. 实验环境与分支

- Windows + WSL2 Ubuntu
- QEMU 8.2.2
- Rust 1.97.1
- RISC-V 目标：`riscv64gc-unknown-none-elf`
- 官方远程仓库：`upstream`
- 个人课设仓库：`origin`
- 本章本地分支：`learning/ch6`
- 本章远程分支：`origin/learning/ch6`

仓库关系为：

```text
upstream/ch6
    │ 官方第六章代码
    ▼
learning/ch6
    │ 本地学习、实验和文档
    ▼
origin/learning/ch6
    个人课设仓库中的提交
```

分支建立方式：

```bash
git fetch upstream
git switch -c learning/ch6 upstream/ch6
git push -u origin learning/ch6
```

---

## 3. 第五章到第六章的主要变化

第六章新增三个重要组成部分：

```text
easy-fs/          文件系统核心实现
easy-fs-fuse/     在宿主机上创建文件系统镜像
os/src/fs/        操作系统文件抽象和文件描述符接口
```

同时新增：

```text
os/src/drivers/block/     VirtIO 块设备驱动
user/src/bin/filetest_simple.rs
user/src/bin/cat_filea.rs
user/src/bin/huge_write.rs
```

第五章中的 `os/src/loader.rs` 被删除。用户程序由“嵌入内核”改成“存放在文件系统中”。

---

## 4. 第六章总体架构

```text
用户程序
open/read/write/close
        │ ecall
        ▼
系统调用层
sys_open/sys_read/sys_write/sys_close
        ▼
进程 fd_table
Arc<dyn File>
        ▼
OSInode / Stdin / Stdout
        ▼
easy_fs::Inode
        ▼
DiskInode 与块索引
        ▼
BlockCache
        ▼
BlockDevice trait
        ▼
VirtIOBlock
        ▼
QEMU 虚拟磁盘 fs.img
```

构建镜像时则使用另一条路径：

```text
Linux 宿主机文件
        ▼
easy-fs-fuse
        ▼
BlockFile 实现 BlockDevice
        ▼
easy-fs
        ▼
fs.img
```

内核和宿主机工具使用同一套 easy-fs，仅底层块设备实现不同。

---

## 5. 块设备抽象

`easy-fs/src/block_dev.rs` 定义：

```rust
pub trait BlockDevice: Send + Sync + Any {
    fn read_block(&self, block_id: usize, buf: &mut [u8]);
    fn write_block(&self, block_id: usize, buf: &[u8]);
}
```

easy-fs 只关心“读写第几个块”，不关心具体硬件。

本章有两种实现：

| 运行位置 | 实现 | 用途 |
|---|---|---|
| Linux 宿主机 | `BlockFile` | 创建并填充 `fs.img` |
| rCore 内核 | `VirtIOBlock` | 运行时访问 QEMU 磁盘 |

这种接口设计降低了文件系统与设备驱动之间的耦合。

---

## 6. 块缓存 BlockCache

每个磁盘块大小为：

```text
BLOCK_SZ = 512 字节
```

`BlockCache` 保存：

```rust
pub struct BlockCache {
    cache: CacheData,
    block_id: usize,
    block_device: Arc<dyn BlockDevice>,
    modified: bool,
}
```

加载缓存时调用：

```rust
block_device.read_block(block_id, cache.as_mut());
```

调用 `get_mut()` 或 `modify()` 时设置：

```rust
self.modified = true;
```

执行 `sync()` 或缓存被销毁时，脏块才写回设备：

```rust
block_device.write_block(self.block_id, self.cache.as_ref());
```

因此它属于写回缓存。

块缓存管理器最多保存 16 个缓存块：

```rust
const BLOCK_CACHE_SIZE: usize = 16;
```

缓存满时，从队列前部开始寻找 `Arc::strong_count() == 1` 的缓存淘汰。该策略不是严格 LRU，而是按队列顺序选择当前没有外部使用者的缓存。

---

## 7. 位图分配器 Bitmap

位图用一个 bit 表示一个资源：

```text
0：空闲
1：已分配
```

一个 bitmap 块包含：

```rust
type BitmapBlock = [u64; 64];
```

因此一个块可记录：

```text
64 × 64 = 4096 个资源
```

`decomposition(bit)` 将资源编号拆成：

```text
bitmap 块位置
→ 块内 u64 位置
→ u64 内 bit 位置
```

文件系统使用两类位图：

- inode bitmap：分配 inode；
- data bitmap：分配数据块。

`alloc()` 找到空闲 bit 并置 1；`dealloc()` 检查资源已分配后将对应 bit 清零。

---

## 8. easy-fs 磁盘布局

磁盘从前到后划分为：

```text
┌──────────────────────┐
│ SuperBlock           │ 块0
├──────────────────────┤
│ inode bitmap         │ inode分配状态
├──────────────────────┤
│ inode area           │ DiskInode
├──────────────────────┤
│ data bitmap          │ 数据块分配状态
├──────────────────────┤
│ data area            │ 文件和目录内容
└──────────────────────┘
```

`SuperBlock` 保存：

- magic number；
- 总块数；
- inode bitmap 块数；
- inode 区块数；
- data bitmap 块数；
- data 区块数。

打开文件系统时检查：

```rust
self.magic == EFS_MAGIC
```

从而避免把无效块设备误认为 easy-fs。

---

## 9. EasyFileSystem

`EasyFileSystem` 保存底层设备、两个位图和区域起始位置：

```rust
pub struct EasyFileSystem {
    pub block_device: Arc<dyn BlockDevice>,
    pub inode_bitmap: Bitmap,
    pub data_bitmap: Bitmap,
    inode_area_start_block: u32,
    data_area_start_block: u32,
}
```

主要方法包括：

- `create()`：格式化并创建文件系统；
- `open()`：读取超级块并打开已有文件系统；
- `root_inode()`：取得根目录 inode；
- `alloc_inode()`：分配 inode；
- `alloc_data()`：分配数据块；
- `dealloc_data()`：清空并回收数据块。

创建时第一个 inode 固定分配给根目录：

```rust
assert_eq!(efs.alloc_inode(), 0);
disk_inode.initialize(DiskInodeType::Directory);
```

所以 inode 0 表示根目录 `/`。

---

## 10. DiskInode

磁盘 inode 定义为：

```rust
pub struct DiskInode {
    pub size: u32,
    pub direct: [u32; 28],
    pub indirect1: u32,
    pub indirect2: u32,
    type_: DiskInodeType,
}
```

其大小为：

```text
size             4字节
direct[28]     112字节
indirect1        4字节
indirect2        4字节
type_            4字节
总计           128字节
```

一个 512 字节块正好存放 4 个 `DiskInode`。

inode 的磁盘位置计算为：

```text
block_id = inode_area_start
         + inode_id / inodes_per_block

offset = inode_id % inodes_per_block
       × inode_size
```

---

## 11. 文件块索引

一个间接索引块可以保存：

```text
512 ÷ 4 = 128 个 u32 块号
```

本章采用直接索引、一级间接索引和二级间接索引：

| 索引方式 | 数据块数 | 容量 |
|---|---:|---:|
| 直接索引 | 28 | 14 KiB |
| 一级间接 | 128 | 64 KiB |
| 二级间接 | 128 × 128 | 8 MiB |
| 总计 | 16540 | 约 8.08 MiB |

寻址过程：

```text
inner_id < 28
    → direct[inner_id]

28 ≤ inner_id < 156
    → indirect1[inner_id - 28]

inner_id ≥ 156
    → last = inner_id - 156
    → indirect2[last / 128]
    → indirect1[last % 128]
    → 数据块
```

二级间接结构为：

```text
DiskInode.indirect2
        ▼
二级索引块
   ├──→ 一级索引块0 ──→ 数据块
   ├──→ 一级索引块1 ──→ 数据块
   └──→ ...
```

---

## 12. 文件扩容和回收

VFS 层先调用：

```rust
disk_inode.blocks_num_needed(new_size)
```

计算新增文件大小需要多少数据块和索引块，然后通过 `alloc_data()` 分配块号，最终调用：

```rust
disk_inode.increase_size(new_size, new_blocks, block_device)
```

安装顺序为：

```text
直接数据块
→ 一级索引块
→ 一级间接数据块
→ 二级索引块
→ 二级索引下的一级索引块
→ 二级间接数据块
```

`clear_size()` 把文件大小设为 0，并返回应当回收的：

- 直接数据块；
- 一级索引块及其数据块；
- 二级索引块；
- 二级索引下的一级索引块及其数据块。

上层再调用 `dealloc_data()` 修改位图并清空磁盘块。

---

## 13. 跨块读写

`DiskInode::read_at()` 和 `write_at()` 按块处理数据。

例如，从偏移 500 读取 30 字节：

```text
块0：[500, 512) 读取12字节
块1：[0, 18)    读取18字节
```

每轮通过：

```rust
get_block_id(start_block)
```

找到实际数据块，并计算当前块内的起止位置。

`DiskInode::write_at()` 的前提是文件大小和数据块已经由上层扩展；它只负责写入已有块。

---

## 14. 目录项

目录项定义为：

```rust
pub struct DirEntry {
    name: [u8; 28],
    inode_number: u32,
}
```

每个目录项大小为 32 字节，一个磁盘块可保存：

```text
512 ÷ 32 = 16 个目录项
```

文件名最长 27 字节，剩余一个字节保存 `\0`。

目录本质上也是一个文件，它的数据内容是连续的：

```text
文件名 → inode编号
文件名 → inode编号
文件名 → inode编号
```

---

## 15. VFS Inode

内存中的 `Inode` 保存磁盘 inode 的定位信息：

```rust
pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}
```

关系为：

```text
Inode
  │ block_id + block_offset
  ▼
DiskInode
  │ direct / indirect1 / indirect2
  ▼
文件数据块
```

VFS 提供：

- `find(name)`；
- `create(name)`；
- `ls()`；
- `read_at(offset, buf)`；
- `write_at(offset, buf)`；
- `clear()`。

---

## 16. 查找与创建文件

`find()` 顺序读取目录中的所有 `DirEntry` 并比较名字，因此当前目录查找为线性搜索，复杂度约为 `O(n)`。

创建 `filea` 的流程：

```text
root_inode.create("filea")
        ↓
检查目录中是否已经存在
        ↓
inode bitmap 分配 inode_id
        ↓
初始化新的 DiskInode 为普通文件
        ↓
扩展根目录文件
        ↓
追加 DirEntry("filea", inode_id)
        ↓
同步块缓存
        ↓
返回 filea 的 Inode
```

创建文件同时修改 inode 区和根目录数据区。

`clear()` 只清空内容并回收数据块，文件对应的 inode 和目录项仍然存在，因此它是文件截断，而不是删除文件。

---

## 17. 内核 File 抽象

内核定义统一接口：

```rust
pub trait File: Send + Sync {
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn read(&self, buf: UserBuffer) -> usize;
    fn write(&self, buf: UserBuffer) -> usize;
}
```

当前实现包括：

- `OSInode`：普通磁盘文件；
- `Stdin`：标准输入；
- `Stdout`：标准输出。

系统调用只操作 `Arc<dyn File>`，不需要判断具体文件类型。

---

## 18. OSInode 与文件偏移量

`OSInode` 在 easy-fs 的 `Inode` 外增加：

```rust
readable
writable
offset
```

每次读写后更新：

```rust
inner.offset += size;
```

因此连续调用 `read()` 会从上一次结束位置继续读取。

打开标志包括：

| 标志 | 作用 |
|---|---|
| `RDONLY` | 只读 |
| `WRONLY` | 只写 |
| `RDWR` | 读写 |
| `CREATE` | 创建文件 |
| `TRUNC` | 清空文件 |

本实现中，使用 `CREATE` 打开已经存在的文件也会调用 `clear()`，属于简化语义。

---

## 19. ROOT_INODE

内核第一次访问 `ROOT_INODE` 时执行：

```text
BLOCK_DEVICE
→ EasyFileSystem::open()
→ 检查 SuperBlock
→ EasyFileSystem::root_inode()
→ 根目录 Inode
```

内核启动时的应用列表来自：

```rust
ROOT_INODE.ls()
```

源码中存在 `Inode::ls()`，并不代表存在名为 `ls` 的用户程序。由于第六章的用户程序列表中没有 `ls`，在用户 Shell 输入 `ls` 会执行失败，这是正常现象。

---

## 20. 文件描述符表

每个进程具有：

```rust
fd_table: Vec<Option<Arc<dyn File + Send + Sync>>>
```

初始值为：

```text
fd 0 → Stdin
fd 1 → Stdout
fd 2 → Stdout（暂时代替stderr）
```

普通文件从 fd 3 开始分配。

`alloc_fd()` 优先复用值为 `None` 的槽位，否则扩展数组。

---

## 21. 文件系统调用

系统调用号：

| 调用 | 编号 |
|---|---:|
| `open` | 56 |
| `close` | 57 |
| `read` | 63 |
| `write` | 64 |

RISC-V 参数约定：

```text
a0/x10：参数0，同时接收返回值
a1/x11：参数1
a2/x12：参数2
a7/x17：系统调用号
```

### sys_open

```text
用户路径
→ translated_str()
→ open_file()
→ ROOT_INODE.find/create()
→ 创建 OSInode
→ alloc_fd()
→ fd_table[fd] = Some(file)
→ 返回 fd
```

路径必须以 `\0` 结束，例如：

```rust
open("filea\0", OpenFlags::RDONLY)
```

### sys_read 与 sys_write

```text
检查fd范围
→ 检查槽位是否有效
→ 检查读写权限
→ 克隆文件Arc
→ 释放TCB独占借用
→ 翻译用户缓冲区
→ 调用File::read/write
```

`translated_byte_buffer()` 将可能跨页的用户缓冲区拆成多个内核可访问的物理内存切片，并封装成 `UserBuffer`。

### sys_close

```rust
inner.fd_table[fd].take();
```

它只关闭当前文件描述符并减少 `Arc` 引用计数，不删除磁盘文件。

---

## 22. 标准输入输出

`Stdin::read()` 每次读取一个字符。当暂时没有输入时：

```rust
suspend_current_and_run_next();
```

当前进程主动让出 CPU，稍后继续检查。

`Stdout::write()` 遍历 `UserBuffer` 的各段，将其转换为 UTF-8 并输出。

标准输入输出和普通文件统一实现 `File`，体现了“一切皆文件”的基本思想。

---

## 23. fork 和 exec 的文件语义

`fork()` 复制文件描述符表时克隆的是 `Arc`：

```rust
new_fd_table.push(Some(file.clone()));
```

所以父子进程共享同一个 `OSInode` 和其中的文件偏移量：

```text
父进程 fd ─┐
            ├──→ OSInode → offset
子进程 fd ─┘
```

`exec()` 替换地址空间和 TrapContext，但不修改 `fd_table`，所以执行新程序后，原来打开的文件仍然有效。

---

## 24. easy-fs-fuse 与 fs.img

`easy-fs-fuse` 在宿主机上把普通文件封装为块设备：

```text
block_id
→ seek(block_id × 512)
→ read/write 512字节
```

生成的镜像大小为：

```text
32768块 × 512字节 = 16 MiB
```

inode bitmap 占一个块，可记录 4096 个 inode。inode 0 已被根目录占用，因此最多还可创建 4095 个文件。

打包流程：

```text
遍历用户程序
→ 读取编译后的 ELF
→ root_inode.create(app_name)
→ inode.write_at(0, ELF数据)
→ 写入 fs.img
```

每次 `make run` 都可能重新创建 `fs.img`，所以运行过程中创建的普通文件不保证在下一次启动后仍然存在。

---

## 25. VirtIO 块设备

QEMU VirtIO MMIO 设备地址为：

```rust
const VIRTIO0: usize = 0x10001000;
```

`VirtIOBlock` 实现 easy-fs 的 `BlockDevice`：

```rust
impl BlockDevice for VirtIOBlock {
    fn read_block(...) { ... }
    fn write_block(...) { ... }
}
```

`VirtioHal` 为驱动提供：

- 连续 DMA 页分配；
- DMA 页回收；
- 物理地址到虚拟地址转换；
- 虚拟地址到物理地址转换。

内核最终通过 VirtIO 驱动访问 QEMU 挂载的 `fs.img`。

---

## 26. 用户程序加载方式

第六章启动 `initproc` 的流程：

```text
open_file("initproc")
→ OSInode::read_all()
→ 读取完整ELF
→ MemorySet::from_elf()
→ 创建TaskControlBlock
→ 加入调度队列
```

用户 Shell 执行程序的流程：

```text
输入 hello_world
→ fork()
→ 子进程 exec("hello_world")
→ open_file("hello_world")
→ read_all()
→ 解析ELF
→ 替换子进程地址空间
```

---

## 27. 实验验证

启动：

```bash
cd ~/code/rCore-Tutorial-v3/os
make run
```

### filetest_simple

在用户 Shell 输入：

```text
filetest_simple
```

程序执行：

```text
CREATE | WRONLY 创建 filea
→ 写入 "Hello, world!"
→ close
→ RDONLY 重新打开
→ read
→ 比较内容
```

预期：

```text
file_test passed!
```

### cat_filea

应先执行 `filetest_simple` 创建 `filea`，再执行：

```text
cat_filea
```

如果直接执行 `cat_filea`，由于文件不存在，`open()` 返回失败，测试程序会 panic。

### huge_write

```text
huge_write
```

程序写入 1 MiB：

```text
1024次 × 1024字节 = 1 MiB
```

对应 2048 个 512 字节数据块，超过直接索引与一级间接索引的 156 块容量，因此会实际使用二级间接索引。

---

## 28. 实验现象说明

### Shell 中执行 ls 失败

虽然 VFS 提供 `Inode::ls()`，但本章没有名为 `ls` 的用户程序。因此：

```text
>> ls
Error when executing!
```

属于正常现象，不表示文件系统损坏。

### cat_filea 首次执行失败

`fs.img` 初始只包含被打包的用户程序，不包含普通文件 `filea`。需要先运行 `filetest_simple` 创建它。

### 退出 QEMU

依次按：

```text
Ctrl+A
松开
X
```

不是同时按下三个键。

---

## 29. 本章总结

第六章完成了从块设备到用户系统调用的完整文件系统链路：

1. `BlockDevice` 抽象底层存储；
2. `BlockCache` 提供块缓存和写回机制；
3. `Bitmap` 管理 inode 与数据块；
4. `DiskInode` 使用直接、一级和二级索引定位文件数据；
5. `Inode` 提供创建、查找、读写和清空接口；
6. `File` 统一普通文件和标准输入输出；
7. 每个进程通过 `fd_table` 管理打开的文件；
8. 系统调用向用户程序提供文件访问接口；
9. `easy-fs-fuse` 将用户程序打包进 `fs.img`；
10. 内核通过 VirtIO 块设备加载并执行用户程序。

与第五章相比，第六章使用户程序获得了持久化存储抽象，也让程序加载方式从内核静态嵌入转变为文件系统动态读取，为后续管道、重定向和更完整的 Shell 奠定了基础。

---

## 30. 提交记录建议

完成验证后，在仓库根目录执行：

```bash
cd ~/code/rCore-Tutorial-v3
git status --short
git add docs/ch6.md
git commit -m "docs: add chapter 6 filesystem notes"
git push
```

检查提交：

```bash
git status -sb
git log --oneline -5
```

汇报时可重点展示：

- easy-fs 的五区磁盘布局；
- 直接、一级和二级间接索引；
- `BlockFile` 与 `VirtIOBlock` 对同一个 trait 的实现；
- `fd_table` 与 `Arc<dyn File>`；
- `open/read/write/close` 完整调用链；
- `huge_write` 如何验证二级间接索引。
