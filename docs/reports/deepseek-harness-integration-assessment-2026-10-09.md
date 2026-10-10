# DeepSeek Harness 的原生接入评估

检查日期：2026-10-09。对象为官方仓库 `deepseek-ai/deepseek-harness`，依据预发布 tag **`dsh-v0.2.1-alpha.2`** 的文档、配置和协议源码。评估目标是保留 DeepSeek 原生 Harness，判断它能否作为 ZJ 的外部 Agent 接入，以及是否符合简洁、低内存的要求。

本次没有安装、登录或运行候选，没有修改 ZJ 源码。功能与接入结论来自发布版资料；没有本机内存、任务成功率或长时间稳定性的实测数据。

## 结论

**官方 DeepSeek Harness 已提供原生 ACP 入口，适合成为独立候选；目前不宜把它视为功能完整、低内存已经得到验证的 ZJ Agent。** 基础新建会话、消息、工具进度、取消和一次性审批有协议基础，但运行中追加、逐字流式输出、历史回放和结构化文件审阅仍有具体缺口。

建议保留「原生引擎负责模型、工具、权限和上下文，ZJ 负责编辑器与通用客户端」的边界。DeepSeek 模型经过 JCode、Pi 或 OpenAI-compatible 路由，并不等于使用 DeepSeek 官方 Harness；是否更换引擎应由用户选择。

对本次分工的建议是：**Codex、Claude 保留现有原生接入；DeepSeek Harness 通过原生 ACP 作为预览候选；其他模型优先验证 JCode，Pi 暂作备选。** 第一阶段只维护一个通用候选，避免同时增加两套功能与生命周期回归矩阵。原有自定义 ACP 入口继续保留。

## ZJ 的最小改动面

ZJ 当前的 DeepSeek 预设是 `claude-code-deepseek`：实际仍由 Claude Code 执行，只把模型路由切到 DeepSeek。新增官方 Harness 应使用独立 Agent ID；旧会话、账号配置和恢复 ID 继续属于旧引擎，不能自动迁移成 DSH 会话。

推荐路径是 `ZJ 通用 ACP 客户端 → dsh --profile acp → DeepSeek Harness`。现有 `Launch::Binary` 和 `UserAgentConfig` 已能表达该启动方式，第一轮探针甚至可以先用自定义配置。正式入口只需增加预设、安装/版本提示和凭据配置指引，不必先给 `agent_bridge` 新建第三种协议桥。安装后的 `dsh` 更适合作为固定入口，避免每次连接临时下载或随 `npx` 的 latest 发生变化。

| 改动 | 当前代码依据 | 处理建议 |
| --- | --- | --- |
| 预设与配置 | `agent_client/src/registry.rs` 已有 Binary / Bridge，DeepSeek 仍走 Claude | 增加独立 Harness 预设，复用环境变量和凭据解析；不接管 DSH 的插件配置 |
| 恢复协议 | `host.rs::init_from` 只记录 `load_session`；`client.rs` 只发 `session/load` | 增加通用 `sessionCapabilities.resume` 协商与 `session/resume` 路径；保留现有 load 行为 |
| 恢复后的显示 | `workbench/agent/history.rs::agent_restore` 已从 SQLite 加载界面历史 | DSH 恢复内部上下文，ZJ 恢复自己的显示记录；不要求 DSH 重放，也不把界面历史重新拼进 prompt |
| 审批详情 | DSH 审批请求只带 `toolCallId`，详情在先前工具事件中；ZJ 当前不会合并这些字段 | 按会话与 tool ID 关联工具标题、输入和审批请求，控制缓存大小并及时释放；完整显示参数后再让用户判断 |
| 能力呈现 | ZJ 已依赖握手开启图片与 steering；DSH 没有对应 commands/plans/elicitation | 按实际协议与验证结果呈现功能；不能把排队或取消后重发伪装成运行中 steering |
| 进程管理 | `AgentPool` 已复用同一启动配置的进程，默认 10 分钟空闲关闭，退出回收自有进程组 | 验证一个 DSH 进程下多个独立会话，关闭一个不影响其他会话；沿用 ZJ 自有进程管理 |

恢复是实际接入的必要改动：原样连接 DSH 时，ZJ 会因没有 `load_session` 能力而跳过恢复并新建会话，虽然界面上仍显示 SQLite 的旧消息。需要验证恢复失败能明确反馈，不能让用户误以为引擎仍记得旧上下文。锁定的 ACP schema 1.9.1 已有 v1 `ResumeSessionRequest`，目前没有证据表明为此必须升级协议依赖。

审批也不能直接算作「已有按钮即可完成」：当前 `events::ToolCall` 不保留 `rawInput`，`thread::PermissionRequested` 直接保存收到的请求，界面从请求本身取操作详情。DSH 的 ID-only 请求按这条路径会落到「Agent 未提供操作详情」。这是值得补的通用 ACP 关联逻辑，不应按每个 Harness 的工具名写特殊解析规则；对于未知参数结构，至少提供可展开的原始输入。

文件审阅的缺口无法靠通用客户端凭空补齐。DSH 没有发出修改前后的结构化内容，也不委托 ZJ 写文件。用整个仓库前后 `git diff` 冒充本轮 Agent 修改，会混入用户和其他 Agent 的并发修改，并漏掉部分未跟踪文件。完整逐块接受/拒绝应先获得上游的可靠变更事件，或评估独立的 DSH 插件；不建议在 ZJ 内复制文件编辑工具或扫描全工作区补偿。

本地依据：[预设](../../crates/agent_client/src/registry.rs)、[握手与进程池](../../crates/agent_client/src/host.rs)、[恢复/文件审阅/审批](../../crates/agent_client/src/client.rs)、[事件模型](../../crates/agent_client/src/events.rs)、[会话视图模型](../../crates/agent_client/src/thread.rs)、[历史恢复](../../crates/app/src/workbench/agent/history.rs)、[审批显示](../../crates/app/src/workbench/agent_panel/cards.rs)、[进程回收](../../crates/agent_client/src/process.rs)。外部依据为下文固定版本的 ACP 合约、入口和事件映射。

## 工作量与验收

以下是代码检查后的工程估算，不是实测工期。假设由一名熟悉 ZJ 的开发者完成、固定 DSH 版本、已有可用账号，不包含上游等待或修复未知 DSH 缺陷。

| 阶段 | 增量估算 | 交付与边界 |
| --- | --- | --- |
| 协议探针 | 0.5–1 人日 | 自定义 ACP 配置验证启动、新建、回复、审批、取消、多会话与退出；记录基础内存，确认 npm 发行包行为与源码一致 |
| 有明确限制的可选接入 | 2–4 人日 | 内置预设、通用 resume、审批详情关联、能力提示和针对性回归；支持 DSH 已有协议能力，仍缺 steering、逐 token 输出及完整文件审阅 |
| 达到现有 Codex/Claude 交互要求 | 暂不能可靠估总工期 | 需上游扩展或另做 DSH 插件，补 steering、增量输出、结构化变更和提问映射；这已超过只加预设，探针后单独决定是否值得维护 |

基础可选版本可按约 **3–5 人日** 预留。若运行中引导和逐块文件审阅必须首版就具备，则前两阶段不能算正式完成，更不应把「支持 ACP」解释成几行配置即可达到功能一致。

可复用 `crates/agent_client/examples/agent_smoke.rs` 的文字、图片、取消、模型配置、多会话和恢复场景；为恢复无回放、仅含 tool ID 的审批请求补通用协议测试。真实验证还应覆盖审批时取消、进程重启后续聊、多工作区会话隔离、一个会话关闭时另一个继续，以及退出后的自有子进程回收。图片按握手和实际模型路线验证，不能仅看预设名称。

内存验证采用 1/5/10 会话、同任务与工具配置，分别采样启动、运行峰值、任务结束、空闲回收和长对话。报告完整进程树的 macOS physical footprint，单列共享外部服务；不把 Linux PSS、RSS 和 macOS footprint 混为一个节省比例。本轮没有运行这些测试。

## JCode 与 Pi 承接其他模型

「其他模型走第三方」适合作为产品分工：用户选 JCode 或 Pi 后，再选择它支持的模型；提示、工具循环和上下文策略由该 Harness 执行。它们不应成为所有其他 Agent CLI 的统一代理网关。ZJ 只维护少量执行入口和通用编辑器能力。

| 候选 | 适合本项目的优势 | 尚须解决的具体问题 | 建议 |
| --- | --- | --- | --- |
| JCode v0.93.0 | Rust 核心、共享 daemon、多模型；公开多会话内存测试有优势信号 | ACP 没映射人工审批、steering 和结构化 Diff；MCP 仅 stdio；默认共享 daemon 的退出所有权要分清 | 优先验证，暂不能称为完整或已验证的默认选项 |
| Pi 1.1.0 | 简洁工具集、官方 RPC 有 steer/follow_up，扩展灵活，支持 stdio/HTTP MCP | Node/Bun 运行时；通用审批依赖扩展；已核对的社区 ACP adapter 每会话再起一个 Pi 进程 | 作为备选，先验证 adapter 的功能与完整内存，再决定是否值得直接维护 RPC 桥 |

JCode 作者的 Linux headless 测试中，旧开发版在 10 个会话完成每会话 5 轮任务后约 66.7 MB PSS；它体现共享引擎的潜力，但不是当前 macOS ACP 的峰值测试。Pi 的公开比较来自不同负载与版本，不能拿两者的现成数字直接判定实际差额。选 JCode 优先，依据是本项目对多会话内存和运行时的偏好，不是已经证明它在 ZJ 中全面胜出。

复用本次会话已核对的来源与限制：[JCode 评估](jcode-integration-assessment-2026-10-09.md)、[Pi / OpenCode 对比](pi-opencode-comparison-2026-10-09.md)。原始依据：[JCode ACP](https://github.com/1jehuang/jcode/blob/v0.93.0/src/cli/acp.rs)、[JCode 测量脚本](https://github.com/1jehuang/jcode/blob/v0.93.0/scripts/bench_headless_memory.py)、[Pi RPC](https://github.com/earendil-works/pi/blob/v1.1.0/packages/coding-agent/docs/rpc.md)、[社区 Pi adapter](https://github.com/svkozak/pi-acp/blob/04d0a15d6bb42bb65cce5c9caedcdcb2cdcb6905/src/pi-rpc/process.ts)。

无论选哪个 Harness，ZJ 都不承担 zvec、浏览器、MCP 等外部工具的内部缓存、索引和常驻服务优化；只管理自己启动的进程与会话，展示运行状态、错误和编辑器交互。旧自定义 Agent 和会话配置也无需因为推荐路线改变而立即删除。

## 官方身份、版本与运行入口

| 项目 | 已确认事实 |
| --- | --- |
| 身份与许可 | DeepSeek AI 官方仓库，MIT |
| 状态 | 官方标为 developer preview，快速迭代，可能出现破坏兼容性的变更 |
| 本次版本 | `dsh-v0.2.1-alpha.2`，2026-10-09 发布，GitHub 标记为 prerelease |
| 运行时 | TypeScript / Node.js；源码声明 Node `^22.19.0 || >=24.0.0` |
| 分发 | npm 包 `@deepseek-ai/dsh`；本次 GitHub release 没有独立二进制 assets |
| 架构 | Cordis 插件组合；CLI、ACP、SDK、Web 使用不同 profile |

来源：[发布页](https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.2.1-alpha.2)、[发布版 README](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/README.md)、[根 package.json](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/package.json)、[CLI package.json](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/apps/cli/package.json)。

官方 CLI 已支持以下入口，无须在 ZJ 内另建 Harness HTTP 服务：

```sh
dsh --profile acp
```

CLI 只启动所选 profile；`acp` 是在 `dsh-base` 上组合的 ACP stdio 应用，不启动 Web 界面。首次使用会初始化 `$DSH_HOME/profiles/acp`。`dsh --profile headless "job"` 是一次任务结束后退出的另一种入口，不应混用成持续 ACP 连接。

ACP profile 默认选择 `deepseek-official` / `deepseek-v4-flash`，可通过 profile patch 调整。它禁用 HMR 和模型生成会话标题，避免连接中途更换服务及额外的标题模型请求。调用目录是 CLI 默认工作目录；ACP 新建会话可传绝对 `cwd`。

来源：[CLI 合约](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/apps/cli/README.md)、[参数解析](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/apps/cli/src/args.ts)、[ACP profile](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/bundle/acp-app/README.md)、[默认配置](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/bundle/acp-app/cordis.patch.yml)。

## Harness 本身的能力

| 方面 | 发布版具备的能力 | 对 ZJ 的边界 |
| --- | --- | --- |
| 模型 | DeepSeek 原生 adapter，其他 provider 和自定义兼容配置；部分路由使用 pi-ai | 接入官方 Harness 可保留其工具循环、提示和上下文实现 |
| 工具与扩展 | 文件读写、编辑、shell、检索、计划、子 Agent，以及按组合启用的插件 | 由 Harness 执行和管理，ZJ 不应复制其工具生态 |
| 图片 | provider 可声明 image 输入；工具结果也可携带图片 | 仍取决于具体模型与 endpoint；配置声明不检测服务端是否真的支持 |
| MCP | stdio 与 Streamable HTTP，按 scope 配置，经过 Harness 权限与取消处理 | opt-in；没有证据表明所有会话自动共用一个 MCP 进程 |
| Skills | 本地技能目录与 `SKILL.md`，按需加载，作用域与可见性控制 | 不意味着所有其他 Agent 的扩展约定都无损兼容 |
| 运行中输入 | 内部 `steer`、`followup`、`inject` 和持久化 inbox | 内部能力不等于 ACP 对外已经支持 |
| 多会话与恢复 | 多个 Agent / Session，事件日志持久化，创建与恢复接口 | 同一会话的并发提示、恢复回放仍受所用入口限制 |
| 上下文 | 按需或自动压缩、工具结果裁剪、图片 offload 等插件能力 | 上下文压缩不等于历史日志立即从内存释放 |

来源：[provider 配置](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/user/guide/providers.md)、[核心与输入路由](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/core.md)、[MCP](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/mcp.md)、[Skills](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/skills.md)、[压缩](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/compaction.md)。

## ACP 对外暴露的能力

发布版将 ACP 定位为 automation-only，提供标准 ACP v1 方法，没有额外的私有 steering 方法。应分别判断引擎功能和编辑器可使用的协议功能。

| ZJ 关注的行为 | `dsh-v0.2.1-alpha.2` ACP 事实 |
| --- | --- |
| 新建、提示、取消、关闭 | 已实现；可传会话目录和标准 MCP 声明 |
| 模型与思考强度 | 有可发现的 model / reasoning 配置 |
| 工具审批 | 有一次性允许/拒绝；不能据此推断持久授权或通用提问已支持 |
| 运行中追加输入 | 同一会话存在 in-flight prompt 时拒绝新 prompt；没有 ZJ steering 扩展 |
| 输出节奏 | 根据已提交的语义事件更新，不转发模型原始逐 token delta |
| 文件审阅 | 工具映射有输入/输出内容，但没有结构化 Diff、locations 或客户端 fs 委托 |
| 恢复 | 有 `session/list`、`session/resume`；可在相同持久化 root 上跨进程恢复，但不回放历史 |
| 历史加载 | 没有 `session/load` / replay；不能直接代替 ZJ 现有 load 路径 |
| 其他界面功能 | 未暴露通用提问、commands、plans、terminals、fs 客户端能力 |

来源：[ACP 协议合约](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/acp/acp/README.md)、[会话实现](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/acp/acp/src/session.ts)、[事件映射](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/acp/acp/src/updates.ts)、[桥接入口](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/acp/acp/src/index.ts)。

SDK 不能自动补全这些交互：发布版 SDK 虽有六类客户端请求，但没有 cancel、session-close 方法，服务端也不发起审批请求。选择专用接口或自定义插件意味着新增映射及维护工作，不能只凭「有 SDK」判断比 ACP 更简单。来源：[SDK 协议合约](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/sdk/protocol/README.md)。

## 进程生命周期与内存

ACP profile 将 stdin EOF 绑定到有界正常退出；ACP 连接关闭、SIGINT、SIGTERM 会清理桥接拥有的 Agent 和 profile 树。stdout 保留给 JSON-RPC 帧。该入口适合由编辑器管理外部子进程，具体异常退出和工具子进程回收仍需集成测试。

**目前没有能与 ZJ、JCode 或 Pi 直接比较的内存数据。** Node 是真实运行依赖；ZJ 主程序和已有桥接用 Rust，并不会消除外部 Harness 的 Node 进程。也不能仅凭 TypeScript 或插件数量估算实际占用。

Session 的事实源是内存中的 append-only 事件日志；默认持久化后端写逻辑 JSONL，并使用带校验的 Zstandard frames。落盘和压缩有助于持久性与模型上下文管理，不能据此认定长会话的历史已完全移出内存。

推荐后续只启动 ACP profile，在相同仓库、模型、工具配置下测单会话和多会话的完整进程树 peak / idle physical footprint，并检查长输出、图片、恢复及关闭后的回收。现阶段不能承诺它比现有原生 Agent 更省内存。

来源：[ACP profile 生命周期](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/packages/bundle/acp-app/README.md)、[Session](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/session.md)、[持久化](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/persistence.md)。

## 权限、沙箱与成熟度

Harness 有实际平台沙箱 backend：Linux 使用 bwrap / Landlock，macOS 使用 Seatbelt，Windows 使用 ACL restricted token。`read-only`、`workspace-write`、`danger-full-access` 与审批策略分别配置；受限策略找不到可用 backend 时会报 `SANDBOX_UNAVAILABLE`，不能静默改成无隔离执行。

这些 mode 的合约主要约束文件效果，不覆盖全部网络和进程可见性。backend 也会报告 full / partial 支持。自定义插件组合是否使用这些执行边界，需要结合实际配置判断。

审批默认 `ask` 将请求交给已配置 answerer；无人处理、取消或失败都会拒绝。这里的 `never` 表示不提问并拒绝每次审批请求，不能误读成自动批准全部操作。命名权限预设把沙箱和审批策略组合，仍须区别于 ACP 实际暴露的控制面。

官方 `SAFETY.md` 明确该项目处于实验性 developer preview，未经安全审计，不应视为已具备生产级安全保证。结合 alpha 预发布状态，应先固定版本做隔离验证，再考虑作为常规选项。

来源：[沙箱合约](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/sandbox.md)、[审批](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/approval.md)、[权限预设](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/docs/subsystems/permission-presets.md)、[官方安全说明](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.1-alpha.2/SAFETY.md)。

此次验证范围为发布版文档、配置、接口及源码对照；没有运行候选、执行 ZJ 项目测试、打包或安装。
