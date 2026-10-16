# 内存调查与渲染缓存预算

> 历史调查与实验记录。文中的 PID、峰值和基准属于各次取样，不代表当前构建；复测时固定负载并重新取样。持续约束见[设计原则](../philosophy/README.md)。

## 2026-10-11 调查

用户提供的 macOS 取样记录：physical footprint **217.7MiB**，峰值 **337.9MiB**。
该文件是 CPU 调用栈取样，可以说明线程在执行或等待什么，不能归因堆分配。
本次另采集 `vmmap`、`footprint` 和 `heap`，并检查实际缓存生命周期。

首次现场取样（GUI PID 53812，单个 1280×801 逻辑像素窗口）约 **91.6MiB**：

| 分类 | 当时占用 | 来源 |
| --- | ---: | --- |
| IOSurface | 32MiB | Retina 窗口显示缓冲 |
| IOAccelerator graphics | 24MiB | 字形、GPUI 图集、实例缓冲等 GPU 资源 |
| Malloc Small | 17MiB | Rust 对象、字体和系统框架等堆分配 |
| Malloc Large | 4.5MiB | 预分配的字形任务队列 |
| 其他 | 约 14MiB | 系统框架、页表、线程、驱动资源等 |

后台终端服务 PID 17955 当时约 **34MiB**，其中 Malloc Small 约 29MiB。
Shell 和终端中的其他程序是独立进程，不能全部归到 GUI 的内存。
虚拟地址空间、共享库 resident 和实际 physical footprint 也不能混算。

后续切换项目、打开设置和运行 btop 时，GUI footprint 曾升到 325.4MiB；
大部分增长来自 IOSurface 和 mapped/unmapped graphics 资源。GPU 资源的可回收状态
也会使相邻的 `vmmap`、`footprint` 结果不同，比较时必须保持工具和页面状态一致。

## 已实施的优化

### 字形纹理按需增长

原来每个 `MetalTerminalRenderer` 首次绘制都会创建 4096×4096 R8 字形纹理，固定占用
16MiB。现在从 512×512（256KiB）开始，宽高分别按需增长。普通文本通常只需
256KiB～2MiB；实际值取决于字号和不同字形数量。

彩色 emoji 图集仍按首次使用创建，但初始大小从 2048×2048 RGBA（16MiB）降为
512×512（1MiB），同样按需增长。

逻辑打包坐标、最大容量和满图集时的重置行为保留。扩容复制旧纹理中的像素，GPU
实例使用像素 UV，着色器按绑定纹理的实际尺寸换算，防止同一批次扩容造成采样错位。
Metal 命令缓冲保留旧纹理，扩容不会覆盖其在途读取的资源。

主窗口共享一个终端渲染器，预览窗口有自己的渲染器；节省量不是简单按标签页数量相乘。

### 字形任务队列按需分配

原来的 `sync_channel(32768)` 即使空闲也预分配约 4.5MiB。
现在使用按需分配的通道，通过原子计数保留 32768 个未完成任务的上限和饱和时的
同步绘制回退。任务入队前预留计数，断开连接时回滚，避免工作线程先完成导致计数下溢。
现场 `heap` 复测中，原来的单块 4608KiB 分配已消失。

### 动态图片自动回收 GPU 图集

直接绘制 `Arc<RenderImage>` 原来只上传到图集，没有随最后一个图片所有者消失自动
删除图块。图标动画产生新图片 ID 时，旧 GPU 图块会继续累积。

现在窗口以弱引用跟踪上传图片。帧保留当前图片，并在复用缓存视图的绘制片段时一起
复用图片引用；旧帧清理后，移除已经没有所有者的图块。跨窗口共享的图片仍然有效。
已有的显式 `drop_image` 路径也同步清理跟踪记录。

### 矢量路径使用局部绘制目标

原来窗口创建/调整尺寸时就分配整窗的 BGRA 路径绘制目标及 4×MSAA 目标。
进程树上一条很小的连线也会用到整窗缓冲。2560×1602 的 BGRA 目标本身约 15.6MiB。

现在仅在场景存在路径时创建资源，按实际路径的裁剪边界分配，增加一像素抗锯齿留白，
并向上取整到可复用的尺寸。小连线可以使用 64×64 的目标（BGRA 16KiB）。
保留 4×MSAA；Apple GPU 的 MSAA 仍使用 memoryless，不能把其理论容量当成常驻物理内存。
相同大小的缓冲复用，窗口缩小时释放过大的资源。

栅格化时减去局部原点；梯度计算时恢复全局坐标；最终纹理采样使用局部边界。
这保持路径位置、遮罩、透明度和渐变含义。

## 验证与测量边界

- 35 个终端 Metal 测试通过，覆盖字体、emoji、图集扩容、旧像素保留、原打包容量、
  队列背压及断开连接清理、着色器实例读取。
- 3 个图片生命周期测试通过：连续替换 300 帧、跨窗口共享、缓存视图复用。
  临时停用回收后的对照测试在第二帧即发现旧图块累积；测试后恢复了源码。
- 3 个原生 Metal 路径测试通过：无路径不分配、缓冲复用和窗口缩放、局部与整窗
  绘制对比。对比分数像素位置、曲线、渐变、重叠透明路径和裁剪，允许最多一个
  8 位通道单位的浮点舍入误差。
- 6 个已有的堆叠图标动画测试通过；架构边界检查和 `git diff --check` 通过。
- dev Eggie 已重启。最后一次重启前后 4 个 session ID 与 shell PID 完全一致。

最终 GUI PID 91389 连续三次 `vmmap -summary` 为 **68.1MiB**。
此时用户已关闭 DevBox 的 btop 终端，页面为该项目的空状态；先前的 242.4MiB
取样有活跃 btop 终端及进程树，因此这两个数不能用于计算整体优化百分比。
固定分配和图片生命周期的改善由资源尺寸、堆取样及回归测试分别验证。
大量字形、多个窗口或较大矢量区域仍然会按实际需求占用更多内存。

## 重复取样

使用已确认的 GUI 或 daemon PID：

```sh
python3 scripts/profile-memory.py <pid> --output /private/tmp/eggie-memory-profile --heap
```

生成工具原始输出和 `summary.json`。默认省略较慢的 heap census；加 `--heap` 后采集。
该脚本只读进程，不启动或切换窗口。比较前固定窗口尺寸、主题、活动标签页、侧面板、
终端内容和独立进程数量，分别记录 GUI 与 daemon，复现同一组交互并重复取样。

普通回归命令：

```sh
cargo test -p eggie-terminal-view --lib metal_terminal::tests
cargo test -p eggie-ui --test image_lifecycle
cargo test -p eggie-ui --lib app::project_icons::tests
python3 scripts/check-architecture.py
```

原生路径测试位于 vendored `gpui_macos` 的 `metal_renderer::memory_tests`。
该 package 不是主 workspace member，Cargo 不直接允许测试其 dev-dependencies；
本次通过临时省略其重复 GPUI dev-dependency，同时选中 `eggie-ui` 提供 test-support
执行原生测试，随后完整恢复了 manifest，未改变依赖配置。

## 补充调查：空终端与关闭窗口后的泄漏

用户反馈只有几个空终端仍达到约 260MB。本次固定在 Home 项目、1280×801 逻辑像素
窗口、信息侧栏和同一组 5 个 shell 进程下检查。旧 GUI PID 5336 当前为 **153.5MiB**，
峰值 **270.0MiB**；后台终端服务 PID 17955 为 **40.8MiB**。

旧 GUI 的 IOSurface 达到 **82.8MiB**：除主窗口显示缓冲外，还能看到已经关闭的
设置窗口（1840×1482）和弹窗（442×518）的显示缓冲。`heap` 中有 3 个 CAMetalLayer。
再次打开、关闭一次设置后，只剩主窗口可见，但 GUI 增至 **181.3MiB**，IOSurface
增至 **104.5MiB**。这是窗口生命周期泄漏，不是空终端的文本缓存。

### 根因与修复

`MacWindowState` 持有 `SubclassingAdapter`，适配器强引用窗口内容视图；内容视图
拥有 GPUIView，GPUIView 的 ivar 又持有 `Arc<MacWindowState>`，形成循环引用。
原来的 Metal `destroy()` 是空操作，关闭后的整个 renderer 因此仍然存活。

改为在自有 GPUIView 上直接实现 AccessKit 的三个原生入口：children、focus、hit test，
并使用只弱引用视图的 `accesskit_macos::Adapter`。原生窗口和视图释放后，窗口状态、
纹理、图集和绘制目标随之释放；保留无障碍树的激活、更新和焦点通知。

验证中曾尝试在关闭时直接销毁动态 subclass 适配器，但其恢复 Objective-C class
会与 macOS 27 的 Touch Bar/KVO 清理冲突，触发异常退出。该方案已移除；最终实现
不动态替换视图 class，也不通过延时释放来规避观察者清理。

### 真实窗口回归与最终占用

最终 GUI PID 23664 反复打开设置、通过原生关闭按钮关闭，并验证设置窗口消失：

| 开关次数 | 关闭后 GUI footprint | IOSurface resident |
| --- | ---: | ---: |
| 1 | 84.8MiB | 32.2MiB |
| 2 | 84.7MiB | 32.2MiB |
| 3 | 85.4MiB | 32.2MiB |

随后连续三次静置取样为 **81.7、81.6、81.6MiB**。`heap` 中只剩主窗口的
**1 个 CAMetalLayer 和 1 个 GPUIView**。修复前后主窗口大小、项目、活动终端、
信息侧栏保持一致，5 个 shell PID 完全相同，最终 dev Eggie 已在运行。
构建、单文件 rustfmt、架构边界检查及 `git diff --check` 通过。

打开设置期间仍存在临时 GPU 分配，最终进程峰值为 223.2MiB；取截图后的单次 vmmap
也曾报告 195.7MiB，随后 footprint/heap 为约 85.5MiB，静置后回到上述 81.6MiB。
因此应区分稳定占用、临时峰值和驱动可回收资源，不能只挑一次最低值。

原始记录：`/private/tmp/eggie-empty-memory-before/`、
`/private/tmp/eggie-empty-memory-old-close.txt`、
`/private/tmp/eggie-empty-memory-native-final/`、
`/private/tmp/eggie-empty-memory-native-cycle-{0,1,2}.txt`、
`/private/tmp/eggie-empty-memory-native-final-steady-{0,1,2}.txt`。


## 补充调查：设置峰值、共享 GPU 程序与关闭后的残留

本轮使用 `libproc.proc_pid_rusage(RUSAGE_INFO_V4)` 连续读取 physical footprint。
它不扫描 GPU 虚拟内存；`footprint -v` 等详细扫描在对照实验中会额外触发约 110MiB
的短暂 graphics 分配，因此类别调查和峰值回归分开进行。采样脚本新增以下模式：

```sh
python3 scripts/profile-memory.py <pid> --output /private/tmp/eggie-memory-samples --sample-seconds 15
```

输出每个样本以及平均值、中位数、P95、测量区间最大值、进程生命周期峰值。
生命周期峰值包含启动和所有此前交互，不能当成当前设置页的稳定占用。

### 223MiB 的来源与仍然存在的峰值

在相同的 1280×801 主窗口、信息侧栏、活动终端和 5 个 shell 下，改动前连续空闲
平均为 **73.43MiB**；第一次打开通用设置的 15 秒区间峰值为 **223.09MiB**。
详细类别调查显示，峰值的大部分增量属于约 **110MiB 的 owned unmapped graphics**，
另有窗口 IOSurface、GPUI 图集、字体和应用堆对象。它不是主题列表一次渲染了全部主题。

临时阶段计数记录显示，主窗口首次提交 GPU 命令后，footprint 从约 29MiB 升到
159MiB；初次设置窗口的构造之前，也已经能看到这段临时增量。
这把增量定位到了 Metal/原生显示路径，但尚未定位到驱动内部的具体分配对象。
离线编译和共享程序并没有消除它，不能将其描述为已经修复。

多个相同场景的优化后测量仍出现 **214～231MiB** 的设置初次显示峰值。
Retina 主窗口每份显示缓冲约 16MiB，设置窗口每份约 10.6MiB；系统保留两份或三份
会改变相邻测量结果。包含截图、滚动和主题预览的交互会产生更高的生命周期峰值。

### 本轮实施

- **构建期编译 Metal shader**：GPUI 和终端直接加载 metallib，避免每个窗口在运行时
  编译 shader 源码。macOS 构建需要 Xcode 的 Metal 编译器，README 已记录要求。
- **共享不可变的 GPU 程序**：同一平台 context 共享 device、8 个 GPUI pipeline 和单位
  顶点缓冲。每个窗口仍独立拥有显示图层、图集、路径目标和命令队列。
- **实例缓冲按需分配**：初始容量从 2MiB 降到 256KiB；复杂帧仍按原有的倍增重试路径
  扩容，保留最大容量和 GPU 完成后回收的时机。
- **GPUI 图集按需增长**：初始页从 1024² 降到 256²，容量不足时扩展至 1024²，再按
  原有容量添加新页。纹理 ID、图块坐标不变；复制旧像素，保留在途命令引用的旧纹理。
- **每帧 autorelease pool**：及时释放当前帧的自动释放编码器和 drawable 引用；命令
  缓冲继续保留 GPU 在途资源。

### 原生视图晚释放时，GPU 资源必须立即解绑

真实回归补充了“打开外观 → 滚动/预览主题 → 截图 → 关闭设置”的路径。
此前关闭后只剩主窗口可见，但 heap 仍存在 **2 个 CAMetalLayer**，IOSurface 为
**79.8MiB**，12 秒轻量采样平均 **133.09MiB**。已经关闭的设置仍保留 3 份
1840×1482 显示缓冲。这条路径没有被之前仅打开通用设置的关闭回归覆盖。

现在 `MacWindowState` 的 renderer 使用 `Option`：GPUI 窗口释放时立即取出并销毁
渲染器，停用帧源和请求帧回调。原生关闭任务解绑 NSView 的 backing layer。
这样 GPU 生命周期跟随 GPUI 窗口，而不依赖原生观察者何时最终释放 NSView；
迟到的 backing/resize/draw 回调能够处理没有渲染器的已关闭状态。
不修改 Objective-C class，也不等待一个猜测的延时再销毁对象。

第一次关闭后的最初 12 秒仍约 **97.35MiB**；后续详细 VM 取样后，只剩主窗口
的 1 个 CAMetalLayer、1 份 2560×1602 drawable，连续 10 秒平均 **62.30MiB**。
这个低值出现在详细扫描之后，不能将它当作所有日常使用的平均值。

### 验证与未采用的尝试

35 个终端 Metal 测试通过，终端实例读取测试同时验证运行时源码编译和打包 metallib。
6 个原生渲染测试、4 个图集测试通过，包含以下新增对照：

- 运行时和预编译 GPUI 程序的连续圆角、细边框、渐变和阴影逐像素对比，最大通道
  误差不超过 1/255。
- 超过 256KiB 的真实帧自动扩至 512KiB，最后一个图元完整显示，下一帧继续复用。
- 两个窗口共享程序但独立持有图集，关闭一个之后另一个仍输出相同像素。
- 图集扩容后的纹理 ID、坐标、旧像素与在途旧纹理引用保持正确。

曾对照限制 Metal 命令队列长度、把小常量合并到实例缓冲，两者都没有改善约 110MiB
的瞬时驱动增量，已经完整撤回。保留原有显示刷新调度、显示缓冲策略、4×MSAA 和
Retina 分辨率；没有通过降低这些设置换取较小的内存数字。

原始记录：`/private/tmp/eggie-memory-old-light-{idle,settings}/`、
`/private/tmp/eggie-memory-final-{idle,settings-open,settings-steady}/`、
`/private/tmp/eggie-memory-final-closed-categories/`、
`/private/tmp/eggie-memory-window-fix-{closed-1,closed-categories,warm-steady}/`。
后台 daemon 的独立取样为约 38～41MiB，不包含在上面 GUI 的数字中。


### 最终构建回归（GUI PID 70331）

最终构建连续执行 3 轮“打开设置 → 外观 → 滚动/预览 → 原生关闭”。
三个设置窗口均正常关闭，没有崩溃，原来的 5 个 shell PID 完全一致。
最后的 heap 只剩 **1 个 CAMetalLayer**，没有任何 1840×1482 设置 drawable，
主窗口此时保留 **3 份 2560×1602 drawable（合计约 48MiB）**。
原生观察者仍可能持有多个 GPUIView，但这些关闭视图已经不再持有渲染器和图层。

| 场景 | 轻量采样结果 |
| --- | ---: |
| 本轮两个初次窗口、相同的两份主窗口显示缓冲 | 改动前平均 73.43MiB；优化后平均 67.35MiB |
| 最终重启后的初次空闲，主窗口三份显示缓冲 | 10 秒平均 85.78MiB |
| 连续三轮设置预览/关闭后 | 12 秒平均 103.06MiB |
| 再静置的 20 秒，包含一次约 110MiB graphics 短暂增长 | 中位数 103.06MiB，平均 115.29MiB |
| 三轮设置交互的 45 秒区间 | 最大值 239.75MiB |

这轮实现降低了初始资源预算，解决了额外的关闭窗口 GPU 残留，保持像素质量和
原有刷新策略。**尚未实现日常持续平均 60～70MiB，也没有明显降低设置操作峰值。**
当前应继续区分窗口显示缓冲、应用/字体堆缓存、临时驱动分配，再评估下一步；
不能通过选择 62MiB 的单次状态来宣称已达成目标。

最终记录：`/private/tmp/eggie-memory-delivery-{idle,cycles,warm-idle,warm-later,categories}/`。
阶段诊断日志：`/private/tmp/eggie-metal-memory-{43326,47497,51364}.log`；
这些临时计数、队列和常量缓冲实验代码均没有留在最终源码中。

## 补充调查：独立 Metal 对照与峰值上限

为排除 Eggie 的终端内容、字体、着色器和窗口路径，新增完全独立的
`scripts/probe-metal-memory.m`。它只有一个 16×16 BGRA 私有纹理，使用公开 Metal API
执行清屏，读取同一组 libproc counters 和 `MTLDevice.currentAllocatedSize`。

```sh
xcrun clang -O2 -fobjc-arc -framework Foundation -framework Metal \
  scripts/probe-metal-memory.m -o /private/tmp/eggie-metal-memory-probe
/private/tmp/eggie-metal-memory-probe clear 16
/private/tmp/eggie-metal-memory-probe empty 16
/private/tmp/eggie-metal-memory-probe blit 16
```

本机对照结果（每次为独立进程）：

| 操作 | 完成时 footprint | Metal 已分配资源 |
| --- | ---: | ---: |
| 空命令提交 | 4.92MiB | 0.45MiB |
| 空 blit encoder | 5.09MiB | 0.45MiB |
| 16×16 清屏 render pass | 114.84MiB | 0.91MiB |

清屏没有应用 shader、GPUI、AppKit、字形或 drawable，也没有加载 Eggie。
在本机系统/GPU 组合中，**执行 render pass 本身就能触发约 109MiB 的临时增量**，
应用可见的 Metal 资源总量不足 1MiB。256×256、限制 tile 大小、非持有式命令缓冲、
Metal 4 的独立实验也出现相同增量。关闭 Instruments 后复测仍可复现。
这进一步收窄到系统 GPU 执行路径，但还不能断言其具体内部对象，或断言其他设备和
系统版本必然如此。当前未找到能消除此增量的公开配置。

### 设置窗口显示缓冲

GPUI 新增 `WindowOptions.buffering`，默认 Triple；Eggie 设置窗口使用 Double。
这使设置窗口最多持有两张显示纹理，避免第三张 1840×1482 纹理可能增加的约
10.6MiB。主窗口的默认容量及 Vsync 保留。它控制容量上限，不会把像素分辨率降低。

真实滚动及悬停预览的对照中，各采集 149 个设置窗口绘制帧：

| 指标 | Triple | Double |
| --- | ---: | ---: |
| 获取 drawable 的 P95 等待 | 0.108ms | 0.109ms |
| 编码/提交 P95 | 0.318ms | 0.352ms |
| 获取 drawable 最大等待 | 0.784ms | 1.438ms |
| 30 秒交互区间峰值 | 238.61MiB | 238.17MiB |

两次 VM 扫描都只看到两张设置窗口 drawable，因此**本次对照并未显示显著峰值改善**；
不能把容量上限减少约 10.6MiB 当成每次都会节省的实测值。
绘制几何、主题效果与抗锯齿未改；本轮负载中未观察到 drawable 等待恶化。
临时帧计数代码已经移除，不会在正常运行时进行文件写入。

### 只枚举需要的字体

原来设置打开时生成所有已安装字体的 descriptor，然后遍历筛选等宽字体。
现在以 `kCTFontMonoSpaceTrait` 为必需属性，直接调用
`CTFontDescriptorCreateMatchingFontDescriptors`。在本机，descriptor 从 659 个降为
36 个，最终 11 个字体家族完全一致，去重、排序和 Menlo 回退保留。
独立进程对照的峰值从 5.20MiB 降至 3.58MiB，约节省 1.6MiB。
这是该项枚举的局部效果，不能据此宣称整个 GUI 峰值已经大幅降低。

原生回归测试将直接查询结果与全部字体筛选结果比较，确保不丢字体选项。
记录：`/private/tmp/eggie-peak-font-test.log`，
`/private/tmp/eggie-font-{all,filtered}-{names,memory}.txt`。
峰值与帧对照：`/private/tmp/eggie-peak-{three,two}-buffers/`，
`/private/tmp/eggie-peak-{frames,layout}-{84564,88759}.csv`。

### 最终构建复测

移除临时帧追踪并重启后的 GUI PID 94520，在相同窗口尺寸、项目、活动空终端和
信息侧栏下，首次设置/主题交互的 30 秒区间峰值为 **234.03MiB**。再完成两轮
设置打开、滚动预览和关闭后，进程生命周期峰值为 **235.72MiB**；返回主窗口的
6 秒平均为 **98.95MiB**。这些数值只说明本轮场景的小幅改善，不能据此推断
所有操作或其他硬件的峰值已经降低相同幅度。

原生字体选项对照测试、dev 构建、架构边界和 diff 检查通过；连续三轮真实设置操作
没有崩溃，5 个 shell PID 在重启和回归后完全一致。主窗口显示缓冲、主题渲染与
抗锯齿策略未改。当前仍未消除主要的系统 GPU 临时增量，未达成显著峰值下降。

最终记录：`/private/tmp/eggie-peak-final-{interactions,after-cycles}/`，
`/private/tmp/eggie-peak-final-build.log`，
`/private/tmp/eggie-peak-sessions-{before,final}.txt`。

## 再次优化：设置按需构建与被撤回的实验

设置的 `render` 原来先构建 General、Appearance、Advanced 的全部控件，再在
`match selected_section` 中选用其中一组。未显示的控件仍然占用当帧 element arena，
并创建字符串、监听闭包和布局对象。现在将各组构建移动到
`settings_window/sections.rs`，只执行当前分类的分支。行顺序、样式、输入 Entity、
主题预览及滚动句柄均保留；五个已有设置测试和架构检查通过。
这是减少临时对象和绘制工作，**不能据此认定已大幅降低设置峰值或正常空闲内存**。

### 分配器碎片回收：未保留

GUI 94520 的扫描中，malloc 实际使用约 19.1MiB，dirty+swap 碎片约 15.4MiB。
尝试在设置 Entity 销毁后的 defer 阶段，以后台任务调用公开 API
`malloc_zone_pressure_relief(NULL, 0)`。它只请求 malloc 归还未使用页，不删除应用缓存。
GUI 15889 的三次关闭分别耗时 0.889、0.478、0.296ms，三次返回的释放字节数均为 0。
未得到收益，已完整移除调用及诊断日志。

### 主窗口 Double：高帧率对照后撤回

两张显示纹理相比三张能少一张 2560×1602 BGRA 显示纹理，容量差约 16MiB，
但必须验证对帧率的影响。首次后台测试被原有约 30fps 节流限制，不能用来判断
120Hz 的行为。第二次仅在临时诊断构建中解除后台节流：前六秒持续绘制原有工作区，
后六秒额外绘制 10,000 个连续圆角图元和八层半透明覆盖；均去掉各阶段最初一秒和
末尾十帧。测试期间未并行编译。

| 场景/指标 | Triple（GUI 32649） | Double（GUI 28404） |
| --- | ---: | ---: |
| 原有工作区 FPS | 116.22 | 116.81 |
| 复杂画面 FPS | 119.71 | 113.46 |
| 复杂画面帧间隔 P95 | 9.082ms | 12.621ms |
| 复杂画面获取 drawable P95 | 0.985ms | 2.795ms |
| 复杂画面获取 drawable 最大等待 | 2.291ms | 21.539ms |

Double 在这个负载下确实增加等待及帧间隔波动，因此主窗口保留 Triple。
主窗口设置、背景节流、诊断图元和日志代码都已逐字节恢复到实验前版本。
此结果也说明此前设置窗口的低负载对照不能外推到所有窗口；设置窗口 Double
仍是上一轮的原有配置。本轮没有为了较小的内存数字降低分辨率、抗锯齿或帧率。

记录：`/private/tmp/eggie-further-{before,baseline-cycles,baseline-closed}/`、
`/private/tmp/eggie-heap-relief.csv`、`/private/tmp/eggie-main-bench-{28404,32649}.csv`。

### 本轮最终构建（GUI 37567）

已恢复所有临时诊断改动并重启 dev Eggie，原来的五个 shell PID 完全一致。
六个设置分类均进行了真实窗口切换检查，三个外观页开关周期的滚动/预览正常。

| 场景 | 轻量 libproc 采样 |
| --- | ---: |
| 最终构建重启后空闲，6 秒 | 平均 83.46MiB |
| 三轮设置交互，30 秒 | 中位数 98.92MiB；最大值 235.06MiB |
| 三轮关闭后空闲，6 秒 | 平均 98.49MiB |

这与上一轮约 99MiB 的暖态及 234～236MiB 的峰值相近，没有证据表明本轮大幅
降低了正常内存或峰值。按需构建降低的是未显示控件的临时分配，其他实验未保留。
后台 daemon 没有重启，其内存不包含在这张 GUI 表中。
原始记录：`/private/tmp/eggie-further-final-{idle,cycles,warm}/`。

## 2026-10-16：运行保护与交互优化期间的复查

本轮没有修改主窗口显示缓冲数量、分辨率、抗锯齿或帧率策略。
GUI 74487、1485×926 pt 窗口的一段轻量 15 秒采样，中位数约 104.31MiB，
平均 141.37MiB，范围 104.25–216.55MiB；当时有两个终端，用户仍可能操作应用。
这不是静止场景，也未与新版完成同场景对照，不能与早期 1280×801 pt 窗口数字直接比较，
更不能据此宣布峰值下降。仍没有新的无损方案证明可大幅消除系统 GPU 临时增量。

本轮明确的优化是 daemon 进程检查共享采样：三个消费者连续请求同一进程树 12 次，
旧路径每次刷新进程快照并运行 lsof 为 411.19ms，新路径为 35.79ms（包含首次采样）。
测试命令：`cargo test -p eggie-daemon --lib process_monitor::tests::shared_inspection_latency -- --ignored --nocapture`。
这是短时多次查询的延迟对照；缓存有效期一秒，不能外推为稳定 CPU 或 GUI 内存下降相同倍数。

新增设置搜索只构建搜索结果，命令输出读取复用既有 scrollback；命令范围数量有上限，
不另外缓存一份输出全文。

### 2026-10-17：新版交互验证后采样

GUI 56778、daemon 26734，dev debug 构建，主窗口 1485×926 pt，设置窗口 920×741 pt。
测试对象清理后保留一个正常等待输入的本地 zsh，原有已结束标签及分屏布局保留。

前次 107.33 MiB GUI、9.45 MiB daemon 和 230.67 MiB 设置峰值的读数撤回为无效基线：
当时 `path_helper` 等待 macOS 桌面访问授权，尚未显示 prompt，部分页面切换也未确认。
系统日志确认 dev 临时签名变化使原桌面访问授权失效。用户授权后于 01:30 点击系统“允许”，
确认 `path_helper` 退出、prompt 正常显示、shell 集成进入 input 阶段，再进行下表采样。

使用 `scripts/profile-memory.py --interval .1` 的轻量 libproc 计数；稳定空闲为 15 秒，
设置场景为 12 秒，包含开启、切换或关闭后的过渡与回收。采样期间不截图、不运行 heap/vmmap；
不同阶段之间的截图与此前 UI 测试仍会影响暖态，因此这不是冷启动或与旧版的严格对照。

| 场景 | 平均 | 中位数 | 采样最大值 |
| --- | ---: | ---: | ---: |
| 单个空终端，稳定后 GUI | 110.05 MiB | 110.05 MiB | 110.05 MiB |
| 同时的 daemon | 13.05 MiB | 13.06 MiB | 13.09 MiB |
| 打开设置通用页，GUI | 159.77 MiB | 123.06 MiB | 235.30 MiB |
| 切换设置外观页，GUI | 166.82 MiB | 137.20 MiB | 249.45 MiB |
| 关闭设置后的 12 秒，GUI | 125.98 MiB | 93.36 MiB | 205.64 MiB |

这轮没有证据表明 GUI 平均内存或峰值已显著降低，仍未达到 60～70 MiB。
进程检查共享采样的延迟收益与 GUI/GPU 内存分开评价；未改动视觉或帧率策略。
