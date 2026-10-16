# Eggie 设计文档

这里保存架构、交互原则、功能设计、产品决策和验证记录，随代码一起由 Git 管理。

## 从哪里开始

- 理解项目：[设计原则](philosophy/README.md) → [架构边界](architecture/workspace.md)
- 修改功能：先读对应设计，再同步修改实现、文档和验证结果
- 查询取舍：[决策记录](decisions/README.md)
- 为项目工作的 agent：先读根目录 [agent.md](../agent.md)

## 文档索引

| 分类 | 文档 | 内容 |
| --- | --- | --- |
| 设计哲学 | [设计原则](philosophy/README.md) | 交互、文案、视觉一致性、性能与工程取舍 |
| 架构 | [crate 与模块边界](architecture/workspace.md) | 依赖、职责、UI 分层和架构检查 |
| 架构 | [多窗口与终端生命周期](architecture/multi-window.md) | 项目归属、展示事务、恢复与结束策略 |
| 架构 | [终端文字传输](architecture/terminal-transport.md) | socket、共享行租约、回退与延迟验证 |
| 架构 | [Lua 配置与事件](architecture/lua-configuration.md) | 注册 API、GUI 偏好、原子重载和执行边界 |
| 架构 | [Shell completion](architecture/completion.md) | provider、shell 编辑契约和稳定补全面板 |
| 功能 | [SSH 与端口转发](features/ssh.md) | 连接恢复、目录继承、服务发现和信息面板 |
| 功能 | [终端悬停预览](features/terminal-preview.md) | 侧边栏与标签栏、动态尺寸和原生弹窗动画 |
| 功能 | [终端尺寸与 prompt 重绘](features/terminal-resize.md) | 活动 prompt 锚点、滚屏与连续 resize 回归 |
| 功能 | [终端查找](features/terminal-search.md) | 后台查询、导航顺序、过期结果与统计缓存 |
| 功能 | [本地控制与 MCP](features/local-control.md) | CLI、类型化控制、MCP、输入与生命周期边界 |
| 功能 | [设置查找与命令历史](features/settings-and-command-history.md) | 搜索定位、输出范围、复制和信息层级 |
| 决策 | [决策索引](decisions/README.md) | 已确认的取舍、待实现方向和后续变更记录 |
| 调查 | [内存调查](investigations/memory.md) | 历史实测、缓存预算、采样方法与撤回的实验 |
| 验证 | [主题选择器历史验证](validation/theme-gallery.md) | 参考图、交互验证及历史对比结果 |

## 写什么、何时更新

- `architecture/` 解释职责、数据流、所有权和稳定约束，避免逐行复述实现
- `philosophy/` 记录跨功能长期适用的产品与工程原则
- `features/` 记录用户目标、操作行为、边界情况和验证方法
- `decisions/` 记录为什么这样选，以及明确推迟或撤回的方向
- `investigations/`、`validation/` 保存带环境和范围的证据，不把历史测量当成当前保证

改变职责、协议、持久化格式、默认值、交互或生命周期时，在**同一改动**中更新对应设计。
新增文档要进入本索引；移动文档要修复引用。纯内部修复若未改变设计契约，无需编写流水账。

功能文档应标明现行、提案、待实现或已替代；“现行”指当前工作区实现，不意味着已经提交或发布。
产品决策与实现状态分开记录，用户同意一个方向不等于代码已经完成。
用户的新指示优先于旧文档；发现冲突时先核对代码与上下文，再更新文档，不默默覆盖历史决策。

## 维护方式

小改动直接更新现有章节；涉及多种方案和长期取舍时新增独立决策文档，并链接到功能设计。
决策最少包含：状态、整理或决策日期、背景、选择、原因、代价、实现状态及相关链接。
早期对话没有可靠日期时只标整理日期，不补造原始日期。

文档里的源文件路径相对仓库，Markdown 链接相对所在文档。不要把 `/tmp` 截图作为唯一证据；
需要长期保留的图片放入所属目录的 `assets/`，控制体积，不包含凭据或私人终端内容。
历史记录中的临时路径仅用作线索。文档以中文为主；迁入的英文资料可以保留。
