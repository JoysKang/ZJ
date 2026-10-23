# 同类产品交互与架构借鉴：MyGo、T3 Code、Tern、Herdr

调研日期：2026-10-22。问题：在保留 Rust／GPUI 原生界面和现有资源预算的前提下，ZJ 可以从这些项目借鉴哪些具体交互？决策周期：近期小改动与后续独立架构评估。

**建议先做两项小改动：让有待审修改的会话持续可见；把运行中「追加指令」的语义直接显示在输入区。第三项可选：给现有审阅入口补一个命令面板动作。** 三项均能复用现有状态和组件，不需要新增依赖。完整消息队列、自动 worktree 管理、跨进程常驻服务分别立项，不捆绑进界面优化。

## 证据边界

- 外部证据为本次 WebSearch 返回的官方域名／官方仓库搜索摘要。上游调研已记录 GitHub、Tern 的 WebFetch 返回 `Unable to verify if domain … is safe to fetch`；本任务没有重复失败请求，也没有通过 curl、代理、浏览器或克隆绕过限制。
- **没有读取这些项目的完整源码、完整文档或许可证文件，没有安装、运行或实测界面。** 以下外部功能均为「官方搜索摘要所述」，不能证明指定发行版已经交付。讨论串中的需求、维护者回复尤其不能替代发布记录。
- 本地 `rg` 对 `docs/` 与 `README.md` 搜索 `mygo|t3code|tern\.build|herdr|peer-product` 未找到这些项目的既有缓存材料。随后读取了 ZJ 当前工作区的相关源码；工作区已有未提交修改，引用反映调研时状态，不等同于 `main` 已发布版本。
- 可信度分为：**高**＝本地源码直接支持；**中**＝官方搜索摘要明确描述，正文未核验；**低**＝身份不确定或讨论阶段线索。建议属于本报告的设计判断，不冒充其他产品已实现的细节。
- 「这些产品内存很高」目前只是待验证的反馈。没有同机、同负载、同版本的 physical footprint 数据，不能给出内存排名，也不能从 Electron、Go、Rust、native 等技术标签直接推算占用。

## 先确认项目身份

| 用户名称 | 本次采用的候选 | 摘要可支持的身份与架构 | 限制 |
| --- | --- | --- | --- |
| mygo | [egoist/mygo](https://github.com/egoist/mygo) | Go 桌面应用框架；支持 Web 前端和 GPU 绘制的原生 UI。摘要称原生路径不需要 HTML、JavaScript 或 WebView | **身份未最终确认。** 这是框架，不能当成与其余三项同类的 Agent 工作台；如果用户指另一个 MyGo，需要补准确链接 |
| t3code | [pingdotgg/t3code](https://github.com/pingdotgg/t3code) | 本地 coding agent 的控制界面，摘要列出移动端、Web 和 Electron 桌面端 | 可借鉴任务语义与入口，不据此迁移 ZJ 的渲染架构 |
| tern | [tern.build](https://www.tern.build/) | 官网摘要自称原生 Mac 应用，以独立 Git worktree 运行 Agent，集中审阅改动；自称闭源 | 原生实现语言、具体 GUI 技术、资源占用均未核验 |
| herdr | [herdrdev/herdr](https://github.com/herdrdev/herdr)、[herdr.dev](https://herdr.dev/docs/concepts/) | 持久工作区／终端、后台 server 与客户端分离；session 是服务器命名空间 | 不把独立第三方 `penso/herdr-gpui` 客户端的 GPUI／许可证信息套给 Herdr 主项目 |

## 具体交互线索与 ZJ 对照

### Tern：围绕审阅收口

官方摘要描述的流程是：⌘N 创建 Agent 与 worktree → 独立执行 → F7 打开集中审阅队列 → 从 Diff 发送评论给 Agent → 合并或创建 PR。桌面文档摘要还提到发现外部创建的 worktree；CLI 文档提到共享后台 daemon。证据为 [入门](https://www.tern.build/docs)、[桌面](https://www.tern.build/docs/desktop)、[CLI](https://www.tern.build/docs/cli)，可信度中。

对 ZJ 有价值的是「任务结束之后，仍有一个明确的人工处理入口」。ZJ 已有当前会话的改动卡片、审阅按钮、文件／块级接受与拒绝，以及上一处／下一处，无须重新做一套 Diff。`agent_panel/changes.rs:8` 以 `changed_files` 渲染卡片，`:84` 已有「审阅」；`agent_review.rs:487`、`:495` 已有块导航。

本地具体缺口：`agent.rs:490` 的活动会话集合只含 `busy() || unread`；`:653` 打开会话后清除未读。于是，运行已结束、回复已读、仍有待处理修改的会话可以退出活动条。与此同时，`:764` 的回收逻辑又明确保留 `changed_files` 非空的会话。**存在内存中、仍需审阅的会话，缺少持续可见的活动入口**，这是可直接修补的差距，可信度高。

建议先保留这种会话的活动条入口，并显示「待审阅」或修改文件数；不把「回复已读」等同于「改动已处理」。集中跨会话审阅队列、行评论回传、自动合并暂后置。只读导航可做成轻量入口；任何接受／拒绝仍走现有内容校验，不在列表聚合时读取所有补丁。

### T3 Code：区分立即追加和下一轮排队

[Discussion #6684](https://github.com/pingdotgg/t3code/discussions/6684) 的搜索摘要转述一条日期为 2026-10-16 的维护者回复：steering 走 `turn/steer`，queue 等当前运行结束再发。原始 [Issue #231](https://github.com/pingdotgg/t3code/issues/231) 的摘要列出显式控件、默认模式、重连后保留队列等诉求。**这些是讨论与回复证据，不足以证明某个正式版的完整行为。** [thread-sidebar 文档](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md) 摘要提到独立 worktree、任务结束后的清理以及重启后恢复被暂停的队列。可信度：语义区分中；发行版完成度低。

ZJ 已支持运行中追加，且按 Agent 能力开放。`agent_panel/composer.rs:230–244` 检查 `supports_steering()`；忙时按钮悬停显示「追加指令（⏎）」，不支持时禁用。`agent/turns.rs:211–216` 忙时调用 `client.steer(parts)`，空闲时调用 `client.prompt(parts)`。因此不能把「支持追加」列为缺失功能。

小改动是把当前发送语义放到输入区可见位置，例如忙时按钮旁显示「追加到当前轮」，并给不能追加的 Agent 显示现有原因；继续由同一能力判断驱动，不新增发送模式。这样键盘直接回车的人也能理解本次操作，不必先悬停。完整「下一轮排队」需要另行定义持久化、取消、失败、审批阻塞、重启暂停和附件生命周期。现有 `LiveSession.queued` 是启动期间暂存提示的槽位（`agent/turns.rs:73`、`:130`），不能仅改名字就宣称是完整后续任务队列。

第三方 teardown 搜索结果提到「Needs you／Running／Recent」，但不是 T3 Code 官方实现证据，本报告不据此声称 T3 Code 已有这套分组。

### Herdr：让语义状态与展示标签分开

[Agents 文档](https://herdr.dev/docs/agents/) 摘要描述完成／需要注意时的通知及侧栏审阅状态；[Socket API](https://herdr.dev/docs/socket-api/) 摘要区分决定通知的语义状态与 `review ready` 等展示元数据。适合 ZJ 借鉴的原则是：审批、运行、错误、未读、待审修改分别表达，避免一个醒目圆点承载所有含义。可信度中。

ZJ 已有 `RowStatus` 和「下一个待审批」，`commands.rs:94`、`agent/permissions.rs:332` 可直接证明；审批导航默认快捷键在函数注释中为 ⇧⌘A。不能把这些已存在能力再次列为新需求。扩展待审标记时，应复用状态计算，维持待批准／错误的可见性，不能让「有文件改动」遮住审批。列表顺序保持稳定，避免每次流式事件都重排导致点击目标移动。

[Concepts](https://herdr.dev/docs/concepts/) 摘要将 session 定义为独立 server namespace，建议先用 workspace，确有完整运行时隔离需求才用 named session。[worktree 配置文档](https://github.com/herdrdev/herdr/blob/master/docs/next/website/src/content/docs/configuration.mdx) 摘要称 worktree workspace 分组在源 workspace 下，关闭父组保留磁盘 checkout 和分支。可借鉴「关闭界面」与「删除 Git 数据」分离的交互；不应复制其术语到 ZJ，因为 ZJ 的 Agent session 已有另一套含义。

[仓库摘要](https://github.com/herdrdev/herdr) 称客户端断开后 server 保留终端进程；server 或机器重启后恢复布局并可恢复支持的 Agent 会话，原进程不会因此存活。这种生命周期与 ZJ 的退出回收目标不同。ZJ `agent_client/host.rs:44` 已有全应用进程池，`:57` 在退出时关闭进程组；`agent.rs:764` 用事件触发回收空闲会话。无需为学习工作区分组先引入永驻 daemon。

### MyGo：目前只能做框架层参考

[仓库摘要](https://github.com/egoist/mygo) 描述原生 GPU UI 和 Web 窗口可并存；[CLI 文档摘要](https://github.com/egoist/mygo/blob/main/docs/cli.md) 提到模板附带维护技能，覆盖元素生命周期、稳定身份、线程与测试。它提供的是框架约束线索，本次没有可信的会话列表、审批卡片、消息队列或代码审阅产品流程可比较。

对 ZJ 的判断：保持 UI 实体稳定身份、后台结果做 generation 校验、关闭后释放资源，这些已是项目约束。没有证据表明迁移到 MyGo 能提高 ZJ 的体积／footprint 表现；不建议新增 Go 运行时、WebView 或第二套窗口层。身份确认与完整原生 UI 文档核验之前，不为 MyGo 指定界面改造。

后续进展：2026-10-23 已按下表实现三项最小交互，验证状态见[未完成交互补充](2026-10-23-ui-followup.md)。本研究正文继续保留调研时的建议与证据边界。

## 近期实现排序

| 顺序 | 建议与用户动作 | 现有接点／预计范围 | 边界与验收 |
| --- | --- | --- | --- |
| 1 | 读完回复后，有待审修改的会话仍留在活动条；点击即可回到该会话和已有改动卡片 | `agent.rs::active_sessions`；`agent_panel.rs` 活动条／状态渲染；复用 `changed_files`。不新增依赖 | 只读回复不会清除待审标记；最后一个改动处理完成后取消标记；运行／待审批／错误状态继续清晰。不得仅为计数后台加载所有 Diff |
| 2 | 运行时直接看见「追加到当前轮」；不支持追加时显示原因 | `agent_panel/composer.rs` 复用 `busy`、`can_steer` 和 theme token；同一发送动作 | 空闲／忙且支持／忙且不支持三态与按钮可用性一致；Enter 和点击一致；不引入假队列；窄面板、亮暗主题可读 |
| 3（可选） | 在命令面板输入「审阅当前会话修改」，直接调用现有审阅入口 | `commands.rs` 加动作，复用 `agent_review_first`；现有 ⇧⌘A 仍只处理审批 | 无会话／无修改时给出真实空状态；不直接照搬 Tern F7，先检查本地快捷键冲突。跨会话队列不在此项范围 |

这是实现建议，**本次没有修改产品代码，也没有测量这些改动的增量**。「不新增依赖」不等于二进制或内存零增长。前两项预计只增加少量状态选择／文字和渲染节点，具体增量必须由 dist 构建与同机测量确认。

## 延后架构及其前置条件

| 能力 | 为何延后 | 真正实施前必须明确 |
| --- | --- | --- |
| 独立 worktree 一键开任务 | 涉及 Git 写操作、路径身份、分支与窗口恢复，不是增加一个按钮 | 外部 worktree 发现、创建失败回滚、脏目录、共享 common directory 锁、关闭与删除分离；明确用户授权后才创建或删除 |
| 跨会话／跨文件审阅队列 | 会话改动快照可能与当前磁盘不同，现有 Diff 还承担块级接受／拒绝 | 只持轻量路径／会话元数据；按需加载当前 Diff；版本校验失败不自动跳过或标为已审；每次处理后稳定定位；与其他 Diff 调研合并设计 |
| 持久后续消息队列 | 需要新的任务状态机，不是已有启动槽位的 UI 包装 | 每条消息稳定 ID、有界数量／内容／附件，明确 steer 与 queue；错误或取消后的保留／暂停规则；重启不得悄悄自动开始工具执行；不绕过审批 |
| 脱离窗口的常驻 server／远程控制 | 新增长期占用和退出语义，削弱当前 idle 回收收益 | 先证明用户需要后台持续运行；单独预算 server 与全部子进程；关闭、崩溃、重连、权限隔离与升级兼容明确后再评估 |
| 完整任务看板、插件市场、内嵌 Web 应用 | 对当前自用小编辑器的收益尚无证据，资源与维护范围明显扩大 | 先用命令面板、会话入口和现有 Diff 解决任务发现与审阅；不预先引入框架 |

## 资源预算与验证方式

本次请求目标为 dist stripped 二进制 ≤50 MB、单窗口 idle footprint ≤70 MB（上限 100 MB）、3 窗口＋20 文档 ≤220 MB（上限 300 MB）、idle CPU p95 ≤1%。最终核对时，当前工作区 `CLAUDE.md:21` 已按用户要求将 CPU 上限更新为 1%，与开发说明一致；任务开始时上下文中的 0.1% 是旧值。二进制 CI 上限仍为 52 MB，不能当成可以主动花满的容量。本报告不修改预算文件。

近期三项设计均不增加轮询、闪烁、常驻动画、预加载 Diff 或第二个运行时。新增状态由既有事件更新，尺寸、色彩和间距继续取 `theme.rs`；沿用 Lucide 和现有配色，运行中标记服从可见／前台门控。

后续实现验证应保留未修改基线与修改后同机数据：固定 `--profile dist`、系统外观、窗口／文档／会话数，分别记录主程序、Git 子进程及 Agent 进程树。编辑器空闲预算和含活跃外部 CLI 的总占用分开报告，不能靠漏计子进程证明达标。现有开发说明要求静止空闲观察 5 分钟并使用 100–250 ms 原始采样；短窗口数据不能代替这一验收。不得为得到更低数字反复挑选样本。

功能验证聚焦真实差异：完成消息已读但修改未处理、处理最后一个文件、两会话一审批一待审、切换窄宽面板、键盘回车、Agent 不支持 steering、取消与错误。UI 可见性变化需要亮暗截图／实际交互核验；状态选择逻辑使用有意义的回归测试。按项目要求执行格式、clippy、测试及适用的资源检查，本次研究文档不声称这些代码检查已运行。

## 许可证与未决问题

MyGo 与 T3 Code 的官方仓库搜索摘要标示 MIT；没有读取固定 commit 的 LICENSE，不能直接作为复制许可审查结论。Tern 官网摘要明确自称闭源。Herdr 主项目许可证本次未核验。**本报告只提炼交互概念，不复制代码、图标、截图或视觉资产。** 未来若移植实现，先固定版本并逐一核对源文件、依赖和资产许可。

未决事项限于会影响决策的几项：MyGo 的准确身份；T3 Code queue／steer 对应的实际发行版与失败行为；Tern 的具体原生栈和审阅队列边界；Herdr 主项目许可及 server 的真实空闲开销；所有产品在同机、同负载下的资源数据。网络可用后优先核验这些，不以广泛二手盘点替代。

## 来源记录

以下均于 2026-10-22 经 WebSearch 获取官方搜索摘要，页面正文未读取，发布日期除特别说明外未核验。搜索引擎摘要可能过期或概括错误。

| 编号 | 官方来源 | 本次用途／置信度 |
| --- | --- | --- |
| S1 | [egoist/mygo](https://github.com/egoist/mygo) | 候选身份、原生／Web 路径、MIT 摘要；功能中，身份低 |
| S2 | [MyGo CLI docs](https://github.com/egoist/mygo/blob/main/docs/cli.md) | 模板维护技能覆盖范围；中 |
| S3 | [pingdotgg/t3code](https://github.com/pingdotgg/t3code) | 产品与 Electron／Web／移动端身份、MIT 摘要；中 |
| S4 | [T3 Code discussion #6684](https://github.com/pingdotgg/t3code/discussions/6684)、[issue #231](https://github.com/pingdotgg/t3code/issues/231) | steer／queue 区分；摘要转述 2026-10-16 回复；语义中、交付状态低 |
| S5 | [T3 Code thread-sidebar](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md) | worktree、清理与重启暂停队列线索；中 |
| S6 | [Tern 首页](https://www.tern.build/)、[入门](https://www.tern.build/docs) | 原生 Mac、闭源、worktree 与审阅入口；中 |
| S7 | [Tern desktop](https://www.tern.build/docs/desktop)、[CLI](https://www.tern.build/docs/cli) | 外部 worktree 发现、daemon、审阅流程；中 |
| S8 | [Herdr 仓库](https://github.com/herdrdev/herdr)、[concepts](https://herdr.dev/docs/concepts/) | namespace 与后台 server 生命周期；中 |
| S9 | [Herdr agents](https://herdr.dev/docs/agents/)、[socket API](https://herdr.dev/docs/socket-api/) | 注意状态、通知与展示元数据分离；中 |
| S10 | [Herdr worktree 配置](https://github.com/herdrdev/herdr/blob/master/docs/next/website/src/content/docs/configuration.mdx) | workspace 分组与关闭后保留 checkout；中 |

本地依据：[`agent.rs`](../../crates/app/src/workbench/agent.rs)、[`composer.rs`](../../crates/app/src/workbench/agent_panel/composer.rs)、[`turns.rs`](../../crates/app/src/workbench/agent/turns.rs)、[`changes.rs`](../../crates/app/src/workbench/agent_panel/changes.rs)、[`agent_review.rs`](../../crates/app/src/workbench/agent_review.rs)、[`commands.rs`](../../crates/app/src/workbench/commands.rs)、[`permissions.rs`](../../crates/app/src/workbench/agent/permissions.rs)、[`host.rs`](../../crates/agent_client/src/host.rs)。与现有 [Diff／升级调研](2026-10-22-upgrades-and-diff-research.md)、[多 Agent 运行时调研](2026-10-22-multi-agent-runtime.md) 相互参照，不重复修改其内容。
