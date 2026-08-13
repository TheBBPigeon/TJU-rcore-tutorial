# Chapter 1：应用程序执行环境

## 1. 实验目标

- 搭建 rCore 开发环境
- 运行最小内核
- 理解内核的启动过程
- 分析 ELF 文件和内存布局

## 2. 开发环境

- WSL：WSL2
- Linux：Ubuntu
- Rust：rustc 1.97.1 (8bab26f4f 2026-07-14)
- QEMU：8.2.2
- 架构：RISC-V 64
- 实验分支：learning/ch1
- 官方提交：85fe952a
- 个人提交：e3873652

## 3. 内核启动流程

QEMU 启动 RISC-V 虚拟机后，首先运行 RustSBI。RustSBI 将内核控制权交给地址 0x80200000 对应的 `_start`。

启动过程：

QEMU
→ RustSBI
→ `_start`
→ 设置启动栈
→ `rust_main`
→ `clear_bss`
→ 输出内核日志
→ SBI shutdown

## 4. 链接脚本

`linker-qemu.ld` 使用：

`ENTRY(_start)`

将 `_start` 设置为 ELF 入口。

通过 `rust-readobj` 得到：

- ELF 格式：elf64-littleriscv
- 架构：RISC-V 64
- 入口地址：0x80200000

通过 `rust-nm` 得到：

- `_start`：0x80200000
- `rust_main`：0x80200492
- `sbss`：0x80214000
- `ebss`：0x80215000

ELF 入口地址与 `_start` 地址一致。

## 5. 内存布局

| 区域 | 起始地址 | 说明 |
|---|---:|---|
| `.text` | `0x80200000` | 程序指令 |
| `.rodata` | `0x80202000` | 只读数据 |
| `.data` | `0x80203000` | 已初始化全局数据 |
| 启动栈 | `0x80204000` | 大小为 64 KiB |
| 普通 `.bss` | `0x80214000` | 未初始化全局数据 |

整个 `.bss` section 大小为 `0x10010`，其中包括：

- `0x10000` 字节启动栈
- `0x10` 字节普通 BSS

## 6. 修改实验

在 `clear_bss()` 后增加日志：

`[kernel] bss has been cleared`

重新运行内核后成功输出该日志，说明程序完成了 BSS 清零，并继续正常执行。

## 7. 遇到的问题

- WSL localhost 代理警告
- Git TLS 连接中断
- 浅克隆没有获取 `ch1` 分支
- Ubuntu 中缺少 `rustup`
- cargo-binutils 首次运行需要安装

## 8. 本章总结

本章完成了从 RustSBI 到 Rust 内核的最小启动流程，并通过 ELF 工具验证了内核入口和各内存段地址。