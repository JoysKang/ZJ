# 多模型 Agent 的低内存运行时方案

调研日期：2026-10-22。范围：Codex、Claude、DeepSeek、Kimi 在 ZJ 的运行时选择、交互保真和内存边界。仅研究；未安装代理、修改实现、读取密钥或发起付费会话。结论以当前仓库源码为一手证据；外部项目能力引用 2026-10-09 历史调研，不当作今天的最新版核验。此前 WebFetch 域名校验失败，本次未绕过，也未用搜索摘要替代一手资料。

## 建议

**保留 Codex／Claude 的现有原生桥接；为 DeepSeek／Kimi 评估同一套可复用运行时。优先验证 JCode 的接口能否补齐，Pi 作为备选，自建 Rust 工具循环暂不实施。**「同一套」首先表示共享实现与生命周期管理，不表示不同账号、权限和凭据必须塞进同一进程。验证不合格就保留现状，不以缩减审批、审阅或恢复能力换取漂亮的内存数字。

省掉适配器、回收闲置会话、避免重复启动工具服务，通常比按模型再起一个常驻 CLI 更值得先做；但冷启动、缓存命中、并发吞吐、上下文压缩和任务质量存在取舍，不能承诺完全没有牺牲。也不建议为了统一引擎而替换已经可用的 Codex／Claude 工作流。

## 当前可靠事实

| 本地证据 | 已确认行为与限制 |
| --- | --- |
| [registry.rs](../../crates/agent_client/src/registry.rs) | 内置 Claude Code、Codex、Claude Code · DeepSeek；前两者走 `Launch::Bridge`，DeepSeek 仍是 Claude bridge 加 provider 环境配置。没有内置 Kimi 预设。配置里已有的模型 ID 不证明今日 API 可用性 |
| [ADR 0009](../adr/0009-agent-bridge.md)、[claude.rs](../../crates/agent_bridge/src/claude.rs)、[codex.rs](../../crates/agent_bridge/src/codex.rs) | ZJ 的 Rust bridge 不加载 GPUI，不再需要 Node ACP 适配器。Claude 每个会话一个 `claude -p`；Codex 同一 host 下共用 app-server，各会话为 thread。保留流式、审批、steering、图片、恢复、模型配置和改动事件 |
| [host.rs](../../crates/agent_client/src/host.rs) | `HostKey` 比较完整 `AgentPreset`、已解析的 env 和 SearchPath，并非只用显示名称；全应用共用 host，通知按 sessionId 路由。跨 provider 的不同 env 当前不会自动共用 host |
| [ADR 0004](../adr/0004-agent-acp-and-history.md) | 无进行中回合、无待答审批的会话默认空闲 10 分钟后关闭；支持 loadSession 才能恢复上下文。SQLite 是 ZJ 的展示历史，不等于完整 Agent 可恢复状态。原 npm 自动安装方案已被 ADR 0009 取代 |
| [process.rs](../../crates/agent_client/src/process.rs)、[registry.rs](../../crates/agent_client/src/registry.rs) | 子进程删除特定继承变量后再覆盖 launch env，没有全面 `env_clear`；因此「环境变量不同分 host」不等于「每个 host 只持有自己 provider 的凭据」 |
| [审阅约束](../../CLAUDE.md)、[ADR 0004](../adr/0004-agent-acp-and-history.md) | 文件可以先落盘，接受更新审阅基线，拒绝还原；ZJ 审阅锁不约束外部 Agent 写入，不合并不同会话的快照。ACP fs 路径限制也不能约束 CLI 自带 shell |

概念边界：**provider** 提供模型推理、计费和账号；**runtime** 管工具循环、权限、会话、压缩和恢复；**bridge** 把 runtime 协议映射到 ZJ；**ZJ** 管交互、历史和审阅。DeepSeek／Kimi 是 provider 选择；Claude Code／Codex／JCode／Pi 是 runtime 选择。同模型换 runtime 仍可能改变提示、工具、压缩和最终质量，不能视为无损替换。

## 最多三条路线

| 路线 | 结构与收益假设 | 主要成本／准入门槛 | 判断 |
| --- | --- | --- | --- |
| A：JCode 共用运行时 | 原生 Codex／Claude 保留；DeepSeek／Kimi 交给按信任域隔离的 JCode host，会话级选择 provider。共享 Rust 引擎有望降低多会话增量 | 旧调研的 JCode v0.93.0 ACP 缺审批、ZJ steering 和完整 diff／locations；原生 SDK 更丰富，但新桥接、私有 daemon 回收和 provider 凭据隔离需验证。不能直接把现有 ACP 入口当等价替换 | **优先做接口验证，尚不接入生产** |
| B：Pi 共用运行时 | DeepSeek／Kimi 复用 Pi provider／工具循环；若要真正共享进程，应验证 SDK 多 session host，而不是假定现成 ACP adapter 已共享 | 旧调研的 `svkozak/pi-acp` 是每 session 一个 Pi RPC 子进程；权限靠扩展拦截，审阅快照映射需补。SDK host 包含 JS runtime；只有实测才知道是否值得 | 备选，JCode 门槛过不了再验证 |
| C：ZJ 自有 Rust 工具循环 | 一个受监督的独立服务，连接两家 provider；工具、权限、快照与队列从一开始按 ZJ 需求设计 | 必须自己维护流式重组、工具参数校验、取消竞态、重试、压缩、恢复、模型切换、图片及沙箱；共享内存收益未知，维护与质量风险最高 | **暂不选择**；只在成熟运行时持续无法满足明确需求时做极小原型 |

路线 A/B 的「provider 可切换」仍需对选定版本与实际 DeepSeek／Kimi endpoint、模型、工具调用和图片做合同测试；支持 OpenAI-compatible／Anthropic-compatible 不代表全部语义等价。使用同一运行时不需要将原生 Claude／Codex 账号迁入其中。

## 能力矩阵

标记：**本地**＝当前源码或 ADR 的历史冒烟；**历史**＝2026-10-09 报告引用的固定外部版本；**待验**＝不能承诺。以下是 ZJ 接口能力，不把引擎宣传页的能力自动算到桥接上。

| 能力 | Codex 原生桥接 | Claude 原生桥接 | JCode → ZJ（历史 v0.93.0） | Pi → ZJ（历史 1.1.0／社区 adapter） | 自建 Rust |
| --- | --- | --- | --- | --- | --- |
| 流式文本／工具进度 | 本地有 | 本地有 | 历史有 | RPC 有，ZJ 映射待验 | 需实现 |
| permission | 审批事件与模式映射 | can_use_tool 与模式映射 | ACP 缺映射；SDK 需另接 | 扩展确认可转 ACP，不是默认完整工具策略 | 每个工具强制拦截，需实现 |
| steering | turn/steer | 追加消息与 UUID 回合收敛 | 原生有安全点输入，ACP 缺 ZJ 扩展 | steer 在工具结束后／下次模型调用前注入，不等于立即打断 token | 安全点与取消语义需设计 |
| images | 本地发送及历史冒烟 | 本地发送及历史冒烟 | ACP 声明；模型和恢复待验 | RPC 支持；provider 与 adapter 待验 | 编码、预算与模型能力需实现 |
| context resume | thread/resume | --resume | 历史有会话恢复；ZJ 闭环待验 | 持久会话历史有；adapter 恢复闭环待验 | 完整工具状态与日志需设计 |
| model switch | 会话配置有 | 控制请求有 | 历史 ACP 模型／推理配置有 | RPC 设置及跨 provider 连续性待验 | 需做历史兼容性转换 |
| review | diff 事件＋ZJ 快照／拒绝 | diff／工具输入快照＋ZJ 审阅 | ACP 缺完整 diff／locations／fs | 写前快照与编辑映射待验 | 所有写入路径必须覆盖 |
| sandbox | 发送 sandboxPolicy；实际限制由 CLI／平台执行 | 权限模式不是 OS 沙箱证明；环境实际配置待验 | 旧源码明确破坏性检查不是 sandbox | trust／扩展审批不是 OS 沙箱 | 需独立 OS／进程工具隔离 |

DeepSeek 现有 Claude 预设复用的是桥接能力，不是对 DeepSeek 图像、工具调用、思考和长上下文的全部认证。Kimi 没有本次一手 API 兼容证据，不能给矩阵打全勾。模型切换时默认在回合边界执行；跨 provider 显示为新分支／新会话，保留用户可读内容与可验证工具结果，不把供应商私有 thinking、缓存或恢复 token 当可移植状态。

## 安全与共享边界

1. **pooling key 不可简化为显示名或 model id。** 当前 key 已包含 preset／env／search；未来 runtime key 至少区分规范化可执行文件及版本、参数、协议版本、endpoint／账号身份、凭据版本、工具／MCP 配置、权限与沙箱信任域。只有能在 session 内可靠隔离的字段才可下沉；进程级 cwd／环境／配置不同就分 host。显示名只用于 UI，密钥值不得写入日志或持久 key。
2. **provider 凭据按请求与账号隔离。** 使用现有 keychain／环境引用，不让模型、shell 子进程、扩展或 MCP 无条件继承所有 provider 密钥。理想结构是无工具执行权限的凭据／HTTP 层与最小环境工具 worker；若运行时无法保证扩展及工具不可读取其他账号密钥，则分进程。换 endpoint 不能沿用另一服务商的认证头，重定向不能跨主机泄露认证。
3. **host 共享不是工作区授权共享。** cwd、读写根目录、审批、快照、工具调用 id、会话 id 和 generation 均按 session 绑定。不同信任域不共用有环境状态的 shell；审批取消、崩溃恢复和旧事件必须 fail closed。共享 host 崩溃影响多个 session，需要独立恢复状态。
4. **多 Agent 写同文件需要协调。** 首轮只让一个 writer 进入同一仓库，其他角色只读；确需并行写时采用用户授权的隔离 worktree 或路径租约及应用前内容校验。不要认为 ZJ 审阅锁可阻止 CLI 并发覆盖。合并结果仍要测试。
5. **只回收自有资源。** ZJ 退出关闭自有 host／会话；共享外部 daemon、MCP 和索引服务不能按名字结束。工具权限与生命周期由 runtime 配置负责，不在 ZJ 加每个工具的专用补丁。

## 内存预算与速度／质量基准

**历史测量不能直接横向相减**：[ADR 0009](../adr/0009-agent-bridge.md) 在 2026-10-08 的 M5 Pro、Claude 2.1.285／Codex 0.160.0、第一轮后空闲记录：bridge 约 3–5 MB；Claude 每会话约 115–145 MB；Codex 一会话约 66–69 MB，两会话约 87 MB。[2026-10-09 进程树报告](agent-memory-2026-10-09.md) 另一次实际负载中 Node／REPL 工具达 375.2 MB，说明去掉 bridge 层 Node 不代表工具树不再启动 Node。不同日期、负载与度量不能拼成现状总量。

模型是远端服务时，本地占用主要来自 runtime、历史／上下文、工具输出、图片、文件快照和工具进程。预算必须用 `ZJ GUI + 各唯一 host + 活跃 session 增量 + 全部自有工具后代 + 共享服务归因` 建账；不能只测顶层 CLI，也不能把各时刻峰值相加。物理 footprint 与 Linux PSS 不混用；短命进程／不可读样本注明遗漏。

下列是**小型验证的建议目标，非已有测量或项目硬门禁**：

| 场景 | 建议目标／观察点 |
| --- | --- |
| 未启用 Agent | 零新增 runtime 常驻进程；GUI 继续遵守现有 CLAUDE.md 预算 |
| 新共享 runtime，握手后空闲，无 MCP／embedding／浏览器 | host＋协议适配层 ≤ 50 MB；每个空闲恢复句柄增量 ≤ 10 MB；两项分别测，不用会话统计掩盖固定成本 |
| DeepSeek＋Kimi 共 4 个短会话，最多 2 个执行，统一测试夹具 | 新 runtime 进程树峰值目标 ≤ 200 MB（包含本测试的 shell 工具），并单列完整应用总量；相对同任务参照至少减少 25% 才值得增加维护面。未达标先分析，不删安全能力凑数 |
| 空闲关闭与恢复 | 已持久化且无回合／审批／子任务时才驱逐；恢复后历史和未决工具不丢、不重放写入；观察关闭后 footprint 与进程数是否回落 |
| 交互与质量 | 同模型同 endpoint 下，桥接新增首个事件延迟 p95 目标 ≤ 100 ms；总任务耗时中位数退化目标 ≤ 10%；取消确认／工具停止分别记录；全部安全与编辑验收通过。小样本不宣称统计等价 |

不默认把空闲 10 分钟改为每轮结束立即退出：这可能损失热启动和缓存。先回收明确关闭会话与闲置工具，再比较现有 10 分钟和候选较短保温期；忙碌、待审批、等待用户和有活跃子任务的会话不驱逐。事件数量有界不代表字节有界，需限制每 session／host 的队列字节、工具输出、图片解码及快照总量；大结果落盘按需读，不能把用于模型的内容静默截断。

**同任务基准**：冻结仓库 commit、runtime／模型版本、账号路由、工具配置和权限；先用相同假 provider 事件流比较 transport／进程开销，再在获得付费授权后用相同模型分别跑 5 个任务（定位 bug、补丁＋测试、多文件改动、图片说明、长会话恢复），每项至少 3 次、交错顺序、区分冷／热。记录首个事件／首个文字、完成时间、token／费用、最大 footprint、审批完整性、补丁正确性、重试次数和恢复成功。跨模型优劣另列，避免把模型变化误算成 runtime 收益。

## 有界验证步骤与停止条件

1. **静态合同核验**：固定 JCode 候选版本，核对 provider 会话配置、SDK 审批／steering／写前快照／close。DeepSeek／Kimi 各核对工具调用、流式片段、图片、错误、限流、上下文与模型列表；目前最新版 API／协议均未核验。
2. **无密钥离线原型**：仅在后续获准实施时，用假 provider／假工具测试两会话并发、交错流、取消期间审批、错误恢复、跨账号 header、跨目录访问、迟到事件和队列压力。不接真实账户，不执行仓库破坏性命令。
3. **单 runtime 门槛**：全部能力矩阵的必需项通过后，才做小额同任务实测；JCode 若必须大面积重写桥接才能补齐，停止并比较 Pi SDK host。Pi 若权限／审阅无法完整覆盖，同样停止。
4. **保留回退**：Codex／Claude 原生桥接不动；DeepSeek 旧预设保留。候选只增加实验入口，不自动迁移历史、不更换默认 Agent、不安装运行时。未测完不宣称「四模型同体验、零损耗」。

## 来源与尚未确认项

本地一手源码、ADR、历史报告均于 2026-10-22 读取。ADR 0004 发布于 2026-10-02、ADR 0009 于 2026-10-08，后续追加内容以现有文件为准；外部来源下列链接继承自 2026-10-09 报告，当时的访问记录不等于本次访问。

- [JCode 本地调研](jcode-integration-assessment-2026-10-09.md)：所引 [v0.93.0 ACP 源码](https://github.com/1jehuang/jcode/blob/v0.93.0/src/cli/acp.rs)、[SDK 生命周期](https://github.com/1jehuang/jcode/blob/v0.93.0/crates/jcode-sdk/src/launch.rs)、[共享服务架构](https://github.com/1jehuang/jcode/blob/v0.93.0/docs/SERVER_ARCHITECTURE.md)。发布日未在本次复核；最新缺口是否已补齐未知。
- [Pi 本地调研](pi-opencode-comparison-2026-10-09.md)：所引 [v1.1.0 RPC](https://github.com/earendil-works/pi/blob/v1.1.0/packages/coding-agent/docs/rpc.md)、[固定版 adapter 会话实现](https://github.com/svkozak/pi-acp/blob/04d0a15d6bb42bb65cce5c9caedcdcb2cdcb6905/src/acp/session.ts)。发布日未在本次复核；SDK 多会话 host 的完整交互与内存尚无 ZJ 实测。
- 官方后续核验入口：[DeepSeek](https://api-docs.deepseek.com/)、[Moonshot／Kimi](https://platform.moonshot.ai/docs/)、[Codex app-server](https://developers.openai.com/codex/app-server)、[Claude Code](https://code.claude.com/docs/)。本次未读取，不据此声称特定 API／模型兼容；不指定未经验证的最新模型 ID。
- 未决：候选最新版本／许可与供应链、DeepSeek／Kimi 实际 capability、同进程凭据与插件隔离、真实任务质量／速度和完整进程树内存。以上阻止直接接入，但不阻止先按本报告缩小验证范围。
