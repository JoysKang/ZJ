# CLAUDE.md

AI 辅助开发时需要遵守的约定。需求细节见 `轻量代码编辑器开发说明.md`，执行记录见 `docs/development.md`。

## 定位

自用、小巧、低资源的 Rust 原生代码编辑器，以多工作区、多 Git 仓库为组织中心。取舍顺序：

1. **颜值**：界面统一、克制、耐看；亮色和暗色都要达到可交付的质量。
2. **轻量**：内存低、CPU 空闲接近 0、二进制小，功能简单。
3. 功能完整度排在最后。LSP、插件不做；终端只做基本的新建、拆分和关闭（docs/adr/0006）。

## 资源预算（M5 Pro，`--profile dist`）

| 指标 | 目标 | 上限 |
| --- | --- | --- |
| 二进制体积（aarch64，stripped）| ≤ 30 MB | 32 MB（CI 会检查，2026-10-06 实测 28.5 MB；语法占用见 docs/adr/0001，Agent 面板见 docs/adr/0004）|
| 冷启动到首帧 | ≤ 250 ms | 400 ms |
| 空闲 footprint，1 个窗口 | ≤ 70 MB | 100 MB |
| 3 个窗口 + 20 个文档 | ≤ 220 MB | 300 MB |
| 空闲 CPU（光标不闪）| 0% | p95 ≤ 0.1% |
| 按键到画面 p99 | ≤ 8.3 ms | 16.7 ms |

内存看 physical footprint（`footprint -p PID` 或 `tools/sample_resources.py`），并把 Git 子进程算在内。

表中“光标不闪”指编辑器光标和欢迎页 logo 光标都不闪：两者都常亮。编辑器光标靠 `vendor/gpui-base` 的 ZJ patch（Kit 原本每 500 ms 闪一次并整窗重画）；logo 光标是静态色块。任何闪烁或动画都会让窗口一直出帧，空闲 CPU 上升，渲染器也等不到空闲去归还帧缓冲、停掉 display link（1 个窗口约 190 MB 对 70 MB）。Dock 图标闪烁（设置 `dock_icon_blink`，默认关）是每 530 ms 一次 `setApplicationIconImage`，一个 App 级定时器，不画窗口；开启“减少动态效果”时停止并恢复完整图标。它不计入空闲 CPU 预算，但不要再加别的常驻动画。

## 依赖准入

新增依赖、或给已有依赖打开新 feature 时，PR 描述里必须写明：

- `cargo tree -e normal --target aarch64-apple-darwin --prefix none | sort -u | wc -l` 的前后变化；
- `--profile dist` 构建出的二进制体积前后差值；
- 空闲 footprint 的前后差值。

二进制增加超过 200 KB，或 footprint 增加超过 5 MB，需要单独写一条决策记录。

取舍：

- 能用标准库、系统 `git` 或 GPUI Kit 现有组件解决的，就不要引入新 crate。小改动（几十行能写完的）不引入依赖。
- 大块、难写对的功能（终端仿真、协议、解析器）不要自己从头写，优先用成熟、维护中的开源实现（如终端用 `alacritty_terminal`），自己只写和界面衔接的部分。先看 GPUI Kit 有没有，再看社区实现；依赖 Zed 版 `gpui` 的组件和这里的 `gpui-pre` 不兼容，只能参考代码。
- GUI 依赖升级单独提交，不要在功能提交里顺带升级。

## 模块边界

- `crates/core`：身份模型与共享常量（`RepoId`、`DocumentId`、`EXCLUDED_DIRS`、按 Git 规则校验 `.git` 的 `git_marker`），**不依赖 GPUI**。
- `crates/git_service`：调用系统 git，负责有界输出、超时、取消、全局限流，**不依赖 GPUI**。仓库发现最多向下 4 层，跳过 `EXCLUDED_DIRS` 和上层仓库忽略的目录，无效的 `.git` 静默跳过（自动发现找不到的仓库可以手动添加，见 `settings.rs` 的 `extra_repos`）。`lib.rs` 是服务本体（进程、限流、发现、查询），解析放在各自的文件里：`status.rs`（porcelain v2）、`refs.rs`（分支）、`log.rs`（未推送的提交）、`graph.rs`（Git 图的分页和提交详情）、`ls_files.rs`（快速打开的路径）、`stash.rs`（stash 和 blame），写操作在 `write.rs`。
- `crates/agent_client`：ACP 客户端（Agent 预设、子进程、会话、权限、`fs/*`、改动前快照与审阅、空闲退出），**不依赖 GPUI**，见 docs/adr/0004。`client.rs` 是一个会话（`AgentClient`），其私有子模块 `steering.rs` 处理运行中追加指令、适配器续跑的结束通知和延迟响应的取消；仅在初始化声明支持时使用 `_session/steering`，不改变权限审批。`host.rs` 是 Agent 进程：同一种 Agent、同样的环境变量在整个应用里共用一个进程（`AgentPool`），请求按会话 id 分发，会话空闲时 `session/close`，没有会话时进程退出。
- `crates/agent_history`：Agent 会话历史（SQLite + FTS5，后台写线程、搜索、钉住、硬删除），**不依赖 GPUI**。
- `crates/app`：GPUI 界面。
  - `theme.rs`：唯一允许写字面尺寸和颜色的地方。
  - `assets.rs`：内嵌资源；`file_icons.rs`：文件类型到图标的映射。
  - `files.rs`：受限的文件读取、紧凑存储的快速打开路径索引和点文件默认隐藏规则；`fuzzy.rs`：模糊匹配打分。
  - 大文件受限查看（A18）：超过 8 MiB、有超过 256 KiB 的行或不是 UTF-8 的文件不进编辑器（`files::Restricted` 说明原因），改在只读的查看标签里打开（二进制文件仍拒绝）。`large_file.rs` 是纯逻辑：一遍扫描建稀疏行索引（每 1024 行一个偏移）、按需读几千行、过长的行只读开头 4 KB；`workbench/large_view.rs` 是标签（一次一个，像 Git 图），只持有视口附近的行，文件在磁盘上变了就重建索引；点击 / ⇧点击选中行、⌘C 复制整行，⌘F 后台逐行查找，⌃G 转到行。内存不随文件大小增长。
  - `save.rs`：保存的纯逻辑（同目录临时文件 + fsync + rename、保留权限 / 换行符 / BOM、符号链接写到目标、多硬链接原地写、只读识别、按设备 / inode / 大小 / mtime / 内容哈希判断外部修改、退出询问的状态机、自动保存防抖、比较用的补丁）；`workbench/documents.rs`：保存、另存为、全部保存、Untitled、关闭 / 退出确认、外部修改横幅、自动保存，以及给 Agent 层的 `buffer_text` / `on_buffer_saved`。
  - 编辑恢复（开发说明 R11 / A16）：`recovery.rs` 是快照记录的纯逻辑（每个未保存缓冲区一个带版本的 JSON，放在设置文件旁的 `recovery/`，临时文件 + rename，权限 0600）；`workbench/recovery.rs` 在编辑停顿 2 秒后经单一后台队列写快照，保存 / 重新加载 / 不保存 / 关窗时删除，退出时同步清空，启动时把异常退出留下的快照恢复进标签（带横幅，可放弃）。只在有待写快照时才有定时器。
  - `settings.rs`：所有窗口共用的设置文件（字号、显示隐藏文件、Diff 布局、隐藏无变更仓库、搜索排除、Dock 图标闪烁、自动保存、按工作区手动添加的仓库 `extra_repos`）。⌘, 在标签里打开它，保存后在所有窗口生效，JSON 写错时保留原设置并在状态栏说明。
  - `perf.rs`：给 `tools/measure_budget.py` 用的打点（首帧耗时；`ZJ_LATENCY_LOG=1` 时记录按键到下一帧画完的耗时），平时只有一行首帧日志。
  - `platform.rs`：少量 macOS 系统接口（“减少动态效果”、Dock 图标替换与闪烁），其他平台为空实现。
  - `workbench/switch_hud.rs`：从另一个 ZJ 窗口切过来时，在窗口中央闪现工作区名称（放大淡入、停留、淡出，约 1 秒的一次性动画，结束后不再出帧；窗口第一次激活、从别的 App 切回时不显示；开启“减少动态效果”时不缩放、不渐变）。
  - `file_ops.rs`：资源管理器的新建、重命名、复制、移动和移到废纸篓；`workbench/explorer_ops.rs`：右键菜单、快捷键和行内改名；`workbench/tab_menu.rs`：编辑器标签页的右键菜单、对应快捷键和 ⇧⌘T 重开已关闭标签的栈。
  - Markdown 预览：`markdown_blocks.rs` 把文件切成顶层块（纯函数）；`md_images.rs` 解析图片地址（本地图片按文件头尺寸检查解码预算，远程 / data: 不加载，纯函数），预览在后台和分块一起解析，渲染时只查表（用 Kit 基础 `TextView` 的 `image_source`）；相对链接在 ZJ 里打开；`workbench/markdown_preview.rs` 按块用 Kit `TextView` 渲染，点击的块换成源码文本框并直接写回缓冲区，⇧⌘V 切换源码。源码视图下不切分。
  - 终端（docs/adr/0006）：`terminal.rs` 是 `alacritty_terminal` 的衔接层（起 shell、事件、按键编码、ANSI 颜色映射），不依赖界面状态；`workbench/terminal_view.rs` 画网格并处理键鼠和输入法；`workbench/terminal_panel.rs` 是底部面板的分组、拆分和关闭。终端没有定时器，只在 shell 有输出时重画。
  - `text_search.rs`：全文搜索（glob 包含 / 排除、默认排除、二进制与大文件跳过、结果上限）；`workbench/search_view.rs`：搜索视图；`workbench/search_replace.rs`：搜索视图里的替换（行内预览、替换前 Diff、原子写入、跳过搜索后改过的文件、撤销）。
  - `replace.rs`：查找替换的共同规则（Aa / ab / .*、`$1` 与大小写转义、保留大小写、CRLF、不跨行）；`workbench/find_widget.rs`：编辑器右上角的查找替换浮层（⌘F / ⌥⌘F）。
  - `watch.rs`：共用原生文件监听、路径引用回收和有界事件信号（带变更路径，超出上限退化为全量刷新）；不持有界面实体。
  - `refresh_plan.rs`：把一批变更路径算成最小刷新（只刷受影响仓库的状态、只重列变化的目录、增量更新索引）；被 Git 忽略的路径（target/、node_modules）按目录缓存判定后丢弃，构建期间不刷新。
  - `workbench.rs`：工作台的状态和逻辑（成组的状态放在各自模块的子结构体里：`sidebar::Explorer`、`diff_view::DiffPane`、`navigation::NavState`、`workspace_refresh::WatchState`）；`workbench/` 下是各区域的渲染（`chrome` 标题栏与状态栏、`sidebar` 侧栏与资源管理器、`scm`、`editor_area`、`quick_open`、`commands` 命令面板的命令表），以及 `workspace_refresh` 的事件刷新编排。
  - `diff_syntax.rs`：重新注册 diff 语法，让新增 / 删除有独立的颜色。
  - `languages.rs`：文件名 / 扩展名到语法的映射；新增语法要同时打开 Kit 的 feature 并在 `SAMPLES` 里加样例。
  - `editing.rs`：行编辑命令的纯逻辑（⌘/ 行注释、⌥↑↓ 移动行、⇧⌥↑↓ 复制行、⌘D 选词和下一个匹配，按 VS Code 规则），不依赖 GPUI；`workbench/edit_commands.rs` 接到文档编辑器上（`DocumentEditor > Input` 键上下文，只用 Kit 公开的主选区）。
  - `indent.rs`：每个文件的缩进（`.editorconfig` > 按内容猜测 > 语言默认），不依赖 GPUI；`.editorconfig` 在后台打开文件时一起读，状态栏的缩进菜单只改当前文档。
  - `diff_doc.rs`：在后台把全上下文补丁还原成两侧全文，做行对齐、语法高亮和字符级差异；`workbench/diff_view.rs`：只切片现成数据的虚拟化左右 / 内联 Diff 编辑器；`workbench/diff_ops.rs`：行选择、复制、概览标尺和块 / 行级暂存；`partial_patch.rs`：从全上下文补丁生成只含所选行的补丁。
  - `workbench/scm_actions.rs`：Git 写操作确认与结果展示，以及 stash 的选择面板和手动添加 / 移除仓库；`git_service/src/write.rs`：仓库锁内校验及有界执行（stash 的 apply / pop / drop 先确认 `stash@{n}` 仍是选中的提交）；`git_service/src/stash.rs`：stash 列表和单行 blame 的解析。
  - `workbench/scm_message.rs`：源码管理输入框右侧的 AI 按钮，用当前选择的 Agent 独立生成提交信息；优先使用暂存区，没有暂存内容时使用磁盘改动（含未跟踪文件），diff 上限 128 KiB。复用 Agent 进程池，拒绝本次会话的 ACP 文件访问和工具审批；支持时选只读 / 规划模式。手动输入、再次点击或关闭工作区取消生成，回填前核对仓库状态；不自动暂存、提交或推送。展开的干净仓库也保留输入框和提交按钮。
  - `workbench/blame.rs`：状态栏的当前行 blame（光标停 400 ms 后 `git blame -L`，编辑中的缓冲区用 `--contents -`，新的请求取消旧的）。
  - 合并冲突：`conflicts.rs` 找冲突标记和三种解决方式（纯函数）；`workbench/conflict_bar.rs` 在冲突文件编辑后后台扫描、两侧着色（`theme` 的 `conflict_*`），编辑器上方的冲突条逐处或全部解决、上一处 / 下一处，解决完后保存并按新状态暂存。
  - `workbench/graph_view.rs`：编辑区里的 Git 图（纯函数的车道布局、分页提交列表、提交详情、打开提交 Diff）。
  - `session.rs`：重启时恢复的窗口记录（`session.json`，每个窗口的文件夹、位置大小、文件标签和激活的标签；重启时只读入激活的标签，其余标签点开时才读）。
  - Agent 面板（docs/adr/0004）：`agent_model.rs` 放不依赖 GPUI 的逻辑（会话分组和时间、`@` 引用、附件、从历史恢复）；回复用 Kit 的 `TextView` 渲染（可选中，代码块带复制按钮），`markdown.rs` 只管代码块的语法高亮（编辑器的语法主题），`workbench/agent/highlights.rs` 在后台算好、绘制时只查表；`quota.rs` 从 Codex 的本地会话日志读账户额度（有界读取，不联网、不读凭据），`workbench/agent/quota.rs` 在显示 Codex 会话、一轮结束和点开额度卡片时后台刷新；`secrets.rs` 解析 Agent 的环境变量（`$变量名` 或 macOS 钥匙串 `keychain:账户名`，设置里不存明文密钥）。`workbench/agent.rs` 是会话与面板视图的状态，其余逻辑按主题放在 `workbench/agent/`（`turns` 发送与事件泵、`permissions` 审批与模式、`composer` 引用与附件、`changes` 改动文件、`history` 会话历史、`buffers` 给 Agent 的未保存缓冲区、`highlights` 代码块高亮）；`agent_panel.rs`（标题、会话条、切换器；`agent_panel/` 下是对话行、卡片、改动文件、输入框、设置）、`agent_history.rs`（会话列表）、`agent_search.rs`（⌘J 搜索）、`agent_review.rs`（编辑区里逐处接受 / 拒绝）只做渲染和交互。转圈只在面板可见、窗口在前台、有会话运行时才有定时器。
  - `symbols.rs`：tree-sitter tags / locals 查询做定义、引用和文件大纲（查询在 `crates/app/queries/`）；`symbol_index.rs`：首次跳转时后台建立的工作区符号索引；`workbench/navigation.rs`：转到定义、符号列表、查找引用、转到行（⌃G，命令中心里的 `:行:列`）和前进后退。不跑语言服务器，见 docs/adr/0002。
- `vendor/`：打过补丁的 GPUI macOS 渲染器和窗口层（`gpui-pre-apple` / `gpui-pre-macos`），以及 GPUI Kit 的 `gpui-base`（编辑器光标常亮），都用 `[patch.crates-io]` 指向。改动都标 `ZJ patch`，说明在 `vendor/README.md`，理由和测量方法在 docs/adr/0005；`ZJ_GPU_LOWMEM=0` 恢复上游行为。升级 GPUI 时要先处理这里。
- UI 的 render 回调里不做 IO、不跑 Git 命令、不做全文解析；这些都通过 `background_spawn` 执行，结果带 generation 校验。

## 视觉规则

- 尺寸、间距、行高、颜色都从 `crates/app/src/theme.rs` 的 token 取，不要在 UI 代码里新写 `px(数字)` 或色值。
- 布局参照 VS Code 工作台的尺寸，按用户要求字号大一号：正文 `TEXT_BODY` 14、标题栏 `TITLE_HEIGHT` 38、标签栏 `TAB_HEIGHT` 36、列表行 `ROW_HEIGHT` 24、状态栏 `STATUS_HEIGHT` 24；间距用 4 px 网格（不用 `gap_0p5` 这类半格工具类，`theme.rs` 的测试会检查；唯一例外是由行高推出的居中偏移 `SCM_LINE_PAD`）。编辑器字号默认 14，可缩放，Diff 行高随之计算（`theme::diff_metrics`）。
- 配色：暗色是 Solarized Dark（按 VS Code 内置主题的映射），亮色是 Nord Light。只改 `theme.rs` 的 `DARK` / `LIGHT` / 语法表，不要在界面代码里写颜色。
- 界面图标只用 Lucide（`IconName`），不要用文本符号充当图标。Kit 默认只内嵌 104 个图标（`gpui-kit-assets` 的 `default-icons.txt`），没内嵌的 `IconName` 会画成空白；需要额外的图标时，把 SVG 放进 `crates/app/assets/icons/`，并登记到 `assets.rs` 的 `EXTRA`。
- 文件类型图标用 vscode-icons 的一个子集（`crates/app/assets/file-icons/`，MIT 许可），映射写在 `file_icons.rs`。这些是彩色 SVG，用 `img()` 绘制：第一次画彩色图片时，GPUI 会分配一张 polychrome 图集（所有窗口共用，vendor 补丁把初始尺寸从 1024² 降到 512²，约 1 MiB）。新增图标前先看体积（当前合计约 58 KB）。
- 文字对比度：正文 ≥ 7:1，次要文字 ≥ 4.5:1（`theme.rs` 里有测试检查）。
- 外观跟随系统亮暗；可以用环境变量 `ZJ_APPEARANCE=light|dark` 强制指定，方便截图和调试。

## 常用命令

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --profile dist --locked          # 产物在 target/dist/workspace-editor
python3 tools/check_git_deadlines.py
python3 tools/measure_budget.py            # macOS：对照上面的资源预算测冷启动、空闲内存 / CPU、3 窗口 + 20 文档、按键延迟
python3 tools/fixtures.py /tmp/zj-fixture-f  # 生成临时夹具，目标路径必须尚不存在
```

## 编码约定

- 错误要让用户看得见，不能被当成"干净"或"零变更"；Git 查询失败显示真实错误。按用户要求静默刷新，不展示扫描、陈旧或过期标记。
- 每个 `unsafe` 块都要写 `// SAFETY:` 注释（lint 会强制检查）。
- 日志用 `eprintln!("event=... key=value")` 这种格式，不输出文件内容和凭据。
- 提交信息用 Conventional Commits 格式（例如 `feat(scm): …`），正文可以写中文；一个提交只做一件事，每个提交都要能构建。
- 测试只在临时夹具里进行，不在真实仓库上做写入、重置或提交。
