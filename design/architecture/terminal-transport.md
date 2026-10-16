# 终端文字传输：不可变共享行

## 通信与归属

文字共享行从协议 v4 引入；当前协议版本以 `eggie-protocol::PROTOCOL_VERSION` 为准。
通信保留 Unix domain socket：输入、尺寸、查询、配置、OSC 和图片控制仍走它。
GUI 的可见终端使用 `WaitForRenderFrame` 长轮询；只有变化的文字行写入 POSIX 共享内存，
socket 发送大小、版本、行租约和光标、选择区、颜色、图片布局等帧元数据。原有
`WaitForSnapshot` 保留为可测量的 socket 路径，CLI、预览和其他控制请求可继续使用普通快照。

- daemon 的 Alacritty damage 信息决定需要生成的行。首次、大小变化、滚屏、备用屏幕及全量
  damage 重建全部行；未变化的行在 16 帧发布历史中共享 `Arc`，无需复制所有 cell。
- `eggie-protocol` 定义 `TerminalCells`、不可变 `TerminalRowStorage` 和借用 `TerminalCellView`。
  序列化仍是 row-major cell 序列；协议和绘制模块不依赖 mmap、daemon 或 Alacritty。
- `eggie-ipc` 拥有映射、槽使用权、边界校验与回收。GUI 不把映射中的行解码成 cell/string Vec；
  渲染时直接读取，Unicode combining 与 hyperlink 字符串也借用映射。
- 终端绘制按行复用一个临时稠密 buffer；不会把较大的借用 cell 视图展开成整屏临时数组。
- 输入排队和 daemon 的确认机制保持原有顺序语义。图片继续使用已有的共享内存与 Metal 路径。

## 共享槽生命周期

每个订阅连接拥有一个池，每行三个槽，槽头按 64 字节对齐。状态通过进程间 lock-free
`AtomicU32` 的 acquire/release 操作交接：

```
FREE → WRITING → TRANSFERRED → READING → FREE
```

生产者只能用 CAS 取得 FREE 槽。写入完成后发布 TRANSFERRED，消费者用 CAS 取得 READING，
校验 generation、范围、字符、颜色、列顺序和 UTF-8 后建立行存储。最后一个引用该行的
`Arc` 释放时，槽才返回 FREE。因此旧画面、预览、准备中的帧可以继续引用它，生产者不能覆盖。
未变化的行直接复用上帧的行 `Arc`，没有重复获取或释放同一个租约。

行记录为 48 字节，另有 combining UTF-32 与 hyperlink UTF-8 尾部。数值显式 little-endian，
仅在 little-endian Unix 启用该路径。映射名为当前用户的 0600 POSIX shm 对象，消费者打开后
立即 unlink；连接退出时生产者也尝试 unlink。映射随行引用继续存活，最后一个映射关闭后
由内核回收，不会因为断线让旧帧指针失效。消费者打开前双方崩溃仍可能留下命名对象，正常
订阅和连接关闭都会清理。

首次帧、订阅重连、版本断层和池重建发送所有行。尺寸或行容量增长创建新池，旧映射随旧帧
存活。池上限为 **64 MiB/连接**，实际按行数与最长行容量分配；240×80 全部填满 ASCII 的
测试池约 **3.76 MiB**。它是 daemon 和 GUI 映射的同一个对象，不是两份独立文字副本。
只有仍被引用的行和当前连接持有映射；该上限不是进程总内存上限。

如果所有槽仍被使用、共享内存创建失败或超出池上限，发送普通 owned socket 快照，继续
输出并等待旧租约释放。下一次共享帧可重新建立完整基准。普通快照会省略尾部空行，消费者
在合并共享行时补齐行数。不会忙等槽或阻塞 PTY 解析，也不会丢失画面。

## 延迟诊断

设置 `EGGIE_RENDER_METRICS=1`：

- 既有 daemon `publish`：内核投影到快照发布耗时。
- 新增 `[eggie-grid] publish`：变化行数、共享字节、socket 字节、池大小和 packing + notification。
- 新增 `[eggie-grid] receive`：socket body 读取、元数据解码、行租约校验/合并耗时。
- 既有 GUI 指标：准备画面、字形、Metal、图片上传等阶段。

`EGGIE_INPUT_LATENCY=1` 保留输入到快照/绘制的关联统计。`last_input_sequence` 是 daemon 已接受
的输入序号，不能独自证明应用已经回显；所以真实输入验证使用下面的 marker benchmark。
socket 长轮询等待、屏幕刷新和物理键盘到屏幕时间不能被当成纯 IPC 解码时间。

`EGGIE_SHARED_GRID=0` 让 GUI 使用 MessagePack 快照/增量，便于同一构建对照；daemon 的变化行
构建机制在两条路径都有效。此变量在每个 watcher 启动时读取，修改后重启 GUI。

## 可重复验证

```sh
env -u NO_COLOR cargo test -p eggie-protocol -p eggie-ipc -p eggie-daemon
env -u NO_COLOR cargo test -p eggie-terminal-view -p eggie-ui --lib
python3 scripts/check-architecture.py

env -u NO_COLOR cargo test -p eggie-ipc --release shared_grid_socket_ab_benchmark -- --ignored --nocapture
env -u NO_COLOR cargo test -p eggie-daemon shared_grid_pty_echo_ab_benchmark -- --ignored --nocapture
```

2026-10-11，本机 Apple M3 Pro / macOS 27.0，最后一次 release socket 对照：60 次预热，
每种负载/路径 600 次，UnixStream::pair 两端分属两个线程，使用生产 wire reader。
计时包含通知、生产端 packing/encoding、socket、接收验证/解码、快照合并；不包括 PTY 或 GPU。
单字符基线用一 cell 的 MessagePack delta，整屏基线用完整快照；增量应用已经使用优化后的
按行结构，结果没有把旧版整屏复制的成本加到基线上。该测试隔离传输，双方在同一进程的
独立映射中交接；实际 daemon/GUI 跨进程路径另外经过原生应用验证。

| 240×80 负载 | MessagePack p50 / p95 | 共享行 p50 / p95 | socket 字节/帧，前 → 后 |
| --- | --- | --- | --- |
| 单 cell 更新 | 15.3 / 35.9 μs | 10.7 / 29.3 μs | 191 → 224 |
| 所有 cell 更新 | 6.79 / 8.13 ms | 0.290 / 0.362 ms | 796,304 → 2,950 |

前一轮整屏 p95 为 7.36 → 0.320 ms，单 cell 为 29.6 → 19.2 μs；小样本的微秒级尾延迟
受系统调度影响。共享内存仍有一次生产者 packing 写入和一次消费者校验，元数据仍会解码，
并非整条绘制链路零复制。

真实 PTY 对照使用 `stty -echo` + `/bin/cat`，输入经生产请求服务端，只有终端正确解析出
对应 ACK marker 才结束计时。预热 10 次，70 次取样（优化 dev 构建）：

| 输入请求开始 → 验证程序回显 | p50 | p95 |
| --- | --- | --- |
| MessagePack | 11.851 ms | 14.098 ms |
| 共享行 | 11.707 ms | 11.941 ms |

这个测试包含输入确认、PTY、程序、8ms 发布节奏和快照接收，不含真实 GUI 合成/显示。
普通输入的中位数主要受发布节奏影响；高频大屏输出得到更明显的传输收益。

回归覆盖旧帧保留、槽耗尽与恢复、池重建、断线重连、普通基准尾部空行、无效范围/版本/字符/
颜色/UTF-8、Unicode/wide/combining/link，以及 damage 快照与完整内核投影在清屏、行插入删除、
滚动、备用屏幕、尺寸和选择区变化下的一致性。

## 历史开发构建验证（2026-10-11）

原生 dev Eggie GUI PID 73766、daemon PID 73345（v4），新终端 shell PID 73939。
通过 GUI 输入验证红/绿/蓝、中文、组合重音和 emoji；显示 `NO_COLOR=UNSET`。
GUI、daemon 的进程环境也都无 `NO_COLOR`，没有关闭共享行路径。
`vmmap` 在两端显示对应的 992 KiB 读写共享映射，名称因打开后 unlink 不再显示。
该时点 physical footprint 为 GUI 94.5 MiB、daemon 10.8 MiB；它是一次空闲观察，
不是与旧版本同负载的内存回归结论，也不包含终端子进程。

协议/IPC/daemon/视图/GUI 合计 461 个常规测试通过，两个延迟 benchmark 通过；
架构边界检查与开发 app 构建通过。
