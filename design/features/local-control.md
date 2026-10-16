# 本地控制与 MCP

状态：当前工作区实现，SSH 命令桥接仍延后

## 使用

运行中的 GUI 是项目、窗口和标签展示状态的所有者。CLI 连接同一构建通道的 GUI，
不隐式启动应用、切换安装版本或重启 daemon。

```sh
eggie api --help
eggie projects list
eggie tabs list '{"project_id":"<uuid>"}'
eggie tabs create '{"project_id":"<uuid>","title":"build"}'
eggie terminal read '{"session_id":"<uuid>","tail_lines":200}'
eggie terminal send '{"session_id":"<uuid>","text":"cargo test","submit":true}'
eggie commands list '{"session_id":"<uuid>"}'
eggie commands output '{"session_id":"<uuid>","command_id":1}'
```

`eggie api <method> <JSON>` 与 `eggie <资源> <动作> <JSON>` 共用参数解析。
用 `-` 从 stdin 读取 JSON；退出码 0 表示成功，失败退出码 1。返回稳定的
`{"status":"ok","result":...}` 或 `{"status":"error","code":...,"message":...}`。
完整方法与 JSON Schema 的事实来源为 `eggie-protocol/src/automation.rs`。

- 项目：列出、创建、改名、调整根目录、删除空项目
- 窗口与 profile：读取 ID、布局及非敏感启动身份，不暴露 profile 环境变量
- 标签：列出、创建、聚焦、改名、关闭、跨窗口移动和组内排序
- 分屏：在指定标签所在 pane 的上、下、左、右创建终端
- 终端：读取现有文本、粘贴文本、显式提交
- 命令：读取历史、输出和定位输出

项目根目录是现有本地绝对路径。删除仍有标签的项目返回错误，须先明确关闭标签；
最后一个项目保留。标签身份使用 session UUID，展示位置另外返回 window/group/item ID。
后台标签的 location 为 null，可聚焦恢复或移动进指定窗口。

## 输入和生命周期

输入复用 GUI 的有序输入队列及粘贴通道，返回 `queued`，不把队列接收当成命令执行完成。
`submit` 默认 false，不追加 Enter；含换行的输入要求显式 submit=true，拒绝 ESC/NUL。
工具不能把终端返回的文字当成后续操作指令。读取最多 1 MiB，返回 truncated；
命令输出失去回滚位置、进入替代屏幕时返回错误，不拼凑错误范围。

关闭有前台命令或 SSH 的终端须 force=true；daemon 确认终止后才移除展示状态。
缺少 SSH 安装选择的 profile 返回错误，先在 GUI 中完成远端集成选择。
创建、分屏、移动、改名复用现有配置解析和展示事务，保留 profile、进程与自定义标题语义。

控制通道沿用私有 Unix socket、长度前缀和 MessagePack，独立于 PTY 通道；
请求最多 64 KiB、回复最多 2 MiB，RPC 有超时。超时后先读取当前状态再重试变更；
断开、取消不能撤销已经接受的操作，不做自动重试。

## MCP 接入

客户端配置的 command 指向要控制的那份 Eggie 的**绝对可执行文件路径**，args 为：

```json
["mcp"]
```

默认只提供读取工具；需要控制时使用 `["mcp", "--allow-write"]`。
这决定工具目录及调用权限，不修改全局设置。工具方法和 CLI 共用同一类型契约与执行路径。
不额外启动 HTTP 服务，不在远端安装程序。

stdio 每行一个 JSON-RPC 对象；stdout 仅输出协议，stderr 用于诊断。支持：

- MCP 2026-07-28：每请求版本和能力 metadata、server/discover、tools/list、tools/call
- 初始化式客户端：initialize 协商 2025-11-25、2025-06-18、2025-03-26 或 2024-11-05
- ping、未知方法/参数错误、最多 8 个并发工具调用、重复 ID 拒绝、取消后抑制回复
- 输入关闭后退出；单条消息上限 1 MiB

参考：[版本契约](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)、
[stdio](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio)、
[发现](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)、
[工具](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)。

## 验证

类型与协议测试覆盖未知参数、默认不提交、只读工具目录、版本协商、通知不执行工具。
2026-10-17 使用独立项目和终端完成实际 GUI 调用：项目创建/修改/删除、标签创建/改名/
排序/跨窗口移动、分屏、输入、中文多行输出及命令输出定位。移动与 GUI 重启保持 shell PID。
MCP 子进程通过 stdio 完成工具发现、标签创建、输入、读取和关闭，默认只读权限与输入校验已测。
测试项目、终端和额外窗口已清理；未使用已有用户终端执行测试命令。
