# 重复开关窗口的资源回收调查（2026-10-23）

## 当前结论

已确认并修复切窗 HUD 的字形缓存增殖：动画连续改变浮点字号，GPUI 按字号精确位保存字形缓存，每轮新时序产生的字号不会随关窗清理。修复仅将 HUD 字号取整为 92–100 px 的 9 个值。单变量实机对照中，20 次开关后空闲 60 秒的增量从 +20.234 MB 降至 +0.917 MB，Metal 纹理从原来的 3→11 改为 3→3。

原观测还包含正常的短期图形占用：停止交互后约 115 MB 会回收。原产物 60 次观测中，窗口、视图、CAMetalLayer 和 Metal command queue 的存活实例均回到 1，不支持整窗对象未释放的判断。问题是应用持续产生不可复用的缓存键，不能把全部增长都归为 GPU 延迟或整窗泄漏。

单独 HUD 修复已通过回归、两阶段审阅和 20 次原生资源对照；按预设提前停止条件结束，没有扩为 60 次。主集成产物也完成 20 次复验，空闲增量 +1.229 MB、纹理 3→3。全部测试实例已退出，未提交或推送。

## 原采样条件

已读取仍存在的 `/tmp/zj-native-harness/reclaim20.py`。脚本启动后等待 2 秒，激活后等待 0.4 秒；每轮菜单「新建窗口」后等待 0.9 秒，点击关闭后等待 0.9 秒，再查询 AX 和 footprint。最后立即采样并结束进程，没有额外长空闲观测。脚本没有保存时间戳，实际轮间隔还包含 System Events 与进程查询耗时，不能写成精确的 1.8 秒。

原数据 `reclaim20.json` 的第 1 至第 20 次采样增长 22.430 MB，19 个间隔平均增长 1.181 MB。第 10 至第 20 次仍增长 9.912 MB。所有操作返回码为 0，AX 窗口数均为 1。

原 dist 与本次冻结副本的 SHA-256 均为 `ca4f0b630618fb6cccf32df66bdae95e142e5c62a691b547438038bd6261a682`，文件大小 49,461,488 bytes。当前 worktree HEAD 为 `c171f2ee0d47b1db3fe0a67b408b14df2ea7fcb7`；没有从 clean HEAD 重编译后与原产物混比。

## 本次有界观测

与其他 session 串行协调 GUI 和构建，使用冻结原产物、复制的工作区夹具、独立 settings/session/Agent DB。隔离设置关闭自动输入法切换及 Dock 图标闪烁，不修改系统设置。每次操作前确认 frontmost 为测试 PID；关闭前确认前端窗口是新建空窗口。每轮检查 AX 窗口数与开关事件差值均为 1。正常采样未打开 MallocStackLogging。

观测 PID 为 82241。先空闲 10 秒取基线，每 20 次开关后等待 10、30、60 秒；上限 60 次。若前 20 次的 60 秒值回到初始值 +5 MB 内则提前结束，这只是采样停止阈值，不是项目预算或泄漏判据。每段末尾保存 `footprint`、`vmmap -summary`、`heap -s`。12 次分类命令均返回 0。脚本完成返回 0，并对自身进程发送 SIGTERM、wait 回收，应用退出码为 -15；随后 `pgrep` 未发现该实例。

| 开关次数 | 关闭后约 0.9 秒 / MB | 10 秒 / MB | 30 秒 / MB | 60 秒 / MB |
| ---: | ---: | ---: | ---: | ---: |
| 0，初始空闲 | — | 58.640 | — | — |
| 20 | 194.561 | 83.052 | 78.874 | 78.874 |
| 40 | 212.584 | 101.172 | 96.978 | 96.978 |
| 60 | 226.543 | 115.115 | 110.921 | 110.921 |

表中 MB 按 `phys_footprint / 1_000_000` 计算。`footprint`、`vmmap` 文本自身使用的取整单位保留在原始输出，不与表中十进制值直接相减。

窗口事件最终为 opened 61、closed 60。全部采样 AX 窗口数为 1，进程树大小为 1。每段 10 至 60 秒的 frame 数不变，未观察到持续重绘。

### 存活对象

| 对象 | 初始 | 20 次后 | 40 次后 | 60 次后 |
| --- | ---: | ---: | ---: | ---: |
| GPUIWindow 子类 | 1 | 1 | 1 | 1 |
| GPUIView | 1 | 1 | 1 | 1 |
| CAMetalLayer | 1 | 1 | 1 | 1 |
| AGX command queue | 1 | 1 | 1 | 1 |
| AGX render pipeline | 8 | 8 | 8 | 8 |
| AGX buffer | 7 | 8 | 8 | 8 |
| AGX texture | 3 | 11 | 18 | 26 |
| TFPFont | 92 | 221 | 329 | 443 |
| CGFont | 62 | 127 | 181 | 238 |
| non-object 分配字节 | 8,081,425 | 18,662,545 | 28,667,049 | 31,953,017 |

20 至 40 次后，IOSurface 占用约保持 16 MiB，Malloc Small 约 30→40 MiB，IOAccelerator graphics 约 15→23 MiB。窗口对象未积累，Metal buffer 数量稳定，纹理与字体增长值得继续追踪。

## 静态证据

- `Workbench::drop` 发出 `window_closed`，只证明工作台销毁；不能代替底层对象计数。
- `session::closed` 移除窗口记录，文件监听 `Subscription::drop` 移除订阅并归还路径引用。本场景重复新建空窗口，没有反复打开文档或仓库。
- `MacWindow::drop` 已通过 `accesskit_adapter.take()` 拆除 adapter/content view/GPUIView/state 强引用环，并取消 frame source、清除回调，异步 close/autorelease native window。native window/view 的 dealloc 归还 ivar 中的 Arc。
- `MetalRenderer::destroy` 为空不能单独认定遗漏释放：Metal 拥有型字段的 Drop 会发送 release，关键是其拥有者是否销毁。
- 渲染器可见空闲回收阈值为 3 秒，原 0.9 秒采样早于此阈值。其后还有系统回收延迟，本次 10 至 30 秒又下降约 4 MB。
- shader cache 是单槽；atlas 的共享入口是 Weak，但仍有窗口时其 key 没有容量淘汰。实例缓冲池默认每块 2 MiB，归还后保留，无数量上限；本次存活 buffer 数量稳定，未支持该池持续增殖的假设。
- 重复新建窗口的标题编号来自当前窗口数，不是累计创建次数；不能用不断变化的标题解释持续新增字形。
- 独立静态复核未确证 retain/release 漏配或未拆除引用环。当时计划继续追踪字体 ID、字形缓存及共享图集；后续已用缓存键链和单变量资源对照完成归因，未运行 MallocStackLogging。

## 证据位置

本 worktree 的 `target/health-check/window-reclaim/`：

- `observe.py`：有界原生交互观测脚本，前台 PID/标题/窗口数保护。
- `run1/observation.json`：带单调时间戳的全部采样、分类命令返回码、二进制哈希。
- `run1/events.log`：应用事件。
- `run1/{baseline,cycle20_60s,cycle40_60s,cycle60_60s}-{footprint,vmmap,heap}.txt`：分类和对象计数。
- `analyze.py`：摘要脚本。

## HUD 字号缓存链与候选修复

`workbench/switch_hud.rs` 在从另一 ZJ 窗口返回时播放 HUD。前 120 ms 的 scale 连续经过 0.92–1.0，原代码以 `SWITCH_HUD_TEXT * scale`（100 px × scale）直接排版，每次实际动画帧时刻不同就生成不同的浮点字号。

GPUI `RenderGlyphParams` 将字号原样作为键，Hash 使用 `font_size.0.to_bits()`。App 级 `TextSystem::raster_bounds` 及共享 Atlas 的 `tiles_by_key` 对新参数插入，不在关闭某一窗口时清除。无需新增 FontId 即可持续产生字形。每个新字号还会走 CoreText `clone_with_font_size` 的排版、栅格化路径。

候选修改只在 HUD 的 `text_size(scale)` 中对像素值 `.round()`，不改变全局缓存、其他文本或主题 token。放大范围、时长、光晕、间距、透明度、减少动态效果分支均保留；字号最大取整偏差为 0.5 px。现有 Div 没有可直接复用的整体文字绘制缩放接口，因此没有新增自定义绘制组件。

回归 `repeated_animation_timings_use_bounded_font_sizes` 对 10,000 个真实 `frame(t)` 时刻调用界面所用字号函数，验证精确浮点键最多为范围内整数步数，并包含起止字号。将实际函数和主题 token 提取到独立 Rust 测试后：原逻辑失败，输出 `1184 font sizes exceed the 9 pixel-sized steps`；仅加 `.round()` 后通过，`1 passed; 0 failed`。项目内 `cargo test -p workspace-editor repeated_animation_timings_use_bounded_font_sizes --locked -- --nocapture` 通过（1 passed），`cargo test -p workspace-editor switch_hud --locked -- --nocapture` 通过（3 passed，454 filtered）。完整 cargo 测试在等待编译时读取的是取整后源码，不将该运行称为修复前失败证据；修复前失败来自前述独立 Rust 提取测试。这些纯逻辑检查不代替资源验收；随后完成的实机对照见下节。

`cargo fmt --all --check` 与 `python3 tools/check_git_deadlines.py` 通过，后者输出 `PASS: timeout, exited parent, cancellation; no live descendant remains`。`cargo build --profile dist --locked` 返回 0，产物 SHA-256 为 `b6c8792851539c22c212730cb9307ccbf4f9fc2094374858ce9e16ef7f516828`，大小 49,444,976 bytes。构建有已有依赖 `block v0.1.6` 的未来兼容警告，无错误。相对原二进制减少 16,512 bytes；没有新增依赖或 feature。

审阅按顺序执行：第一阶段 `ponytail-review` 为 0 findings；第二阶段 `code-review` 的 Standards 与 Spec 分别为 0 findings。Spec 复核独立重算得到原始 1,184 个字号、取整后 9 个字号。审阅只覆盖 HUD 增量，忽略载入的 49 个既有修改。全工作区检查由主协调 session 串行整合运行，已读取真实日志核验：合入 HUD 前全工作区测试 638 passed、0 failed、4 ignored；合入后 HUD 专项 3 passed、0 failed，另重跑全工作区 Clippy、格式检查和 dist 构建均成功。主集成 HUD 文件哈希与本次审阅版本一致。日志位于主 checkout `target/health-check/native-repairs-20261023/`。

## 修复后的单变量资源对照

运行产物 SHA-256 为 `b6c8792851539c22c212730cb9307ccbf4f9fc2094374858ce9e16ef7f516828`，PID 34868。冻结 `before-source.json` 中除报告外的 50 项文件哈希全部匹配，无缺失；额外生产改动仅 `switch_hud.rs`。脚本使用相同的采样器与流程，静态 AST 对比确认 `usage`、`tree`、`footprint_mb`、`App` 在载入冻结基线前后相同。

| 采样点 | 原产物 / MB | 单独 HUD 修复 / MB |
| --- | ---: | ---: |
| 初始空闲 10 秒 | 58.640 | 58.263 |
| 第 20 次关闭后约 0.9 秒 | 194.561 | 174.720 |
| 停止操作后 10 秒 | 83.052 | 63.374 |
| 停止操作后 30 秒 | 78.874 | 59.180 |
| 停止操作后 60 秒 | 78.874 | 59.180 |
| 60 秒值相对初始增量 | +20.234 | +0.917 |

修复后的短间隔采样在第 5/10/15/20 次为 174.392/174.605/174.622/174.720 MB，已接近平台。20 次后的 Metal texture 仍为 3（初始 3），TFPFont 92→105、CGFont 62→69，符合少量字号预热；GPUIWindow、GPUIView、CAMetalLayer、command queue 各为 1，buffer 7→8。6 次内存分类命令均返回 0，全部 AX 窗口数与进程树大小为 1。opened 21、closed 20，脚本返回 0，测试实例 SIGTERM 后 wait 返回 -15。

60 秒值落在初始值 +5 MB 内，按开始前约定停止，不再扩为 60 次。单点字号改动同时消除了纹理持续新增与大部分空闲残留增长，足以支持缓存键增殖的归因，不再增加 MallocStackLogging 运行。该结果不声称所有工作区文字、显示器缩放变化或长时间使用下缓存均恒定。

证据为 `run2-fixed/observation.json` 与同目录的 heap、footprint、vmmap 和 events.log。

## 主集成产物复验

主 checkout 的最终产物还包含独立完成的按键计时改动，默认未启用计时探针；因此先保留上述只改 HUD 的因果对照，再做集成验收。最终 SHA-256 为 `b46b04b3ae8136ab2f5efe5b8745dd4d2428e3edab4c17fb7dd26a1492ace12d`，49,461,504 bytes。与单独 HUD 修复的源文件哈希一致，实际运行前再次核对产物哈希。

PID 35977，20 次开关后 opened 21、closed 20，全部 AX 窗口数与进程树大小均为 1，6 次内存分类命令返回 0。

| 采样点 | 主集成产物 / MB |
| --- | ---: |
| 初始空闲 10 秒 | 58.394 |
| 第 20 次关闭后约 0.9 秒 | 175.244 |
| 停止操作后 10 秒 | 63.817 |
| 停止操作后 30 秒 | 59.623 |
| 停止操作后 60 秒 | 59.623 |
| 60 秒值相对初始增量 | +1.229 |

第 5/10/15/20 次短采样为 174.884/174.933/175.097/175.244 MB。纹理保持 3→3，TFPFont 92→105、CGFont 62→69；窗口、视图、layer、command queue 仍各为 1。10 至 60 秒 frame 数保持 2,516。脚本返回 0，SIGTERM 后 wait 返回 -15；确认实例退出后，将 GUI 交给延迟验收 session。

证据为 `run3-integrated/observation.json` 及分类文件。主协调已将原始和修复证据复制到主 checkout 的 `target/health-check/native-repairs-20261023/window-reclaim/`，对应 `before`、`hud-only`、`integrated`；本报告与原 worktree 数据共同保留具体采样来源。

## 限制

- 没有执行 4 小时回放。修复后的每个产物各做 20 次，原产物扩展到 60 次；不声称长时缓存恒定或所有资源问题已排除。
- 未验证屏幕像素层面的动画平滑度。实际路径有完整帧事件，原激活/动画测试通过，但不能代替逐帧视觉检查。
- 未做 MallocStackLogging：静态缓存键链、回归红绿及单变量资源对照已经解释本次主要增长，未继续扩大诊断。
- +5 MB 是事先约定的有界采样停止条件，不是新的性能预算。未更改系统设置或其他 session 的窗口。
