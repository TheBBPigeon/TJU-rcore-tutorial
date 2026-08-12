# 成员4：TTY 与 Shell 任务设计方案

> 保存日期：2026-08-11
> 分支：`feature/tty-shell`
> 集成目标：`course-dev`

## 1. 背景与目标

本项目基于 rCore-Tutorial-v3。成员4负责终端（TTY）与 Shell：

- 内核侧：输入缓冲、阻塞读取、行编辑所需的 raw/canonical 模式、Ctrl-C 终止前台任务。
- Shell 侧：行编辑、历史记录、内建命令、后台执行与作业控制。
- 演示目标：命令编辑、管道、重定向、后台任务、Ctrl-C 终止前台任务。

## 2. 现状盘点

### 内核

- UART（NS16550A）中断将字符推入 `VecDeque`，`Condvar` 唤醒等待者；`read()` 已是阻塞读。
- `Stdin::read` 每次只读 1 字节，无行缓冲、无 echo 控制、无控制字符处理。
- 信号仅有位标志，默认动作是杀掉当前进程；`sys_kill` 只支持单进程，无进程组广播。
- PCB 无 `pgid`，无前台进程组概念。
- GUI 键盘（virtio-keyboard）与控制台 UART 是两条独立输入链路，TTY 只改造 UART 链路。

### Shell

- `user_shell.rs` 已有：单字符读取、Backspace、管道 `|`、重定向 `<`/`>`。
- 缺少：内建命令、历史、光标移动、引号/转义解析、后台任务、作业控制。
- `cd/pwd` 依赖成员3的多级目录系统，当前 FS 为平铺结构。

## 3. 总体架构

```text
UART 硬件 --> 串口驱动(IRQ→环形缓冲+Condvar) --> TTY 行规程(raw/canonical、echo、Ctrl-C)
    --> 系统调用(read/tty_ctl/setpgid/kill(-pgid)) --> user_lib --> user_shell
    --> 进程组/信号(PCB.pgid + TTY.fg_pgrp)
```

设计取舍：内核只负责行规程与控制字符；行编辑、历史、补全等复杂逻辑放在用户态 Shell。
备选（内核 canonical 内做完整行编辑）不采用：复杂度高、不利于调试。

## 4. 内核侧设计

### 4.1 TTY 行规程（新增 `os/src/tty/`）

- 数据结构：输入环形缓冲 + 行队列 + 等待队列（复用 Condvar 阻塞/唤醒机制）。
- 模式标志（termios 子集）：
  - `ICANON`：按行返回；关闭后 raw 模式，有字节即返回。
  - `ECHO`：内核回显；Shell 交互时关闭，由 Shell 回显。
  - `ISIG`：识别 Ctrl-C（0x03）/Ctrl-Z（0x1A）。
- 控制字符：CR/LF 归一化；Backspace/DEL 删缓冲；Ctrl-D 返回 EOF（读 0）；Ctrl-C 触发信号，不进入行缓冲。
- `Stdin` 改为读取 TTY，而不是直接读 UART。

### 4.2 进程组与前台终端

- PCB 增加 `pgid`，fork 继承，exec 保留。
- TTY 记录 `fg_pgrp`，由 `tcsetpgrp` 设置。
- `sys_kill` 扩展：`pid < 0` 表示向 `|pid|` 进程组广播；`pid == 0` 表示当前进程组（可选）。
- 后台进程读终端：可选实现 SIGTTIN，或至少文档说明并重定向后台 stdin。

### 4.3 信号

- 必做：SIGINT 默认杀进程（沿用现有逻辑），支持发往整个前台进程组。
- 建议：极简 `sigaction`，仅支持 `SIG_IGN/SIG_DFL`；Shell 必须忽略 SIGINT，否则无前台作业时 Ctrl-C 会杀死 Shell（initproc 不负责重启 Shell）。
- 可选：SIGTSTP/SIGCONT（TaskStatus 增加 Stopped）、SIGCHLD。

### 4.4 新增系统调用（1100 段，避免与 1000/2000/3000 段冲突）

| 系统调用 | 号 | 说明 |
|---|---|---|
| `sys_setpgid(pid, pgid)` | 1100 | 设置进程组 |
| `sys_getpgrp()` | 1101 | 查询自身进程组 |
| `sys_tcsetpgrp(fd, pgrp)` | 1102 | 设置终端前台进程组 |
| `sys_tcgetpgrp(fd)` | 1103 | 查询终端前台进程组 |
| `sys_tty_ctl(fd, cmd, arg)` | 1104 | 设置 raw/canonical、echo、isig |
| `sys_sigaction(sig, act)` | 1105 | SIG_IGN/SIG_DFL |

修改内核 `os/src/syscall/mod.rs` 时，必须同步修改 `user/src/syscall.rs` 与 user_lib 封装。

## 5. Shell 侧设计

### 5.1 解析器

- 支持单双引号、`\` 转义。
- 支持 `;` 顺序执行、`&` 后台执行、管道 `|`、重定向 `<`/`>`（可选 `>>`、`2>`）。
- 命令不存在时给出友好错误。

### 5.2 行编辑与历史

- 缓冲区 + 光标：左右移动、Home/End、Backspace、Delete、Ctrl-A/E/U/K/W。
- 上下方向键翻历史（raw 模式识别 `ESC [ A/B` 等 ANSI 转义序列）。
- 历史存内存 `VecDeque`，上限约 100 条；持久化可选（依赖成员3）。

### 5.3 内建命令

`cd`、`pwd`、`echo`、`exit`、`history`、`jobs`、`fg`、`bg`、`kill`、`help`、`clear`。

`cd/pwd` 在成员3合入前，Shell 先自维护 cwd 字符串，把相对路径拼成绝对路径传给 `open`。

### 5.4 后台与作业控制

- `cmd &`：fork 后子进程 `setpgid(0,0)`；父进程登记作业表 `{jid, pid, pgid, cmdline, status}`，不等待。
- 前台作业：`tcsetpgrp(0, job_pgid)`，Shell 等待回收；结束后收回终端并打印状态。
- `jobs` 显示状态；`fg/bg` 使用 `kill(-pgid, SIGCONT)`（SIGCONT 为可选，若未实现则 `bg` 仅支持直接放后台）。
- 回收：`waitpid` 非阻塞语义（运行中返回 -2），Shell 轮询 + sleep；可选新增阻塞等待。

### 5.5 Ctrl-C 流程

```text
用户按 Ctrl-C
  -> UART 中断收 0x03
  -> TTY 检测 ISIG，不进入行缓冲
  -> 向 TTY.fg_pgrp 广播 SIGINT
  -> 前台作业进程 trap 返回前检查信号 -> exit(-2)
  -> Shell waitpid 回收 -> 报告 "Killed by SIGINT" -> 收回前台并打印提示符
```

## 6. 里程碑

| 阶段 | 内容 | 验收 |
|---|---|---|
| M0 | 建分支、跑通基线构建 | 能进入现有 Shell |
| M1 | 内核 TTY：raw/canonical、echo、阻塞读、Ctrl-C/D | `tty_test` 通过 |
| M2 | 进程组 + 前台终端 + `kill(-pgid)` + `sigaction` | 多进程组信号测试通过 |
| M3 | Shell 重构：解析、内建、历史、行编辑、`&` 后台 | 手动交互通过 |
| M4 | 作业控制：jobs/fg/bg、回收、Ctrl-C 前台终止 | 验收脚本通过 |
| M5 | 与成员1/3 联调、文档、PR 合入 `course-dev` | 小组演示通过 |

## 6.1 实现状态（2026-08-11）

- 已完成：M0 分支与基线、M1 内核 TTY、M2 进程组/信号、M3 Shell 重构、M4 作业控制。
- 已验证（QEMU 实测）：行编辑、历史、内建命令、管道（含 `echo | cmd`）、重定向、
  后台任务、jobs/fg、Ctrl-C 终止前台任务、tty_test/pgid_test/sigint_test。
- A 组优化（已实现）：exec 失败错误提示到 stderr、前台等待/后台回收改为休眠轮询、
  TTY 行读取去掉 O(n²)、Ctrl-D 部分行立即返回、Ctrl-C 回显 `^C`、后台进程读终端
  返回 EOF 防止抢输入。
- B 组优化（已实现）：SIGTSTP/SIGCONT 停止/恢复（Ctrl-Z 停止前台作业，`bg`/`fg`
  恢复）、waitpid 支持 WUNTRACED（停止状态返回 -3）、setpgid/tcsetpgrp 权限校验、
  UART 输入缓冲上限、exec 同步读盘 RAII guard。
- C 组优化（已实现）：`>>` 追加重定向、`2>` 错误重定向、`&&`/`||` 条件执行、
  `kill -SIG`（INT/TSTP/CONT 等）、Delete 键、Ctrl-W 删词、`history -c`、
  Tab 补全（文件系统应用名 + 内建命令）。
- 未实现（后续增强）：
  历史持久化；`cd/pwd` 目前为 Shell 侧字符串维护，待成员3的
  `chdir/getcwd` 合入后对接；阻塞式 waitpid 未实现，当前用 `waitpid_nb + sleep`
  轮询替代。
- 环境修复：bootloader 升级为 RustSBI 0.3.1（QEMU 11 需要）；exec 加载 ELF
  期间使用同步磁盘读，避免 easy-fs spinlock 跨阻塞 I/O 导致的管道并发死锁。

## 7. 测试与验收

- 内核测试程序：`tty_test`（canonical/raw/EOF）、`pgid_test`、`sigint_test`。
- 交互演示：行编辑 → 历史 → 管道/重定向 → 后台任务 → Ctrl-C 终止前台 `infloop`。
- 每个里程碑先 `cargo build` 再进 QEMU 验证，避免积压调试。

## 8. 注意要点

1. echo 冲突：Shell 行编辑时内核关 ECHO，避免双重回显。
2. Ctrl-C 不能进入行缓冲、不能当普通字符 echo。
3. Shell 必须忽略 SIGINT，否则无前台作业时按 Ctrl-C 会自杀。
4. 与成员1约定：调度器重构不得破坏 Blocked 语义和 wakeup 接口。
5. 与成员3约定 `chdir/getcwd` 接口；先做 Shell 侧 cwd 管理 + stub。
6. 与成员2约定：PCB 新增字段必须写入 fork 复制逻辑；fork 后父子幂等 `setpgid`。
7. 系统调用号统一登记，避免各分支冲突。
8. IRQ 上下文不能阻塞/睡眠/大量分配；行处理与信号广播放到安全点。
9. 多进程抢 stdin：后台作业不应读终端，至少重定向 stdin。
10. ANSI 依赖：QEMU stdio 支持；串口工具不支持时提供降级（Ctrl-P/N 翻历史）。
11. 不触碰 GUI 键盘路径（sys_event_get/sys_key_pressed）。
12. 后台作业结束必须被 Shell 回收，避免僵尸进程占 PID。

## 9. Git 协作

- 本地开发分支：`feature/tty-shell`，小步提交，每完成一个里程碑推送一次。
- 合并目标：`course-dev`；合入前先拉最新集成分支解决冲突。
