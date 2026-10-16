# Eggie Lua 配置与事件脚本

Eggie 采用 Rex 风格的注册式 API。`init.lua` 执行一次并通过 `eggie.*` 调用注册设置、快捷键和动作，不返回配置表。事件监听脚本通过 `eggie do` 在独立 Lua 5.4 VM 中运行。两种环境都允许标准 Lua 库和从脚本所在目录 `require` 模块；它们运行的是用户信任的本地代码。

## 文件与设置界面

- 配置路径是 `$XDG_CONFIG_HOME/eggie/init.lua`，默认 `~/.config/eggie/init.lua`；`EGGIE_CONFIG` 可覆盖。
- GUI 的可编辑偏好保存在应用数据目录的 `preferences.json`。daemon 首次发现旧 `settings.json` 时将它一次性迁移，旧文件保留为备份。
- GUI 设置仍然存在。Lua 明确设置的界面主题、终端主题或字体优先；对应控件显示 `init.lua` 来源并禁用编辑。移除 Lua 调用并重载后，原 GUI 偏好重新生效。设置窗口标题栏可以打开已有的 `init.lua`。
- 项目、profile 和会话状态继续使用各自的数据存储；Lua 不接管这些状态文件。

## 配置 API

```lua
-- ~/.config/eggie/init.lua
eggie.ui.theme("light", "Ayu Light")
eggie.ui.theme("dark", "Catppuccin Mocha")
eggie.terminal.theme("dark", "Catppuccin Mocha")
eggie.terminal.font { family = "JetBrains Mono", size = 14 }

eggie.bind("cmd+d", "pane.split", { direction = "right" })
eggie.unbind("cmd+shift+d")

eggie.action {
  name = "scratch",
  title = "Open Scratch Tab",
  run = function(ctx, args)
    eggie.client.queue("tab.new")
  end,
}
eggie.bind("cmd+shift+s", "scratch")
```

主题名必须存在于内置主题目录，且匹配 `light` 或 `dark`。字体大小范围为 8–32。快捷键支持 `cmd+d` 或 `cmd-d` 写法，连续按键用 `>` 分隔；Lua 可给一个动作绑定多个按键。`pane.split` 的方向为 `right`、`left`、`up`、`down`。`tab.new` 和现有内置动作不接受参数。

动作回调收到 `ctx`（`cwd`、`session_id`、`window_id`）和绑定时给出的 `args`。回调在 daemon 中串行执行；它调用 `eggie.client.queue` 暂存 GUI 动作，只有回调成功，队列才会提交到发起操作的窗口。配置加载阶段不能调用客户端动作，动作回调中也不能修改注册表。内置动作和自定义动作都可出现在快捷键与命令面板中。

## 检查与重载

```sh
eggie config check [path]
eggie config reload
```

`check` 在本进程解析并验证配置，不发布配置版本；Lua 标准库本身仍可能产生用户代码指定的副作用。`reload` 请求 daemon 创建候选 VM；成功后替换有效配置并通知所有 GUI 窗口。失败时沿用上一版主题、快捷键和动作；命令行输出错误，设置窗口显示相同诊断。首次启动配置错误时使用默认值及 GUI 偏好。重载是显式操作，目前不监视文件变化。

daemon 持有配置 VM、偏好和事件总线。配置快照传递纯数据；动作请求携带配置版本和窗口上下文，Lua 函数不跨 IPC 传输。设置界面修改偏好时写给 daemon，再由 GUI 将 Lua 覆盖层应用到有效设置。

## 事件脚本

```lua
-- status.lua
eggie.on("terminal.program_status_changed", function(ctx, ev)
  print(ctx.session_id, ev.id, ev.status)
end)

eggie.on("event.gap", function(_, ev)
  -- 事件历史被截断；可调用 eggie.sessions() 获取当前会话快照。
  print("event gap after sequence", ev.sequence)
end)
```

```sh
eggie do status.lua [--session <session-id>] [args...]
```

`eggie.on(name, callback)` 在脚本加载时注册监听器；回调按注册顺序串行调用。`eggie.wait(name, timeout_seconds)` 从同一游标等待一个事件，返回 `event, ctx`，超时返回 `nil, "timeout"`。还有 `eggie.args`、`eggie.session_id`、`eggie.sessions()`、`eggie.sleep(seconds)`、`eggie.stop(code)`、`eggie.invoke(name, args)`。后者没有 GUI 窗口上下文，因此不能运行需要客户端动作的回调。

首批事件：`terminal.title_changed`、`terminal.pwd_changed`、`terminal.process_changed`、`terminal.bell`、`terminal.progress_report`、`terminal.program_status_changed`、`terminal.program_status_removed`、`terminal.child_exited`。事件包含 `name`、`sequence`、`timestamp_ms`、`session_id`、`payload`，并把 payload 的字段直接映射到事件表。总线保留最近 1024 条；消费者发现缺口时收到 `event.gap`，可用 `eggie.sessions()` 重新读取当前状态。

## 当前边界

GUI 原有快捷键编辑器仍为每个内置动作保存一个偏好按键；Lua 的绑定集合叠加在其上。Lua 动作只能通过注册的客户端命令操作 GUI，尚无修改终端策略、项目或 profile 的配置 API。事件脚本只支持明确启动的进程，不随 GUI 自动启动。

设计参考：[Rex 配置](https://www.superlogical.com/rex/docs/customize/config)、[Lua API](https://www.superlogical.com/rex/docs/reference/lua)、[事件](https://www.superlogical.com/rex/docs/reference/events)。
