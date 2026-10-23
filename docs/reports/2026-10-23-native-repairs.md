# 原生交互与资源问题修复

日期：2026-10-23。用户授权继续修复快捷键、窗口内存增长与延迟，修复后重新验收。本文记录修复及最终集成证据；进行中的判断不视为已完成。

## 固定基线与分工

主工作区 HEAD 为 `c171f2ee0d47b1db3fe0a67b408b14df2ea7fcb7`，已包含未提交的审计与界面修复；不能只用该 HEAD 重建当前状态。修改前的 dirty 文件、差异及 dist 保存在 `target/health-check/native-repairs-20261023/`。基线二进制 49,461,488 bytes，SHA-256 `ca4f0b630618fb6cccf32df66bdae95e142e5c62a691b547438038bd6261a682`。

三个用户启动的独立会话负责快捷键、窗口回收、计时与延迟，主会话负责保留现有修改、协调 GUI 时段、整合、审阅和最终验收。真实交互与性能采样串行，避免抢焦点及编译干扰。

## 待解释的证据

1. 编辑器输入可用，但自动化发送 `⌘S` 等快捷键无预期结果，移焦或点击菜单有效；需核对 AX 焦点与 GPUI 焦点、原生输入分发，不预设绑定错误。
2. 20 次开关窗口后 AX 窗口数回到 1，footprint 从 172.4 MB 升至 194.9 MB；需核对采样等待、缓存及资源引用，不能直接定性泄漏。
3. 旧 `key_latency` 回调早于 draw/present，不能验收画面延迟。独立计时修正会话初测 native keyDown → Metal CPU 提交返回 200/200 样本，p50 10.039 ms、p99 38.689 ms，超 16.7 ms；终点不含 GPU 完成及屏幕扫描输出。

原始实机记录见[原生交互验收](2026-10-23-native-interaction-validation.md)。

## 测量前核对

- 旧 20 次脚本新建后只等 0.9 s、关闭后只等 0.9 s，末尾没有延长空闲；因此原增长不能单独证明泄漏。下一步先补关闭后 10／30／60 s 的观察。
- 三个独立 worktree 最初均为 clean `c171f2e`，不含主工作区的审计／UI 修改。实现与最终测试必须合并主 dirty 基线；计时修正的初次 38.689 ms 数据另标测量对象，不归属当前主产物。
- GUI 顺序：快捷键复现 → 窗口回收观察 → 延迟诊断；性能采样期间避免其他构建和 GUI 操作。

## 诊断进展

快捷键原测试把尚未提交的拼音组合文字当作编辑器内容。`defaults AppleCurrentKeyboardLayoutInputSourceID` 不能代表实时 IME 模式，Escape 会取消标记；原生验收报告已撤回「已排除输入法」结论。对原 `ca4f0b63…` 产物的复验读到实时源 `com.tencent.inputmethod.wetype.pinyin`：未提交时 ⌘S 不保存，Return 提交且 Escape 不再改变内容后，两种 ⌘S 注入、⌘P、⌘,、⌘W 全部通过，输入源前后一致，进程退出并回收。证据为快捷键工作树的 `target/shortcut-evidence/committed-original2/result.json`。临时进程内探针进一步确认 `focused=true`，上下文为 `[Root, WorkspaceEditor, DocumentEditor, Input]`；组合期间 Command 事件 `composing=true → IME handled=true`，提交后为 `composing=false → dispatch handled=true → document_saved`，AX 同时仍报告 AXWindow。探针记录位于同一工作树的 `target/shortcut-evidence/instrumented/app.log:101`。未发现需修改生产输入分发的缺陷，修正落在可复用验收 driver 与报告；临时探针不进入交付。

内存测试使用冻结的 `ca4f0b63…` 产物。初始空闲 10 s 为 58.640 MB；20／40／60 次开关后各空闲 60 s，footprint 分别为 78.874／96.978／110.921 MB。第 20 次关闭后 0.9 s 曾为 194.561 MB，说明大部分瞬时图形内存会回收，但仍有持续增长。GPUIWindow、GPUIView、CAMetalLayer 与 AGXCommandQueue 对象计数均保持 1；TFPFont 为 92→221→329→443，纹理为 3→11→18→26。后续沿字体缓存键追踪，确认窗口切换提示的连续小数字号进入 `RenderGlyphParams.font_size.to_bits()`，产生不断新增的缓存键；采用单变量 A/B 验证后，不再追加 MallocStackLogging。

独立计时工作树在无编译干扰下复测三次，每次 200／200 样本，CPU 提交 p99 为 27.617／30.264／28.333 ms，仍超 16.7 ms。阶段探针中一条 28.122 ms 慢样本有 24.660 ms 发生在帧回调之前，Metal draw 至 commit 约 0.232 ms；关闭低内存模式的对照 p99 为 29.069 ms，不支持将低内存模式单独认定为根因。采样后实时 TIS 输入源为微信拼音，因此这些结果可能包含组合输入绘制，不能证明英文文本已提交。以上仍是独立 clean HEAD 加计时补丁的诊断，最终集成产物尚待测量。

## 修复与验收结果

计时增量已合入主工作区，保留现有 CPU 1% 上限和其他未提交修改。新事件记录去重后的原生 keyDown 至同窗口下一次成功 Metal commit 返回；默认关闭，每窗口最多 256 个待测时间戳，不增加轮询或强制出帧。脚本严格核对样本数量与边界，并在每次发送按键前确认前台 PID。CPU 提交仅是画面延迟的一部分，完整显示预算仍保留。

计时第一阶段审阅发现 1 项 Nit：新加的输入超时异常会绕过既有「延迟未测」处理，使此前资源数据无法写成报告。已将其转换为 `RuntimeError` 并补模拟超时回归；窄复核 0 findings。实际执行 `python3 tools/test_measure_budget.py`：6 tests、OK；`python3 tools/test_sample_resources.py`：1 test、OK；`git diff --check` 退出码 0。

计时第二阶段 Spec 为 0 findings；Standards 提出 3 项 Nit，其中失焦被统一提示为权限错误成立，已纳入后续 ABC 测量脚本增量修正。另两项未采纳：抽取一行 p99 算法属于风格建议；将画面预算改为 CPU 代理会缩小原验收目标，现有正文已说明两者边界，保留原表述。

修改过的四个 vendor Rust 文件另经 `rustfmt --check --edition 2024 --config skip_children=true` 检查，退出码 0。

计时集成的完整检查退出码均为 0：`cargo test --workspace --locked` 为 638 passed、0 failed、4 ignored（20 个 suite；App 456 passed、3 ignored）；`cargo clippy --workspace --all-targets --locked -- -D warnings` 完成于 30.43 s；fmt、差异检查、dist 均通过。该中间 dist 为 49,461,504 bytes，SHA-256 `94ae79860840601d73bca0ec358d7a84906d3683ced40675970a0d82f92b7d97`。Git deadline 实测 timeout 1.011 s、parent_exits 1.003 s、cancel 1.015 s，PASS，无存活后代。原始日志位于 `target/health-check/native-repairs-20261023/`。

HUD 字号修复随后合入：实际渲染改用取整后的字号，使 92–100 px 放大过程只使用有限的 9 个字号，其他动画时长、透明度、布局缩放保持原规则。回归遍历 10,000 个动画时刻，检查实际渲染字号集合有界。独立工作树提取真实函数做红绿对照：原实现出现 1184 个字号，修正后通过；完整 Cargo 专项随后通过。该工作树的 Cargo 红测试因编译排队读取了已修源码，不计作红测试证据。

HUD 增量按顺序完成 ponytail-review（0 findings）、code-review Standards（0）与 Spec（0）；这些是源码审阅，不代替资源 A/B。快捷键验收脚本、3 项判据测试与诊断报告也已集成，主工作区再次执行 `python3 -I tools/test_check_native_shortcuts.py`：3 tests、OK。生产源码检索未发现临时 `DEBUG-shortcut`／`DEBUG-latency-stage` 探针。

HUD 集成后 `cargo test -p workspace-editor switch_hud --locked` 为 3 passed、0 failed、457 filtered out；fmt、全目标 Clippy、dist 与差异检查均退出码 0。最终产物为 49,461,504 bytes，SHA-256 `b46b04b3ae8136ab2f5efe5b8745dd4d2428e3edab4c17fb7dd26a1492ace12d`；本轮修改的 7 个 Rust 文件哈希已记录在 `final-rust-source.json`。验证顺序为完整计时基线 638 项通过，随后新增 HUD 回归及既有相关测试通过；最终状态未再次运行全工作区测试。

单变量资源 A/B 已通过：冻结原 dirty 基线仅加 HUD 修复的产物，初始 58.263 MB，20 次开关后空闲 60 s 为 59.180 MB（+0.917 MB）；原产物同阶段为 +20.234 MB。纹理数量 3→3，TFPFont 92→105、CGFont 62→69，窗口／视图／层／队列仍为 1。按预设「20 轮末尾回到初始 +5 MB 内」停止，没有追加 60 轮，也不据此宣称四小时稳定性。脚本退出码 0，应用 stop/wait 退出码 -15。

最终集成产物的窗口回收复验通过：初始 58.394 MB，20 次开关并空闲 60 s 后为 59.623 MB（+1.229 MB）。纹理 3→3、TFPFont 92→105、CGFont 62→69；最终 opened 21、closed 20、AX 窗口 1，进程树仅测试主进程。6 项分类命令退出码 0，测试应用已 stop/wait 退出（-15）。详情见[窗口回收调查](2026-10-23-window-reclamation.md)，三组证据副本在主工作区 `target/health-check/native-repairs-20261023/window-reclaim/{before,hud-only,integrated}/`。

ABC 测量脚本现会选择已启用的 ABC 输入源，严格采集 200 个输入样本后再单独保存，核对临时文件中的 ASCII 修改，最终恢复原输入源；失败也会恢复与退出。新增 helper 与脚本增量两阶段审阅均为 0 findings，主工作区执行 4 项输入源模拟测试、9 项测量测试均 OK。快捷键工具复用只读输入源 helper，去掉重复 ctypes，实现窄复核 0 findings、3 项判据测试 OK。

ABC 首次实际运行在输入前因 AppleScript guard 解析歧义报 -1700，未切换输入源或发送按键，实例已回收。已将 PID 表达式加明确括号并更新回归；9 项测量测试再次通过，表达式实际解析验证和两阶段窄复核均通过。该失败记录保留，不计为产品性能失败。

最终 `b46b04b3…` 产物通过主工作区正式 `measure_latency` 的三轮 ABC 输入验收：

| 轮次 | 样本／保存字符数 | CPU 提交 p50 | CPU 提交 p99 |
| --- | ---: | ---: | ---: |
| 1 | 200／200 | 3.211 ms | 8.743 ms |
| 2 | 200／200 | 3.196 ms | 8.328 ms |
| 3 | 200／200 | 3.343 ms | 8.983 ms |

三轮均低于 16.7 ms 上限，略高于 8.3 ms 目标；每轮开始、结束核对实际 ABC 输入源，原微信拼音已恢复。统计在后续保存按键之前冻结，不把保存事件混入 200 个输入样本。与旧 clean HEAD／IME 采样存在多个条件差异，不将数值变化直接归因于输入法或宣称帧调度优化；本轮没有修改帧调度策略。CPU 提交验收不含 GPU 完成和屏幕 scanout。

最后在同一最终产物上执行 `tools/check_native_shortcuts.py`，退出码 0：微信拼音文本提交后两种 ⌘S、⌘P、⌘,、⌘W 全通过，原源前后一致；测试 PID 38019 已 stop/wait 退出（-15）。结果位于 `shortcut-final/result.json`；延迟原始数据与摘要位于 `latency-evidence/integrated-abc-*`，详情见[CPU 提交计时与 ABC 编辑验收](2026-10-23-key-latency.md)。

收尾再次执行输入源 4 项、测量 9 项、快捷键判据 3 项、资源换算 1 项 Python 回归，合计 17 项全部 OK；`git diff --check` 通过。`pgrep -fl workspace-editor` 返回 1、无输出，确认无测试应用遗留；组合检查命令因此返回 1，不是测试失败。最终二进制 SHA 与所有集成验收记录一致。

已完成本轮内存修复、计时及验收脚本修正与有界原生复验。未完成四小时回放、完整屏幕延迟、所有 IME／VoiceOver 专项，不能据此宣称 G1 全部通过。没有提交或推送。
