# JCode 的功能完整性与 ZJ 接入评估

检查日期：2026-10-09。对象为 `1jehuang/jcode`，以正式发布的 **v0.93.0**（2026-10-09）文档和源码为主。问题是它是否比 ZJ 当前 Agent 实现更完善，是否值得接入或替换现有方案。

本次没有安装、登录或运行 JCode，没有修改 ZJ 源码。功能结论来自发布版接口与实现；内存数据来自项目作者的公开测试，不是本机复测。仓库默认分支是 `master`，不能把设计文档和主分支新增能力全部视为已发布、已验证能力。

## 结论

**JCode 是功能相当完整、值得试用的 Rust Agent 引擎，但不能据此认定它比“ZJ + Codex/Claude”整套体验更完善。** 它原生支持 ACP，接入基础聊天不困难；当前 ACP 对审批、运行中追加输入和结构化改动的覆盖不足，会影响 ZJ 已有的关键交互。

建议把它评估为第三个可选 Agent，先保留 Codex/Claude。不要直接用 JCode 替换当前桥接，更不要把 JCode 的完整引擎和生态嵌进 ZJ。

JCode 直接接模型、自己实现工具循环和会话管理；ZJ 当前是编辑器加 Rust 协议桥，实际运行的工具、模型交互和上下文逻辑仍由 Codex/Claude CLI 承担。换成 JCode 是更换 Agent 引擎，不只是换一层更省内存的翻译程序。同一个模型或登录账号，也不代表原生 Agent 行为完全相同。

来源：[JCode v0.93.0](https://github.com/1jehuang/jcode/releases/tag/v0.93.0)、[ZJ ADR 0009](../adr/0009-agent-bridge.md)。

## JCode 引擎具备的能力

| 方面 | v0.93.0 已有证据 | 边界与对 ZJ 的意义 |
| --- | --- | --- |
| 模型与账号 | Claude、OpenAI、Gemini、Copilot 等登录；API key、OpenAI-compatible/Anthropic-compatible 配置、多账号选择 | 模型接入覆盖广；不等于完整复用各原生 CLI 的提示、工具和权限语义 |
| 基础工具 | 文件读写、编辑、检索、shell、工具批处理、会话搜索等；可选 codemode | 已是完整工具执行引擎；ZJ 当前通过原生 CLI 也有这些能力，不需要自行重写 |
| MCP | 全局和项目配置，兼容部分 Claude 配置，并可导入 Codex 配置 | **当前仅支持 stdio；HTTP/SSE 项会被跳过**，是明确的兼容短板 |
| Skills | `SKILL.md`；项目 `.jcode/.agents/.claude`；全局技能和首次 Claude/Codex 技能导入 | 具备既有技能的迁移入口；不是每种扩展机制完全等价 |
| 多 Agent 与规划 | 共享 daemon、swarm、DM、文件变化通知、任务 DAG、普通与深度 swarm、可选 worktree | 这是突出能力；不保证所有并行修改都能自动正确合并 |
| 上下文 | 手动和自动压缩、上下文超限恢复、历史搜索 | 实际实现中按 provider 能力判断；压缩不代表所有历史内存立即释放 |
| 历史与恢复 | 自有会话恢复、保存；导入 Claude/Codex/OpenCode/Pi 会话 | 导入是转换为 JCode 会话，不是接管原生进程继续执行 |
| 运行中输入 | 原生会话有安全点注入和排队机制 | JCode 原生支持，不代表其 ACP 入口已映射给 ZJ |
| 图片 | 原生图片处理，ACP 也声明 image 能力 | 具体模型支持及 ZJ 发送/恢复仍需实测 |
| 扩展与其他功能 | lifecycle hooks、浏览器工具、memory、ambient、自开发和原生终端 UI | 功能面很宽；这部分不应成为 ZJ 必须接管的职责 |

来源：[发布版 README](https://github.com/1jehuang/jcode/blob/v0.93.0/README.md)、[工具注册](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-app-core/src/tool/mod.rs)、[skills 实现](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-base/src/skill.rs)、[压缩实现](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-app-core/src/agent/compaction.rs)、[恢复行为](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/RESUME_BEHAVIOR.md)。

Swarm 架构采用乐观协作，没有对所有文件操作加锁。文件变化通知、任务协调和可选 worktree 能帮助并行工作，但正确整合仍依赖 Agent 的判断与验证。不能将 README 的“自动解决冲突”概括为保证。来源：[发布版 Swarm 架构](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/SWARM_ARCHITECTURE.md)。

Rust 实现指 JCode 核心；它调用的 MCP、浏览器和其他工具仍可启动 Node、Python 或额外服务，不能承诺整个进程树没有这些运行时。也不能假定所有 MCP server 都自动跨会话共享。

MCP 仅 stdio 对用户当前场景尤其重要：之前通过外部工具的直接 HTTP 连接减少每会话转发进程的方案，不能未经核对直接搬到 JCode。是否引入额外转发或使用别的工具，属于 JCode/工具配置的取舍，不应让 ZJ 为每种工具补适配。

## ACP 接口存在，但覆盖还不完整

发布版 `jcode acp` 已实现初始化、新建、加载、恢复、prompt、取消、关闭、模型及 reasoning 配置等入口，声明图片和嵌入上下文支持。它不是只有 TUI、必须靠抓取终端输出才能接入的工具。

但“引擎有功能”与“编辑器通过 ACP 能得到功能”需要分别判断：

| ZJ 当前关心的行为 | JCode v0.93.0 ACP 核对结果 | 影响 |
| --- | --- | --- |
| 基础流式聊天、工具进度 | 有文字、工具输入/输出及 usage/config 映射 | 可做基础接入验证 |
| 运行中继续输入引导 Agent | 未声明 ZJ 使用的 steering 扩展；运行中再次 prompt 返回 already processing | 不能直接保留 ZJ 当前的运行中追加体验 |
| 工具人工审批 | ACP 文件未实现 `session/request_permission`；原生 harness API 另有 PermissionRequest | 原生权限事件不会自动成为 ZJ 审批卡片 |
| 文件修改定位和逐块审阅 | EventMapper 未生成 ACP diff、locations，也未走客户端 fs 读写委托 | ZJ 依赖这些事件的变更卡片和 hunk 审阅不能假定保持完整 |
| 登录 | `authMethods` 为空 | ACP 握手不提供完整登录引导，需要预先配置 provider |
| MCP 传入 | HTTP/SSE 能力声明为 false | 不能视作支持通用远程 MCP 的入口 |

来源：[发布版 ACP 实现](https://github.com/1jehuang/jcode/blob/v0.93.0/src/cli/acp.rs)。本地对照：`crates/agent_client/src/client.rs` 对 ACP diff/locations/fs 事件的处理，以及 `crates/agent_client/src/thread.rs` 的变更记录。表格是当前接口覆盖判断，不是声称 JCode 引擎完全没有审批、steering 或 diff 能力。

专有 harness API 和 SDK 提供更丰富事件，但选择它们意味着为 ZJ 新增接口映射、生命周期和兼容性维护。只为增加一个候选，应先评估能否在 JCode 的 ACP 实现补齐必要事件，或暂时明确其功能限制。

## 缺失能力的公开计划核查

补充检查日期：2026-10-09。问题是 ZJ 当前已有、JCode ACP 尚未开放的行为，是否已经进入 JCode 的后续计划。依据公开 issue、评论、相关提交、正式版 SDK 与当前主分支；未向维护者发消息或提交 issue。

**能确认 JCode 在持续完善 ACP，但没有找到这些缺口全部补齐的明确排期。** 有社区需求、fork 实现和原生接口基础，不等于维护者已接受，也不等于未来 ACP 版本已经承诺支持。

| 行为 | 当前可用性与公开进展 | 能否视为已确定计划 |
| --- | --- | --- |
| 工具人工审批 | ACP 没有 permission 映射；普通交互执行权限问题 #568 仍 open，标为 `triage: needs-decision`、`autonomous: no`，无 milestone | 已识别问题，仍待维护者决定；不能承诺何时实现 |
| 运行中追加指令 | 原生引擎及正式版 Rust SDK 已有 `soft_interrupt_with_images`；ACP 仍拒绝同会话第二个 prompt | 是当前接口覆盖差异，未找到 ACP steering 的明确实现计划 |
| 文件定位与逐块审阅 | ACP mapper 没有 Diff / locations / 客户端 fs；当前主分支没有补上 | 未找到针对输出给编辑器的完整审阅计划；工具自身会编辑、TUI 会显示 diff 不代表 ACP 已提供可靠审阅事件 |
| 交互提问 | #1600 / #1624 有社区 `ask_user` fork 和测试；#1600 仍 open、needs-decision。方案只让 TUI opt-in，SDK/ACP 不接 chooser、降级普通文字提问 | 有具体提案，尚未合入；即使该方案原样合入也不会自动补齐 ZJ 的提问卡片 |
| HTTP / SSE MCP | #761 有贡献者声称完成并测试的 `feat/mcp-http-sse` fork；issue open、needs-decision、无 milestone；#1129 仍是开放请求 | 有社区实现基础，但没有查到维护者接受和发布时间 |
| 编辑器退出后的自有进程回收 | 正式版 Rust SDK `launch` 已提供私有 daemon + API bridge，Drop 清理自有进程；普通 `jcode acp` 仍复用共享 daemon | 现在已有替代入口，不能描述为 ACP 将来会改成退出即关闭全局 daemon |

来源：[审批问题 #568](https://github.com/1jehuang/jcode/issues/568)、[提问主提案 #1600](https://github.com/1jehuang/jcode/issues/1600)、[提问的 ACP 限制 #1624](https://github.com/1jehuang/jcode/issues/1624)、[HTTP MCP #761](https://github.com/1jehuang/jcode/issues/761)、[远程 MCP 请求 #1129](https://github.com/1jehuang/jcode/issues/1129)、[正式版 ACP](https://github.com/1jehuang/jcode/blob/v0.93.0/src/cli/acp.rs)、[正式版 Rust SDK](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-sdk/src/client.rs)、[SDK 私有启动与回收](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-sdk/src/launch.rs)。fork 的实现与测试状态是贡献者报告，本轮未运行或独立复测。

公开 README 的 planned features 主要讨论多 Agent 版本管理与构建速度，未列出这里的 ACP 审批、steering 或完整文件审阅。这只是公开计划列表的范围，不能据此推断作者永远不会支持。来源：[核查时的 README 计划项](https://github.com/1jehuang/jcode/blob/04c7d2b04f57aedcba389cbd81afe1debd3690e0/README.md#other-planned-features)。

维护者曾在 ACP 请求 #127 中表示会实现，之后于 2026-05-31 明确宣布基础 ACP 已交付，并邀请继续提交包括 permission mapping 在内的具体缺口。这是对 ACP 方向的支持和接受反馈的意愿，不能扩展为所有交互都会补齐的承诺。后续确有模型/思考强度、slash commands、usage 和恢复修复提交。来源：[维护者交付回复](https://github.com/1jehuang/jcode/issues/127#issuecomment-4588507751)、[模型控制与命令提交](https://github.com/1jehuang/jcode/commit/b544bc18f333ee66a77cc0048181f8306c8a1f02)、[usage 修复](https://github.com/1jehuang/jcode/commit/b17a1077f3d1240fd7445062828ec7424d75a7de)。

需要识别自动分诊：#568、#566 和 #1624 的部分回复虽然显示维护者账号，但末尾明确标注由 JCode agent 自动分诊、代维护者发出。`needs-decision` 的含义是等待决策，不是已承诺执行。类似地，issue 仍 open 也不证明功能仍缺失：#851 仍开放，但命令广告与分发已有后续代码，应以当前实现为准。来源：[审批分诊回复](https://github.com/1jehuang/jcode/issues/568#issuecomment-5073682048)、[目录权限分诊回复](https://github.com/1jehuang/jcode/issues/566#issuecomment-5073680687)、[命令请求 #851](https://github.com/1jehuang/jcode/issues/851)。

本次直接比较 `master` 与 `v0.93.0` 的 `src/cli/acp.rs`，两者 blob SHA 均为 `f52404403fa93691f9a462ec3fbd30e7896e7e64`；没有出现「正式版尚缺、主分支已经补齐」的差别。这个结论只针对该 ACP 文件，不能外推成所有模块均无变化。

如果希望更早接入，专用 Rust SDK 的运行中注入、权限回复和私有启动已可作为候选，不必为了这些能力再引入 Node。但 ZJ 需要新增事件与生命周期映射，而且 SDK 中存在权限事件并不证明普通交互已有默认完整审批策略；累计 edit stats 也不能代替逐块审阅所需的变更前后内容。适合按当前实际能力评估一个可选接入，不能把尚未确立的未来计划作为默认替换依据。

## 进程生命周期也需要区分

`jcode acp` 会确保共享 daemon 已启动，并复用它。ACP close 取消会话并清理连接侧记录，不等于立即结束全局 JCode 服务。全局 daemon 可能还服务其他终端、客户端或 workers；默认在客户端与 workers 都退出后按约 5 分钟空闲策略回收。

因此不能承诺关闭 ZJ 就同步终止所有 JCode 进程，也不应由 ZJ 杀掉用户共享的 daemon。SDK 的 launch 路径可以管理私有 daemon 和 API bridge，可作为隔离验证候选，但这又是另一种进程拓扑，内存与退出行为要重新测量。

来源：[服务架构](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/SERVER_ARCHITECTURE.md)、[ACP 实现](https://github.com/1jehuang/jcode/blob/v0.93.0/src/cli/acp.rs)、[官方 SDK](https://jcode.sh/sdk)。

## 权限保护有实现，但不等同于强隔离

JCode 发布版有默认的 shell 破坏性命令检查：危险目标如根目录、home、凭据目录等可被硬拒绝；不确定的破坏性行为要求模型补充 justification 后重试。这个“确认”是模型反思，不是自动请求用户手动批准。底层实现明确说明它是防御措施，**不是 sandbox**。

通用 `pre_tool` hook 可在执行前拦截，退出码 2 表示阻止。但它是可选配置，超时、启动失败或其他异常默认 fail-open。不能把可插入策略脚本描述成已具备 Codex/Claude 等价的默认权限与操作系统隔离。

Ambient 模式确实有持久审批队列和 `request_permission` 工具；工具只在 ambient 专属注册路径启用。`SAFETY_SYSTEM.md` 在发布版仍标为 Design，不能照着设计文档把全部通知通道、普通会话审批和隔离视为已完成。

来源：[bash gate](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-app-core/src/tool/bash_destructive_gate.rs)、[风险分类及限制](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-command-risk/src/lib.rs)、[hooks 合约](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/HOOKS.md)、[ambient 工具注册](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-app-core/src/tool/mod.rs)、[安全设计文档](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/SAFETY_SYSTEM.md)。

## 内存值得关注，但不能直接套用到 ZJ

除旧的交互 CLI 基线（关闭本地 embedding 后，1/10 会话 PSS 为 27.8/117.0 MB）外，JCode 发布版 README 还提供了更接近多 Agent 的 headless 测量：

| 完成 5 轮任务后的并发会话数 | JCode 0.89.19-dev | Claude Code 2.1.267 |
| --- | ---: | ---: |
| 1 | 32.6 MB | 261.0 MB |
| 5 | 51.0 MB | 908.6 MB |
| 10 | 66.7 MB | 1,749.7 MB |
| 20 | 90.6 MB | 3,376.8 MB |

这是作者 2026-09-29 在 Linux 上的进程树 PSS 测量：两者使用 `claude-sonnet-4-6`，每会话完成列文件、读文件、检索、摘要和回复等 5 轮真实调用；JCode 共享私有 daemon，Claude 每会话一个 `claude -p` 进程。

它支持“共享 Rust 引擎有明显减少多会话内存的潜力”，但采样发生在回合完成后等待约 1 秒，**不是执行峰值**；也不是 ACP、长时间真实编辑、本机 macOS 或最新 0.93.0 的实测。不能把表中的比例当作用户实际节省承诺。

来源：[发布版 README 数据](https://github.com/1jehuang/jcode/blob/v0.93.0/README.md)、[原测试脚本](https://github.com/1jehuang/jcode/blob/v0.93.0/scripts/bench_headless_memory.py)。

默认构建当前不编译本地 ONNX/tokenizer，embedding 已改为 opt-in；持久 memory 的相关性检索走 Jev 远程服务。README 的 memory 段还保留旧 embedding 描述，应以更新的架构和构建配置为准。省下本地模型内存不代表 memory 功能完全本地。缓存上限和观测指标也不等于所有会话都有统一硬内存上限。

来源：[内存架构](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/MEMORY_ARCHITECTURE.md)、[构建特性](https://github.com/1jehuang/jcode/blob/v0.93.0/Cargo.toml)、[内存预算文档](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/MEMORY_BUDGET.md)。

ZJ 已有测量中，Rust bridge 自身约为 Claude 4.3 MB、Codex 4.9 MB；主要占用在原生 CLI。Codex 还已共享 app-server，第二会话的观察增量约 18 MB。这些本机数据与 Linux PSS 的平台、负载、口径不同，不能直接相减计算 JCode 收益。来源：[ZJ ADR 0009](../adr/0009-agent-bridge.md)、[已有 Agent 内存报告](agent-memory-2026-10-09.md)。

## 完整性和成熟度的判断

JCode 有持续发布、多个平台的二进制、专门模块及测试，已超过“极简示例 Agent”的范围。发布版包含模型接入、工具执行、压缩恢复、协作和界面功能，功能广度值得认真评估。

但本次也建立了具体缺口：MCP 仅 stdio，ACP 交互映射不全，部分文档混合设计与现状，安全检查不能代替沙箱；尚未跑同任务成功率、长会话稳定性和 ZJ 回归验证。因此没有证据证明其整体质量或任务能力已经优于当前 Codex/Claude 引擎。

适合 ZJ 的后续验证是隔离接入一个可选 JCode：先验证文字/图片/取消/恢复，再核对审批、steering、文件审阅和关闭回收；对同仓库、同模型、同工具配置测单/多会话 peak 与 idle physical footprint。通过这些检查后，再决定是否给用户推荐为低内存选项。

此次仅新增研究报告；验证为发布版资料、接口、源码与公开测试方法核对，未运行候选或修改源码，因此未执行项目测试、打包或安装。
