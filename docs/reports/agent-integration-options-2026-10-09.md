# ZJ 的轻量 Agent 接入方案

检查日期：2026-10-09。问题：如何更方便地接入 GitHub 上的开源 coding agents，同时保持 ZJ 简洁、低内存。范围是编辑器与 Agent 的会话接口，不接管 Agent 的模型、MCP、索引或其他工具配置。

本次核对 ZJ 源码、官方 ACP 仓库和三个 Agent 的一方文档、源码；未安装候选、运行会话或测量内存。下文的启动方式与运行时是已核对事实，接入顺序是建议，兼容性和内存排序仍待实测。

## 结论

**继续沿用 ACP，补齐“添加 Agent”的交互与轻量目录，比换一个聚合框架更适合 ZJ。** 当前基础已经具备，新增原生 ACP Agent 通常应落在启动配置和通用能力协商上，而不是为每个工具写新桥接。

推荐路径：

1. 先提供添加表单和少量已验证预设：名称、可执行文件、参数、环境变量；选择后检测和连接。
2. 后续按需读取官方 ACP Registry，把它作为目录和分发元数据来源，缓存到本地；打开编辑器不扫描或启动所有 Agent。
3. 原生 ACP Agent 直接用 stdio；Claude/Codex 保留 ZJ 现有 Rust 桥接。新的非 ACP Agent 优先采用其维护者提供的适配器，不持续扩大自研桥接名单。
4. 连接能力和资源占用分别评价：支持 ACP 能减少接入工作，不承诺 Agent 自身低内存。

这是基于当前架构与用户边界的设计建议，不是已经实现的功能。

## 当前 ZJ 已经具备什么

本地核对位置：

| 现有能力 | 源码证据 | 对后续工作的含义 |
| --- | --- | --- |
| Rust ACP 客户端，依赖 `agent-client-protocol = 2.2.0` | `crates/agent_client/Cargo.toml` | 无需换协议或引入另一个 Agent 框架 |
| `UserAgentConfig` 包含 `id/name/command/args/env`，转换为 `Launch::Binary`；支持自定义 Agent | `crates/agent_client/src/registry.rs` | 已有通用执行入口，主要缺更易用的添加界面与经过验证的配置 |
| ACP V1；声明文件读写能力，但 `terminal(false)` | `crates/agent_client/src/host.rs` | 依赖客户端 terminal 服务的 Agent 不能直接假定完整可用 |
| 按 Agent 预设与配置共享进程，支持声明了 `session/close` 的 Agent 关闭会话；默认空闲 10 分钟回收 | `crates/agent_client/src/host.rs`（AgentPool）、`client.rs`（默认空闲时长） | 新 Agent 仍需验证并发、多目录会话和关闭行为 |
| Claude/Codex 使用现有 Rust 桥接 | `crates/agent_bridge`、`docs/adr/0009-agent-bridge.md` | 这两项保留既有行为，其他 Agent 优先直连 |

“只加配置即可启动”与“在 ZJ 中全部能力可用”不同。至少应验证审批、取消、图片、会话恢复、并发，以及 Agent 实际依赖的客户端能力。运行中追加输入等扩展也应依据协商结果，不能按 Agent 名称一概开启。

## 官方 ACP 生态可复用的部分

### 协议与 Rust SDK

ACP 专门统一代码编辑器与 coding agent 的通信。官方 Rust SDK 提供客户端、Agent 和协议类型；同一仓库也提供 HTTP transport、proxy 和 conductor，但这些组件不是 stdio 直连的必要条件。

当前官方 SDK 文档把 `Client.builder()` 等入口标为稳定 V1，把 `.v2()` 标为草案 API，并要求显式启用 `unstable_protocol_v2`。ZJ 可以继续以当前稳定能力为基线；此次没有证据表明，为增加 Agent 数量必须整体升级到草案 V2 或引入代理链。

来源：[官方 Rust SDK，2026-10-08 快照](https://github.com/agentclientprotocol/rust-sdk/blob/5c41d62297eb74cce06daac8b3a487c6c453d552/README.md)。

### 官方 Registry

官方 Registry 是可获取的 JSON 目录，包含名称、说明、版本、项目地址和分发信息；分发方式包括 `binary`、`npx`、`uvx`。因此使用目录本身不要求 ZJ 再启动一个常驻服务。

需要区分分发元数据和本机启动配置：`binary.cmd` 是下载包解压后的相对命令，不保证是用户机器 PATH 中的命令；`binary` 也不表示程序由 Rust/Go 实现。自动下载会再引入校验、解压、版本管理等工作，建议先做“选择已安装的程序”，之后按需要扩展单个 Agent 的安装流程。

官方协议矩阵在 2026-10-09 记录了候选 Agent 的握手检查。握手成功只能证明相应协议入口可响应，不证明 ZJ 的所有交互兼容，也不构成内存基准。

来源：[Registry README](https://github.com/agentclientprotocol/registry/blob/669691cfb847b6e7c911326aabea14e8586c5ae1/README.md)、[分发格式](https://github.com/agentclientprotocol/registry/blob/669691cfb847b6e7c911326aabea14e8586c5ae1/FORMAT.md)、[协议矩阵](https://github.com/agentclientprotocol/registry/blob/669691cfb847b6e7c911326aabea14e8586c5ae1/.protocol-matrix/latest.md)。

建议只对用户选中的 Agent 检测可执行文件、握手和必要能力。目录更新按需进行；目录版本更新不自动升级已配置的 Agent，升级前验证兼容性。不为展示“可安装列表”启动每个 Agent，也不自动安装 Node、Python 或所有适配器。

## 值得先验证的三个 Agent

| 候选 | 已核对接入方式 | 运行时证据 | 对 ZJ 的建议 |
| --- | --- | --- | --- |
| Goose | `goose acp`，stdio ACP | Rust 实现，Registry 提供平台二进制 | 优先验证；无需增加 Node/Python 协议适配器，但尚未证明整体内存最低 |
| OpenCode | `opencode acp`，JSON-RPC stdio | TypeScript；构建脚本使用 `Bun.build` 的 `compile` | 多模型 Agent 的候选；单个可执行文件仍含 Bun 运行时，不能按文件形态判断低内存 |
| Gemini CLI | 当前官方文档为 `gemini --acp`，JSON-RPC stdio | npm 入口为 JavaScript，`engines.node >=20` | 接入路径直接；保留用户安装的运行时，不为此把 Node 嵌进 ZJ |

### Goose

官方文档明确由 ACP 客户端管理 `goose acp` 子进程生命周期。Goose 也能把其他 ACP Agent 当作 provider，但 ZJ 已有客户端，不需要借 Goose 再转发 Claude/Codex。Goose 的扩展、工具和模型仍由 Goose 配置和管理。

它是优先测试候选，因为 Rust 原生入口和 stdio 路径适合 ZJ 当前结构；这不是内存排名。其功能、依赖、启用的扩展和上下文都会影响真实占用。

来源：[Goose CLI 命令](https://github.com/aaif-goose/goose/blob/cd1da62ac539451948cb38d9e4f245ddd03a3a50/documentation/docs/guides/goose-cli-commands.md)、[Rust crate](https://github.com/aaif-goose/goose/blob/cd1da62ac539451948cb38d9e4f245ddd03a3a50/crates/goose/Cargo.toml)、[Registry 启动描述](https://github.com/agentclientprotocol/registry/blob/669691cfb847b6e7c911326aabea14e8586c5ae1/goose/agent.json)。Goose 仓库现位于 `aaif-goose/goose`；Registry 的项目地址仍使用原 `block/goose` 地址。

### OpenCode

官方文档明确 `opencode acp` 是编辑器可直接启动的 ACP 子进程。当前源码为 TypeScript，发行构建使用 Bun 编译；不能将“无需单独安装 Node”推导为“没有 JavaScript 运行时开销”。

来源：[OpenCode ACP 文档](https://opencode.ai/docs/acp/)、[构建脚本，2026-10-08 快照](https://github.com/anomalyco/opencode/blob/388406238bd5ca15564a762840a2362c3a45bd9c/packages/opencode/script/build.ts)。

### Gemini CLI

当前官方文档使用 `--acp`，不宜只沿用早期示例中的 `--experimental-acp`。npm 包声明 Node >=20；这是 Agent 自身运行时，不是 ZJ 的依赖。不同已安装版本仍应通过实际命令和握手检测，不能仅根据当前主分支生成全版本通用配置。

来源：[Gemini ACP 模式](https://geminicli.com/docs/cli/acp-mode/)、[package.json，2026-10-08 快照](https://github.com/google-gemini/gemini-cli/blob/2ce1a6963e9e53a04afaf76111e4527cfa7c5dd7/package.json)。

上述源码快照和当前文档说明接入方向，不代表三个候选已经通过 ZJ 实际会话验证，也不保证主分支所有功能已进入用户安装的正式版本。

## 不建议增加通用终端网关

`coder/agentapi` 曾用 HTTP API 和内存中的终端模拟器统一驱动多个 coding agent。但其官方 README 当前明确标为已弃用、不再维护。对于已有 ACP 客户端的 ZJ，它还增加终端状态、网关进程和另一套交互映射，当前不推荐。

来源：[agentapi README](https://github.com/coder/agentapi/blob/main/README.md)。

MCP 工具接入、Agent 编排框架和编辑器 Agent 会话接口也不应混为一层。ZJ 只处理会话、审批、界面更新和自有进程生命周期；Agent 使用哪些外部工具，以及工具如何共享模型、索引或 daemon，继续由外部工具负责。

## 低内存的可验证边界

采用上述方案，合理预期是**减少 ZJ 为接入 Agent 增加的额外层**：不增加常驻目录服务，不为原生 ACP Agent 增加协议适配器，不提前启动未使用的 Agent。不能据此承诺整个进程树的具体节省量。

验证每个候选时应使用同一仓库、相同启用工具与相近任务，分别记录：

- 仅打开 ZJ、尚未启动 Agent；一个会话；多个并发会话。
- 冷启动、长输出和多次工具调用后的峰值；任务完成和空闲回收后的占用。
- ZJ、Agent 主进程、适配器及其工具子进程各自的 physical footprint，同时观察共享外部服务，避免重复归因。
- 多会话是否串扰，跨工作目录是否隔离；取消、审批和退出后是否正确回收。

没有这些数据，本次不能确定 Goose、OpenCode、Gemini 谁在用户实际任务中最省内存。语言、安装包大小、是否单 binary、支持 ACP，都不能替代同负载测量。

此次仅新增研究报告；验证为一方资料、源码接口和文档差异检查，未修改源码，因此未运行测试或打包。
