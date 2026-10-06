# ADR 0004：Agent 走 ACP，会话历史存本地 SQLite

日期：2026-10-02。状态：已采纳（界面在第 6 批之后接入）。

## 背景

用户要在右侧面板里用 Claude Code、Codex、Gemini CLI，以及「Claude Code · DeepSeek」，并且能找回、钉住、删除以前的会话。要求：

- 质量优先，接入方式尽量通用；
- Agent 标识用单色字形；布局以 A（会话列表 + 对话）为主，B / C 按需出现；
- 文件直接落盘，审阅对比 Agent 改动前的快照（2026-10 起去掉了「接受后才落盘」，见文末更新）；
- 不导入 `~/.claude`、`~/.codex` 的原生历史；
- 历史默认按工作区区分，可以切到「全部工作区」；本地全文搜索；
- 会话可以删除，删除要真正删掉数据库里的记录，库文件不能一直变大。

## 决定

### 1. 所有 Agent 都走 ACP（Agent Client Protocol v1）

新增 `crates/agent_client`（包名 `workspace-editor-agent`，不依赖 GPUI），用官方 Rust crate `agent-client-protocol =2.2.0`。这个 crate 不依赖 tokio，基于 `futures` / `async-io`；每个 Agent 一个监督线程，用 `async_io::block_on` 驱动连接。

| 预设 | 启动方式（先找本机命令，再用 ZJ 装好的 npm 包）| 字形（Lucide）|
| --- | --- | --- |
| Claude Code | `claude-agent-acp`，或 `node <数据目录>/…/@agentclientprotocol/claude-agent-acp@0.85.0` | `asterisk` |
| Codex | `codex-acp`，或 `node <数据目录>/…/@agentclientprotocol/codex-acp@2.1.1` | `square-terminal` |
| Claude Code · DeepSeek | 同 Claude Code，另加环境变量：`ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic`、`ANTHROPIC_AUTH_TOKEN`（设置里填，或取自 `DEEPSEEK_API_KEY`）、`ANTHROPIC_MODEL` 等按 DeepSeek 官方文档设为 `deepseek-v4-pro[1m]` / `deepseek-v4-flash[1m]` | `fish` |
| 用户自定义 | 设置 JSON 的 `agents` 数组：`id / name / command / args / env`（`"$NAME"` 表示取环境变量）| `bot` |

新会话默认用 Codex（`agent.default_agent`，设置页「新会话默认使用」可以改）。

版本号与 ACP registry（2026-10-01）一致。Gemini CLI 的预设已去掉（暂不考虑；`sparkle` 字形和配色保留，以后可以加回）。Claude Code 和 Codex 的适配器在 registry 里都只有 npm 包（TypeScript），没有原生二进制，所以两者都离不开 Node.js。本机装了命令（`npm i -g @agentclientprotocol/claude-agent-acp` / `codex-acp`）就直接启动它。否则第一次使用时自动安装（`provision`）：npm 包装进 ZJ 数据目录（macOS 为 `~/Library/Application Support/ZJ/agents`），用 `node <入口>` 启动，不经过 npx（省掉常驻约 120 MB 的 `npm exec`）；本机没有 Node.js 22+ 时下载固定版本 Node.js 24.21.0（系统 `curl` / `tar`，校验 sha256）。安装先放进临时目录，完成后 rename 并写标记，中断不会留下半装好的目录。本机 `claude` / `codex` 版本够新时通过 `CLAUDE_CODE_EXECUTABLE` / `CODEX_PATH` 交给适配器，并用 `--omit=optional` 跳过适配器自带的 CLI（Claude 284 MB → 60 MB）。Node.js 不打进 `.app`：二进制预算放不下，适配器更新也不必跟着 ZJ 发版。进度用 `AgentEvent::Progress` 显示；安装失败或缺 Key 时给出中文提示。从 Finder 启动时 PATH 只有系统目录，所以搜索路径补上 Homebrew、nvm、mise（`installs/node/<最新版本>/bin` 和 `shims`，认 `MISE_DATA_DIR`）、volta、bun、pnpm 等常见位置，子进程的 PATH 以找到的 node 所在目录打头。

**进程**：工作区根目录作为 cwd，独立进程组；去掉继承的 `GIT_*`、`ZJ_*`、`CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `CLAUDE_CODE_SSE_PORT`（否则 Claude Code 会拒绝「嵌套启动」）；退出时先关 stdin，再对整组 SIGTERM，1.5 秒后 SIGKILL。stderr 只保留最后 16 KB，崩溃时显示给用户，不写日志。

**没有文件夹时**：窗口没有打开文件夹（从 Dock 或 Finder 启动）也能直接对话，会话在默认工作区里运行：macOS 是 `~/Library/Application Support/ZJ/workspace`，其他平台 `$XDG_DATA_HOME/zj/workspace`，`ZJ_AGENT_WORKSPACE` 可覆盖；第一次启动 Agent 时才创建这个空目录。历史按这个路径记录，没有文件夹的窗口里会话列表、⌘J 的范围是「默认工作区」，在其他工作区里这些会话标成「默认工作区」，筛选方式和普通工作区一样。会话的工作区在 Agent 第一次启动时确定，之后窗口再打开文件夹也不变；从历史打开的会话回到它原来的工作区。默认工作区不建文件索引，`@` 只能引用已打开的文件。

**协议**：`initialize` → `session/new`（`cwd` = 工作区根目录，`mcpServers` 为空）→ `session/prompt`。提示内容是文本加 `resource_link`（文件；选区带 `#L起:止`），Agent 声明 `embeddedContext` 时选区原文内嵌。`session/update` 转成类型化事件：回复 / 思考片段、工具调用（类别、状态、位置、Diff）、计划、可用命令、模式、模型设置、用量、标题。模型设置取自 `configOptions`（会话开始、`config_option_update`、`session/set_config_option` 的响应），只保留 `model` / `thought_level` / `model_config` 类的单选项，在输入框下方合并成一个菜单（如「Opus · High」）；`mode` 类交给已有的模式菜单（带预设的过滤），其余类别不显示，`set_config_option` 也只接受最近一次列出的选项和取值。可用命令在输入框开头输入 `/` 时补全（Agent 第一次启动前还没有列表，不为此提前启动进程）；以 `/` 开头的消息把文字放在附件前面，适配器只认第一段文字里的命令。`session/cancel` 中断，同时把未答的权限请求一律回 `cancelled`。权限请求作为事件交给界面，界面调用 `respond_permission` 作答；等待期间不占用 ACP 的分发循环。

**登录**：`initialize` 声明 `auth.terminal`（仅 macOS）。`session/new`（Codex）或 `session/prompt`（Claude Code，会话能建，发提示词时才拒绝）返回 `auth_required` 时，客户端发出 `AgentEvent::AuthRequired`，列出 Agent 给的登录方式，面板显示登录卡片，这一轮保持进行中，提示词暂存。Agent 自己的方式（Codex 的 ChatGPT 打开浏览器、API Key 读环境变量）走 `authenticate`，最多等 10 分钟，成功后自动重试；`terminal` 方式（Claude 的订阅 / Console 登录）写一个只有本人可读的 `.command` 脚本，在「终端」里运行适配器加该方式的参数，用户完成后点「已登录，重试」。脚本只带 `PATH` 和本机 CLI 路径变量，不写入 Key 等其他环境变量，运行后自删。重试仍未登录时卡片显示「仍未登录」；在 `session/prompt` 阶段重试用同一个会话重发暂存的提示词。等待登录期间取消或空闲超时会结束这一轮。ZJ 不读取、不保存任何凭据，登录状态由 Agent 自己管理。

**客户端能力**：

- `fs/read_text_file`：先取编辑器里未保存的缓冲区（`BufferProvider`），再读磁盘。`BufferProvider` 是异步的：编辑器在 UI 线程上回答，客户端最多等 5 秒，`AgentClient` 关闭时立即放弃，所以关会话不会因为等编辑器而卡住；路径必须在工作区内（规范化后比较，拒绝 `..` 和符号链接逃逸），16 MB 上限。
- `fs/write_text_file`：先记下文件改动前的内容，再原子写入并发出 `FileWritten`，编辑器据此重载。
- `terminal/*`：首版不声明。Agent 用自己的工具执行命令，执行前走权限请求；编辑器不实现终端。

**空闲与重启**：没有进行中的回合、也没有待答的权限请求时，空闲 N 分钟（默认 10）结束进程，状态栏 / 面板收到 `Exited { Idle }`。下次提问时透明重启：Agent 支持 `loadSession` 就恢复原会话（重放的历史不再发给界面），否则新建会话。崩溃（`Exited { Crashed }`）之后同样在下次提问时重启。

**有界**：事件通道 512 条（界面不读时会反压到 Agent），命令通道 64 条，单行 JSON 32 MB，stderr 16 KB，读文件 16 MB。日志只写 `event=agent_spawn / agent_exit / agent_turn_end …` 和 pid、结果类别，不写提示、回复、文件内容、环境变量值。

### 2. 会话历史：SQLite + FTS5

新增 `crates/agent_history`（包名 `workspace-editor-agent-history`），`rusqlite =0.40.2`，`bundled`（SQLite 3.53.2），关掉默认的语句缓存。

- **位置与打开**：macOS 是 `~/Library/Application Support/ZJ/agent.db`，其他平台 `$XDG_DATA_HOME/zj/agent.db`，`ZJ_AGENT_DB` 可覆盖。第一次调用才建文件、开连接、起写线程；文件 0600（`-wal` / `-shm` 跟随），目录 0700；WAL、`synchronous=NORMAL`、每个连接 1 MiB 页缓存、`mmap_size=0`、外键开启；`auto_vacuum=INCREMENTAL` 在建表前设置。迁移按 `user_version` 编号执行。
- **表**：`workspaces`、`sessions`（Agent、ACP 会话 id、标题及来源、仓库、分支、创建 / 更新时间、`pinned_rank`、归档、状态）、`messages`、`session_files`。
- **写入**：一个后台写线程，队列 4096 条，一批一个事务，提交后才回复；`flush()` 返回期间的写错误，不吞掉。
- **搜索**：两张无内容（contentless，`contentless_delete=1`）FTS5 表，正文只存一份：
  - `trigram` 表：≥ 3 个字的子串，中英文都适用，大小写不敏感；
  - CJK 二元词组表：写入时把连续汉字 / 假名 / 谚文切成重叠的两字词，`unicode61` 分词，用来查「重连」这类两个字的词；
  - 单字（或两个字但不全是 CJK）：退回到标题、文件路径、Agent / 仓库 / 分支的 `LIKE`，不扫正文。
  - 多个词要求在同一会话里都命中（可以在不同消息里）；每个会话一条结果，钉住的在前，其余按命中位置（标题 > 文件 > 元数据 > 正文）、命中次数和时间衰减（半衰期 14 天）排序；返回摘要和高亮字节范围。没有用 `bm25()`：常见词命中几千行时它比整次搜索的其余部分都慢。
  - 范围：默认当前工作区，可选全部；筛选 Agent、仓库、时间、状态、只看钉住、归档（排除 / 只看 / 包括）；可按种类（标题 / 正文 / 文件 / 元数据）搜索。
- **钉住**：新钉的在最上；拖动用分数插空，只改一行，间隙太小时重排。
- **标题**：先存首条提示去掉 `@` 引用后的前 24 个字，Agent 或后续逻辑给出标题后更新；用户改过的不再覆盖。
- **删除是硬删除**：`delete_session(s)`、`delete_archived(范围)`、`delete_workspace` 在一个事务里删会话、消息、文件、钉住和两张 FTS 表的对应行；提交后对 FTS 做 `optimize`、`PRAGMA incremental_vacuum`、`wal_checkpoint(TRUNCATE)`，库文件和 WAL 都缩回去。

## 代价（实测）

环境：Linux x86_64（本机无法构建 aarch64-apple-darwin），合入界面前要在 M5 Pro 上按 CLAUDE.md 重测。

**依赖**：`cargo tree -e normal --target aarch64-apple-darwin --prefix none | sort -u | wc -l`，应用本身 582 行；应用加两个新 crate 603 行（新增 agent-client-protocol 及 -derive / -schema、serde_with 及 macros、darling 三个、ident_case、strsim、shell-words、rusqlite、libsqlite3-sys、fallible-iterator 两个）。`cargo deny check` 的 bans / licenses / sources 通过，重复版本告警数不变（26 条），advisories 只有基线就有的 instant / rustybuzz / ttf-parser / paste（unmaintained）和 yoke-derive（yanked），与基线逐条相同。

**二进制**：应用目前还没有引用这两个 crate，链接器会把它们全部丢掉，所以直接比应用体积看不出增量；完整的应用 dist 构建也受限于本机磁盘。改用独立探针程序：对照组只用应用已有的 serde_json（preserve_order）、futures、async-io、async-channel，实验组在此基础上调用两个 crate 的全部公开 API，都用 `--profile dist`。

| | 默认编译 | 本 ADR 的设置 |
| --- | --- | --- |
| agent_history（含 SQLite）| +2.27 MB | +1.15 MB |
| agent_client（含 ACP crate）| +1.82 MB | +1.32 MB |
| 两者合计 | +4.06 MB | **+2.38 MB** |

「本 ADR 的设置」是：dist 下 ACP、serde_with、rusqlite、libsqlite3-sys 和两个新 crate 用 `opt-level = "s"`（这些代码不在按键 / 渲染路径上；改成 `"z"` 只再省 70 KB），以及 `.cargo/config.toml` 里的 `LIBSQLITE3_FLAGS` 去掉 FTS3、R*Tree、dbstat、STAT4、soundex、column metadata、load_extension、JSON 等。超过 200 KB 门槛，因此有这份 ADR。

第 6 批的 Linux dist 二进制是 49,185,392 字节（ADR 0003），CI 预算 52 MB，加上这 2.4 MB 约 51.6 MB；接上界面后很可能逼近预算，届时要么在 macOS 上实测后重新校准预算，要么改用系统自带的 `libsqlite3`（可再省约 1.1 MB；需要 ≥ 3.43 才有 `contentless_delete`，还要带 FTS5 和 trigram，得先在目标 macOS 版本上用 `PRAGMA compile_options` 确认）。

**内存**（Linux VmRSS，dist 探针，1 万条消息的库）：`History::new` 不占内存；第一次列表（打开文件、建两个连接、起写线程）+1.7 MB；再跑 4 次搜索后共 +3.5 MB；`AgentClient::start`（只有监督线程，还没起 Agent）再 +0.1 MB。面板不用时以上都不发生，空闲 footprint 不变。

**搜索**（release，1 万条消息 / 250 个会话 / 正文 2.2 MB；测试里的词表很小，「快照」「缓存」这类词出现在约一半的消息里，属于最坏情况）：各查询中位数 0.4–8.7 ms，最慢 9.2 ms，全部低于 20 ms（`ten_thousand_messages_search_fast`）。

**库文件**：1 万条消息 8.26 MB（正文 2.2 MB，trigram 索引占大头）；含 WAL 约 13.8 MB。全部删除后回到 73,728 字节，与只建了表的空库完全相同；两张 FTS 表的行数为 0。

**Agent 进程**（不计入编辑器 footprint，单独列为整应用验收项）：真实的 `claude-agent-acp 0.85.0` 握手成功（`loadSession`、`embeddedContext`、图片均支持），首次 npx 下载约 10 秒（早期测量，当时还经过 npx；现在由 `provision` 安装，不再经过 npx）。进程组（npx + 适配器 + Claude Code）在开始建会话时约 490 MB RSS。本机以 root 运行，Claude Code 拒绝 `--dangerously-skip-permissions`，会话没建起来；错误详情会原样显示在面板里。用 `npm i -g @agentclientprotocol/claude-agent-acp` 装成本机命令可以省掉 npx 那层 Node 进程，预设会优先用它。

## 后果

- 没有声明 `terminal`，所以工具卡片里不会有可交互的终端输出，只有 Agent 回报的文本。
- 空闲重启后，不支持 `loadSession` 的 Agent 会丢失上下文；界面要提示「已开始新会话」。
- 搜索结果是「会话」粒度的；一个词在 2 万行以上都出现时只看前 2 万行，这种词本来也没有区分度。
- 历史只记录 ZJ 自己的会话，不导入 `~/.claude`、`~/.codex`。
- 测试用的假 Agent 是 `crates/agent_client/src/bin/fake_acp_agent.rs`，`cargo build --profile dist`（只构建 app）不会构建它。

## 界面接入（claude/agent-ui，基于第 7 批 56aa7b8）

**布局**：以 A（会话列表 + 对话）为主体。同一窗口有两个以上活动会话（运行中、待批准、完成未读）时，面板顶部出现 B 的会话架；面板窄于 360 px 时切到 C 的紧凑密度，标题下拉切换会话。Agent 字形是单色 Lucide 图标加淡色底，不用品牌 logo。

**快捷键**（与第 7 批的 ⌘F / ⌥⌘F / ⌘G / ⌥⌘L 等没有冲突）：

| 键 | 作用 | 上下文 |
| --- | --- | --- |
| ⌥⌘B | 打开 / 关闭 Agent 面板（VS Code 辅助侧栏的键） | 工作台 |
| ⌘J | 搜索全部会话 | 工作台 |
| ⌘L | 把编辑器选区（没有选区时是当前文件）加入输入框 | 工作台 |
| ⌘⇧A | 跳到下一个待批准 | 工作台 |
| ⌘N | 新会话 | Agent 面板 |
| ⏎ / ⇧⏎ | 发送 / 换行；输入框为空时 ⏎ 允许一次、Esc 拒绝当前审批 | 输入框 |
| ⌘Y / ⌘⌫ | 接受 / 拒绝当前这一处修改 | Agent 审阅的 Diff |

**写入与审阅**：直接写入。`AgentClient` 在 Agent 第一次改某个文件前记下原文，面板的「改动文件」卡和编辑区审阅都对比这份快照：逐处接受把那一处并入快照，逐处拒绝在磁盘上还原那一处，整文件拒绝还原原文（Agent 新建的文件会被删除）。逐处操作前会校验界面显示的补丁仍是当前内容，否则提示已过期并重新载入。每一处改动的「拒绝 / 接受」放在这一处第一行的右上角，固定在可见区域的右边缘：长行横向滚动时只有内容移动，按钮不动。

**审批**：ZJ 从不自动批准写入或命令，每个审批请求都由用户回答。「始终允许」只在 Agent 自己提供该选项时出现，选它就是把 Agent 的 allow-always 选项交回去，规则由 Agent 自己记（通常在本会话内有效）。（2026-10-06 之前 ZJ 按工作区记命令的前两个词并代为批准，`timeout 120`、`bash -c` 这类前缀会放开任意命令，已去掉。）Claude Code 以 `default`（询问）模式启动，`bypassPermissions` 不出现在模式列表里，也不会被请求（会话 `_meta` + 模式过滤，假 Agent 测试覆盖）。

**密钥**：`agent.env` 的值只能是 `$变量名` 或 `keychain:账户名`。钥匙串通过系统自带的 `/usr/bin/security find-generic-password` 读取（服务名「ZJ Agent」），不新增 `security-framework` 依赖；名字里含 KEY / TOKEN / SECRET / PASSWORD 的明文值会被拒绝。从程序坞启动的 App 读不到 shell 的环境变量，所以 DeepSeek 推荐用钥匙串。

**资源**：转圈是 8 帧 / 秒的定时器，只在面板可见、窗口在前台、并且有会话在运行时存在；其余时候面板没有任何定时器。Linux / Xvfb（debug 构建）欢迎页空闲 10 秒的 CPU tick：面板关 19，面板开 16（差别是欢迎页光标本身的抖动）；打开文件后的数字与第 6 批记录的编辑器光标闪烁开销同级，与面板无关。对话最多 400 项留在内存，更早的从数据库按 200 条分页读回；对话、会话列表和 ⌘J 结果都是只布局可见行的列表。回复、思考过程、工具输出和用户消息用 Kit 的 `TextView` 渲染，可以选中、⌘C 复制，代码块右上角有复制按钮（工具输出只显示前 40 行，复制的是全部）；Kit 对 4 KB 以内的文本同步解析，更大的在它的后台线程解析。代码块高亮在后台线程：Kit 绘制时只查表，没查到的交给一个等在 channel 上的任务高亮后换表；流式中的回复不高亮，结束后才高亮。

**系统 libsqlite3**：不切换。`contentless_delete` 需要 SQLite ≥ 3.43，macOS 13 自带 3.39；Apple 的编译选项（FTS5、trigram）也无法在 Linux 上确认。继续用 bundled。

**体积**（Linux x86_64 dist，strip 后）：第 6 批 49,185,392 字节；agent-ui 接在第 6 批上 52,496,744（+3,311,352，ACP + SQLite + 面板）；接在第 7 批上 52,795,496 字节，超过原来的 52 MB 预算。CI 预算提高到 56,000,000 字节，CLAUDE.md 的上限同步改为 56 MB；macOS aarch64 上需要实测确认。2026-10-06 macOS aarch64 实测 28.5 MB 后，CI 上限收紧到 32,000,000 字节。

**未保存的缓冲区**（第 8 批 a 有了保存之后接上）：Agent 读文件时拿到的是编辑器里未保存的内容；直接写入模式记的「改动前」也取这份内容，审阅里不会把你自己的修改算成 Agent 的。Agent 写回后，没有修改的已打开文件静默跟随；有未保存修改的，如果 Agent 读过之后你没再输入，说明它的版本已经包含你的修改，编辑器直接换成它的版本（磁盘与缓冲区一致，变为已保存）；读过之后又输入过，就显示「磁盘上的文件已更改」横幅（重新加载 / 比较 / 保留我的），不覆盖。Agent 用自己的命令（`cat`、`sed -i`）读写时看到的仍是磁盘。

**已知差距**：没有终端工具卡（客户端不声明 `terminal`）。中文标点后紧跟行内代码时，GPUI 的换行可能把标点放到行首。

## 更新（2026-10-05）：去掉「先审阅再写入」

去掉了 `WriteMode::AcceptFirst` 和影子副本（`ShadowStore`），设置里的「写入方式」一并删除（旧设置文件里的 `agent.write_mode` 会被忽略）。原因：

- 只拦得住经 ACP `fs/write_text_file` 的写入，Agent 自己执行的命令照样直接改磁盘，给人的安全感是虚的；
- 基线取自磁盘，Agent 读到的却是未保存的缓冲区，用户自己的修改会被算成 Agent 的；
- 两种模式切换时，影子副本里的旧提案会被读回、混进审阅，维护成本不值得。

直接写入加改动前快照已经能逐处对比和还原，保留这一条路径。

