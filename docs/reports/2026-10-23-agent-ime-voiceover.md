# Agent 忙碌态、IME 与 VoiceOver 验收（2026-10-23）

用户要求继续解决 Agent 输入定位并完成忙碌态、IME／VoiceOver 验收。本轮在临时工作区中执行，不调用外部模型、不读凭据，不提交或推送。

## Agent：已完成的核心链路

旧产物 `9da85a2d…` 上，反馈脚本等待文档与 AX 界面稳定，⌘L 聚焦输入框，然后在实际 ABC 输入源下输入。首轮即成功发送 `steerable`，追加后日志出现 `steering=true`。此前定位／提交失败没有复现，不能确认为生产输入逻辑缺陷。当前证据支持改进验收前置条件，但未做单变量对照，因此不把所有旧失败归于某一个确定原因。

产品只增加两处可访问名称：发送按钮在空闲时为「发送」、忙碌时为「追加指令」，停止按钮为「停止」。未改变布局、颜色、动作或协议。终端和 Agent 两处出现同类遗漏，已在 CLAUDE.md 增加图标按钮可访问名称规则。

最终产物 SHA-256 为 `9cc970ecb304785c104cfdca5eef866a6454c92bf0e846a6442e62aa91fbd975`，49,461,504 bytes，与改前相同。`agent-review-final/result.json` 及日志确认：

| 场景 | 结果 |
| --- | --- |
| 首条消息发送 | `agent_prompt turn=1 steering=false` |
| 忙时追加 | 同轮 `steering=true`，AX 名称为「追加指令」 |
| 停止 | 按名称点击「停止」，`outcome=cancelled` |
| 审批 | 实际生成「允许一次」按钮，点击后记录 `choice=once` 并结束回合 |
| 修改文件 | 本地 demo 通过 ACP 写临时 `sample.rs`，磁盘为预期新内容 |
| 待审入口 | 回合结束后「审阅」按钮仍存在，可按名称打开 |
| 拒绝修改 | 点击「拒绝此文件」，磁盘精确恢复原内容 |

上述是实际原生窗口与本地 ACP stub 的集成验收，不等于真实提供商端到端验证。此前空闲 AX 树没有审批／审阅按钮，不能推断动态卡片不可访问；本轮已用实际卡片反证该推断。运行中「执行中」与结束后「执行过程」按钮也出现在快照中，但折叠内容、滚动跟随及视觉布局未据此宣称全部通过。

有一次回合结束时 AX 窗口读取得到 -1719，驱动增加有界窗口就绪等待后完成。保留该次失败，不将其当作应用崩溃。

## IME：微信拼音扩展场景通过

冻结 `b46b04b3…` 产物的原生编辑器中执行，使用当前微信拼音，不更改系统默认输入法：

- 全选原文后输入 `nihao`，AX 可见组合文字 `ni'hao`；选择候选后精确替换为「你好」。
- Escape 保留已提交文字；撤销恢复原文，重做恢复「你好」。
- 再开始组合输入并按 Escape，只取消组合，不改变已提交文字。
- 粘贴两行中英文，全文精确一致；撤销粘贴恢复此前中文。
- 磁盘原文件未改变，剪贴板原有内容按原始记录保存并恢复；测试前后输入源都是微信拼音。

证据为 `ime-expanded/result.json`，`passed=true`，测试进程退出码 -15。未覆盖其他输入法、候选翻页、候选窗几何和所有 Unicode 组合场景。

## VoiceOver：按用户要求取消

用户随后明确不需要 VoiceOver（2026-10-23），已删除专项验收脚本，不再将读屏验收列为待完成项。产品没有新增 VoiceOver 启动或控制逻辑；通用 AX 名称继续用于原生自动化定位。以下仅保留已发生的测试记录。

开始时 VoiceOver 关闭。本轮启动后处理其首次欢迎面板，选择「使用旁白」；确认 VoiceOver PID 51980 存在。在隔离 ZJ 窗口中打开终端，发送 VO-Shift-F4 与 VO-Right 导航键。应用没有退出，磁盘原文未改变。

无法建立朗读／游标断言：读取 `content of last phrase` 返回 AppleScript -1728，AXVisualSupportAgent 未提供可读窗口内容，普通 AX 焦点仍为 AXWindow。这些不能证明 VoiceOver 导航或朗读成功，也不足以定位为 ZJ 的读屏缺陷。没有更改 VoiceOver 安全设置或录音／屏幕权限来绕过限制。

验收后用 ⌘F5 关闭 VoiceOver，`pgrep -x VoiceOver` 返回 1，恢复原先关闭状态。证据在 `voiceover/result.json`。该记录不代表读屏验收通过；此项已按用户要求取消，不再安排后续验证。

## 验证与清理

- 独立窄审：两处可访问名称 0 findings。
- `cargo fmt --all --check`、全目标 Clippy、dist 构建均退出码 0。
- `cargo test -p workspace-editor steering_ --locked`：3 passed、0 failed。
- `cargo test -p workspace-editor a_permission_waits --locked`：1 passed、0 failed。
- `cargo test -p workspace-editor rejecting_a_hunk --locked`：1 passed、0 failed。
- `block 0.1.6` 未来兼容性警告仍存在。

所有证据位于 `target/health-check/daytime-followup-20261023/`。最终 Agent 测试 PID 53463 退出码 -15，输入源恢复微信拼音。四小时回放没有启动；本轮不重复资源与按键延迟测量。
