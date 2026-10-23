# Agent 方案（调研归并）

调研时间：2026 年 10 月 1 日至 22 日。由同主题旧报告归并；原文与完整来源清单见 Git 提交 `3c14cd8` 的 `docs/reports/`。

每节先写调研当时的事实与建议，再标「现状」表示按当前代码与 ADR 核对后的落地情况。旧文的版本号、能力与内存数字是历史证据，不代表今天的最新版本；旧建议也不自动等于现状。完整链接清单可从上述历史提交查阅。

## 现状（2026-10-23 按代码与 ADR 核对）

- 协议路线为 **ACP v1 + 按需启动的外部 Agent 进程**（ADR 0004）：Rust 客户端锁定 `agent-client-protocol = 2.2.0`，本地 stdio，`agent_client/host.rs` 的 `AgentPool` 按完整预设与环境复用进程，会话空闲 10 分钟发 `session/close`。
- 内置预设只有 Claude Code、Codex、Claude Code · DeepSeek；前两者是 ZJ 自己的 `Launch::Bridge`，DeepSeek 仍是 Claude bridge 加 provider 环境变量。没有 Kimi、JCode、Pi、DeepSeek Harness 预设。
- npm 适配器已被 ADR 0009 的 Rust 桥接取代（Claude 每会话一个 `claude -p`，Codex 共用 `codex app-server`），这也落实了 2026-10-09 报告「不复用 Node」的方向。
- 自定义 Agent 通过设置文件 `custom: Vec<UserAgentConfig>` 添加；官方 ACP Registry 未接入，也没有内置目录缓存或自动安装。
- 子 Agent 订阅回收与 HTTP／STDIO MCP 对照已在 2026-10-09 报告后续章节实现（`agent_bridge/src/codex/subagents.rs`）。
- 2026-10-22 的三项轻量交互（待审阅会话保留、输入区显示「追加到当前轮」、审阅命令）已于 2026-10-23 落地，见[体检结论](../验收/体检结论.md)。

## 先分四层，再谈选型

| 层 | 作用 | 关键区别 |
| --- | --- | --- |
| 编辑器协议 | 会话、流式消息、工具状态、权限、Diff | ACP；Codex 专用 app-server |
| Agent 运行时 | 工具循环、文件／命令、会话持久化 | Codex、Claude Code、JCode、Pi、OpenCode |
| 模型 API | 推理与工具调用输出 | OpenAI、Anthropic、DeepSeek、Kimi |
| 外部工具协议 | 向 Agent 提供工具与资源 | MCP（stdio / Streamable HTTP） |

ACP 管「编辑器 ↔ coding agent」，MCP 管「AI 应用 ↔ 外部系统」。接入 MCP 不会自动得到聊天会话、审批界面或文件恢复；换模型 provider 也不会自动改变运行时行为。模型是远端服务时，本地占用主要来自运行时、上下文、工具输出、快照和工具进程。

## 路线比较（2026-10-01 / 10-09 调研）

| 路线 | 接入方式 | 取舍 |
| --- | --- | --- |
| ACP + 外部 Agent | 官方 Rust SDK，JSON-RPC stdio | 通用；适配器、运行时、协议版本都需锁定 |
| Codex app-server | JSONL，按版本生成 Schema | Codex 深度集成最直接；专用协议，实验字段不是稳定承诺 |
| `codex exec --json` | 一次性 CLI + JSONL | 适合独立任务／CI，不是双向审批协议 |
| Claude Agent SDK | Python / TS sidecar 或 ACP 适配器 | 无 Rust SDK；额外运行时成本 |
| OpenCode | `opencode acp` | 原生 ACP；默认权限较宽，部分内置命令在 ACP 下不可用 |
| 直接 API / Rig | HTTP / SSE | 自己承担工具、权限、上下文和变更事务，成本最大 |

未采纳理由与边界：

- ACP `fs/read_text_file`、`fs/write_text_file` 只在客户端声明能力时可用；关闭 ACP 写能力不等于禁止 Agent 自行写盘，必须同时配置工具权限与隔离（ACP v1 file-system / tool-calls 文档，2026-10-01 查阅）。
- `session/request_permission` 是可选请求，Diff 只是展示数据，协议不保证「所有修改必先审批」。
- Codex `thread/shellCommand` 官方说明在沙箱外执行且不继承线程沙箱，不能当作受限工具入口。
- Claude `canUseTool` 不是每次工具调用的统一授权钩子；`acceptEdits` / `bypassPermissions` 的语义不能靠模式名推断安全。文件 checkpoint 只覆盖 Write / Edit 等指定工具，不能替代 ZJ 自己的文档事务与恢复。
- 认证：第三方产品未经批准不得提供 claude.ai 登录或复用其额度；app-server 既有认证接口不得用于商业／托管服务。凭据不得写入配置、命令行、日志或回传。

来源：<https://agentclientprotocol.com/protocol/v1/initialization>、<https://learn.chatgpt.com/docs/app-server>、<https://code.claude.com/docs/en/agent-sdk/permissions>、<https://opencode.ai/docs/acp/>（均为 2026-10-01 访问）。

## 自定义 Agent 与官方生态（2026-10-09）

调研建议：先做添加表单 + 少量已验证预设（名称、可执行文件、参数、环境变量），再按需读取 ACP Registry 作为目录；原生 ACP Agent 直接用 stdio，Claude／Codex 保留 ZJ 桥接；不为「可安装列表」启动或安装任何 Agent。

- Registry 的 `binary.cmd` 是下载包内相对命令，不保证在 PATH；自动下载会引入校验、解压、版本管理。
- 握手成功只证明协议入口可响应，不证明审批、取消、图片、恢复、并发都兼容。
- 当时建议优先验证 Goose（Rust，`goose acp`）、OpenCode、Gemini CLI；三者都未在 ZJ 实测。
- 通用终端网关 `coder/agentapi` 已停止维护，不采用；MCP、Agent 编排与会话接口不混为一层。

现状：只落地了设置文件里的通用 Binary 配置，表单、Registry、Goose／Gemini 目录均未实现。

来源：<https://github.com/agentclientprotocol/registry>、<https://github.com/coder/agentapi>。

## DeepSeek、JCode、Pi、Harness、腾讯记忆

- **DeepSeek 模型**：官方确认 Chat Completions、Anthropic 格式、Responses API 与工具调用，并给出 Codex／Claude Code／OpenCode 接入指南。Responses 是不完整子集：`previous_response_id`、`store`、`background`、内置 `web_search` 等不支持，部分参数静默忽略。因此不能把 Responses 端点当作 app-server，也不能假定与 OpenAI 全面等价。现状：ZJ 只保留 Claude Code · DeepSeek 预设（`https://api.deepseek.com/anthropic`）。
- **DeepSeek Harness**：`dsh --profile acp` 已有原生 ACP；版本 `dsh-v0.2.1-alpha.2`（2026-10-09），Node ≥22、npm 分发、developer preview。ACP 缺 steering、逐 token 输出、结构化 Diff 与客户端 fs；有 `session/resume` 但无 `session/load` 回放。接入需新增通用 resume 路径与审批详情关联（DSH 审批请求只带 `toolCallId`）。估算基础可选版 3–5 人日，未实施。
- **JCode v0.93.0**：Rust 引擎、共享 daemon、多模型。ACP 已实现建会话、prompt、取消、关闭、模型配置，但缺 `session/request_permission`、ZJ steering 扩展和 diff／locations／客户端 fs；MCP 仅 stdio；登录 `authMethods` 为空。issue #568、#1600、#761 等仍未合入，且部分回复是自动分诊，不能当承诺。作者公开的 Linux headless PSS（1/5/10/20 会话）为 32.6 / 51.0 / 66.7 / 90.6 MB，是同任务完成后约 1 秒的采样，不是执行峰值，也不是 ACP 或 macOS 实测。
- **Pi 1.1.0**：TypeScript/Bun；默认工具简洁，但已有 MCP、扩展、持久会话；官方 RPC 有 `steer` / `follow_up`（在工具执行后、下次模型调用前注入，不是立即打断 token）。社区 `svkozak/pi-acp` 每个会话再起一个 `pi --mode rpc` 子进程，并把扩展确认转成 ACP 权限请求；不能据此认为默认具备完整工具审批。
- **OpenCode 1.18.35**：原生 `opencode acp`，内置 allow/ask/deny，但默认多数工具 allow。历史内存对照（不同口径，不能互比或拼表）：Linux CLI PSS Pi 0.62 144.4 / 833.0 MB（1／10 会话）对 OpenCode 1.0.203 371.5 / 3237.2 MB；macOS 脚本化长会话 RSS Pi 0.85.1 181.4 / 227.9 MB 对 OpenCode 1.17.12 798.9 / 943.0 MB。
- **腾讯 TencentDB Agent Memory**：提交 `b1f79fc`，Node ≥22.16，默认三容器（memory-core / hub / proxy），主推「Agent → MemoryProxy → 上游模型」，需改模型端点配置。CodeGraph／Wiki 与 zvec-grep 部分重叠；Chat Memory 提炼 L0→L3 是主要增量。未验证与 ZJ 非交互桥接、Plan 模式和订阅登录的兼容；不建议内置或替换 zvec-grep。

来源：<https://api-docs.deepseek.com/guides/responses_api>、<https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v0.2.1-alpha.2>、<https://github.com/1jehuang/jcode/tree/v0.93.0>、<https://github.com/earendil-works/pi/tree/v1.1.0>、<https://github.com/TencentCloud/TencentDB-Agent-Memory/tree/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4>。

## 内存与进程生命周期的本机实测（2026-10-09，历史数据）

- 一次真实负载快照（48 样本 × 250 ms）：ZJ GUI 平均 583.9 MB（组内峰值 599.9），bridge 6.9 MB，Codex CLI 155.9 MB，Node/REPL 工具 375.2 MB（12 个进程，其中 3 个 `zvec_grep` STDIO 转发 270.7 MB），可读进程树合计 1140.2 MB。各组峰值不同时发生，不能相加。
- 搜索服务：共享守护进程 201–285 MB（不同时刻快照，不能算增长），三份 STDIO 转发进程合计 284.5 MB；安装包的 `zg server --stdio` 先找或启动守护进程，再走 HTTP 转发，因此三份转发不等于三份索引。
- HTTP 与 STDIO 隔离对照（同一守护进程、两会话、无模型请求）：HTTP 无转发进程，STDIO 两份各约 105 MB，合计 210.5 MB；切换收益来自去掉转发层，不是索引共享。
- 会话回收：ZJ 侧空闲 10 分钟后发 `session/close`；Codex 取消订阅后还有默认 60 秒卸载等待才退出工具。已确认的缺口是主会话 close 不递归覆盖子 Agent 订阅；桥接现已按归属、状态和 generation 取消空闲后代订阅，运行中／待审批／待回答的保留。
- 结论：已保存回复文本很小（1.16 MB，其中回复 177.7 KB），不能把高占用主要归因于长回复；图形资源与工具进程是主要已观测项。

局限：单日单机、Linux PSS 与 macOS footprint 不可混用；诊断时运行版 Codex 与当前版本可能不同；`process.rs` 只删除特定继承变量，没有 `env_clear`，「按 env 分 host」不等于「每个 host 只持有自己 provider 的凭据」。

## 多模型运行时（2026-10-22）

结论：保留 Codex／Claude 原生桥接，为 DeepSeek／Kimi 评估同一套可复用运行时；优先验证 JCode 接口能否补齐，Pi 备选，自建 Rust 工具循环暂不实施。

| 路线 | 收益假设 | 主要门槛 | 判断 |
| --- | --- | --- | --- |
| A：JCode 共用运行时 | 共享 Rust 引擎降低多会话增量 | ACP 缺审批／steering／完整 Diff；私有 daemon 回收与凭据隔离需验证 | 先做接口验证 |
| B：Pi 共用运行时 | 复用 provider 与工具循环 | 现成 ACP adapter 每会话一个进程；权限靠扩展 | 备选 |
| C：ZJ 自建工具循环 | 完全按 ZJ 需求设计 | 流式、取消、重试、压缩、恢复、沙箱全自担 | 暂不选择 |

共享边界（未变）：pooling key 不能只用显示名或 model id；不同信任域分进程；host 共享不等于工作区授权共享；多 Agent 同文件写入需单写者或隔离 worktree；只回收 ZJ 自有进程。建议验证目标（非已测量、非项目门禁）：新 runtime host + 适配层 ≤50 MB，每空闲恢复句柄增量 ≤10 MB，4 个短会话（最多 2 个执行）进程树峰值 ≤200 MB；同任务首个事件 p95 增量 ≤100 ms、中位总耗时退化 ≤10%。同任务基准需冻结 commit、runtime／模型版本、账号路由与权限。

现状：以上路线均未实施；没有 Kimi 预设或一手 API 兼容证据；`HostKey` 已比较 preset、env、SearchPath。

## 同类产品交互借鉴（2026-10-22）

- Tern（闭源，官方摘要）：worktree + 集中审阅队列。可借鉴「回复已读不等于改动已处理」；ZJ 已有改动卡片、块级接受／拒绝、上一处／下一处。跨会话审阅队列未做。
- T3 Code（官方摘要）：`turn/steer` 与下一轮 queue 分离。ZJ 已有 steering，缺的是输入区可见语义；完整队列需要新的持久状态机，未做。
- Herdr：通知语义状态与展示标签分离；ZJ 已有 `RowStatus` 与「下一个待审批」。后台常驻 server 与 ZJ 退出回收目标冲突，不引入。
- MyGo：只能作框架约束参考（稳定实体身份、generation 校验），身份未最终确认，不迁移。
- 许可证只作概念参考，未读固定 commit 的 LICENSE；不复制代码、图标或视觉资产。

现状：三项最小交互已落地；队列、worktree、常驻 server、任务看板均延后。

交互参考来源：[Tern](https://www.tern.build/)、[T3 Code](https://github.com/pingdotgg/t3code)、[Herdr](https://github.com/herdrdev/herdr)。

## 未决与不确定性

- 未做真实登录、付费模型、端到端取消后的进程清理、数据地区政策与资源增量的实测。
- JCode／Pi／Harness 的最新版本、许可与供应链未在 2026-10-22 复核；2026-10-22 的 WebFetch 均被域名校验阻断。
- Kimi 只有入口链接，没有一手兼容证据。
- 旧报告的「首版排除 Agent」已被 ADR 0004／0009 取代；引用旧文时不要沿用该范围判断。
