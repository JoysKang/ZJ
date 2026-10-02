# ZJ

Rust 原生多工作区代码编辑器的 P0 / P1 开发原型。需求与阶段门禁见 [开发说明](轻量代码编辑器开发说明.md)，执行记录见 [开发记录](docs/development.md)。界面按 VS Code 布局（暗色 Solarized Dark、亮色 Nord Light，跟随系统）：统一标题栏带命令中心，侧栏顶部切换资源管理器、搜索和源代码管理，右侧是标签页、面包屑和编辑区，底部状态栏。可打开 UTF-8 文件作临时编辑，支持全部仓库 Changes、左右对照 Diff 和 Git 日常操作。尚不支持保存和异常恢复；编辑内容关闭或退出后不保留。

## 构建与运行

环境：macOS Apple Silicon、Rust 1.98.0、系统 Git。GPUI Kit 固定为 0.7.0，兼容 GPUI snapshot 固定为 0.3.7，依赖记录在 `Cargo.lock`。运行时 shader 路径已在只有 Xcode Command Line Tools 的 M5 Pro 上编译和启动，不需要本阶段安装完整 Xcode。

在项目目录执行：

```sh
export CARGO_HOME="$PWD/target/cargo-home"
cargo build --release --locked
./target/release/workspace-editor --windows 3
```

每个根目录对应一个窗口；未指定根目录时默认只开一个空白窗口，可在欢迎页或资源管理器里点「打开文件夹」选择工作区，或用「打开文件」单独编辑文件。可用 `--windows 1`、`--windows 3`、`--windows 5` 对照基础占用。原型最多五个窗口。

生成临时功能夹具后启动三个工作区：

```sh
python3 tools/fixtures.py /tmp/zj-fixture-f
./target/release/workspace-editor /tmp/zj-fixture-f/workspace-1 /tmp/zj-fixture-f/workspace-2 /tmp/zj-fixture-f/workspace-3
```

夹具路径必须尚不存在。脚本拒绝覆盖旧目录，不访问真实仓库，不推送，也不联网。

本地 `.app` 原型包由 `tools/package.sh` 用 `dist` profile 生成，位置为 `target/ZJ.app`。这是本机开发用的应用包，未签名公证，也不是 v0.1 发布产物。

应用图标为水墨“个”字竹叶加一枚朱红光标，矢量源在 `crates/app/assets/app-icon/zj.svg`（16 / 32 px 用简化版 `zj-small.svg`），多尺寸 PNG 在同目录的 `ZJ.iconset/`。打包时在 macOS 上用 `iconutil -c icns` 从 iconset 生成 `zj.icns`，没有 iconutil 时复制已提交的 `zj.icns`（由 `tools/make_icns.py` 用同一组 PNG 生成）。改了 SVG 后用 Node.js 与 sharp 运行 `node tools/generate_app_icon.cjs` 重建 iconset、icns 和 Dock 闪烁用的无光标帧，不增加应用运行时依赖。

## 已实现的交互

- 「新建窗口」（`⌘⇧N`）可从 macOS「文件」菜单、标题栏或欢迎页打开空白窗口，共用 Git 服务和文档所有权；关闭最后一个标签后仍可使用快捷键。
- 「打开文件夹」（`⌘⇧O`）和「打开文件」（`⌘O`）使用 macOS 原生选择窗口，欢迎页也有对应入口。取消选择保留当前内容。空白窗口加载所选工作区；已有工作区时，新开窗口并保留原标签，达到五窗口上限时提示先关闭窗口。
- 单独选择文件会在当前窗口打开，也支持工作区外的文件；文件树和搜索范围保持当前工作区。已有文件仍按身份和路径定位到原标签，所有大小与编码限制继续生效。
- 侧栏顶部切换资源管理器、文件名搜索和源代码管理（角标显示变更数）；`⌘B` 显示或隐藏侧栏，拖动分隔线可调整宽度。
- `⌘P` 或点击标题栏的命令中心打开「转到文件」：对全工作区路径做模糊匹配，↑↓ 选择，回车打开；查询为空时列出已打开的文件。
- 资源管理器按 VS Code 规则显示 Git 装饰：文件名按状态着色，文件右侧显示状态字母，文件夹右侧显示圆点。文件类型图标来自 vscode-icons 子集。
- 目录按展开层级异步枚举，列表只绘制可见行；搜索打开文件后自动展开祖先目录并定位当前文件。手动折叠优先于后台定位。
- 搜索对文件名和相对路径做模糊（子序列）匹配，文件名命中、连续命中和词首命中的结果排在前面；点击结果会打开文件，或定位到已有标签。打开工作区时在后台建立路径索引：Git 仓库（含嵌套仓库和已初始化的 submodule）用 `git ls-files --cached --others --exclude-standard`，遵循 `.gitignore`；非 Git 目录做受限遍历，跳过 `core::EXCLUDED_DIRS`（`.git`、`target`、`node_modules`、`.venv`、`__pycache__`）。两种方式都不跟随目录链接。结果最多显示 300 项，被截断或索引不完整时会提示「部分结果」。文件事件和「刷新目录」都会重建索引。
- 标签页保留各自的缓冲区与撤销历史；未保存的标签显示圆点，悬停时变成关闭按钮，有未保存修改的窗口在 macOS 标题栏上标为已编辑。状态栏显示分支、行列、编码、换行符和语言，点击 LF / CRLF 可转换换行符（下次保存时写入）。相同文件按设备 / inode 与规范路径去重；从另一窗口打开会定位到原窗口。外部原子替换同一路径后再次打开仍保留原缓冲区。
- 保存按 VS Code：`⌘S` 保存，`⇧⌘S` 另存为（系统保存面板），`⌥⌘S` 全部保存，「文件」菜单里有同样的项目；`⌘N` 新建 Untitled-1，第一次保存时询问位置。写入用同目录临时文件、fsync 后 rename，保留权限；保留原有的换行符和 UTF-8 BOM，文件末尾是否有换行保持原样。符号链接写到它指向的文件，多硬链接文件原地改写；没有写权限的文件提示原因并提供「另存为」。
- 关闭有未保存修改的标签、窗口，或用 `⌘Q` / 「退出 ZJ」退出时，询问「要保存对“X”的更改吗？」（保存 / 不保存 / 取消）；多个文件时列出文件名（全部保存 / 全部不保存 / 取消）。退出会等每个窗口回答，取消则不退出。
- 打开着的文件在磁盘上被其他程序修改：没有未保存修改时静默重新加载；有未保存修改时编辑器上方出现「磁盘上的文件已更改」（重新加载 / 比较 / 保留我的）。保存时发现磁盘已变，询问覆盖 / 重新加载（丢弃我的修改）/ 比较；「比较」打开磁盘版本 ↔ 缓冲区的 Diff。磁盘上被删除的文件保留缓冲区，标签划线并标「已删除」，保存会重新创建。资源管理器里重命名或移动时标签跟随。
- 「文件 → 自动保存」：关闭（默认）、编辑后 1 秒、失去焦点时，写在设置文件里。
- 每窗口最多打开 20 个文件、合计 20 MiB 读入原文；单文件超过 8 MiB、单行超过 256 KiB、非 UTF-8 或二进制内容会拒绝打开。编辑增长与撤销的字节预算、异常退出后的编辑恢复仍待 P4。
- 全部仓库显示在同一份可滚动列表中，可同时展开多个分组；只渲染可见行。
- 同一文件同时有 index 与工作树修改时，分别出现在「已暂存」和「Changes（磁盘）」中；点击行加载对应只读 diff，新增行与删除行分别着色。刷新期间仍可打开；未跟踪文件按新增内容显示，冲突文件显示 Git 合并差异，二进制显示 Git 差异说明；空结果与加载错误可见。
- Diff 编辑器按 VS Code 的样子：默认并排，左边 HEAD / 索引，右边索引 / 工作树，标签页写作 `main.rs (索引)` / `main.rs (工作树)`。展示整份文件，两侧行号、整行红绿底、字符级深色高亮、斜纹填充行、语法高亮和同步滚动；标签栏右侧有上一个 / 下一个更改、并排 / 内联切换和打开文件。新增或删除的文件直接用内联视图。二进制和 combined conflict 补丁回退为 Git 原文。右侧概览标尺标出所有更改位置，点击跳转。可以按行选择（拖动、⇧ 单击，⌘A 全选），⌘C 复制 Git 原文。每个更改块悬停时在行号处显示「暂存块 / 还原块」（暂存区 Diff 为「取消暂存块」）；选中若干行后标签栏出现「暂存 / 还原 / 取消暂存所选范围」，用生成的部分补丁 `git apply` 完成，保留 CRLF、制表符和文件末尾无换行；还原前确认。重命名和删除的文件只支持整文件操作。没有三方合并编辑器。
- 源代码管理按 VS Code 的样子：每个仓库一行（名称、分支，悬停时显示提交 ✓、刷新和 ··· 菜单）；「暂存的更改」「更改」带计数，悬停时可全部取消暂存、全部暂存或放弃全部更改；文件行显示图标、文件名、灰色目录和右侧彩色状态字母（M / A / D / U / R），悬停时可打开文件、放弃更改和暂存 / 取消暂存，点击打开 Diff。放弃前一律确认；未跟踪文件的「放弃」就是删除文件（`git clean -f`，同样先确认）；冲突文件不提供放弃。
- 顶部是所选仓库的提交框（占位「消息（⌘Enter 提交）」）和「提交」主按钮；⌘Enter 或按钮提交暂存区，遵循 hooks 和签名设置；空消息和失败原因以小字显示在按钮下方，成功后清空输入并刷新。多个仓库时点击仓库行切换提交目标。对未保存文件暂存时确认使用磁盘版本，缓冲区仍保留。
- 仓库 ··· 菜单里的「推送」确认当前仓库和上游后，只将当前 HEAD 推到已配置上游，不强制覆盖、不连带其他分支或 tags。首次发布分支须在终端使用 `git push -u` 配置上游；没有内建凭据管理、fetch / pull / sync 或自动发布。
- 目录、路径索引与 Changes 使用 macOS 原生 FSEvents 自动刷新；共用一个监听器，窗口关闭后释放无用路径。工作区外的 Git common/private 目录也在首次状态查询前接入监听，覆盖 gitfile 和 linked worktree。事件按 200 ms 合并，连续变化最多合并 1 秒；在途扫描完成后补查，空闲时不轮询磁盘。已发现工作树内的事件不会因生成目录名被过滤，Changes 仍由 Git 判断。
- 自动刷新保留目录展开状态、文件标签、dirty 内容和撤销历史；手动折叠优先。打开的 Diff 保留原内容，磁盘变更后自动重新查询；补丁未变时保留滚动位置和光标。编辑缓冲区不会随外部修改自动重载。
- 激活窗口会补查；「刷新 Changes」和「刷新目录」仍可手动使用。源代码管理静默刷新，保留原列表，不插入扫描进度或取消行；再次刷新与关闭窗口会取消旧任务，查询仍有超时保护。不显示扫描、陈旧或过期标记；已有列表、空状态和错误在查询期间保持，只有新结果才更新。首次发现完成前不会提示「未发现 Git 仓库」。监听失败会在状态栏显示异常；事件丢失或溢出触发重新扫描。
- 编辑器复用 Kit 的真实 Editor，带行号、Rust、Python、JavaScript / JSX、TypeScript / TSX、JSON、TOML、YAML、Go、Shell、HTML、CSS、C、C++、Java、SQL、Markdown、Diff 高亮（映射见 `crates/app/src/languages.rs`）、选择、复制粘贴、撤销重做及组件内置查找替换。JSONC 使用已有 JSON grammar 的注释与容错高亮，不作为语法校验器；其他语言是纯文本。中文组合输入仍需要实机专项验收。
- 代码跳转（不跑语言服务器，基于 tree-sitter）：⌘ 悬停显示下划线，⌘ 单击或 F12 转到定义（同文件局部变量按作用域解析，跨文件用后台建立的符号索引；多个候选时弹出列表），⌘⇧O 转到文件中的符号，⇧F12 查找引用，⌃- / ⌃⇧- 后退 / 前进。支持 Rust、Python、JS、TS / TSX、Go、C、C++、Java、Bash。打开文件夹的快捷键改为 ⌘K ⌘O（⌘⇧O 让给符号列表，与 VS Code 一致）。
- Markdown 预览（原型样例已移除）将在 P6 接入。

后台 Git 采用独立 argv、porcelain v2 NUL 格式、全局最多两个命令和每仓库串行；状态和 diff 结果带 generation，旧任务不能覆盖新刷新。Git 命令清除继承的 `GIT_*` 定位与配置变量，避免命令被导向其他仓库。查询禁用 optional locks 和 fsmonitor。写操作在同一仓库锁内核对身份及状态版本；刷新不取消已开始的写操作，操作期间禁止关闭对应窗口，失败会刷新状态并显示错误。

## 验证与测量

```sh
export CARGO_HOME="$PWD/target/cargo-home"
cargo fmt --all --check
cargo test --workspace --release --locked
cargo clippy --workspace --all-targets --release --locked -- -D warnings
cargo build --release --locked
python3 tools/check_git_deadlines.py
```

集成测试会生成并清理自己的临时 36 工作树夹具。APFS 无法创建非法 UTF-8 文件名；原始非 UTF-8 路径由解析器字节测试验证，真实 Git 测试覆盖中文、空格和换行路径。deadline 检查使用临时假 Git，验证子进程持有管道、Git 提前退出和取消场景；检查约 1 秒内返回并清理子进程。

只读真实画像统计 Git index 路径、未跟踪路径和文件元数据，不调用 Git status、diff 或内容转换过滤器。输出不含仓库名、文件名、源码或机器序列号：

```sh
python3 tools/profile_workspaces.py ~/work/5g ~/work/code ~/work/prod ~/work/waibu --output target/real-workload.json
cargo run --release --locked -p workspace-editor-git --example probe -- /tmp/zj-fixture-f/workspace-1 /tmp/zj-fixture-f/workspace-2 /tmp/zj-fixture-f/workspace-3
python3 tools/sample_resources.py APP_PID --seconds 30 --output target/resources.csv
```

资源工具使用 macOS `proc_pid_rusage` 的 physical footprint，对同一采样时点的应用与子进程求和，另列 RSS 和 CPU。CSV 保留原始分项，JSON 保留摘要和采样开销。短命子进程可能落在采样间隔之外，需要后续生命周期诊断补充；此工具未测 GPU 命令和绘制次数。

## 当前门禁

G1 尚未通过。初步三窗口 F 夹具的 30 秒观测约 182 MB 来自早期构建，不能代表当前布局、正常 N / R 工作集、完整操作峰值或长期使用结果。仍需正常 20 文档工作集、1 / 3 / 5 窗口回收、中文 IME、空闲绘制与 GPU 诊断，以及正式时长的资源测量。自动刷新仅是 T12 的一批实现；跨窗口共享仓库快照、增量调度、会话恢复和编辑恢复仍待后续开发。

后续方案调研：[代码高亮](docs/reports/syntax-highlighting-research.md)、[Agent 集成](docs/reports/agent-integration-research.md)。调研不表示新增语言 grammar 或 Agent 已接入；首版范围保持当前约定。
