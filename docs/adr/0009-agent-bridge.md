# ADR 0009：Claude Code 和 Codex 改走 ZJ 自己的桥接，不再需要 Node.js

日期：2026-10-08。状态：试行（分支 `feat/agent-bridge`）。

## 背景

ADR 0004 里，Claude Code 和 Codex 通过 npm 适配器（`claude-agent-acp`、`codex-acp`）接入 ACP。适配器只有 TypeScript 版本，所以：

- 每种 Agent 常驻一个 Node 进程。2026-10-08 实测（同一台 M5 Pro、`claude` 2.1.285、`codex` 0.160.0，第一轮对话后空闲）：Claude 的适配器约 71 MB，Codex 的约 45 MB；
- 本机没有适配器时，ZJ 要自动安装：npm 包装进数据目录，没有 Node.js 22+ 就下载 Node.js 24（`provision.rs`，约 580 行）。用户机器上这部分占了约 290 MB（适配器）加 199 MB（Node.js）；
- 适配器本身也只是把 ACP 翻译成 CLI 的协议：Claude Code 的 `claude -p --input-format stream-json`（Agent SDK 用的同一套控制协议）和 Codex 的 `codex app-server`（官方 JSON-RPC，可以生成 JSON Schema）。

参考 Orbvane（github.com/sbaruwal/orbvane，MIT OR Apache-2.0）的做法：在编辑器内用 Rust 直接驱动两个 CLI。直接用它的代码不够：它一个进程只有一个会话，不支持图片、运行中追加指令、`session/close`、`/` 命令、按模型切换思考强度和登录引导，模型列表用的也是 ZJ 不读的旧字段。

用户要求：去掉 Node，保证现有用法；本机没有 CLI 时不要主动安装，切换到这个 Agent 时提示即可。

## 决定

### 1. 新增 `crates/agent_bridge`，由 ZJ 自己的可执行文件运行

包名 `workspace-editor-agent-bridge`，只依赖 `serde_json`（已在依赖树里），不依赖 GPUI。`workspace-editor --agent-bridge claude|codex <CLI 路径>` 在 `main` 里、GPUI 初始化之前就转进桥接，进程里不加载界面，空闲约 2–3 MB。`agent_client` 把它当成普通的 ACP Agent 启动，进程组、共用进程池、空闲关闭、崩溃恢复、权限、快照和审阅都沿用 ADR 0004，没有改动。开发和测试时用 `ZJ_AGENT_BRIDGE` 指向单独构建的 `zj-agent-bridge`。

协议形状对齐原来的两个适配器，界面、设置和历史不用迁移：

- 会话 id：Claude 是它的会话 UUID（`--session-id` / `--resume`），Codex 是 thread id；旧历史里的会话照常恢复；
- 模式 id：Claude 是 `default` / `acceptEdits` / `plan` / `auto`，Codex 是 `read-only` / `workspace-write` / `agent`，策略照搬 codex-acp。跳过审批的模式根本不提供；
- 模型设置 id：Claude 是 `model` / `effort` / `fast`，Codex 是 `model` / `reasoning_effort` / `fast-mode`；
- 引用写成 `[@名字](uri)`，选区放进 `<context ref=…>`，和适配器一样；
- 声明 `loadSession`、图片、`embeddedContext`、`sessionCapabilities.close` 和 `_meta.steering.supported`。

### 2. Claude Code：每个会话一个 `claude -p`

参数为 `--input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --permission-mode <模式>`，加上 `--session-id` 或 `--resume`。不传 `--allow-dangerously-skip-permissions`。

- 控制协议的 `initialize` 返回模型（含每个模型支持的思考强度和是否支持快速模式）和命令（技能、自定义命令），分别变成 `configOptions` 和 `available_commands_update`；
- 切换用控制请求：`set_permission_mode`、`set_model`、`apply_flag_settings`（`effortLevel` / `fastMode`）；
- 我们发出的每条用户消息都带 uuid，`result` 会标明它回答的是哪几条（`user_message_uuid(s)`，已用 CLI 实测）。所以一轮在所有消息都得到回答后才结束；
- 追加指令就是再写一条用户消息，优先级 `now`，有未答的权限请求时用 `later`。它会打断正在生成的那一段，被打断的那段先产生一个 `result`，这个 `result` 不结束本轮；
- `can_use_tool` 变成权限请求，Bash 命令的原文放在 `rawInput` 里，审批卡片照常显示命令。其余工具复用「本会话都允许」的规则；
- `ExitPlanMode` 问「按计划开始改动？」，选择决定之后的模式。`AskUserQuestion` 拒绝，并让模型直接在回复里提问；
- 编辑的 diff 是 ZJ 做改动前快照和审阅的依据，而 Claude Code 可能在 diff 送到前就已经写了文件。所以单处替换（`Edit`）直接发送这一处的「改前 → 改后」片段，ZJ 不论文件写没写都能还原改前的内容；其余情况（`Write`、`MultiEdit`、`replace_all`、删除文本）发送整个文件，改前内容在工具调用的流式输入结束时就读取。子 Agent 的编辑也会作为工具调用显示，进入审阅；它的其他步骤不显示；
- 某个会话的 `claude` 意外退出，只影响这一个会话：下一条消息用 `--resume` 重新启动。

### 3. Codex：所有会话共用一个 `codex app-server`

- `session/new` 对应 `thread/start`，`session/load` 对应 `thread/resume`（`excludeTurns`），`session/close` 对应 `thread/unsubscribe`；
- `session/prompt` 对应 `turn/start`，带上本会话的审批策略、沙箱、审查方和模型设置。快速模式用 `serviceTier: "fast"`；
- 追加指令用 `turn/steer`，取消用 `turn/interrupt`；
- 图片用 data URL 发送。工作区的技能（`skills/list`）作为 `/` 命令，消息开头是 `/技能名` 时按技能输入发送；
- `app-server` 意外退出时桥接也退出，由 ZJ 按崩溃处理：每个会话在下一条消息时恢复。

### 4. 不再安装任何东西

- 删除 `provision.rs`、`Launch::Package`、`LocalCli`、`ClientOptions::install_root` 和安装进度。内置预设改成 `Launch::Bridge`，只在本机查找 `claude` / `codex`（沿用 `SearchPath`，Homebrew、`~/.local/bin`、nvm、mise 等）。设置或环境里的 `CLAUDE_CODE_EXECUTABLE` / `CODEX_PATH` 仍然有效。
- 最低版本：Claude Code 2.1.0、Codex 0.159.0。找不到或版本太旧时返回 `LaunchError`，消息里带上安装命令（`brew install --cask claude-code` 或官方安装脚本；`brew install --cask codex` 或 `npm i -g @openai/codex`）。
- 在面板里选中一个 Agent 时（包括打开面板时的第一个会话），后台检查它能否启动：CLI 是否存在、版本是否够新、Key 是否设置。不能启动就在空会话里直接显示原因，不必等发出第一条消息。每次选中都重新检查，装好之后再选一次即可。
- 登录：Claude 提供「Claude 订阅」「Anthropic Console」，Codex 提供「ChatGPT」。都是终端登录方式，ZJ 在「终端」里运行 `workspace-editor --agent-bridge <类型> <CLI> --login …`，桥接再 `exec` CLI 自己的登录命令（`claude auth login --claudeai|--console`、`codex login`）。脚本只带 `PATH`。
- 数据目录里以前装的适配器和 Node.js（`~/Library/Application Support/ZJ/agents`）不再使用，ZJ 不自动删除，用户可以自己删掉。

## 依赖与资源

| 指标 | 之前 | 之后 |
| --- | ---: | ---: |
| normal 依赖树去重行数 | 614 | 615（新增的是 bridge crate 本身）|
| dist 二进制，字节 | 29,396,624 | 29,611,648（+215,024）|
| Claude Code 一个会话：适配器 / 桥接 + CLI | 71 MB + 115 MB | 4.3 MB + 115 MB |
| Claude Code 两个会话 | — | 3.0 MB + 144 MB + 118 MB |
| Codex 一个会话：适配器 / 桥接 + `codex` | 45 MB + 86 MB | 4.9 MB + 66–69 MB |
| Codex 两个会话 | — | 3.3 MB + 87 MB |

内存为 `footprint`，第一轮对话后空闲时采样。桥接一栏是 dist 版 `workspace-editor --agent-bridge` 的实测；用单独构建的 `zj-agent-bridge` 是 2.4–3.3 MB（两个会话的那两行就是用它测的）。CLI 本身的占用和以前一样，每种 Agent 省下的是整个 Node 适配器。二进制增加超过 200 KB，所以写这条记录；仍低于 30 MB 目标。ZJ 主进程的代码路径没有变化，没有另测空闲 footprint。

## 验证

- 单元测试：桥接 15 项（steering 和取消的回合结束判定、结果到停止原因、模型设置跟随模型、提示内容、工具调用和 diff、Codex 模式和输入）；`agent_client` 的启动解析改为测试找不到 CLI、版本太旧和用户指定路径；界面新增「选中未安装的 Agent 时直接提示」的回归。
- 真实 CLI 冒烟（`crates/agent_client/examples/agent_smoke.rs`，经过 ZJ 自己的 `AgentClient` 和进程池）：Claude Code 和 Codex 都通过了以下场景：回复、新建文件和修改已有文件（快照等于改前内容）、追加指令、取消、切换模型设置和模式后继续对话、同一进程上的第二个会话（第一个会话仍能继续）、进程重启后恢复会话、生成提交信息。图片两边都能送达；Codex 对 8×8 的纯红测试图回答过 Red / Orange / Peach，是模型对极小图片的判断波动，codex-acp 发送的也是同样的 data URL。

- 旧适配器建立的会话：从历史库里各取一个 claude-agent-acp 和 codex-acp 的会话 id，用桥接 `session/load` 都成功（只加载，没有发消息）。

## 代价与风险

- Claude Code 的 stream-json 控制协议没有完整的公开文档（Agent SDK 用的就是它）。CLI 大版本升级后要跑一遍冒烟。Codex 可以对照 `codex app-server generate-json-schema`。
- 维护方式从「跟着适配器版本走」变成「自己跟着两个 CLI 的协议走」。适配器里的一些附加功能没有做：上下文用量（面板本来就不显示）、自动生成会话标题、子 Agent 的过程展示、IDE 专用扩展。
- Codex 的取消在新旧两种方式下都要 40 多秒才收到 `turn/completed`，这是 Codex 本身的问题，不是这次的回退。
- 两种适配器原来都没有通过 ACP 的 `fs/read_text_file` 读取编辑器里未保存的内容，现在也一样，都是直接读磁盘。
- Codex 的 `/` 命令只有工作区的技能（实测 79 个），codex-acp 自己实现的内置命令（`/review`、`/compact`、`/init` 等，约 11 个）没有提供。
- Claude Code 的 `AskUserQuestion`（选择题）面板显示不了，会被拒绝，并让模型直接在回复里提问。
- 只在 `claude` 2.1.285 和 `codex` 0.160.0 上做过冒烟。更早但高于最低版本的 CLI 如果缺少 `user_message_uuid(s)`，追加指令会在被打断的那一段结束时就结束本轮（功能退化，不会卡住）。
