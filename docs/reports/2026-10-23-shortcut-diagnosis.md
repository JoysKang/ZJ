# 原生快捷键与 IME 输入状态诊断（2026-10-23）

结论：原验收把 IME 预编辑文本误当成已提交文本。真实 GPUI 编辑器焦点及上下文正常；组合期间 Command-S 被输入法消费，提交后进入 GPUI 保存动作。原 dist 与临时探针构建的四种快捷键对照均通过。本轮修正验收脚本与证据，没有修改产品键绑定或原生分发。

## 基线与隔离

本报告中的 `target/shortcut-evidence/` 指独立快捷键工作树的原证据目录。集成时已将两轮结果、日志与补丁副本保存到主工作区 `target/health-check/native-repairs-20261023/shortcut-evidence/`。

工作树基线为 `c171f2ee0d47b1db3fe0a67b408b14df2ea7fcb7`，随后应用主工作区保存的 `target/health-check/native-repairs-20261023/before.patch`，保留既有 dirty 改动。初始复现使用原产物，SHA-256 为 `ca4f0b630618fb6cccf32df66bdae95e142e5c62a691b547438038bd6261a682`。未提交、未推送。

与主会话、内存测量会话、延迟测量会话协调 GUI 使用。每次仅启动自己的临时工作区，隔离设置、会话、恢复与 Agent 历史。每次发送按键前检查前台 PID；一次丢焦后立即停止并退出测试进程。未切换系统输入源。

## 已确认的原始证据缺口

- `ime_escape.py` 读取 `defaults read com.apple.HIToolbox AppleCurrentKeyboardLayoutInputSourceID`。这是键盘布局信息，不能证明当时的实时输入法模式；新脚本改用 `TISCopyCurrentKeyboardInputSource`。
- 原日志期望 `IMECASE1`，AX 文本实际只有 `IMECASE`。没有先确认完整标记进入缓冲区，就用磁盘缺少完整标记判定保存失败，判据不成立。
- AX 文本值包含预编辑文本。`replace_and_mark_text_in_range` 在组合期间就更新 Kit 文本及显示，AX 出现标记不等于已经提交。
- `AXFocusedUIElement` 属于 application。改正查询层级后仍得到 `AXWindow`，但这不能证明 GPUI 编辑器失焦。

## 焦点与原生事件边界

Kit component 把 AXTextArea 的 role/value 放在输入框外框，真实输入焦点则属于内部 `InputBaseState`。内部节点设置 `track_focus`，却没有 AX role；GPUI 的辅助功能树无法把这个内部焦点节点作为可访问焦点，因而回退到窗口。应以进程内 `FocusHandle::is_focused` 和 `Window::context_stack` 验证实际焦点，而不是要求 AXFocusedUIElement 必须为 AXTextArea。

`vendor/gpui-pre-macos/src/window.rs` 的 `handle_key_event` 在存在 marked text 时先交给 `inputContext.handleEvent`，包括 Command 组合键。输入系统返回已处理且没有调用 `doCommandBySelector` 时，事件不会再进入 GPUI callback。没有组合文本时，Command 组合键先进入 GPUI 分发。这是上游已有行为，当前没有依据修改它。

无头 `window.press` 直接进入 GPUI 分发，绕过 AppKit 的原生输入上下文，不能单独覆盖本次现象。

## 初始最小复现

`target/shortcut-evidence/baseline3/result.json`：文本标记进入 AX，物理 key code 1 和 System Events keystroke 两种 Command-S 均未把标记写盘；菜单保存写入标记。application 级焦点角色为 AXWindow。此时尚未证明标记已提交，因而只能确认现象，不能把它归因为键绑定故障。

加入 Escape 的另一次对照中，应用日志出现三次 `event=document_saved bytes=39`，保存的是夹具原始内容。这个结果与 Escape 取消预编辑文本相容；不能用 Escape 后磁盘没有标记来排除 IME。

## 回归入口

`tools/check_native_shortcuts.py` 使用新建临时夹具，明确关闭 `auto_save` 和自动输入源切换。Return 提交后，再验证 Escape 不改变 AX 文本，才开始保存断言。它记录实时输入源及 application 级 AX 焦点；焦点角色仅用于诊断。

运行前必须协调 GUI 使用。脚本会激活自己的测试窗口；如果前台 PID 改变，立即失败并清理自己的进程。证据目录必须不存在。

```bash
python3 -I tools/check_native_shortcuts.py /absolute/path/to/workspace-editor /absolute/path/to/new-evidence-directory
```

轻量判据测试：

```bash
python3 -I tools/test_check_native_shortcuts.py
```

已运行：3 tests，OK。

## 原 dist 提交输入后的对照

命令：

```bash
python3 -I tools/check_native_shortcuts.py /Users/joys/work/code/ZJ/target/dist/workspace-editor target/shortcut-evidence/committed-original2
```

退出码为 0；应用测试进程 PID 87022 已由脚本终止并等待退出（`exit_code: -15`）。`result.json` 的结果如下：

| 判据 | 实测 |
| --- | --- |
| 实时输入源，前后均相同 | `com.tencent.inputmethod.wetype.pinyin` |
| application AX 焦点 | `AXWindow` |
| 提交前 Command-S 写入标记 | false |
| Return 后 Escape 保留全部文本 | true |
| 已提交文本，物理 key code 1 + Command 保存 | true，磁盘全文等于 AX 文本 |
| 再次编辑并提交，keystroke Command-S 保存 | true，磁盘全文等于 AX 文本 |
| Command-P 打开快速搜索 | true |
| Command-, 打开隔离的设置文档 | true，内容等于夹具设置 |
| Command-W 关闭设置并回到文档 | true |

第一次对照的 Command-, 已触发 `event=document_opened`，但设置全文断言失败：夹具漏了 `hidden_files`，ZJ 按既有迁移规则向缓冲区补入默认规则。补成显式空数组后全部通过。这个失败来自夹具，不是快捷键故障。

当前结论：原报告的「US 输入源」与「Escape 排除 IME」证据不成立。至少在本次实时微信拼音下，未提交输入的现象可复现，明确提交后所有被报告的快捷键均正常。修正应首先落在验收脚本和报告，不应据此改产品绑定或绕过 IME。进程内证据见下一节。

## 进程内焦点与事件链

在导入的 dirty 基线上，临时加入三个边界的观测点：Kit 输入 state 的 marked 查询、macOS 原生按键及 IME 返回、GPUI 按键截获和 dispatch 返回。只记录焦点、上下文及状态，不记录文档文本或按键正文。临时补丁保存在 `target/shortcut-evidence/temporary-probe.patch`，验证后已完整反向撤销。

`cargo build --profile dist --locked` 退出码 0。临时构建 SHA-256 为 `f1e26ea36b9cf6074ac86ad3e3abd49a132c2a39893fc3ee6f0e1b7b5018fceb`，仅供诊断，不作为交付二进制。

```bash
ZJ_SHORTCUT_PROBE=1 ZJ_LATENCY_LOG=1 python3 -I tools/check_native_shortcuts.py target/dist/workspace-editor target/shortcut-evidence/instrumented
```

命令退出码 0，四种快捷键全部通过。PID 16655 已终止并等待退出。`instrumented/app.log` 的关键证据：

- 第 7 行：`input focused=true marked=false contexts=[Root, WorkspaceEditor, DocumentEditor, Input]`。真实编辑器焦点和上下文齐全，同次 AX 查询却为 `AXWindow`，证实这是 AX provider 与内部焦点节点的差别。
- 第 101–105 行：输入已变为 `marked=true`；Command key-equivalent 到达原生层时 `cmd=true composing=true ime=true`，输入法返回 `handled=true`。该次 Command-S 没有执行 GPUI KeyDown callback，磁盘标记仍为 false。日志中其他 GPUI 截获事件包含 KeyUp 等，不能只凭出现截获日志认定 KeyDown 已分发。
- 第 108–113 行：Return 经过 IME 后，marked 状态从 true 变为 false。
- 第 124–129 行：下一次 Command-S 为 `composing=false`，GPUI dispatch 返回 `handled=true`，接着出现 `event=document_saved bytes=52`；磁盘全文与提交后的 AX 文本一致。

探针同时开启了旧延迟日志，但本轮有诊断开销和并行构建，所有延迟数值均不用于性能结论。

## 审查与检查

按顺序完成 `ponytail-review`、`code-review`。

- 质量审查：增加 AX 有界稳定读取，校验输入源前后一致，将通过标记延后到进程退出检查之后。稳定读取只解决时序问题，不等于原生 marked 状态的证明。
- Standards：合并两处 Return／Escape 协议为 `Driver.commit_text`。不强制某一种输入法；证据明确记录本轮实际输入源，不把其他输入源的通过宣称为微信拼音覆盖。
- Spec：额外 Return 分支使用同一提交校验；Command-P 前先确认没有搜索输入框，再验证打开和关闭。固定 AX 结构是当前回归接口，结构改变会显式失败。
- `python3 -I tools/test_check_native_shortcuts.py`：3 tests，OK。
- `python3 -I -m py_compile tools/check_native_shortcuts.py tools/test_check_native_shortcuts.py`：退出码 0。
- `python3 tools/check_git_deadlines.py`：timeout、exited parent、cancellation 均 PASS，无存活后代进程。

- 清理后 `cargo fmt --all --check`、`git diff --check`：退出码 0。
- 原生回归在原 dist 与临时探针构建上各完整通过一次；最新脚本含审查后的提交路径和 Command-P 前置检查，已由探针构建运行覆盖。

未在本工作树重新运行 Rust 全量 clippy／workspace tests：最终没有 Rust 变更，导入的主 dirty 基线由主会话执行集成检查。本轮没有验证其他输入法模式、候选窗交互或 VoiceOver；不会将微信拼音下的结果扩展为全输入法兼容性结论。

最终交付仅新增验收脚本、判据测试和本报告，不包含临时原生探针，也不包含主工作区已有 dirty 改动。
