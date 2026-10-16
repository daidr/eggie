# Eggie agent 工作约定

## 开始工作

1. 阅读 [设计索引](design/README.md)、[设计原则](design/philosophy/README.md) 和相关功能文档
2. 检查当前分支、`git status` 和目标实现；保留用户及其他任务尚未提交的改动
3. 需要了解历史取舍时查[决策记录](design/decisions/README.md)；用户当前明确要求优先于历史文档

按用户授权把改动完整做到可验证；常规可逆修改不反复索要确认。缺少关键产品语义时尽早澄清，
同时继续不依赖答案的工作。不要把临时路径、旧测试结论或提案当作当前事实。

## 同步维护 design

- 架构、协议、数据所有权、持久化、默认行为、功能和交互契约变化时，**同一改动更新对应文档**
- 新功能补充目标、用户操作、异常/边界、实现边界和验证方法；新增或迁移文档同步修复索引与链接
- 用户明确作出的长期决定记入决策索引；区分确认、待实现、现行实现、已替代和撤回
- 修改取舍保留历史与替代关系，避免复制多份相互冲突的设计；参数与版本优先链接事实来源
- 纯实现修复且设计契约未变化时无需新增文档；历史测量和视觉检查标明环境与局限
- 文档和必要的小型参考资产可提交 Git；不收录凭据、私人终端内容、构建产物或临时截图路径作为唯一证据

## 实现边界

- 遵循 [crate 与模块边界](design/architecture/workspace.md)；通用控件进 `eggie-ui-kit`，终端绘制进 `eggie-terminal-view`
- daemon 管理 PTY/内核，GUI 管理展示；输入与会话身份不能随窗口移动丢失
- 复用已有选择器、连续 R 角和 popup；可交互节点使用稳定 ID，hover 不依赖其他刷新
- 新增进程图标使用本地 SVG，校准容器内视觉比例并验证实际渲染；环境提供 `eggie-process-icons` skill 时使用它
- 交互和动画验证取消、中断、反向、过期异步结果与清理；自定义绘制须继承透明度和裁剪
- 优化提供同负载证据，保持视觉、功能和流畅度，不用删除体验来降低指标
- 产品文案直接、简短；删去复述标题或选项的说明，控件标签与短说明不加句号

## 验证与交付

按实际风险选择验证，先运行聚焦测试；不为纯文档改动编译整个应用，不重复无关测试。

```sh
python3 scripts/check-architecture.py
env -u NO_COLOR cargo check --workspace
env -u NO_COLOR cargo test -p <受影响的 crate> --lib
git diff --check
```

文档改动检查相对链接、索引覆盖和旧路径残留。主题变更使用 `scripts/theme-palette-audit.sh`；
shell 补全变更可使用 `python3 scripts/tests/shell-completion.py`。视觉与交互改动还要在实际应用检查，
自动测试未覆盖的场景明确说明，不能把构建成功当成视觉验证。

完成用户要求的修改后重启 **dev Eggie**，除非用户显式说不重启。GUI 重启复用兼容 daemon，
保留会话。协议变更可能终止旧 daemon 和会话，交付前说明影响并处理既有授权。

- 开发构建：`env -u NO_COLOR scripts/make-app.sh --debug`
- 开发应用：当前仓库 `target/app/Eggie.app`，启动时明确这个路径，避免误启 `/Applications/Eggie.app`
- 本地 dev 使用 `me.daidr.Eggie.dev` bundle ID；`make-app.sh --debug` 自动设置该 ID 并签名
- GUI 与 daemon 的运行环境均不能包含 `NO_COLOR`；构建命令去掉它不能证明继承中的旧 daemon 已清除
- 使用当前环境允许的 GUI 工具操作和验证应用；不要通过重启 daemon 来代替单纯 GUI 重启

提交仅限用户要求的范围，检查 staged diff，排除 `target/`、`.tui-test/`、日志和其他任务的改动。
只有用户要求时才提交或推送；交付说明改了什么、如何验证、哪些尚未提交或未验证。
