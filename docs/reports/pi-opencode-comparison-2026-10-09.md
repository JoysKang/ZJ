# Pi 与 OpenCode：功能、ZJ 接入与内存

检查日期：2026-10-09。对象为用户指定的 `earendil-works/pi` 与 `anomalyco/opencode`，重点是简洁程度、内存和 ZJ 的接入成本。当前正式发布版本分别为 Pi 1.1.0（10 月 7 日 UTC）与 OpenCode 1.18.35（10 月 6 日 UTC）；功能优先核对正式版，公开内存结果保留原测试版本。

本次未安装或运行两个候选，也未改 ZJ 源码。本机未发现可直接使用的 Pi/OpenCode 安装；下文内存来自原测试作者公开的数据与方法，不是本机测量，也不是 Pi/OpenCode 官方性能承诺。

## 结论

**历史实测倾向 Pi 更省内存；OpenCode 接入当前 ZJ 更直接。当前版本、实际任务及 ACP 完整进程树的内存胜负尚未确定。**

- 若要尽快添加一个功能完整的 Agent，优先验证 OpenCode 的原生 `opencode acp`。
- 若最看重简洁与多会话内存，Pi 值得作为下一项对照测试；已有两类公开测量支持这一方向，不能只凭“极简”宣传判断。
- 不宜现在把 ZJ 改成 Pi 的宿主框架，也不宜仅凭旧版 CLI 测试把 Pi 列为已验证的低内存默认选项。

## 项目身份与运行时

`earendil-works/pi` 是原 `badlogic/pi-mono` 的迁移后项目，不是另一个同名 Agent。官方 2026-05-07 说明，从 0.74.0 开始 npm scope 由 `@mariozechner` 改为 `@earendil-works`，旧 scope 最后版本为 0.73.1；CLI 仍叫 `pi`。来源：[官方迁移说明](https://pi.dev/changelog/2026/5/7/pi-has-a-new-home)。

Pi 1.1.0 是 TypeScript/JavaScript 项目：npm 版要求 Node.js >=22.19，官方 standalone 版通过 `bun build --compile` 打包，可不另装 Node，但仍包含 JavaScript 运行时。OpenCode 当前源码也为 TypeScript，发行构建使用 Bun 编译。两者都不能归类为 Rust/Go 原生 Agent；单个可执行文件也不等于没有 JavaScript 运行时。

来源：[Pi 1.1.0 README](https://github.com/earendil-works/pi/blob/v1.1.0/README.md)、[Pi 独立版构建](https://github.com/earendil-works/pi/blob/v1.1.0/scripts/build-binaries.sh)、[OpenCode 构建脚本](https://github.com/anomalyco/opencode/blob/v1.18.35/packages/opencode/script/build.ts)、[Pi release](https://github.com/earendil-works/pi/releases/tag/v1.1.0)、[OpenCode release](https://github.com/anomalyco/opencode/releases/tag/v1.18.35)。

## 功能与维护成本

| 方面 | Pi 1.1.0 | OpenCode |
| --- | --- | --- |
| 默认使用方式 | 默认启用 `read/bash/edit/write`，通过扩展增加工作流 | 内置 build、plan，以及 general/explore 等子 Agent |
| 扩展 | 进程内 TypeScript 扩展、skills、prompt templates、packages | plugins、skills、自定义工具与 Agent 配置 |
| MCP | 已内置 stdio 和 streamable HTTP；不能沿用旧版“Pi 无 MCP”的印象 | 已有 MCP 接入，继续由 OpenCode 管理 |
| 权限交互 | 核心不提供内置逐工具权限规则；项目 trust 控制资源加载，扩展可实现工具拦截与确认 | 内置 `allow/ask/deny` 和细粒度规则；默认多数工具为 allow，不能描述为默认逐项审批 |
| 运行中追加输入 | 官方 RPC 有 `steer`、`follow_up`，支持图片 | ACP 入口可直接接入；具体运行中追加能力应按版本协商、实测，不能由“支持 ACP”推定 |
| 编辑器接口 | 官方提供自有 JSONL RPC 和 TypeScript SDK；当前核对文档未给出原生 ACP 模式 | 官方 `opencode acp`，JSON-RPC stdio |

Pi 的“简洁”主要体现在默认工具和工作流选择；当前版本已有 MCP、codemode、扩展、持久会话等能力，不能从四个默认工具推导启动进程很小。Pi 会在会话启动时后台连接所有启用的 MCP server，外部工具仍可能是主要内存来源。

Pi 的 `steer` 是等当前 assistant turn 的工具执行完成后、下一次模型调用前注入；`follow_up` 等当前工作结束。它不是立刻中断正在生成的 token。该能力经 ACP adapter 接入后，还要验证 adapter 与 ZJ 的映射。

来源：[Pi 1.1.0 CLI](https://github.com/earendil-works/pi/blob/v1.1.0/packages/coding-agent/docs/cli.md)、[Pi 1.1.0 MCP](https://github.com/earendil-works/pi/blob/v1.1.0/packages/coding-agent/docs/mcp.md)、[Pi RPC commands](https://github.com/earendil-works/pi/blob/f1b2e77f5b13b2a199b1052cb79c235451afe7d7/packages/coding-agent/docs/rpc-commands.md)、[OpenCode agents](https://opencode.ai/docs/agents/)、[permissions](https://opencode.ai/docs/permissions/)、[plugins](https://opencode.ai/docs/plugins/)、[MCP](https://opencode.ai/docs/mcp-servers/)。权限规则与项目 trust 都不能直接视为操作系统沙箱。

## 接入 ZJ 的实际差别

OpenCode 的直接路径是：`ZJ ACP 客户端 → opencode acp`。不需要再添加一个专门做协议翻译的 Agent 进程。来源：[官方 ACP 文档](https://opencode.ai/docs/acp/)。这不代表 OpenCode 不会再启动 MCP、语言服务器等工具。

Pi 官方 RPC 路径为 `pi --mode rpc`，使用自有 JSONL command/event 协议，与 ACP JSON-RPC 不同。TypeScript SDK 可以在 Node/Bun 宿主中直接创建 session，但不能直接链接进 Rust ZJ 而不增加运行时或桥接工作。来源：[Pi 1.1.0 RPC](https://github.com/earendil-works/pi/blob/v1.1.0/packages/coding-agent/docs/rpc.md)。

官方 ACP Registry 当前列出的是社区维护的 `svkozak/pi-acp@0.0.34`。其实现为：`ZJ → pi-acp → 每个会话一个 pi --mode rpc --no-themes 子进程`。因此 Pi CLI 单体内存不能直接充当 ZJ 使用它的总占用；还需算 adapter 和多个 Pi 会话。

这个 adapter **确实支持 ACP `requestPermission`**：它将 Pi 扩展产生的 `select/confirm` UI 请求转成 ACP 请求，再把答案传回扩展。这不等于 adapter 默认为每个 `bash/write/edit` 建立完整权限规则；使用哪种工具审批扩展仍需要另外配置和验证。不能因为 Pi 核心没有内置权限系统，就断言 adapter 无法显示审批。

来源：[Registry 条目](https://github.com/agentclientprotocol/registry/blob/669691cfb847b6e7c911326aabea14e8586c5ae1/pi-acp/agent.json)、[adapter README](https://github.com/svkozak/pi-acp/blob/04d0a15d6bb42bb65cce5c9caedcdcb2cdcb6905/README.md)、[会话创建与扩展确认转发](https://github.com/svkozak/pi-acp/blob/04d0a15d6bb42bb65cce5c9caedcdcb2cdcb6905/src/acp/session.ts)、[Pi 子进程启动](https://github.com/svkozak/pi-acp/blob/04d0a15d6bb42bb65cce5c9caedcdcb2cdcb6905/src/pi-rpc/process.ts)。

社区另有通过 SDK 在同一进程创建 Pi sessions 的 adapter，因此不是所有 Pi ACP 方案都必然有上述进程数。此次未确认这些变体的发布身份、完整兼容性和内存，不将它们列为推荐替换。来源：[一种 SDK 内嵌实现](https://github.com/Afrowave/pi-acp-1)。

对 ZJ，先采用现有通用 ACP 配置评估即可。若 Pi 的实际收益足够大，再决定采用维护可靠的 SDK adapter 或承担直接 RPC 桥接；不要为了未经验证的节省先增加一套专属协议维护。

## 公开内存证据

### Linux：可输入后的 CLI 基线与多会话

JCode 作者公布了同组 CLI 内存比较。脚本通过 PTY 直接启动程序，等待界面可输入后默认再等 1 秒，读取 `/proc/.../smaps_rollup` 的 PSS，并统计后代及进程组；不执行真实模型任务。

| 原测试版本 | 1 个 CLI 会话 PSS | 10 个 CLI 会话合计 PSS | 作者报告每新增会话的边际 PSS |
| --- | ---: | ---: | ---: |
| Pi 0.62.0 | 144.4 MB | 833.0 MB | 76.5 MB |
| OpenCode 1.0.203 | 371.5 MB | 3,237.2 MB | 318.4 MB |

这组结果支持“该条件下 Pi 基线更小，多会话增长更低”。局限是版本较旧、每个会话独立启动 CLI、用户环境和配置未隔离；不能套用为 ZJ 共享 ACP 进程的边际成本。测试作者开发竞争产品 JCode，这是一份可查看方法的原始公开测试，不是中立官方认证。

来源：[JCode 原始结果](https://github.com/1jehuang/jcode/blob/master/README.md)、[测量脚本](https://github.com/1jehuang/jcode/blob/master/scripts/bench_memory_cli.py)。

### macOS：脚本化长会话的历史结果

另一测试作者保留的 2026-09-19 报告在 Apple Silicon、18 核主机上，用 `proc_pid_rusage` 每 100 ms 采样，统计进程树 RSS。工作负载是 headless、mock 响应和脚本化工具调用，约 100 轮、每轮约 4 KB 回复正文和一次工具调用。

| 原测试版本 | 平均进程树 RSS | 峰值进程树 RSS |
| --- | ---: | ---: |
| Pi 0.85.1 | 181.4 MB | 227.9 MB |
| OpenCode 1.17.12 | 798.9 MB | 943.0 MB |

这组数据也倾向 Pi 更省，但每项只有一次运行、主机 load 约 10–16、Pi 稍后补跑、缓存已有预热。实际事件量也不完全相同：Pi 100 工具轮及 30 个压缩相关事件，OpenCode 99 工具轮及 1 个标题相关调用。它不是 ACP、真实模型或严格相同的业务任务。

必须使用这份历史快照中的成对数字：后续文档更新了 Pi 数据并移除了旧 OpenCode，不能把更新后的 Pi 均值 186.2 MB 与旧 OpenCode 798.9 MB 拼成新对照。来源：[原始报告固定快照](https://github.com/KonghaYao/harness-perf-benchmark/blob/ca1982da6f64fd23ed6a176e7ef90ddbae0371af/docs/perf-compare.md)。

### 能确定和不能确定的部分

两种公开测试方向一致：被测旧版本的 Pi 小于 OpenCode。但 PSS 与 RSS 口径不同，不能跨表相加或比较；上述数据也不能替代 macOS physical footprint。

Pi 已从测试的 0.62/0.85 进入 1.1，功能与实现存在明显版本差距。当前 Pi 1.1.0 对 OpenCode 1.18.35、同模型同任务、同 MCP 配置、经 ZJ ACP 的完整占用，仍没有本次建立的成对实测证据。

下一步若要决定默认推荐，应测“ZJ + adapter（如有）+ Agent + 自有工具子进程”：1/5/10 个会话、长输出、任务结束和空闲回收，并单列共享外部服务。只启动 Pi/OpenCode 不足以验证用户多 Agent、多上下文场景。

此次仅新增研究报告；验证为版本、一方文档、源码接口及原测试作者方法核对，未修改源码，因此未执行测试或打包。
