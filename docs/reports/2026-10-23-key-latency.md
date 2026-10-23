# 按键 CPU 提交计时与 ABC 编辑验收（2026-10-23）

## 结论与边界

计时终点已从 GPUI 绘制前回调改为同窗口 Metal `commit()` 返回。起点为原生 keyDown 去重后、IME／GPUI 分发前。该指标只覆盖 CPU 提交，不包含输入到达前的硬件／系统延迟，不证明 GPU 完成、合成器呈现或屏幕 scanout。Diff 的双回调 `frame_upper` 仍是包含额外帧间隔的上界，不参与精确按键统计。

本轮没有修改帧调度、刷新率或 drawable 策略，没有实施性能修复。修正的是测量边界和编辑验收的输入条件。ABC 编辑验收的 CPU 提交 p99 均低于 16.7 ms 上限，略高于 8.3 ms 目标；完整按键到画面延迟仍未测量。

## 主集成产物验收

二进制：主项目 `target/dist/workspace-editor`，49,461,504 字节，SHA256 `b46b04b3ae8136ab2f5efe5b8745dd4d2428e3edab4c17fb7dd26a1492ace12d`。运行前、各轮之间及结束后核对同一 SHA。使用主目录已合入的 `tools/measure_budget.py::measure_latency`，不是独立工作树的旧工具。

| 轮次 | 输入样本 | p50 ms | p99 ms | 临时文件保存验证 |
| --- | --- | --- | --- | --- |
| 1 | 200／200 | 3.211 | 8.743 | 200 个 ASCII 字符 |
| 2 | 200／200 | 3.1955 | 8.328 | 200 个 ASCII 字符 |
| 3 | 200／200 | 3.3435 | 8.983 | 200 个 ASCII 字符 |

三轮均使用已启用的 `com.apple.keylayout.ABC`，切换后、输入后和保存后读取真实 TIS 状态；原源均为 `com.tencent.inputmethod.wetype.pinyin`，每轮结束实际核对已恢复。仅操作临时合成 Rust 文件，设置／会话／恢复／历史库隔离；所有测试实例均已停止并回收，GUI 时段已释放。输入源恢复失败会显式报错。

输入每键和保存之前均检查目标 PID 在前台，激活等待后也先检查再切源。输入统计先冻结为 200 个完整样本，之后的 ⌘S 只验证内容已保存，不计入输入延迟。原始日志保留保存键产生的额外事件，不能直接把整份原始日志当作输入样本集。未丢弃首批输入、预热或慢样本。

## 诊断证据

独立 clean HEAD `c171f2ee0d47b1db3fe0a67b408b14df2ea7fcb7` 加计时修正的首次 p99 为 38.689 ms；随后三次完整 200 键复测为 27.617／30.264／28.333 ms。临时探针为 27.117 ms，关闭低内存策略对照为 29.069 ms。这些运行的输入源未逐轮固定，采样后实时查询为微信拼音，不能视为已提交的英文文档编辑。

探针中最慢样本 28.122 ms，帧回调之前已耗时 24.660 ms；GPUI 绘制随后耗时 3.230 ms，drawable 获取约 0.052 ms，编码约 0.151 ms，commit 约 0.029 ms。现有边界不能把帧回调前的等待继续拆分为 IME 处理、等待显示 tick 或主队列排队，因此未据此改动调度策略。关闭低内存策略仍超限，不支持其为唯一根因。

主集成版本与独立版本同时存在代码和输入条件差异，不能把两个场景的数值差异全部归因于输入法或称作性能优化收益。

## 工具修正与回归

正式新增 `tools/native_input_source.py`：标准库 ctypes 调用 TIS，只选择已有 ABC，保留原源句柄，finally 恢复与校验并释放句柄。`measure_budget.py` 复用该辅助模块，保留主会话原有 CPU 统计、逐键焦点检查和超时保护；没有新增模式或第二套采样器。错误提示保留真实原因，失焦不再统一归因权限。

真实首轮曾在激活后的 AppleScript PID 检查中因解析歧义报错 `-1700`：原表达式将 `true is not PID` 纳入条件解析。该轮在切源和输入之前中止，实例清理完成；不是产品延迟失败，也不进入结果。修正为 `(unix id of (first application process whose frontmost is true)) is not PID`，只读原生表达式验证通过，随后三轮真实验收通过。持久回归固定带括号的表达式，并验证激活失焦不会进入切源上下文。

Python 持久测试 13 项通过，包含源恢复／失败、缺少 ABC、焦点丢失、超时、样本边界与保存分段。Python 增量按 ponytail-review → code-review Standards／Spec 顺序审阅，均无剩余问题；末次 PID 括号修复另做定向复核。独立 clean 基线的计时 Rust 修正先前通过 606 项测试（4 项忽略）；主 dirty 集成后全工作区为 638 passed、0 failed、4 ignored，随后 HUD 专项 3 项、Clippy、格式与最终 dist 通过，完整检查证据见[原生修复记录](2026-10-23-native-repairs.md)。Python 增量未再触发无关 Rust 测试。

临时 Rust 阶段探针已归档并从源码移除，恢复到此前验证过的 measurement-only 快照。未提交或推送，没有覆盖主产物。

## 证据位置

独立计时工作树的原证据目录为 `target/latency-evidence/`；主工作区已保存关键结果、原始日志、阶段分析和失败说明副本于 `target/health-check/native-repairs-20261023/latency-evidence/`。原目录包括：`integrated-abc-results.json`、`integrated-abc-{1,2,3}.log`、`stage-analysis.json`、`baseline-results.json`、`probe-results.json`、`upstream-results.json`、`measurement-fix.patch` 与 `diagnostic-probe-only.patch`。四文件 ABC 增量起点为主测量脚本 SHA256 `50bdf41e94db06323173704f6e46fe803c0fc404e8ef1d9fa84545795b1d8b9b`，已由主会话合入；PID 括号修复随后同步。
