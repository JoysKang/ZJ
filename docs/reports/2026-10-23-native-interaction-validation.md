# 真实原生交互验收（2026-10-23）

本轮在用户授权辅助功能与 System Events 自动化后，对真实 macOS 窗口做有界交互验收。产物是 `target/dist/workspace-editor`，49,461,488 bytes，mtime `2026-10-23 08:54:40`，SHA-256 `ca4f0b630618fb6cccf32df66bdae95e142e5c62a691b547438038bd6261a682`（记录于 `logs/binary.txt`）。未编译、未改生产代码。

所有应用实例使用隔离数据目录与临时工作区：

```bash
ZJ_SETTINGS=…/native-interaction-20261023/data/settings.json
ZJ_SESSION=…/data/session.json ZJ_AGENT_DB=…/data/agent.db
工作区：…/native-interaction-20261023/fixture/ws（sample.rs 6795 bytes、README.md）
```

每步操作前核对 frontmost 是本进程 PID；没有向其他应用发送按键。结束时 `pgrep -fl workspace-editor` 为空。所有原始记录在 `target/health-check/native-interaction-20261023/logs/`，脚本在 `/tmp/zj-native-harness/`。

权限状态（只读查询）：`AXIsProcessTrusted` = `true`；`AEDeterminePermissionToAutomateTarget(com.apple.systemevents)` = `0`（允许）。屏幕录制未授权（`CGPreflightScreenCaptureAccess` = `false`），`screencapture -l` 返回 `could not create image from window`，**没有整桌面截图**，也未申请该权限。

## 结论

后续对同一产物补做实时输入源与文本提交对照，确认原快捷键异常来自未提交的微信拼音组合文字；提交后的 ⌘S、⌘P、⌘,、⌘W 全部通过。下面保留初测记录及其限制，修复与复验进展见[原生问题修复](2026-10-23-native-repairs.md)。

**passed**

1. 窗口激活与 frontmost 核对：`set frontmost` 后读取的 frontmost unix id 与目标 PID 一致；AX 前端窗口标题为 `ZJ · ws`，位置尺寸 `60, 60, 1280, 800`。
2. 按键确实到达应用并进入编辑器：System Events 注入的标记出现在 AX `text area 1` 的 `value` 中（例如 `aXMARK//! …`、`SAVEAZZ`）；`event=key_latency` 在短交互中出现 4–9 条。
3. 文件 → 保存：菜单点击后磁盘从 6795 bytes 变为 6801 bytes，且包含注入标记（`disk_has_marker_after_menu: true`）。
4. 关闭确认框中「不保存」：点击后窗口从 2 个（含确认层）回到 1 个，磁盘未写入标记。
5. 菜单「退出 ZJ」（未以 ⌘Q 键盘触发）：进程退出码 0；`session.json` 写入窗口 root、frame、两个标签与 active；随后空参数启动恢复 1 个窗口并打开 active 标签（`event=document_opened` 1 次），另一个标签按设计保持延迟（`pending_tabs`）。
6. 20 次「新建窗口 → 关闭」：每轮菜单与关闭按钮返回码均为 0，`event=window_opened` 21 次、`event=window_closed` 20 次，AX 窗口数每轮回到 1，进程树始终只有主进程。没有丢窗口、没有残留进程。

**初测异常（后续复核中）**

1. 编辑器获得焦点时，`⌘S` 不保存：`keystroke "s" using command down` 与物理键 `key code 1 using command down` 都不写盘；焦点移出编辑器后 `⌘S` 成功。原先根据 `defaults` 返回的 `com.apple.keylayout.US` 与 Escape 对照宣称排除了输入法，但后续复核发现该值不能代表实时 IME 模式，Escape 还会取消输入标记；这一排除结论撤回（原记录 `logs/ime-escape.json`）。AX 文本可能包含尚未提交的组合文字，需以实时输入源与提交后对照重新判定。同一现象出现在 `⌘,`、`⌘P`（编辑器聚焦时无可见变化）以及 `⌘W`（聚焦时无反应，移出焦点后才出现关闭确认）。已按要求停止追根，不归咎于绑定定义本身。
**未完成的验收步骤（不判定产品缺陷）**

1. 用键盘驱动「保存 / 取消」两个确认分支未成功：`sheet 1 of window 1` 在部分轮次不存在（`-1719 无效的索引`），只有「不保存」一轮定位到并可点击。记为未完成，不推断保存/取消的实现有问题。
2. 200 键的按键延迟采集为 0 条 `event=key_latency`。后续短交互能采到 4–9 条，说明部分事件可记录，但缺失原因尚未确认；不能直接断定注入方式与采集窗口不匹配。**该指标只代表「按键到下一帧回调」，不含本帧 draw/present，更不含 scan-out**；本报告不用它判断 8.3 / 16.7 ms 的按键到画面预算。

**not tested / blocked**

- 屏幕真实呈现延迟（需要 instrumentation 或屏幕录制）：blocked，未申请屏幕录制权限。
- 中文输入法组合输入（候选窗、marked text、提交后的文本）：not tested。只做了只读判断——系统启用 ABC 与拼音，但按指示未切换用户输入源。
- VoiceOver：not tested，未启动，未改系统设置。
- 真实 busy Agent UI、终端输入、系统保存面板、Finder 打开：not in scope。

## 20 次开关的 footprint 观察

| 轮次 | 新建/关闭事件数 | AX 窗口 | footprint |
| ---: | --- | ---: | ---: |
| 1 | 2 / 1 | 1 | 172.4 MB |
| 5 | 6 / 5 | 1 | 179.8 MB |
| 10 | 11 / 10 | 1 | 184.9 MB |
| 15 | 16 / 15 | 1 | 189.9 MB |
| 20 | 21 / 20 | 1 | 194.9 MB |

footprint 在 20 轮内单调上升 172.4 → 194.9 MB（+22.4 MB，约 1.1 MB/轮），进程树与 AX 窗口数保持 1。本轮只记录观测，没有定位增长来源，也没有做更长时间的回收判定（开发说明要求 20 次开关与 4 小时回放，后者未做）。

## 复现要点

```bash
# 所有交互脚本都只对本进程 PID 的窗口操作，开始时读取并记录 frontmost
osascript -e 'tell application "System Events" to get unix id of first application process whose frontmost is true'
osascript -e 'tell application "System Events" to tell (first process whose unix id is <PID>) to keystroke "s" using command down'
osascript -e 'tell application "System Events" to tell (first process whose unix id is <PID>) to click menu item "保存" of menu 1 of menu bar item "文件" of menu bar 1'
```

菜单栏实际名称是 `workspace-editor`、`文件`、`查看`、`转到`；应用菜单项为「设置…」「服务」「退出 ZJ」，没有名为 "ZJ" 的菜单栏项。

## 证据索引

- `logs/latency.json`、`logs/latency.log`：200 键采集（0 样本）与焦点核对
- `logs/diag-keys.json`、`logs/diag-click.json`、`logs/diag-nav.json`、`logs/diag-axfocus.json`、`logs/diag-save.json`、`logs/diag-save2.json`：按键进入缓冲区与保存路径诊断
- `logs/keycode-compare.json`、`logs/ime-escape.json`：⌘S 的 key code / keystroke 与 Escape 对照；原输入法排除结论已撤回
- `logs/close-flow.json`、`logs/close-flow2.json`：⌘W 与关闭确认分支
- `logs/session-quit-restore*.json`：⌘Q、session 写入与重启恢复
- `logs/reclaim20.json`：20 次开关与 footprint
- `logs/fixture-sha.txt`、`logs/binary.txt`：夹具与产物身份

未修改生产代码、设置或既有报告。
