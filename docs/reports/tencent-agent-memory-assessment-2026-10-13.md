# TencentDB Agent Memory 与 ZJ／zvec-grep 接入评估

核对日期：2026-10-13。问题：能否用于 ZJ，是否与工作区的 zvec-grep 重叠，是否符合简洁、低内存和外部工具自行管理的边界。

腾讯项目固定于默认分支 `feat/server_team` 的提交 `b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4`（提交日期 2026-10-12）。以下事实来自该提交的官方 README、安装说明、模块源码和依赖清单。ZJ 依据当前工作树及 2026-10-09 的既有 zvec-grep 调查；旧资源采样不作为腾讯项目的资源预测。本次没有安装服务、配置 Agent、发送模型请求、修改索引或测量运行时内存。

## 结论

可以评估作为原生 Agent 使用的外部记忆服务，现阶段不建议内置进 ZJ，也不建议替换 zvec-grep。主要增量是跨会话、跨 Agent 复用偏好、事实、决策和 Skills；Wiki／CodeGraph 与现有代码、文档检索部分重叠。当前主推接入方式是透明模型代理，需要修改原生 Agent 的模型端点配置，接入工作不应只按「添加一个 MCP」估算。以上是基于接口与源码的可行性判断，尚未验证 ZJ 的完整运行链路。

## 腾讯项目实际提供什么

- 四类资产：Chat Memory、Skill、LLM-Wiki、CodeGraph。Chat Memory 从原始对话 L0 提炼到 L1 原子事实、L2 场景及 L3 Persona；记忆检索使用 BM25、向量和融合排序，输出有条数、字符预算及超时限制。[1]
- Memory Hub 提供团队、用户、Agent、任务、资产绑定和 ACL；官方明确当前为 Team Memory Beta，自动资产路由还在迭代。[1]
- Wiki 把文档加工成结构化页面和链接图；CodeGraph 索引代码符号、文件和调用关系，支持 callers/callees 与影响范围查询。[1][2]
- 推荐部署启动 `memory-core`、`memory-hub`、`proxy` 三个容器；还提供 Knowledge 服务端口。安装需要分别配置记忆处理模型与代理上游模型，两组可复用。[3]
- 核心是 Node.js／TypeScript 服务。MemoryCore 要求 Node ≥ 22.16.0，依赖 `sqlite-vec`；Knowledge 要求 Node ≥ 22，并使用 `@colbymchenry/codegraph`、SQLite 及 MCP SDK。它不是可直接链接进 ZJ 的 Rust 库。[4][5]
- 当前默认存储为本地 SQLite，MongoDB 可选且标为实验性；Proxy 文档支持关闭 Redis、改用 SQLite 状态存储。因此腾讯云数据库不是必选项，但模型处理与模型上游仍须配置。[3][6]
- 固定提交的 LICENSE 是 MIT，保留版权及许可声明的条件明确。GitHub 元数据的 `NOASSERTION` 不能替代许可正文。[7]

## 接入 ZJ 的路径与限制

官方当前推荐 `Agent → MemoryProxy → 上游模型`；Proxy 另行调用 MemoryCore，完成初始化、记忆／Skill／Knowledge 注入和对话回写。Memory 的存储、提炼和检索留在外部服务。[1][6]

Claude Code 文档通过 `ANTHROPIC_BASE_URL`、业务用户的 `ANTHROPIC_AUTH_TOKEN` 和上游模型配置接入。Codex 文档要求配置自定义 `model_provider`、`wire_api = "responses"`、Proxy `base_url` 和业务用户 token；首次初始化必须使用 Plan 模式，否则可能跳过记忆初始化。[8][9]

ZJ 的内置桥接仍调用本机 `claude -p` 和 `codex app-server`，可以沿原生 CLI 的配置层评估这条路径；理论上不需要把 MemoryProxy 逻辑加入编辑器。但尚未验证以下具体兼容点：

1. ZJ 的非交互桥接、Plan 模式和 Team／Agent／Task 初始化表单能否完整通过。
2. 流式回复、工具、审批、追加指令、恢复和取消是否继续符合现有行为。
3. 当前使用的 Claude／Codex 订阅登录能否用于该代理上游。官方配置要求自己的模型上游地址与凭据，不能直接推定原生订阅通道仍可沿用。

Knowledge 模块另有 STDIO MCP：它是独立 Node 进程，把工具调用转发到 Knowledge HTTP API。该模块可作为代码／文档检索入口，但这条 MCP 路径不能直接等同于上述 Chat Memory 自动采集与注入闭环。[2][10]

ZJ 当前 ACP 新会话传入的 `mcp_servers` 为空，没有 ZJ 统一管理的 Memory Hub 或代理部署入口；Agent 自己的配置与 ZJ 生成的会话历史是不同层。接入新服务不会自动导入 ZJ 的 SQLite 历史；腾讯支持导入原生 Agent 会话，但是否直接支持 ZJ 格式尚未验证。[1][11]

## 与 zvec-grep 的重叠

这里区分向量存储库 zvec 与当前通过 MCP 使用的应用 zvec-grep。本次比较对象主要是后者。

| 能力 | 当前 zvec-grep／ZJ | 腾讯 Agent Memory | 判断 |
| --- | --- | --- | --- |
| 查代码、文件和项目文档 | zvec-grep 负责工作区检索；ZJ 自带文件搜索使用路径模糊匹配与文本搜索 | Wiki、CodeGraph 提供代码／文档检索 | 部分重叠，可能重复维护数据和索引 |
| 查调用关系、影响范围 | zvec-grep 可以检索相关代码；不据此认定它具有等价关系图 | CodeGraph 提供显式关系索引 | 有增量，但不是全面替换的证据 |
| 保存、搜索旧对话 | ZJ 使用 SQLite＋FTS5 保存会话与消息 | L0 保存原对话 | 数据内容会重叠，目的和控制层不同 |
| 从对话提炼偏好、约束、决策 | ZJ 自己的发送路径没有自动提炼／召回步骤；原生 CLI 的能力由各自实现负责 | Chat Memory 提炼、召回并注入 L1–L3 | 是主要增量 |
| 提炼 Skills、分配给多个 Agent、人工管理资产 | 不属于 zvec-grep 的主要职责 | Skill／Hub 支持这些流程 | 是主要增量，也带来额外管理工作 |

CodeGraph 当前主要通过 `git clone` 导入仓库，官方优先支持公开 HTTPS 仓库，私有仓库和 SSH 凭据仍在完善。可选 Auto-Sync 默认关闭；开启后定时拉取 Git 更新并重建索引，默认扫描间隔 10 分钟。这与直接围绕当前工作目录检索的用途不同，不能假定对未提交修改、私有工作区或更新及时性已经等价。[1][2]

## 资源与使用建议

官方 README 的 PersonaMem 48% → 76% 是用户信息理解／应用效果指标，不能作为内存、CPU、延迟或 token 节省量。本次已读官方材料没有提供与当前 zvec-grep 在同一机器、同一负载下的 physical footprint 对比；没有可验证的「更省内存」结论。[1]

默认三容器方案、Node 运行时、图索引、记忆提炼和模型调用都形成额外成本；只根据依赖和架构判断，它不适合作为 ZJ 默认附带的运行环境。远端部署可以把部分服务成本移出本机，但也改变数据流与运维方式，需在明确要试用时评估，不能记作已验证收益。

建议先保留 zvec-grep 与 ZJ 本地历史，若要尝试则只选一个工作区、一种 Agent 和少量 Chat Memory／项目决策；先不导入 CodeGraph／Wiki，减少重复索引。对比新会话是否少重复解释、记忆是否准确、额外模型开销及本机总进程占用，再决定是否扩大。

继续遵循此前边界：ZJ 提供原生 Agent 会话、界面和通用参数／环境变量；代理、索引、记忆抽取、部署和内部缓存由外部工具自身负责。通用连接不意味着自动兼容全部代理或协议扩展，不为腾讯项目增加专属容器管理和资产索引逻辑。[12]

## 本地证据与来源

[1] [官方 README](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/README.md)：L25（Beta）、L57（Proxy 接入）、L94（资产提炼）、L100（Chat Memory）、L240（分层和检索）、L275（PersonaMem）、L282（仓库支持边界）、L304（版本／路线）。

[2] [MemoryKnowledge README](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/MemoryKnowledge/README.md)：L13（克隆／图索引）、L36（MCP）、L119（Auto-Sync）。

[3] [INSTALL](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/INSTALL.md)：L15（三容器部署）、L33（两组模型配置）、L68（默认 SQLite、可选 MongoDB）。

[4] [MemoryCore package.json](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/MemoryCore/package.json)：L100（Node）、L125（sqlite-vec）。

[5] [MemoryKnowledge package.json](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/MemoryKnowledge/package.json)：L23（Node）、L28（CodeGraph 依赖）。

[6] [MemoryProxy README](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/MemoryProxy/README.md)：L3（代理作用）、L27（注入与回写）、L76（运行依赖）、L102（上游凭据）、L105（本地 SQLite 替代 Redis）。

[7] [LICENSE](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/LICENSE)。

[8] [Claude Code 接入](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/agents/claude-code/README.md)：L14（环境变量）、L37（地址和凭据）、L65（初始化表单）。

[9] [Codex 接入](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/agents/codex/README.md)：L15（自定义 provider）、L23（base URL）、L38（首次 Plan 模式）、L92（Default mode gate）。本报告只描述该项目的接入要求，不将其旧客户端说明视为所有当前 Codex 版本的通用结论。

[10] [Knowledge MCP 源码](https://github.com/TencentCloud/TencentDB-Agent-Memory/blob/b1f79fc4714b1e20a1ab94ec6b98e50100af6dc4/MemoryKnowledge/src/mcp/server.ts)：L2（用途）、L15（STDIO）、L63（HTTP 转发）。

[11] ZJ 本地实现：`crates/agent_bridge/src/claude.rs:471`、`crates/agent_bridge/src/codex.rs:533`（原生 CLI）；`crates/agent_client/src/client.rs:1083`（空 MCP 列表）；`crates/app/src/workbench/agent/turns.rs:210`（正文／附件发送）；`crates/agent_history/src/schema.rs:65`、`crates/agent_history/src/lib.rs:582`（FTS5／历史搜索）；`crates/app/src/files.rs:293`、`crates/app/src/text_search.rs`（编辑器自身搜索）。对 `crates`、Cargo.toml、Cargo.lock 精确查询 `zvec` 未发现内置依赖或调用。

[12] `docs/reports/agent-memory-2026-10-09.md:107`、`:252`、`:270`：既有外部 MCP／守护进程结构与工具责任边界。这些是 2026-10-09 的证据，不代表已重新测量当前版本。
