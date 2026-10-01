# Agent 集成方案调研

查阅日期：2026 年 10 月 1 日，Asia/Shanghai。目标是为 Rust / GPUI 轻量编辑器选择后续可接入 Codex、Claude 和 DeepSeek 的路线。本轮仅阅读公开文档和源码；没有调用模型、读取凭据、运行安装脚本、改写用户配置或向服务发送项目源码。

## 建议

后续通用编辑器接入优先验证 **ACP v1 + 按需启动的外部 Agent 进程**。ACP 有 Rust SDK，已有 Codex 和 Claude 适配器，OpenCode 也原生提供 ACP 入口，可作为 DeepSeek 的运行载体。[S01][S02][S03][S08][S12] 如果首个试点只接 Codex，直接使用官方 `codex app-server` 的本地 stdio 接口更直接；是否保留原生接口，应以 ACP 适配器的能力缺口和实测资源成本决定。[S06][S08]

DeepSeek 已公开支持 Responses API，并提供 Codex 和 Claude Code 的配置指南；不能沿用「只有 Chat Completions」的旧结论。其 Responses 实现仍是有明确限制的子集，模型 API、coding-agent 运行时和编辑器协议需要分别验证。[S15][S16][S17]

本报告不改变首版范围。开发说明和 `CLAUDE.md` 都将 Agent 后置；当前开发记录仍未通过 G1，可靠保存和编辑恢复也未完成。先完成文档身份、冲突处理、保存与恢复，再允许 Agent 提议的修改进入真实文件。[L01][L02][L03]

## 先区分四层

| 层级 | 作用 | 本次候选 |
| --- | --- | --- |
| 编辑器协议 | 会话、流式消息、工具状态、权限请求、Diff 展示 | ACP；Codex 专用 app-server 协议 |
| Agent 运行时 | 上下文与工具循环、文件操作、命令、会话持久化 | Codex；Claude Agent SDK；OpenCode |
| 模型 API | 推理、文本或工具调用输出 | OpenAI、Anthropic、DeepSeek API |
| 外部工具协议 | 将工具、资源和数据源提供给 AI 应用 | MCP |

ACP 面向「编辑器 ↔ coding agent」；MCP 面向「AI 应用 ↔ 外部系统」。两者可以组合，但接入 MCP 不会自动获得聊天会话、Agent 执行、审批界面或文件恢复。[S01][S04] 模型返回 function call 后，宿主或 Agent 执行工具并回传结果；模型 API 本身不负责执行本机文件修改。[S14][S18]

## 路线比较

| 路线 | Rust 接入方式 | 已核验的能力 | 取舍 |
| --- | --- | --- | --- |
| ACP + 外部 Agent | 官方 Rust `Client` 实现，JSON-RPC / stdio 子进程 | 会话、流式更新、取消、工具状态、权限请求、Diff；恢复等能力需协商 | 通用路线；适配器、运行时、协议版本都需锁定。[S01][S02][S03][S05] |
| Codex app-server | Rust 管道读写 JSONL；按安装版本生成 JSON Schema | thread start / resume / fork、turn stream / interrupt、登录、审批、review | Codex 深度集成最直接；专用协议，额外实验性字段不可当作稳定承诺。[S06] |
| `codex exec --json` | 每次任务启动 CLI，解析 JSONL | 事件流、预设权限、结构化最终结果、指定会话 resume | 适合一次性任务；不是双向交互审批协议，不宜作为完整聊天面板的长期接口。[S07] |
| Claude Agent SDK | Python / TypeScript sidecar；或使用其 ACP 适配器 | 内置 coding tools、流式消息、会话、权限回调、hooks、文件 checkpoint | 官方未提供 Rust SDK；额外运行时与子进程成本需测量。[S09][S10][S11] |
| OpenCode | `opencode acp` 子进程 | ACP、工具和权限系统、多个模型 provider；有 DeepSeek 官方接入指南 | 可复用完整 Agent；默认权限较宽，必须显式限制；ACP 下部分 `/undo`、`/redo` 命令不支持。[S12][S13][S19] |
| 直接 API / Rust Rig | HTTP / SSE；或使用 Rig 的 provider 与工具循环 | Rig 包含 OpenAI、Anthropic、DeepSeek provider，支持多轮和流式工作流 | 自己承担 coding tools、权限、上下文和变更事务；成本最大，暂不优先。[S14][S18][S20] |

表中的「能力存在」不代表本项目已验证可用，也不代表各运行时具有相同的工具和权限语义。没有对这些候选运行基准、真实登录或端到端模型测试。

### ACP 的实际支持边界

ACP 稳定 v1 的基础方法包括 `session/new`、`session/prompt`、`session/cancel` 和 `session/update`。会话各自保存上下文；`session/load`、`session/resume`、`session/close` 等扩展能力需要以 `initialize` 返回值为准，未声明的能力视为不支持。会话数量不是同时执行数量；并发策略和资源上限仍由编辑器控制。[S05]

本地 stdio 是明确的传输规范：编辑器启动子进程，双方按行收发 JSON-RPC，日志走 stderr。远程支持仍在推进，Streamable HTTP 为草案；本项目优先本机 stdio。官网已发布 v2 草案，不宜在首次集成时直接依赖 v2 或适配器的 AIR / `_meta` 私有扩展。[S01][S03][S21]

当前维护地址是 `agentclientprotocol/codex-acp` 与 `agentclientprotocol/claude-agent-acp`。Codex 适配器启动 app-server，再将 ACP 请求和事件转换为 Codex 操作；Claude 适配器使用官方 Agent SDK。两者的 README 都列出工具权限与编辑审查支持；这些是适配器项目声明，需要对发布版本实测。[S08][S22]

ACP `fs/read_text_file` 可读取未保存的编辑器状态，`fs/write_text_file` 可把文件写入交给客户端；仅在客户端声明相应能力时允许调用。但 Agent 仍可通过自己的工具访问磁盘，关闭 ACP 写入能力不等于禁止 Agent 写磁盘。`session/request_permission` 是可选请求，Diff 是展示数据，协议本身不保证「所有修改必先审批」。[S23][S24] 因此必须同时配置 Agent 工具权限和实际隔离，不能只拦截界面回调。

### Codex：原生 app-server 与 exec

app-server 官方定位是丰富客户端的集成接口，提供认证、历史、审批和事件流；本地默认 stdio，线上消息省略 JSON-RPC 的 `jsonrpc` 字段。每个连接先 `initialize` / `initialized`，以 thread / turn ID 关联状态；按 CLI 版本生成 Schema 可以避免直接跟随最新示例。[S06]

取消使用 `turn/interrupt`，以 `turn/completed` 的最终状态确认收敛。命令和文件修改的审批请求分别有对应的 server-initiated request；审批行为取决于权限配置，并非所有操作都一定请求客户端审批。`review/start` 是模型代码审查，不代替用户对最终 Diff 的接受与可靠写入。[S06]

首次集成不需要 WebSocket、远程执行或 app-server 的实验性插件 API。特别注意 `thread/shellCommand` 官方说明在沙箱外执行且不继承线程沙箱；不能把它当作普通受限 Agent 工具入口。[S06]

`codex exec --json` 适合独立任务或 CI，提供事件 JSONL、`--output-schema` 和 `resume`，默认只读沙箱。它依赖预设的执行策略；如果产品需要暂停任务、呈现审批后再继续，优先 app-server 或 ACP，而不是解析终端提示。[S07]

### Claude：SDK、CLI 与恢复

官方 Agent SDK 是 Python / TypeScript 库，内部运行 Claude Code binary；官方也明确其他语言可以把 `claude -p` 当作子进程。CLI `--output-format stream-json` 配合 `--verbose`、`--include-partial-messages` 可获得增量消息；完整权限交互优先 SDK 的 `canUseTool` 或已实现该交互的 ACP 桥。[S09][S10][S11]

多会话应保存明确的 session ID 并使用 `resume`，避免同目录多任务误用「继续最近一次会话」。SDK 会话持久化的是对话，不是文件状态；fork 也不会创建隔离文件系统。[S25]

`canUseTool` 只处理权限评估后需要交互的操作，不是每次工具调用都会经过的统一授权钩子。`acceptEdits` 自动批准一定范围内的文件操作；`bypassPermissions` 不是只允许 `allowedTools`，且具有广泛系统访问能力。只读试点应显式拒绝写操作，限制命令并使用实际隔离，不能仅靠模式名称或提示词。[S11]

Claude 文件 checkpoint 仅跟踪 Write / Edit / NotebookEdit 等指定工具；Bash 写入和大部分子 Agent 修改不在恢复范围内。不能以 SDK checkpoint 代替本编辑器的文档日志、外部版本复核和恢复事务。[S26]

## DeepSeek：模型与 Agent 的组合

DeepSeek 官方文档确认 Chat Completions、Anthropic 格式、Responses API 和工具调用；同时列出 Codex、Claude Code、OpenCode 的接入指南。DeepSeek API 是模型接口，需要 Codex、Claude SDK、OpenCode 或自建运行时提供工具执行、权限、会话和文件操作闭环。[S15][S16][S17][S18][S19]

可验证的组合按实现成本排序：

1. **已有 Codex 运行时 + DeepSeek provider**：官方指南配置 `wire_api = "responses"`、模型目录和 provider；编辑器继续通过 app-server 或 ACP 控制 Codex。无需重新实现 coding-agent 工具循环。[S16]
2. **OpenCode ACP + DeepSeek provider**：原生 ACP 入口，DeepSeek 官方有 `/connect` 配置指南；适合验证多 provider 的独立 Agent。[S12][S19]
3. **Claude 运行时 + DeepSeek Anthropic 接口**：DeepSeek 官方指南通过 provider 环境变量切换。属于 DeepSeek 方给出的兼容路线；本轮未证实其与特定 Claude SDK / ACP 发布版本的全部能力相容。[S17]
4. **直接 DeepSeek API / Rig**：适合以后需要自有工具、可控上下文和模型切换时再评估；不能只接 HTTP 客户端就宣称完成 coding-agent 集成。[S18][S20]

DeepSeek Responses 的限制已经明确写在官方文档中：`previous_response_id`、`conversation`、`store`、`background` 等不支持；部分不支持的参数静默忽略；`web_search`、`file_search`、`code_interpreter`、`mcp` 等内置工具忽略；custom tool 仅支持特定 `apply_patch`。因此要在客户端维持完整上下文并按 provider 能力选择工具，不能假定与 OpenAI Responses 全面等价，也不能把 Responses 端点视为 Codex app-server 端点。[S15][S06]

DeepSeek 首页还公布了 **DeepSeek Harness developer preview**；其指南展示 Web UI、Python SDK 和本地任务执行。它是新增运行时候选，不能忽略，但本轮尚未验证成熟度、Rust / ACP 接口、发行与资源成本，因此暂不选为首个编辑器接入方案。[S27]

## 认证、费用与资源归属

| 选项 | 已核验的边界 | 本项目建议 |
| --- | --- | --- |
| Codex ChatGPT 登录 | Codex 本人使用支持订阅登录；API key 按 API 用量计费。app-server 文档明确其既有认证接口不得用于商业或托管服务，并推荐正式 Sign in with ChatGPT。[S06][S28] | 自用原型可验证官方登录流程；分发或商业化前重新核验授权路线。不得自行复制用户 token。 |
| Claude 登录 / API key | SDK 官方明确：第三方产品未经批准不得提供 claude.ai 登录或使用其额度，应采用 API key；不能因为 CLI 可用订阅就推断嵌入产品可以复用。[S09] | 首次 SDK / ACP 原型用 API key；不把 Claude Pro / Max 纳入通用 provider 登录。 |
| DeepSeek API key | 官方按输入 / 输出 token 扣除充值或赠送余额，价格可调整。[S29] | provider 身份与费用单独展示；不把 ChatGPT / Claude 订阅视为 DeepSeek API 额度。 |
| 外部运行时 | ACP 运行时可能包含 adapter、Agent binary、Node / Python、工具、MCP 和 shell 子进程。[S03][S08][S09] | 所有为编辑器启动的进程纳入 footprint / CPU / 峰值测量，不以移到 sidecar 宣称轻量达标。 |

模型 API key、OAuth token、工作区路径和源码都不要写入普通配置、命令行、日志或错误回传。建议使用系统凭据存储或运行时官方认证流程；环境变量只在必需的子进程中设置，并防止执行的项目脚本继承真实密钥。Codex 官方支持 OS credential store，并明确 auth 文件是敏感信息；OpenCode 的公开文档说明其默认 key 保存位置，需要单独处理存储风险。[S28][S07][S13]

本项目没有测试任何候选的体积、内存或 CPU，不能给出资源增量。后续依赖准入仍按 `CLAUDE.md`：记录 cargo tree、dist 二进制和相同工作集的 footprint 前后差值；超过原约定阈值写决策记录。测量既包含编辑器自身，也包含新启动的 Agent、适配器及其工具进程。[L02]

## 分阶段实施建议

以下是工程建议，尚未实现；不是对首版范围或门禁状态的变更。

1. **先完成文档可靠性。** 文件身份、单一可编辑实例、外部版本检查、原子保存、dirty 关闭语义和恢复日志通过验收。现有自动刷新只保留缓冲区，尚不具备这些完整前提。[L01][L03]
2. **只读接入试点。** 在自建夹具中锁定 ACP v1、Rust SDK 和一个发布版 Agent，验证流式、明确 session ID、取消、审批拒绝、进程退出与有界输出。若选 Codex 原生接口，同样锁定 CLI / Schema；不自动导入用户全局配置、插件或 hooks。[S05][S06][S10]
3. **完成权限闭环再开放编辑。** 每个请求绑定会话、工作区和目标；用户选择可见的读入范围；真实沙箱限制磁盘与命令。权限请求完整显示动作、路径 / argv / cwd、网络目的地和影响，取消时撤销待答审批。Diff 需要与当前文档版本绑定，由可靠的文档事务应用；存在 dirty 或外部修改时先比较，不能静默覆盖。[S06][S23][S24][L01]
4. **开放一个修改会话。** 优先使用独立 worktree 或临时复制，使 Agent 的直接写盘不会抢占真实 dirty 文档；记录修改前后状态与失败恢复。直接 API 生成的 patch 也要走相同校验和事务，不在渲染线程落盘。单一会话取消确认后收集最终修改，不能将「停止生成」理解为「已撤销磁盘修改」。[S25][S26][L01]
5. **再扩展三方与资源控制。** 加入 Claude ACP 和 DeepSeek 组合，验证能力协商和不同 provider 的退化路径；按需启动、不预热全部 Agent、全局限制活动任务，关闭或空闲时回收。完整记录主进程与归属子进程资源和成本；未通过预算则缩减并发或能力。[L02]

首个兼容性验证应覆盖：两个会话不串消息、流式 JSON 分段 / 超限、未知事件兼容、审批拒绝、审批期间取消、工具退出、Agent 崩溃、工作区关闭、恢复失败、symlink / 越界路径、dirty 与外部修改冲突、工具进程泄漏，以及重复应用修改的幂等性。后续真实登录和付费 API 验证应按届时授权范围进行；本次调研没有执行这些操作。

## 来源与未验证项

来源以实际打开的官方文档和项目维护方源码为主。页面和 GitHub `main` 随发布变化；下列能力不绑定某个二进制发布版。需要实施时固定协议 / SDK / Agent / 适配器版本，并重新核验认证条款、provider 兼容性和测试结果。本轮未验证真实账户权限、付费模型行为、端到端取消后的进程清理、数据保留 / 地区政策和资源增量；这些不能由文档声明推断通过。

### 项目依据

- [L01：轻量代码编辑器开发说明](../../轻量代码编辑器开发说明.md)：首版范围、文档身份、保存、恢复与资源归属。
- [L02：CLAUDE.md](../../CLAUDE.md)：首版排除 Agent、资源预算、依赖准入和临时夹具测试约定。
- [L03：开发记录](../development.md)：本批完成范围与 G1、可靠保存的未完成状态。

### 公开来源

- [S01：ACP Introduction](https://agentclientprotocol.com/get-started/introduction)：协议目的、MCP 关系与远程支持状态。
- [S02：ACP Rust library](https://agentclientprotocol.com/libraries/rust)：Rust SDK、Client / Agent 接口与 Zed 使用情况。
- [S03：ACP v1 Transports](https://agentclientprotocol.com/protocol/v1/transports)：stdio 规范与远程传输草案。
- [S04：MCP Introduction](https://modelcontextprotocol.io/docs/getting-started/intro)：外部系统、工具和资源协议定位。
- [S05：ACP v1 Initialization](https://agentclientprotocol.com/protocol/v1/initialization)、[Session Setup](https://agentclientprotocol.com/protocol/v1/session-setup)、[Prompt Turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)、[Cancellation](https://agentclientprotocol.com/protocol/v1/cancellation)：能力协商、会话、流式与取消。
- [S06：OpenAI Docs — Codex App Server](https://learn.chatgpt.com/docs/app-server)：协议、Schema、审批、review、认证及实验性接口限制。
- [S07：OpenAI Docs — Non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode)：exec、JSONL、沙箱、结构化输出、resume 与凭据环境。
- [S08：Codex ACP adapter README](https://github.com/agentclientprotocol/codex-acp/blob/main/README.md)：当前适配器结构、发行方式、能力与认证入口。
- [S09：Claude Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)：运行时、SDK 语言、CLI 接入、认证与使用条款。
- [S10：Run Claude Code programmatically](https://code.claude.com/docs/en/headless)：stream-json、子进程、信号与 bare 模式的配置加载边界。
- [S11：Claude Agent SDK permissions](https://code.claude.com/docs/en/agent-sdk/permissions)：权限评估、canUseTool、acceptEdits 与 bypassPermissions。
- [S12：OpenCode ACP](https://opencode.ai/docs/acp/)：`opencode acp` 与不支持的内置命令。
- [S13：OpenCode Providers](https://opencode.ai/docs/providers/)、[Permissions](https://opencode.ai/docs/permissions/)：provider、key 保存位置与默认权限。
- [S14：OpenAI Docs — Function calling](https://developers.openai.com/api/docs/guides/function-calling)：工具调用循环中宿主的执行职责。
- [S15：DeepSeek Responses API](https://api-docs.deepseek.com/guides/responses_api)：SSE 与逐项兼容性限制。
- [S16：DeepSeek Integrate with Codex](https://api-docs.deepseek.com/quick_start/agent_integrations/codex)：Responses provider、模型目录与配置要求；未运行页面中的配置脚本。
- [S17：DeepSeek Integrate with Claude Code](https://api-docs.deepseek.com/quick_start/agent_integrations/claude_code)：Anthropic 格式与 provider 切换。
- [S18：DeepSeek Tool Calls](https://api-docs.deepseek.com/guides/tool_calls)：模型与宿主执行工具的分工。
- [S19：DeepSeek Integrate with OpenCode](https://api-docs.deepseek.com/quick_start/agent_integrations/opencode)：DeepSeek provider 的配置入口。
- [S20：Rig README](https://github.com/0xPlaygrounds/rig/blob/main/README.md)、[provider modules](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/mod.rs)：Rust 多 provider 和 agent runtime；不将其视为现成编辑器集成。
- [S21：ACP documentation index](https://agentclientprotocol.com/llms.txt)：稳定 v1 与 v2 draft 的当前文档入口。
- [S22：Claude ACP adapter README](https://github.com/agentclientprotocol/claude-agent-acp/blob/main/README.md)：当前维护地址与 SDK 桥接能力。
- [S23：ACP v1 Tool Calls](https://agentclientprotocol.com/protocol/v1/tool-calls)：权限请求、工具状态、Diff 与终端内容。
- [S24：ACP v1 File System](https://agentclientprotocol.com/protocol/v1/file-system)：未保存缓冲区、客户端文件能力与读写请求。
- [S25：Claude Agent SDK Sessions](https://code.claude.com/docs/en/agent-sdk/sessions)：明确 session ID、resume / fork 与对话持久化边界。
- [S26：Claude file checkpointing](https://code.claude.com/docs/en/agent-sdk/file-checkpointing)：覆盖的工具和未覆盖的文件修改。
- [S27：DeepSeek API introduction](https://api-docs.deepseek.com/)、[DeepSeek Harness Quickstart](https://deepseek-harness.github.io/deepseek-harness/en/guide/quickstart)：developer preview 与独立运行时入口。
- [S28：OpenAI Docs — Authentication](https://learn.chatgpt.com/docs/auth)、[Custom model providers](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers)：订阅 / API key 差异、凭据存储和自定义 provider 认证。
- [S29：DeepSeek Models & Pricing](https://api-docs.deepseek.com/quick_start/pricing)：按 token 扣费与价格变化说明；没有把当日价格固化为产品承诺。
