# ZJ 多会话内存统计（2026-10-09）

前期做内存统计与生命周期检查，未修改应用源码。用户随后授权「回收和验证都执行」，实现与验证结果见文末。整个过程没有重启 ZJ 或停止现有 Agent；隔离测试仅创建并清理自身进程。

## 进程实测

2026-10-09 10:58 左右，48 个样本，间隔 250 ms，约 12 秒。使用 macOS `proc_pid_rusage` 的 physical footprint；MB 为十进制。采样脚本自身及其子进程已排除。

| 分组 | 平均 MB | 组内合计最高 MB |
| --- | ---: | ---: |
| ZJ GUI | 583.9 | 599.9 |
| ZJ bridge | 6.9 | 6.9 |
| Codex CLI | 155.9 | 155.9 |
| Node and REPL tools | 375.2 | 375.5 |
| Codex code mode host | 13.5 | 14.7 |
| Other descendants | 4.8 | 4.8 |
| 可读进程树合计 | 1140.2 | 1156.6 |

组内峰值未必同时发生，不能把各组峰值相加。52 次子进程观测不可读，表中是可读部分；短命子进程可能漏采。该时刻不等同用户截图中的 1.39 GB。

ZJ 主进程 PID 39784；桥接 PID 40361；共享 Codex CLI PID 40362。样本中没有读到 Claude CLI。Node/REPL 分组含 12 个有占用的进程；存在多组 ChatGPT 电脑操作运行时，但不能据其数量直接推断正在运行的会话数。

## ZJ 主进程分类

- `vmmap` 后续单次快照中，IOSurface、IOAccelerator (graphics)、owned unmapped (graphics) 的 dirty+swap 合计约 352.1 MiB，即 369.2 MB。这是分类快照，不能与上表的时段平均值直接相减。
- `heap` 后续快照看到 100,779,019 字节（100.8 MB）的已分配堆块，其中 92,602,819 字节（92.6 MB）为 non-object。它包含 Rust、编辑器、系统库等分配，未启用分配栈，不能将其全部算作 Agent 文本或渲染缓存。

## SQLite 已保存文本统计

2026-10-09 10:54 左右，以只读连接和读事务统计 `length(CAST(text AS BLOB))`。只输出长度和条数，不输出聊天内容。

| 类型 | 已保存条数 | UTF-8 原文字节 | 最大单条字节 |
| --- | ---: | ---: | ---: |
| agent | 469 | 177,666 | 5,589 |
| tool | 3390 | 935,453 | 21,015 |
| user | 95 | 46,794 | 15,902 |

| 会话 ID | Agent | 数据库状态 | 全部已保存文本 KB | 最近最多 400 条已保存记录 KB | 其中回复 KB |
| --- | --- | --- | ---: | ---: | ---: |
| 8 | codex | running | 447.7 | 139.7 | 18.3 |
| 7 | codex | completed | 149.7 | 149.7 | 20.2 |
| 16 | codex | completed | 122.1 | 122.1 | 4.2 |
| 15 | codex | completed | 15.4 | 15.4 | 5.4 |
| 14 | codex | completed | 54.0 | 54.0 | 4.1 |
| 10 | codex | completed | 147.3 | 147.3 | 9.8 |
| 9 | claude-code | completed | 159.3 | 131.0 | 30.9 |
| 13 | codex | completed | 11.5 | 11.5 | 1.9 |
| 12 | codex | completed | 15.9 | 15.9 | 4.9 |
| 11 | codex | completed | 15.8 | 15.8 | 4.9 |
| 6 | codex | completed | 15.0 | 15.0 | 3.6 |
| 5 | codex | completed | 4.4 | 4.4 | 2.9 |
| 4 | claude-code | completed | 1.7 | 1.7 | 1.6 |
| 2 | codex | completed | 0.1 | 0.1 | 0.0 |
| 1 | codex | completed | 0.0 | 0.0 | 0.0 |

全部已保存原文合计 1,159,913 字节（1.16 MB）；其中用户与回复合计 224,460 字节（224.5 KB），回复合计 177,666 字节（177.7 KB）。数据库文件约 5.88 MB，WAL 约 4.44 MB；文件大小不代表内存占用。

## 结论与未覆盖部分

已保存回复的原文很小，当前统计不支持把高占用主要归因于长回复。主要已观测占用在 ZJ 主进程图形资源及 Agent 的工具进程。

SQLite 中工具记录主要是标题；思考正文、完整工具输出、结构化输入、diff、流式中的未保存回复，以及 Markdown/高亮缓存没有由本次 SQL 统计覆盖。因此，最近 400 条数据库记录的大小不是内存中最近 400 个条目的实际大小。

当前运行版本没有导出 LiveSession 内存字节的接口。若继续做精确归因，需要增加只输出计数和字节数的诊断，统计每会话回复、思考、工具正文、结构化输入/diff，以及回复/高亮缓存；应在下一次正常启动后采样，保留正在运行的任务。暂不据此实施执行详情外置或缓存策略变更。

## 原始证据

- `/private/tmp/zj-process-memory-stats.json`
- `/private/tmp/zj-process-memory-stats.csv`
- `/private/tmp/zj-session-stored-text-stats.json`
- `/private/tmp/zj-memory-39784-vmmap-summary.txt`
- `/private/tmp/zj-memory-39784-heap-summary.txt`

## 后续核对：Node / REPL 的具体来源

通过 macOS 进程参数接口读取可执行文件与脚本路径，仅输出路径和分类，不输出连接参数、环境变量或聊天正文。以下分项使用上文同一组 48 次采样的平均值，启动来源在后续只读检查中确认。

| 工具 | 实例 / 进程 | 平均占用 MB | 用途与依据 |
| --- | --- | ---: | --- |
| zvec_grep | 3 个 Node 进程 | 270.7 | 本地代码与文档检索。命令为 `zg server --stdio`；脚本指向 `@zvec/zvec-grep/dist/cli/index.js`，shebang 为 `node --liftoff-only`。 |
| cua-repl | 3 组启动器与子 REPL，共 6 个进程 | 74.7 | ChatGPT 的浏览器及电脑操作运行时；脚本为 `@oai/cua-repl/bin/cua-repl.mjs`。 |
| node_repl | 3 个进程 | 29.8 | 本机配置启用的 JavaScript REPL；程序来自 ChatGPT 的 `cua_node/bin/node_repl`。 |
| 合计 | 12 个进程 | 375.2 | 与前文 Node / REPL 分组一致。 |

最大的部分是 zvec_grep，占该组约 72%。这些进程的直接或间接父进程为共享 Codex CLI，不是 ZJ 原来的 Node ACP 适配器。多个实例的存在已确认，但尚未将每个实例绑定到具体会话，也未验证完成或空闲后的退出时机，不能据数量认定泄漏。

本机 `~/.codex/config.toml` 配置了 `node_repl` 和 `zvec_grep`。OpenAI Docs 说明本地 Codex 客户端共享 MCP 配置，STDIO MCP 服务通过配置的命令启动本地进程：[Model Context Protocol](https://developers.openai.com/codex/mcp)。cua-repl 的具体功能由随应用安装的 README 确认。

本次没有关闭这些服务或修改 Codex 配置。启动来源的原始分类保存在 `/private/tmp/zj-node-tool-sources.json`。

## 后续核对：搜索服务共享与会话回收

检查范围是三份 `zvec_grep` 进程的连接、会话归属和关闭行为。依据包括本机安装包源码、只读进程与日志快照，以及与运行版本一致的 Codex `rust-v0.160.0` 源码。未修改 ZJ 源码或 Codex 全局配置；生命周期测试仅创建并清理独立测试进程，没有发送模型请求。

### 搜索守护进程已共享，STDIO 转发进程未共享

2026-10-09 11:44:51 的单次 physical footprint 快照如下。MB 为十进制；这组数字不能与 10:58 的时段平均值直接相加。

| 进程 | PID | 父 PID | 占用 MB | 连接或监听 |
| --- | ---: | ---: | ---: | --- |
| 共享搜索守护进程 | 40404 | 1 | 201.3 | `127.0.0.1:7999` |
| STDIO 转发进程 | 42959 | 40362 | 97.8 | `127.0.0.1:60971 → 127.0.0.1:7999` |
| STDIO 转发进程 | 43521 | 40362 | 94.7 | `127.0.0.1:61292 → 127.0.0.1:7999` |
| STDIO 转发进程 | 44434 | 40362 | 92.0 | `127.0.0.1:61852 → 127.0.0.1:7999` |
| 三份转发进程合计 | — | — | 284.5 | 同一个守护进程 |

`zg server --stdio` 在本机安装包中进入 `runStdioBootstrapBridge`，先查找或启动守护进程，再通过 Streamable HTTP 转发 MCP 请求。因此，三份 STDIO 进程不代表三份索引加载。

守护进程 PID 40404 的父进程已是 PID 1，未包含在此前以 ZJ 为根的可读进程树统计中。本快照中，搜索守护进程与三份转发进程的 footprint 合计约 485.8 MB；这个进程 footprint 合计不能当作去重后的系统物理内存，也不能直接加到此前 1140.2 MB 的时段平均值上。

STDIO 转发连接关闭时，安装包源码会关闭上游和下游连接，不会停止共享守护进程。守护进程的默认 4 小时空闲设置用于回收文件 watcher，不能据此认定整个服务会在 4 小时后退出。

### 三份转发进程的归属证据

| PID | 启动时间 | 对应证据 | 确定程度 |
| --- | --- | --- | --- |
| 43521 | 10:36:32 | `/Users/joys/work/5g` 主会话在同秒记录 `zvec_grep` 启动；ZJ 会话 ID 为 7 | 启动时间与日志关联，未获得直接 PID → thread 标识 |
| 44434 | 10:52:06 | 当前 `/Users/joys/work/code` 主会话在同秒记录 `zvec_grep` 启动；ZJ 会话 ID 为 8 | 启动时间与日志关联，未获得直接 PID → thread 标识 |
| 42959 | 10:28:39 | `/Users/joys/work/prod` 主会话的 MCP 初始化日志，与其 `standards_review` 子 Agent 启动在同秒发生；ZJ 主会话 ID 为 16 | 无法区分主会话连接、子会话连接或旧连接残留 |

这些进程的直接父进程均为共享 Codex CLI PID 40362。进程环境中没有可用的会话 UUID。仅凭共同父进程和启动时间，不能声称已经将第三份进程精确绑定到某个子 Agent。

`prod` 主会话在 10:42:31 完成任务；10:53:31 的 Codex 日志已记录主会话 shutdown 与 listener teardown。PID 42959 在 11:44:51 仍存活并连接共享守护进程。这已超过普通主会话的空闲等待期，但尚未证明该进程原本由主会话持有。

### 普通会话的空闲回收路径生效

ZJ 的 `agent_client` 在本轮结束后重新开始空闲计时，运行中不启动该计时；当前空闲时间为 10 分钟。计时结束时发送 ACP `session/close`。

诊断时的运行版 Codex 桥接在收到 `session/close` 后只对指定主会话发送 `thread/unsubscribe`，并立即回复 ACP 成功。取消订阅的响应使用 `Pending::Ignore`，因此 ACP 成功不等于 Codex 已完成卸载。

运行版本 Codex 的默认 `thread_unload_delay_secs` 为 60 秒。会话同时满足「无订阅者」和「不活动」，经过这段等待后才 shutdown、移除运行时并发送 `thread/closed`。会话 shutdown 会调用 MCP runtime 的 shutdown，停止它持有的 STDIO 工具进程。若 shutdown 超时，Codex 会保留会话并记录警告。

本机主会话日志符合这条路径，不能将当前占用解释为 ZJ 从未回收任何会话。任务 completed 也不等于会话已关闭：空闲等待期间，工具可以继续存活。

### 已确认的子 Agent 订阅回收缺口

Codex app-server 收到新会话创建事件后，会将已初始化的客户端连接订阅到新会话；这条路径也包含新建的子 Agent。诊断时的 ZJ 桥接只管理自己创建的主会话及标题生成会话，未登记子 Agent 的归属。`session/close` 仅取消指定主会话的订阅，没有递归取消子会话订阅。

因此，主会话空闲关闭不足以保证它创建的子 Agent 也满足「无订阅者」条件。只要子会话仍被订阅且未通过其他路径关闭，工具和会话上下文就可能继续被保留。这是源码已确认的生命周期缺口；仍需区分它与 PID 42959 的实际持有关系。

`prod` 的两条子 Agent 关系在只读状态数据库中仍为 `open`，未查到对应的 shutdown 日志。但持久关系 `open` 不等于运行时仍已加载，日志缺失也不能单独证明泄漏。当前证据不足以认定所有三份转发进程都是泄漏，或认定第三份一定由这两条子关系持有。

### 隔离生命周期验证

使用本机 Codex CLI、独立临时 `CODEX_HOME` 和 fake STDIO MCP 创建两个临时会话，将测试环境的卸载等待设为 1 秒。没有发送 `turn/start` 或模型输入，也没有读取或改写真实 MCP 配置。fake MCP 仅记录自己的 PID、initialize、退出事件。

| 操作 | 已加载会话数 | fake MCP 存活数 | 结果 |
| --- | ---: | ---: | --- |
| 创建两个会话 | 2 | 2 | 相同 STDIO 配置仍按会话创建进程 |
| 取消第一个会话的订阅，等待卸载 | 1 | 1 | 第一个工具退出，第二个继续存活 |
| 取消第二个会话的订阅，等待卸载 | 0 | 0 | 第二个工具退出 |
| 测试进程结束 | 0 | 0 | app-server 正常退出，测试子进程全部退出 |

验证断言通过，收到 `thread/closed` 通知。这证明普通会话的创建与关闭路径可回收 STDIO 进程；测试没有复现真实子 Agent 的创建、工具重连或长时间执行，不能代替对第三份真实进程的归属证明。

### 优化方向与验证边界

1. 优先评估让 Codex 直接连接已有搜索守护进程的 HTTP MCP 地址，省去每会话的 Node 转发进程。索引服务已共享，收益应来自取消转发层。实施前需核实守护进程自动启动、鉴权、roots 和交互请求转发，当前 284.5 MB 转发占用不能直接当作已验证的节省量。
2. 补充主会话与子 Agent 的归属、订阅及关闭状态，确保空闲回收覆盖已经结束的子会话，同时保留正在运行、等待审批或仍需协作的子 Agent。当前共享 Codex 还有活跃任务，不能靠停止整个进程验证回收。
3. 若继续精确归因，诊断需同时记录 loaded thread、订阅者和 MCP 子进程 PID。ZJ 的 ACP close 成功和数据库 completed 状态均不足以证明工具已经退出。

上述诊断阶段没有实施优化；后续授权实施与验证见下一节。现有证据支持优先检查工具转发开销与子会话回收；尚不支持将主要占用归因于长回复，或立即把全部执行详情迁移到 SQLite。

### 本轮证据与源码

- `/private/tmp/zj-mcp-lifecycle-live.json`：真实搜索进程与本机 TCP 连接快照。
- `/private/tmp/zj-mcp-lifecycle-log-summary.json`：会话关系与固定生命周期事件；不含聊天正文。
- `/private/tmp/zj-mcp-lifecycle-probe-result.json`：隔离测试结果。
- ZJ：`crates/agent_client/src/client.rs` 的 `close_session` 与空闲计时；`crates/agent_bridge/src/codex.rs` 的 `session/close` 和 `notification`。
- 本机 zvec-grep：`dist/mcp/stdio-bridge.js`、`dist/daemon/server-controller.js`、`dist/daemon/config.js`。
- [Codex MCP runtime](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/codex-mcp/src/runtime.rs)：普通 MCP 运行时按 thread 持有；共享配置和工具目录不等于共享所有连接。
- [Codex MCP connection manager](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/codex-mcp/src/connection_manager.rs)：连接复用与 shutdown。
- [Codex 会话 shutdown](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/session/handlers.rs)：停止 MCP runtime。
- [Codex thread 生命周期](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/thread_lifecycle.rs)：无订阅、不活动后的卸载条件。
- [Codex 卸载等待配置](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/config/mod.rs#L3911)：本次运行版本默认为 60 秒；不能套用其他版本默认值。
- [Codex 自动订阅新会话](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/lib.rs#L1282) 与 [取消指定会话订阅](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/thread_processor.rs#L1025)：主会话取消订阅未递归覆盖子会话。
- [App-server API overview](https://learn.chatgpt.com/docs/app-server#api-overview)：`thread/unsubscribe` 与 `thread/closed` 的语义。


## 授权实施：子 Agent 回收与 HTTP MCP 对照验证

用户要求「回收和验证都执行」。本节记录 2026-10-09 后续工作；前文的运行版描述及现场占用不代表新代码已在原 ZJ 进程中生效。

### 子 Agent 回收

Codex 桥接现在等待主会话 `thread/unsubscribe` 的结果再回复 ACP；失败返回真实错误。成功后分页查询已加载会话，使用 `thread/read` 的 `includeTurns: false` 读取元数据，不读取会话正文。桥接结合父会话标识、创建通知和 spawnAgent 工具事件维护归属，仅对归属明确且状态为 `idle` 的后代取消订阅。正在运行、待审批、待回答、归属未知以及其他打开的 ZJ 会话及其后代保留。

嵌套父会话先关闭时保留祖先关系，直到仍在运行的后代完成。旧清理结果以 generation 隔离；每次元数据读取和状态通知推进版本，防止旧空闲结果覆盖新运行状态。发送恢复请求前保护目标子树，恢复成功后撤销关闭意图；失败时用新 generation 重新发现并确认状态。旧关闭失败不会取消新恢复请求。

普通主会话的 10 分钟空闲计时未调整。子会话取消订阅后仍由 Codex 按自身卸载条件和等待时间回收；取消订阅成功不等于已经立即退出。没有新增依赖，也没有修改 Agent 输出存储或渲染缓存。

真实子 Agent 探针在独立临时工作区运行：主会话创建一个仅回复固定词的子 Agent，等待其完成后关闭主会话。记录确认主会话取消订阅、子会话元数据读取、子会话取消订阅及 `thread/closed` 均发生，桥接退出码为 0。探针将自身 app-server 的卸载等待设为 1 秒，不改变用户配置；仅记录方法名、会话标识和状态，不记录聊天正文或凭据。

### HTTP 与 STDIO 对照

使用隔离的临时 Codex 配置，分别以 HTTP 和 `zg server --stdio` 连接同一个已运行的 `127.0.0.1:7999/mcp`。每组创建两个临时会话，均完成工具发现与真实只读检索；未发送模型请求，检索使用 `autoUpdate: false`，没有创建或更新索引。

| 路径 | 工具发现 / 只读检索 | Node 转发进程 | 转发进程单次 footprint | 取消订阅后已加载会话数 |
| --- | --- | ---: | ---: | ---: |
| HTTP | 通过 | 0 | 无转发进程 | 0 |
| STDIO | 通过 | 2 | 105.3 MB + 105.1 MB = 210.5 MB | 0 |

两组 app-server 都以退出码 0 结束。实测表明直接 HTTP 可以省去这两份 Node 转发层；210.5 MB 是此次隔离对照的转发进程物理占用，不是原运行中 ZJ 的已实现降幅。共享守护进程仍存在，不能把不同时间的守护进程或 Codex footprint 差额算作优化收益。

本次已有守护进程且没有鉴权 token。HTTP URL 配置本身不会自动启动服务，而当前 STDIO 启动器会查找或启动它。带鉴权服务、自动启动和需要交互的 MCP 请求仍需单独验证；本轮没有永久切换全局 Codex 配置。

### 检查与交付边界

- 全工作区测试在恢复竞态修复前通过：481 项通过、4 项忽略；工作区 Clippy、格式和 diff 检查通过。
- 后续竞态修复后，桥接定向测试 38 项通过，桥接全 target Clippy（`-D warnings`）、全仓格式及 diff 检查通过。
- 真实 Codex CLI 冒烟覆盖基础回复、共享进程多会话、首会话继续响应，以及进程重启后的恢复。
- 审查按 ponytail-review → code-review 顺序执行。Standards 轴没有文档规范违规，初次发现的重复归属遍历已合并；Spec 轴发现的恢复窗口、重复读取乱序和旧关闭失败影响新恢复三项问题均修复，并通过定向复核。

新回收逻辑需要包含此次修改的新版本启动后生效。现有 ZJ 和用户 Agent 未重启或停止；未提交、推送或部署代码。

### 新增证据

- `/private/tmp/zj-http-mcp-probe-result.json`：HTTP 工具发现、只读检索、无 Node 转发及两会话卸载。
- `/private/tmp/zj-stdio-mcp-probe-result.json`：相同守护进程上的 STDIO 对照及转发进程 footprint。
- `/private/tmp/zj-codex-metadata-probe-result.json`：本机 CLI 对已加载 ephemeral 会话的 metadata-only 读取。
- `/private/tmp/zj-native-child-reclaim-result.json`：真实子 Agent 回收结果。
- `/private/tmp/zj-child-cleanup-workspace-tests.log`：全工作区测试。
- `/private/tmp/zj-child-cleanup-focused-tests.log`：最终桥接定向测试。
- `crates/agent_bridge/src/codex.rs` 与 `crates/agent_bridge/src/codex/subagents.rs`：此次实现及竞态回归。


## 后续分析：zvec-grep 的启动占用与升级选择

检查日期为 2026-10-09。问题是转发进程为何有较高占用、升级是否能降低，以及有哪些可行的进一步优化。此次只读检查安装包、官方发布记录与索引元数据；隔离导入探针没有连接守护进程、创建模型或打开索引，没有修改安装、全局配置或索引。

### 版本与已发布优化

本机安装的 `@zvec/zvec-grep` 是 0.2.2，Node 为 24.14.0。官方 npm registry 的 `dist-tags.latest` 同样为 0.2.2，发布时间为 2026-09-07 07:53:29 UTC；GitHub 最新 release v0.2.0 标为 Public Preview，npm latest 不等于项目已宣告稳定版。因此，目前没有更高的 npm 已发布版本可直接升级并获得新的减内存代码。[官方 registry](https://registry.npmjs.org/@zvec%2fzvec-grep)

0.2.2 已使用 `--liftoff-only`，Model2Vec 的向量表通过 `SharedArrayBuffer` 在 worker 间共享；不能将这些现有优化再算作升级收益。不能仅根据 PR 合并时间推断整个 PR 均包含在发布包中。

官方维护者在 [issue #72 的回复](https://github.com/zvec-ai/zvec-grep/issues/72#issuecomment-5586481736)确认：0.2.2 的主要版本变动是升级 zvec 依赖以支持 darwin-x64；当前 TypeScript 实现不计划大幅重构懒加载，后续 Rust 实现将移除该启动依赖结构。这是路线说明，不代表已有可替换当前安装的 Rust 正式版，也不是内存降幅实测。

### 转发层：仅加载代码就有明显成本

对本机安装包做两轮独立 Node 进程测试，在导入完成后显式 GC，再读取 physical footprint。无 MCP 会话、无模型创建、无索引打开。

| 导入范围 | 两轮 physical footprint | 已加载的包内原生模块 |
| --- | ---: | --- |
| 仅 Node | 14.5–14.6 MB | 无 |
| `mcp/stdio-bridge.js` | 60.7–61.3 MB | `zvec_node_binding.node` |
| `engine/storage/zvec.js` | 18.94–18.96 MB | `zvec_node_binding.node` |
| CLI commands 与 bridge | 71.6–72.6 MB | `zvec_node_binding.node` |

另做没有显式 GC 的两轮导入测试：bridge 为约 64.0 MB，CLI 与 bridge 为 68.9–69.4 MB。不同 GC 条件的数字不直接相减；两组均说明启动依赖本身有明显开销。原生模块已加载不等于打开了索引或加载了模型；这些导入探针未看到 ONNX 原生库。

静态依赖链可确认为 `stdio-bridge → server-controller → daemon-lease → service/root → storage/index → storage/zvec`。CLI 入口还静态导入 commands 及搜索、模型工厂等模块。MCP 客户端、服务端、协议 schema 和 CLI 依赖共同形成转发进程的基线成本，不能把差额全部归因于某一个库。

此前真实两会话测试中，每份转发进程约 105 MB，包含协议初始化与连接运行时；导入探针不是它们的等价工作负载，不能把两者的差额直接认作泄漏。多个 Agent 会重复承担转发进程成本，而搜索守护进程和索引服务仍共享。直接 HTTP 连接已验证可以省去整份转发层。

### 守护进程：已有小模型，继续检查缓存与并发

只读 manifest 确认 code、5g、prod 三个工作区均使用 `local/potion-code-16m-v2`，256 维。模型目录条目中的权重文件约 32.49 MB；文件大小不等于运行时占用。当前安装的 Model2Vec 使用专用 tokenizer 库和 worker，不应将它解释为每个转发进程都加载了一份大型 Transformers 模型。

随后读取当前守护进程的单次 footprint 为约 285.3 MB；它与之前 201.3 MB、379.3 MB、393.5 MB 的快照不是相同进程、时刻和负载，不能据此计算增长率或优化降幅。尚未分别量化模型、worker、索引原生内存和项目缓存的贡献。

本机安装包已有以下回收逻辑：读索引会话默认闲置 60 秒关闭；模型在没有租约后默认闲置 15 分钟释放，空闲模型池目标为 1 个；项目运行时及 watcher 默认闲置 4 小时回收。活动租约仍会保留资源，模型池的目标数量不是并发活动模型的硬上限。

Model2Vec worker 按需创建，上限默认为 `availableParallelism()`；向量表共享，但各 worker 有自己的 JS 运行环境和 tokenizer。高并发下限制 worker 数可能减少额外占用，不过本次没有测出实际 worker 数或这项节省量。

官方 main 另有 [Node 索引任务历史保留修复](https://github.com/zvec-ai/zvec-grep/commit/156aa1fd003bae22714fe236b7b0279b1b4e5be0)：清除完成任务的执行闭包与监听器、限制历史数量，并在根回收时清理相关任务。该改动未发布到 npm 0.2.2；属于长期保留的优化机会，未证明它解释当前守护进程占用。main 的另外一些模型/工作区 TTL 修复只涉及 Rust 实现，不能当作当前 Node 版本缺少已有回收逻辑的证据。

### 建议顺序与代价

职责说明：以下建议中，子 Agent 会话订阅回收属于 ZJ 的 Codex 桥接职责；HTTP 接入、工具空闲策略、worker 与内部依赖优化属于 Agent 配置或工具上游，不是 ZJ 的功能计划。工具兼容、启动和释放机制由工具自身提供，ZJ 不为这些建议增加工具专属管理逻辑。

1. **在 Agent／工具配置层评估直接 HTTP 接入。** 两会话已验证可取消 210.5 MB 的 Node 转发层。实际切换仍需核实工具支持的服务启动、鉴权与交互请求；共享守护进程继续存在。ZJ 不接管这些机制。
2. **使用此次实现的子 Agent 回收。** 减少完成后仍被订阅的会话及其工具进程，不停止活动会话；需要包含修改的新 ZJ 版本启动。
3. **缩短闲置项目保留。** 当前版本支持守护进程启动环境 `ZVEC_GREP_WATCHER_IDLE_TIMEOUT_SECONDS`，可从默认 4 小时评估改为 600–900 秒。它用于回收项目运行时及 watcher，不会停止整个服务；之后的搜索可能需要重新打开并核对文件变化。已有进程需要按新环境重新启动才生效，本次未调整。
4. **再评估依赖内部优化。** worker 上限可尝试 2–4 个，模型空闲 TTL 可尝试 2–5 分钟，并评估任务历史保留修复。当前 CLI 未暴露前两项配置，需要维护补丁或等待正式版本；会牺牲并发吞吐或增加重新加载延迟，节省量应通过隔离对照实测。

现有模型已很小，暂不优先通过更换模型和重建索引降低占用。当前没有证据需要把搜索全部改成另一种实现，或把模型文件的磁盘大小等同内存。

导入探针及结果：`/private/tmp/zj-zvec-import-memory.py`、`/private/tmp/zj-zvec-import-memory-result.json`、`/private/tmp/zj-zvec-import-memory-no-gc.py`、`/private/tmp/zj-zvec-import-memory-no-gc-result.json`。官方版本研究见 `/private/tmp/zj-zvec-release-research.md`，官方源码核对固定于 main `a09cd1236eee003feb699a17c5bf63d76f83f06c`。本地源码依据为安装包 `dist/cli/index.js`、`dist/cli/commands.js`、`dist/mcp/stdio-bridge.js`、`dist/daemon/model-pool.js`、`dist/daemon/runtime-manager.js`、`dist/daemon/workspace-read-session-cache.js` 和 `dist/engine/models/backends/model2vec-worker-pool.js`。
