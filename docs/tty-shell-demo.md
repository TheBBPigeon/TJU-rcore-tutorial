# TTY 与 Shell 演示脚本（成员4）

> 对应里程碑 M4：行编辑、历史、管道、重定向、后台任务、Ctrl-C 前台终止。

## 0. 准备

```bash
cd os
make run
```

进入 QEMU 串口后看到 `Rust user shell (tty-shell)` 和提示符 `>> `。

## 1. 行编辑

```text
>> hello_world<Left><Left>x<Enter>
```

输入 `hello_world` 后按两次左方向键，再输入 `x`，应得到 `hello_worldx` 并执行。
可用键：左右方向键、Home/End、Backspace、Delete、Ctrl-A/E/U/K。

## 2. 历史记录

```text
>> echo a
>> echo b
>> <Up><Up><Enter>
```

上方向键应能翻出 `echo b`、`echo a`；`history` 内建命令可查看列表。

## 3. 内建命令

```text
>> help
>> pwd
>> cd sub
>> pwd
>> cd
```

`help` 列出内建命令；`pwd/cd` 由 Shell 维护当前目录字符串（成员3多级目录合入后对接内核）。

## 4. 管道与重定向

```text
>> echo hello > out.txt
>> cat out.txt
>> echo hello | count_lines
```

预期：`out.txt` 内容为 `hello`；`echo hello | count_lines` 输出 `2`
（`count_lines` 对非空输入会额外 +1，属于该测试程序的统计口径）。
注意：重新 `make build` 会重建文件系统镜像，之前创建的文件会被清掉，演示时先执行
`echo hello > out.txt` 再使用。

## 5. 后台任务与作业控制

```text
>> sleep 1000 &
>> jobs
>> echo back
>> fg 1
```

`sleep 1000 &` 立即返回 `[1] <pid>`，Shell 可继续输入；`jobs` 显示 Running；`fg 1` 把它收回前台等待完成。

## 6. Ctrl-C 终止前台任务

```text
>> infloop
<按 Ctrl-C>
```

前台 `infloop` 应被 SIGINT 终止并回到提示符。再验证后台不受影响：

```text
>> infloop &
>> <Ctrl-C>
>> jobs
```

Shell 空闲时按 Ctrl-C 不退出（Shell 忽略 SIGINT 且在独立进程组）。

## 7. 内核 TTY 验证程序

```text
>> tty_test
```

依次验证 canonical 整行读取、raw 无回显读取、Ctrl-D EOF。

```text
>> pgid_test
>> sigint_test
```

`pgid_test` 验证 fork 后 `setpgid` 生效；`sigint_test` 前台运行时按 Ctrl-C 应被杀死（返回码 -2）。
