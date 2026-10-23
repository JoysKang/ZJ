# ZJ 全面体检

审计日期：2026-10-22。基线：`c171f2ee0d47b1db3fe0a67b408b14df2ea7fcb7`。本报告保留基线问题及当时的证据，修复和复测结果另列。编号发现的行号取自审计基线，修改后的工作区可能有偏移；当前实现请结合函数名和「本轮实现」部分定位。

ZJ 是自用 Rust／GPUI 原生多工作区编辑器，集成文件编辑、Git、终端与 Agent。按单用户、最多 5 个窗口、每窗口最多 20 个文档及并发后台任务检查，资源门槛采用当前 CLAUDE.md。重点是用户内容、操作目标和异步状态一致性。

## 结论

后续补充已完成源码编辑器失焦自动保存、CPU 采样单位修复，并把 CPU 上限按用户要求调整为 1%。2026-10-22 构建为 **629 passed、0 failed、4 ignored**，体积 49.46 MB；2026-10-23 后续交互与当前验证见[补充记录](2026-10-23-ui-followup.md)；详见[自主迭代补充记录](2026-10-22-overnight-followup.md)与[原生运行时补充验收](2026-10-22-native-validation-followup.md)。以下两批记录保留其当时的测试数和测量对象。

基线检查虽然全部通过，审计仍找到了数据丢失、写错目标和旧结果污染界面的具体路径。两批修复完成时，全工作区测试为 627 passed、0 failed、4 ignored，clippy 和 dist 构建通过。当时产物 49.46 MB，单文件窗口 65.50 MB，三窗口加 20 文档 153.39 MB；固定窗口空闲 CPU p95 约 0.72%，低于用户于 2026-10-22 调整后的 1% 上限。已测项目符合新上限，但按键延迟等缺项仍使**完整资源验收未完成**；这次结论变化来自门槛调整，不代表 CPU 性能改善。测试通过不等于真实桌面交互、所有并发时序和性能预算均已验收。

共记录 15 个编号发现：12 项列为必须修复（Important）、2 项功能／刷新问题及 1 项文档问题。它们均相对于本轮实现前基线记录（Pre-existing），现已完成对应修复。第一批处理第 8、13、15 项，另按要求调整自动保存；随后按用户授权修复其余 12 项。原触发与证据保留，不因修改而删除；「尚未执行」等措辞描述发现时的证据边界，当前验证以两批验收记录为准。修复中另发现部分文本替换行序问题，已纳入同模块修复和回归。

证据等级：**运行复现**表示执行实际仓库函数或真实 Git 临时夹具；**源码确认**表示完整调用链支持具体场景，但未强制执行 GUI 并发时序。简化状态模型不等同于 GPUI 集成复现。问题严重性与引入时间分开：多数问题早于最近 5 个 UI 提交，不能归因于图标调整。

## 必须修复

### 1. 另存为后重开原文件会复用文档 ID

- **作用**：文档 ID 关联标签、编辑器、所有权和退出检查。
- **问题**：打开 A→另存为 B→重开 A，B 保留原 inode 构造的 ID，新 A 再取得同一 ID；标签查找命中首个文档，关闭按 ID 删除所有匹配文档。跨窗口时新 owner 覆盖旧 owner，未保存窗口可能从退出检查中消失。
- **修复**：为编辑缓冲区分配稳定且唯一的 ID；磁盘身份仅用于文件查重。不能只在另存为时改 ID，因为编辑事件已捕获它。
- **不修后果**：选中错误文件、同时关闭两个标签，跨窗口存在未获丢弃授权便退出的路径。
- **证据**：真实 GPUI 回归复现（exit 101）：原文件与另存文件均为 `DocumentId { device: 16777229, inode: 82259755 }`，打开 `a.rs` 后 active 实际为 `b.rs`。日志：[save-as-regression.log](../../target/health-check/save-as-regression.log)。[files.rs:670](../../crates/app/src/files.rs:670)、[documents.rs:869](../../crates/app/src/workbench/documents.rs:869)、[workbench.rs:1713](../../crates/app/src/workbench.rs:1713)。原测试只确认两个路径存在；跨窗退出后果为源码链路确认，尚未执行真实退出。

### 2. 手动重新加载会覆盖请求之后的新编辑

- **作用**：外部修改横幅和恢复横幅的「重新加载」用磁盘文本替换缓冲区。
- **问题**：手动入口传入 `only_at=None`；后台返回时不校验发起时路径和文本版本，直接替换内容、清 dirty、删除恢复快照。
- **修复**：所有入口捕获并验证路径、版本和请求身份；不匹配时保留新编辑并提示重试，处理在途保存。
- **不修后果**：点击重新加载之后立即输入或另存为，新内容会被旧读取覆盖。
- **证据**：源码确认；[documents.rs:1255](../../crates/app/src/workbench/documents.rs:1255)、[documents.rs:1293](../../crates/app/src/workbench/documents.rs:1293)。尚未执行受控返回顺序的 GPUI 回归。

### 3. 行内重命名没有复用跨窗口移动保护

- **作用**：资源管理器重命名应让所有打开的文档跟随路径变化。
- **问题**：重命名只更新发起窗口，也未采用拖动移动已有的 saving／MovingFiles 保护。其他窗口仍持旧路径；继续保存会重建旧文件，改名后的文件保留旧内容。
- **修复**：复用移动操作的路径预留、保存串行及 App 层跨窗口完成处理。
- **不修后果**：用户以为保存到改名后的文件，实际生成两个内容不同的文件。
- **证据**：源码确认；[explorer_ops.rs:478](../../crates/app/src/workbench/explorer_ops.rs:478)，对照 [explorer_ops.rs:773](../../crates/app/src/workbench/explorer_ops.rs:773)。

### 4. 正常退出可能留下已保存或已丢弃的恢复快照

- **作用**：保存／丢弃之后删除旧恢复记录，正常退出同步清理。
- **问题**：删除先取走文档的快照 key，再排队 `Remove`；立即退出时 key 已不在文档中，而关闭标志会让后台连 `Remove` 也跳过。
- **修复**：追踪待删除 key，退出时同步完成删除；关闭后禁止新增写入，但不能丢弃已授权的删除。
- **不修后果**：下次启动恢复已丢弃或已过时的文本，用户可能误保存旧版本。
- **证据**：源码确认；[recovery.rs:48](../../crates/app/src/workbench/recovery.rs:48)、[recovery.rs:108](../../crates/app/src/workbench/recovery.rs:108)、[recovery.rs:202](../../crates/app/src/workbench/recovery.rs:202)。尚未强制调度该队列竞态。

### 5. Agent 逐处审阅漏判文件末尾换行变化

- **作用**：逐处接受／拒绝前校验磁盘仍是用户看到的版本。
- **问题**：`full_context_patch` 去掉原行尾再统一加换行，未记录缺少末尾换行标记。`new` 与 `new\n` 生成相同补丁，补丁相等校验无法识别审阅后的独立换行修改。
- **修复**：保留标准 EOF 标记，或使用无损文本／版本指纹校验；不要把有损展示文本当作内容身份。
- **不修后果**：逐处接受或拒绝会处理未展示的新版本；已有的过期保护失效。
- **证据**：直接引用真实 `review.rs`／`diff.rs`，客户端快照接口 mock；`newline_shown_collision=true`、`newline_hunk_result=Ok(())`、拒绝后为 `old`。[review.rs:29](../../crates/agent_client/src/review.rs:29)、[review.rs:178](../../crates/agent_client/src/review.rs:178)。独立复核再次复现。

### 6. Agent 文件读取没有拒绝特殊文件

- **作用**：ACP 读取工作区文件和记录审阅快照。
- **问题**：仅检查 metadata 长度后调用 `fs::read`；命名管道长度通常为 0，读取会等待写端，长度限制不能阻止。调用位于共享 Agent host 的异步处理路径内。
- **修复**：打开前后确认普通文件，使用有界读取并检查增长；把可能阻塞的文件操作与协议事件循环隔离，保留退出能力。
- **不修后果**：读取工作区管道可能挂住会话，影响共享该进程的其他会话。
- **证据**：真实 `fs.rs` 临时 FIFO 复现，250 ms 内未返回；[fs.rs:88](../../crates/agent_client/src/fs.rs:88)、[host.rs:471](../../crates/agent_client/src/host.rs:471)。250 ms 是探测窗口，不是项目超时承诺。

### 7. 单标签推送可能额外发布未选择的标签

- **作用**：按确认框只推送选中的标签。
- **问题**：`tag_push_args` 未设置 `--no-follow-tags`。用户 Git 配置 `push.followTags=true` 时，Git 可顺带推送其他 annotated tag；分支推送已显式关闭该选项。
- **修复**：在共用标签推送参数中加入 `--no-follow-tags`，本地裸仓测试只出现所选 ref。
- **不修后果**：发布范围超出用户确认，可能提前公开其他版本标签。
- **证据**：真实本地 Git 夹具复现，只推 `selected` 却同时出现 `refs/tags/extra`；[write.rs:800](../../crates/git_service/src/write.rs:800)、[scm_actions.rs:292](../../crates/app/src/workbench/scm_actions.rs:292)。无网络推送。

### 8. Git 图旧请求会污染新仓库的图（本轮已实现修复）

- **作用**：多仓库切换、分页和提交详情后台加载。
- **问题**：每个新图实例 generation 从 0 起，首次请求均为 1；替换图未取消旧请求，返回只比较 generation。A 晚到的页面可写入 B，B 的仓库身份却不变。
- **修复**：同时校验仓库和不复用的实例／请求身份，替换或关闭时取消旧任务；详情请求也使用该保护。
- **不修后果**：显示另一仓库的提交，点击后查询对象不存在或详情错位。
- **证据**：基线源码确认；原位置 `graph_view.rs:293,326,395`。本轮在 [graph_view.rs](../../crates/app/src/workbench/graph_view.rs) 增加页面请求的 `Arc::ptr_eq` 身份检查、独立详情取消令牌及 `Drop` 取消；旧定位行号不代表修改后的函数位置。

### 9. 按行暂存夹带可执行权限变化

- **作用**：仅暂存或还原选中的文本行。
- **问题**：部分补丁保留原始 `old mode`／`new mode` 头；100644 文件改文本并 chmod 755 后，只暂存一行也会暂存 mode 变化。
- **修复**：已有文件的按行补丁不携带整文件 mode 变化；新文件保留必要的 `new file mode`，分别覆盖 Stage／Undo。
- **不修后果**：「只选文本」与实际写入 index 的内容不一致。
- **证据**：直接编译真实 `partial_patch.rs` 并在本地 Git 夹具应用，`git diff --cached --summary` 输出 `mode change 100644 => 100755 f`；[partial_patch.rs:77](../../crates/app/src/partial_patch.rs:77)、[partial_patch.rs:247](../../crates/app/src/partial_patch.rs:247)。

### 10. 大文件前部改写后行索引仍沿用旧值

- **作用**：大文件受限查看中的行号、跳转和搜索。
- **问题**：同 inode、长度不减、旧末尾 256 字节相同即被认作追加。前部换行改动而尾部不变时复用旧检查点，并赋予新 stamp，使后续验证通过。
- **修复**：无法证明旧前缀不变时完整重建；不能仅以尾部样本证明追加。
- **不修后果**：行号错位，跳转、复制和搜索对不上实际文件。
- **证据**：直接引用真实 `large_file.rs` 运行：增量 2078 行、完整索引 2077 行；第 1025 行增量读为 `row 1024`，正确为 `row 1025`。[large_file.rs:82](../../crates/app/src/large_file.rs:82)。

### 11. 路径变化没有同步恢复标签、排序和会话缓存

- **作用**：重启恢复、延迟打开标签与原有顺序。
- **问题**：另存为／重命名／移动只更新已加载文档；`pending_tabs`、`tab_order` 和会话 Tracker 未同步。尚未加载的标签保留不存在路径；另存后直接退出仍恢复旧文件，Untitled 首次保存可能不恢复。
- **修复**：在统一路径变更完成点更新已加载／待加载标签和顺序，再刷新会话；跨窗遍历包含只有待加载标签的窗口。
- **不修后果**：幽灵标签、顺序变化、重启丢标签或打开旧文件。
- **证据**：源码确认；[explorer_ops.rs:544](../../crates/app/src/workbench/explorer_ops.rs:544)、[workbench.rs:1545](../../crates/app/src/workbench.rs:1545)、[documents.rs:869](../../crates/app/src/workbench/documents.rs:869)、[session.rs:281](../../crates/app/src/session.rs:281)。排序状态问题涉及最近标签修复，其他路径同步问题既存。

### 12. Agent 临时文件创建失败会删除非本次创建的文件

- **作用**：Agent 原子写入的临时文件清理。
- **问题**：`create_new` 因同名文件存在失败后，公共错误分支仍执行 `remove_file`；PID 与计数不能证明既有文件属于本次调用。
- **修复**：成功创建后才取得清理权，创建失败直接返回。
- **不修后果**：残留或他方创建的临时文件被误删。
- **证据**：直接引用真实 `fs.rs`，预建冲突文件后返回 `AlreadyExists` 且 `preexisting_temp_survives=false`；[fs.rs:134](../../crates/agent_client/src/fs.rs:134)。这是项目明确禁止的同类错误。

## 应修复与交互改进

**整文件拒绝的产品加固建议，不计为实现违约**：运行确认该动作直接恢复 Agent 前快照，会覆盖后续独立修改。但 ADR 0004 明确采用整文件恢复语义，不能将这个行为本身报成实现错误。单文件按钮缺少全部拒绝时的警示，建议显式说明影响范围并校验所见版本；逐处 EOF 校验漏洞仍是独立缺陷。终端 OSC 52 读取剪贴板的怀疑已排除：锁定依赖默认 `OnlyCopy`，解析器在产生读取事件前阻断，未发现读取泄露证据。

### 13. 标签正文没有进入数据模型（本轮已实现）

- **作用**：Git 图中查看 annotated tag 的说明。
- **问题**：图查询 `%D` 只取得名称，`GraphRef` 没有正文；提交消息不等于标签消息。UI 没有详情入口。
- **修复**：按需后台读取标签对象，在只读详情显示名称、创建者、日期和完整正文；轻量标签明确无独立说明。
- **不修后果**：用户在 ZJ 创建了说明，却不能在 ZJ 查看。
- **证据**：真实 Git 对象存在正文而图数据未读取；[graph.rs:15](../../crates/git_service/src/graph.rs:15)、[graph_view.rs:1162](../../crates/app/src/workbench/graph_view.rs:1162)。用户已要求实现，结果在最终交付记录中说明。

### 14. 文档与测量工具的说明已经过时

- **作用**：README 告知当前能力，资源脚本解释门槛。
- **问题**：README 仍称没有异常恢复、Markdown 预览待接入、Kit 0.7.0；测量脚本末行仍打印二进制目标 30 MB，当前标准为 50 MB／52 MB。
- **修复**：按当前实现更新这些明确不符的表述；预算输出从统一值生成。
- **不修后果**：误判功能缺失或预算超标，后续开发依据错误现状。
- **证据**：[README.md:3](../../README.md:3)、[README.md:7](../../README.md:7)、[README.md:61](../../README.md:61)、[measure_budget.py:406](../../tools/measure_budget.py:406)。

## 用户补充反馈：自动保存与 Diff 刷新

基线已有自动保存实现，但默认 `off`。本轮按用户要求将默认值改为 `afterDelay`（编辑停顿 1 秒后保存）；旧配置明确设置的 `off` 仍保留。macOS「文件 → 自动保存」及命令面板提供模式切换，配置键为 `auto_save`，值为 `off`／`afterDelay`／`onFocusChange`。未命名文件不自动弹保存面板；已删除文件不会被自动重建。延时回调现在复核当前模式，切换到关闭或失去焦点模式后，不再执行原先排队的延时保存。

初次体检时 `onFocusChange` 只接入窗口失活和编辑区标签变化，同窗输入控件切换不触发；后续已接入源码编辑器的 `InputEvent::Blur`，复用既有保存保护。真实 GPUI 焦点切换回归移除该分支时失败，恢复后自动保存专项 22 项通过；最终全工作区 629 passed、0 failed、4 ignored。当时尚未扩展 Markdown 块编辑失焦和菜单勾选；2026-10-23 已继续实现这两项，验证结果见[未完成交互补充](2026-10-23-ui-followup.md)。此前记录见[自主迭代补充记录](2026-10-22-overnight-followup.md)。

### 15. 排队的另一仓库刷新会抹掉当前 Diff 的重载请求

- **作用**：监听仓库变化并刷新当前工作树／暂存区 Diff。
- **问题**：A 的 Diff 失效后开始 A status 刷新，此时 B 的刷新排队。A 完成先启动 B，`refresh_repos` 把 `diff.stale` 直接赋为「本次 targets 是否包含 A」，因此清掉尚未消化的重载请求；两个完成回调最终都不加载 A 的新补丁。
- **修复**：待重载状态应累积，直到真正加载或关闭时消化；取消旧 Diff 请求后也需确保 loading 能完成。
- **不修后果**：多仓库并发变化后，Diff 长期停在旧内容，或被取消的加载状态不结束。
- **证据**：[workbench.rs:1838](../../crates/app/src/workbench.rs:1838)、[workbench.rs:2120](../../crates/app/src/workbench.rs:2120)、[workspace_refresh.rs:396](../../crates/app/src/workbench/workspace_refresh.rs:396)。真实原生 watcher／Git fixture 验证 `NATIVE_EDIT_EVENT=true`、`PLAN_A=true`、`PATCH_CHANGED=true`、`STALE_AFTER_ASSIGNMENT=false`。后续已完成两条真实 GPUI 回归：旧码均失败（旧补丁／加载不结束），将 `stale` 改为累积后两条均通过（2 passed、0 failed）。日志：[diff-refresh-red.log](../../target/health-check/diff-refresh-red.log)、[diff-refresh-green.log](../../target/health-check/diff-refresh-green.log)。本条已修复，并通过两批最终全量验证。

这是一条能解释用户反馈的确定代码路径，未声称就是当时现场。已排除「M 状态没变所以服务不返回新补丁」：M→M 时版本和补丁实测均变化。外部暂存后工作树 Diff 变空、暂存 Diff 更新；提交后暂存 Diff 变空，也符合语义。历史提交 Diff 固定提交哈希，仓库后续变化时保持原文是正确行为。

## 本轮实现与验收范围

| 项目 | 已实现行为 | 验收范围 |
| --- | --- | --- |
| 自动保存默认开启 | 缺省设置为编辑停顿 1 秒后保存；显式 `off` 保留；延时结束复核模式 | 设置解析与 GPUI 保存／关停回归，最终全量结果另列 |
| 标签说明（问题 13） | 直接点击图中标签、提交详情中的标签或右键菜单，查看创建者、日期、完整正文并复制；轻量标签明确没有独立说明 | Git 服务夹具 1 passed；图视图 focused 14 passed、0 failed，包含真实 GPUI 元素点击及选择／关闭／重开／删除时序；标签对象上限 256 KiB，超限显示错误；签名文本保留但不验证签名 |
| Git 图请求隔离（问题 8） | 页面和详情回填核对请求身份；替换、关闭和销毁取消请求 | 随标签功能修复；不再仅凭可复用的 generation 接收结果 |
| 多仓库 Diff 刷新（问题 15） | 保留尚未消化的重载标记，直到实际加载或关闭 | 两条真实 GPUI RED／GREEN 回归已通过；不能据此断言覆盖用户当时未知的全部现场 |

以上为第一批实现，未改 Agent 运行时、文件身份模型、手动 reload 或审阅写入行为，未增加依赖。标签复制保留解码后的完整文本；遇到非 UTF-8 字节会明确提示替代解码，因此不承诺字节级无损复制。多 Agent 的运行时选择、能力门槛和建议预算见 [低内存运行时方案](2026-10-22-multi-agent-runtime.md)。随后用户授权继续修复剩余问题，第二批记录如下。

### 第二批修复

- 文档生命周期（1、2、3、4、11）：稳定缓冲区 ID 与磁盘身份分开；重新加载检查发起时路径、版本及磁盘状态；重命名复用保存／移动预留，跨窗口同步已打开和延迟标签、顺序、会话；退出同步处理待删除恢复记录，同 key 多次删除按计数追踪。
- Agent（5、6、12）：无末尾换行使用标准补丁标记；打开后验证普通文件并限制实际读取，正文读盘移到后台且等待可被停止；创建临时文件失败不取得清理权。聚焦测试 82 passed、1 ignored，clippy 通过；`blocking` 已有间接依赖改为直接引用，正常依赖数 629 → 629，无新增包。
- Git（7、9）：标签操作显式关闭 follow-tags；已有文件的部分文本补丁不携带 mode 变更，新文件保留必要 mode。标签回归已 RED → GREEN；测试另外发现相邻／不等长替换的选中行位置问题，已改为复用 Diff 显示层的单调行映射，避免第二套配对算法，App 集成回归已包含在 445 passed、3 ignored 的完整套件中。
- 大文件（10）：删除仅凭尾部样本判断追加的索引复用，统一后台重建，并在完成时复核文件 stamp。持续增长的日志每次刷新都重扫，正确性优先，内存仍为稀疏索引与固定读缓冲。
- 文档与测量（14）：README 更新恢复、Markdown 预览、受限大文件查看、标签详情、当前锁定版本与会话恢复说明。预算工具统一 50／52 MB 常量，CPU 按实际单调时间间隔计算，p95 使用向上取整名次。默认预算窗口保持加载完成后 10 秒起取样 30 秒；可选较晚窗口仅作诊断，独立写入 JSON，拒绝重叠窗口并记录实际开始时间。

最终检查使用项目 `CARGO_HOME` 和系统临时目录，结果如下：

| 命令 | 真实结果 |
| --- | --- |
| `cargo test --workspace --locked` | exit 0；**627 passed、0 failed、4 ignored**，其中 App 445 passed、3 ignored；[完整日志](../../target/repairs-test.log) |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0；`Finished dev profile … in 5.09s`，仅既有 `block v0.1.6` future-incompat 提示；[日志](../../target/repairs-clippy.log) |
| `cargo build --profile dist --locked` | exit 0；`Finished dist profile … in 1m 22s`；[日志](../../target/repairs-dist.log) |
| `cargo fmt --all --check` | 最后一次测试断言格式调整后再次 exit 0，无输出 |
| `git diff --check` | exit 0，无输出 |
| `python3 tools/check_git_deadlines.py` | exit 0；timeout 1.007 s、parent_exits 1.003 s、cancel 1.013 s；`PASS: timeout, exited parent, cancellation; no live descendant remains` |

最终产物为 49,461,456 bytes：比保留的第一批产物增加 16,592 bytes，比审计前产物增加 33,184 bytes，低于 50 MB 目标。这个差值是本地构建产物比较，不是逐依赖归因。clippy 首轮发现两处新测试断言的无必要 clone，改为 `std::slice::from_ref` 后通过；只调整断言结构与换行，未重复执行已通过的完整测试。

第一阶段 `ponytail-review` 独立复核：Agent 组 0 发现；文档组发现异步硬链接打开遗漏身份回退，已修并补回归；大文件组 0 发现；Git 部分补丁发现不等长替换错位，改用显示层对齐后复核为 0 项未解决。测量工具发现较早诊断窗口会被错误标为请求时间，已拒绝重叠参数并记录实际起点，复核为 0 项未解决。

第二阶段 `code-review`：**Standards 0 项未解决，Spec 0 项未解决**。应用代码复核及随后工具／README 增量复核均分别通过。审阅者只读检查，没有用审阅结论代替执行结果。

重命名新回归首次未将 Enter 派发到输入框，修正为渲染后真实按键，单项 1 passed，随后全量通过；此前通用等待失败不能解释为某个后半步骤已经通过。

## 基线验证

以下命令在指定基线运行，`CARGO_HOME=/Users/joys/work/code/ZJ/target/cargo-home`。日志位于 [target/health-check](../../target/health-check)。

| 命令 | 真实结果 |
| --- | --- |
| `cargo fmt --all --check` | exit 0，无输出 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0；仍有 Cargo 的 `block v0.1.6` future-incompat 提示 |
| `cargo test --workspace --locked` | exit 0；603 passed、0 failed、4 ignored |
| `python3 tools/check_git_deadlines.py` | exit 0；timeout 1.014 s，parent_exits 1.011 s，cancel 1.008 s；无残留子进程 |
| `cargo build --profile dist --locked` | exit 0；49,428,272 bytes |

首次测试因验证者把 TMPDIR 放进 `target/`，索引按排除规则过滤夹具而失败；换到仓库外临时目录后全通过。原始失败日志保留为 `test-tmpdir-inside-target.log`，不得把该轮误报为代码回归。历史 CI 的 `remote_check_survives_an_ordinary_refresh` 本次全量加 3 次单测均通过；watch debounce 导致额外取消／重试是待确认时序，尚无失败事件顺序证据。

4 个忽略项为 3 个性能探针及真实 Agent handshake，不代表真实 CLI 已验收。

### 资源首轮实测

命令：`python3 tools/measure_budget.py --runs 3 --idle-seconds 15 --skip-latency --output target/health-check/resources-before.json`，exit 0；设置、会话、历史与文件都隔离在临时目录。结果是短时观测，期间与其他后台验证有重叠，不作为长期性能结论。

| 指标 | 实测 | 当前目标／上限 | 判断 |
| --- | ---: | ---: | --- |
| dist 体积 | 49.43 MB | 50／52 MB | 达标 |
| 首帧中位数 | 193.88 ms | 250／400 ms | 中位达标 |
| 首帧最慢一次 | 1190.09 ms | 400 ms | 存在离群值，需安静环境复测 |
| 单窗口文件 | 52.94 MB | 70／100 MB | 达标 |
| 单窗口欢迎页 | 54.90 MB | 70／100 MB | 达标 |
| 三窗口＋20 文档 | 251.99 MB | 220／300 MB | 超目标，未超上限 |
| 文件窗口空闲 CPU p95 | 0.237% | ≤0.1% | 首轮超上限，需复测 |
| 欢迎页空闲 CPU p95 | 0.022% | ≤0.1% | 达标 |

文件窗口测量期间出帧 3 次，欢迎页 0 次，两者 display link 唤醒计数均为 0。不能据此将 CPU 开销归因于绘制。

第二轮仍使用同一基线 dist，`--runs 5 --idle-seconds 30 --skip-latency`：首帧中位 200.30 ms、最慢 207.60 ms；单窗口文件 65.39 MB、CPU p95 0.830%；欢迎页 52.72 MB、CPU p95 0.1003%；三窗口 20 文档 153.39 MB。启动离群值未复现，内存与 CPU 仍波动，期间与编译重叠。这不是严格 A/B，不能据此宣称改动提升或退化；最终构建完成后需在无本轮构建竞争时单独复测。原始结果见 [resources-repeat.json](../../target/health-check/resources-repeat.json)。另用 0.25 秒受控 CPU 循环核对计时单位：`time.process_time` 为 0.250001 s，当前 Mach timebase 换算为 0.250024 s；不换算仅 0.006001 s。因此本机脚本的时间换算有交叉证据，不能把 CPU 超标简单归因于单位错误。

## 第一批实现的最终验证

以下结果对应自动保存、标签详情和 Diff 刷新这一批，尚不包含随后授权的剩余问题修复。使用项目 `CARGO_HOME` 与系统临时目录。

| 命令 | 真实结果 |
| --- | --- |
| `cargo fmt --all --check` | exit 0，无输出 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | exit 0，仅既有 `block v0.1.6` future-incompat 提示 |
| `cargo test --workspace --locked` | exit 0；611 passed、0 failed、4 ignored |
| `cargo build --profile dist --locked` | exit 0；49,444,864 bytes，较基线增加 16,592 bytes |
| `python3 tools/check_git_deadlines.py` | exit 0；timeout 1.011 s、parent_exits 1.009 s、cancel 1.015 s，无残留后代 |

全量日志为 `target/health-check/final-{fmt,clippy,test,dist-build,git-deadlines}.log`。编译期间发现的标签元素 ID 类型错误已修正，以上是修正后的结果。

编译结束后独立执行 `python3 tools/measure_budget.py --runs 5 --idle-seconds 30 --skip-latency --output target/health-check/resources-final.json`，exit 0。结果见 [原始 JSON](../../target/health-check/resources-final.json)：首帧中位 183.65 ms 达标，但最慢 764.50 ms 超过 400 ms 上限；单文件窗口 81.84 MB 超过 70 MB 目标、低于 100 MB 上限；欢迎页 54.95 MB；三窗口＋20 文档 167.92 MB。空闲 CPU p95 文件窗口 1.236%、欢迎页 0.943%，均超过 0.1% 上限。**资源验收未通过**；不能把脚本 exit 0 或中位数达标当作全部预算通过。按键延迟未测。

## 第二批资源复测方法

应用构建和检查结束后，用最终测量脚本顺序测量保留的第一批产物与第二批产物。两次均为 `--runs 5 --idle-seconds 30 --steady-window 60 --skip-latency`，配置、会话、历史和工作区采用临时夹具。预算窗口始于加载完成后约 10 秒，连续取样 30 秒；诊断窗口请求从加载完成后约 70 秒起再取样 30 秒，JSON 保留实际起点。原预算样本不裁剪，不因出现高值重跑挑选低结果。

本轮修正了脚本中「以固定 0.2 秒代替真实采样间隔」的误差，历史 CPU 百分比与新口径不完全可比。最终前后比较均使用修正后的同一脚本，仍只是各一次运行，不能据此建立长期性能结论。第一批已经记录的首帧离群值和 CPU 超标保留为历史事实。

前期短时探针既出现整段低 CPU，也出现约 2 秒突发；突发采样栈包含 AppKit 自动填充延迟回调和输入源通知。没有取得能把全部波动归因于同一原因的证据，也未据此修改 vendor 或关闭系统功能。按实际间隔计时只能修正测量偏差，不能保证消除超标。

两次实际命令均 exit 0，各约 3 分 38 秒；脚本退出成功只代表完成测量。

```bash
python3 tools/measure_budget.py --binary target/health-check/pre-lifecycle-workspace-editor --runs 5 --idle-seconds 30 --steady-window 60 --skip-latency --output target/health-check/resources-before-repairs.json
```

```bash
python3 tools/measure_budget.py --binary target/dist/workspace-editor --runs 5 --idle-seconds 30 --steady-window 60 --skip-latency --output target/health-check/resources-after-repairs.json
```

| 指标 | 第一批产物 | 第二批最终产物 | 当前目标／上限 |
| --- | ---: | ---: | --- |
| 二进制 | 49,444,864 bytes | 49,461,456 bytes | 50／52 MB |
| 首帧中位数 | 186.62 ms | 192.40 ms | 250／400 ms |
| 首帧最慢一次 | **1075.89 ms** | 199.02 ms | 400 ms |
| 单文件窗口 footprint | 65.37 MB | 65.50 MB | 70／100 MB |
| 欢迎页 footprint | 55.02 MB | 55.07 MB | 70／100 MB |
| 三窗口＋20 文档 footprint | 152.65 MB | 153.39 MB | 220／300 MB |
| 文件窗口空闲 CPU p95 | 0.0148% | 0.7238% | ≤1% |
| 欢迎页空闲 CPU p95 | 0.6947% | 0.7218% | ≤1% |

独立诊断窗口的文件 CPU p95 为 0.7033% → 0.9514%，欢迎页为 0.7197% → 0.0119%；实际起点均在预算起点后 60.00～60.01 秒。较晚欢迎页的低值不替代上表预算窗口结果。原始记录：[修复前 JSON](../../target/health-check/resources-before-repairs.json)、[修复后 JSON](../../target/health-check/resources-after-repairs.json)、[修复前日志](../../target/health-check/resources-before-repairs.log)、[修复后日志](../../target/health-check/resources-after-repairs.log)。

**按新门槛重判：已测空闲 CPU 低于 1% 上限，完整资源验收仍缺按键延迟等检查。** 用户于 2026-10-22 明确把 CPU 上限从 0.1% 调整为 1%，接近 0% 的目标不变；不改写早期测量和当时门槛下的历史结论。 本次新构建的体积、内存和 5 次启动均达目标；旧构建再次出现启动离群值，不能宣称启动波动已根治。单文件 footprint 差值为 +0.13 MB，欢迎页 +0.05 MB，三窗口 +0.74 MB；二进制 +16,592 bytes，正常依赖数 629 → 629。本次没有触及增加 200 KB／5 MB 的决策记录阈值，单轮差值也不能独立归因于 `blocking` 的直接引用。

工具自检：p95 的空、单个及 5／20／40／100 个样本名次断言通过；模拟 0.01 秒 CPU／0.5 秒实际间隔得到 2.0000%；重叠窗口参数 `--idle-seconds 30 --steady-window 10` 返回 exit 2。新时间字段由上述最终两份 JSON 验证，早期 `tool-selfcheck.json` 使用旧字段，仅保留为历史证据。

## 分阶段复核

1. `ponytail-audit`／`ponytail-review`：按文档生命周期、Git／异步、Agent／终端和资源分工，追调用链并用临时夹具反证。
2. `code-review` Standards 轴：确认 reload、快照删除、临时文件清理、大文件索引与项目约定不一致；近期排序状态未随路径迁移。自定义图标是用户已批准例外，不报违规。
3. `code-review` Spec 轴：确认文档身份、重命名、会话缓存、Git 图身份及路径排序违背数据可靠／恢复／异步隔离要求。原始规范在仓库内，无 issue tracker 核验。

两轴是交叉复核，不把重复问题累计为新增数量。一般保存完成的版本检查、Save As 目标预留、拖动移动保护、关闭确认版本检查、Git 超时和子进程清理已有实现或测试证据，未列为问题。

本轮实现另外按顺序复核：

- `ponytail-review`：未确认新增运行时缺陷；发现恢复测试仍依赖默认关闭自动保存，需要在该夹具中显式设为 `Off`。此项由全量测试验证，不把旧测试偶然通过视为充分证据。
- **Standards**：0 findings。标签查询与回填、取消、输出限额、theme token 和静默 Diff 刷新符合项目约定；未发现有具体影响的代码异味。
- **Spec**：0 findings。默认保存、显式关闭、标签详情与复制、轻量标签空态和多仓库 Diff 刷新均有对应实现；图实例取消属于异步正确性的配套修改。升级和多 Agent 调研明确标记未实施部分。

审阅者未运行 Cargo／真实 GUI，源码复核不能替代最终执行结果。其后编译检查发现标签点击元素 ID 的类型不受 GPUI 支持，已改为 `SharedString`；点击测试登记仅在 `cfg(test)` 下启用。主审定点检查这部分增量，图视图 focused 14 项通过。恢复快照测试已显式设为 `Off`，只改变该测试的未保存前提，未覆盖产品默认值。

## 后续工作

1. 完成真实 macOS 鼠标键盘、中文 IME、系统保存面板、VoiceOver 和按键延迟验收；GPUI 测试窗口不能替代这些检查。
2. 对资源测试中超标或波动的指标保留独立跟踪，不用延后采样的诊断结果替代固定预算窗口。
3. 官方版本核验恢复后，将 Rust、GPUI 及 vendor 迁移作为独立变更。Diff 新交互和 Agent 新运行时按各自调研的验收门槛推进。

当前改动未提交、未推送。

## 未检查与升级限制

- 原生辅助功能查询返回 `false`，没有完成桌面窗口的真实鼠标键盘操作、中文 IME、VoiceOver 或按键延迟验收。标签点击已通过 GPUI 测试窗口的实际元素命中测试，但没有真实 macOS 窗口截图或手工选中复制验收。已启动真实 dist 程序进行隔离资源测量，不把启动当完整交互验收。
- 未执行真实 Agent 付费会话、真实远端认证／推送／签名 hooks；Git 写验证只在临时本地仓库。
- Agent 正文读盘已脱离协议分发并支持等待超时／取消，但操作系统阻塞中的网络文件系统读取不能被强制中断，后台线程可能暂留；路径规范化和既有写入路径未在本轮全面改为异步。
- 大文件变化时完整重建稀疏索引，避免错误复用；持续增长日志会增加扫描 I/O，尚未做长时吞吐测量。
- 未逐行覆盖整个 vendor、全部语法查询和所有文件格式；未运行长时间压力、磁盘满／断电测试、cargo-deny advisories。
- 官方 crates.io、Rust stable channel 和 GitHub 发布页受工具域名校验阻断，未可靠核验最新版本，未升级依赖。现有锁定 Rust 1.99.0、Kit 0.7.1、GPUI 0.3.8。
- Diff 借鉴方向、vendor 迁移清单和来源限制见 [升级与 Diff 调研](2026-10-22-upgrades-and-diff-research.md)。优先考虑未改动区域折叠、比较来源标识、跨文件审阅队列；外部实现和许可证未核验前不复制代码。
