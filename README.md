# workspace-editor

Rust 原生多工作区代码编辑器的 P0 / P1 开发原型。需求与阶段门禁见 [开发说明](轻量代码编辑器开发说明.md)，执行记录见 [开发记录](docs/development.md)。界面按 VS Code 布局（暗色 Solarized Dark、亮色 Nord Light，跟随系统）：统一标题栏带命令中心，侧栏顶部切换资源管理器、搜索和源代码管理，右侧是标签页、面包屑和编辑区，底部状态栏。可打开 UTF-8 文件作临时编辑，支持全部仓库 Changes 和只读双状态 diff。尚不支持保存和异常恢复；编辑内容关闭或退出后不保留。

## 构建与运行

环境：macOS Apple Silicon、Rust 1.98.0、系统 Git。GPUI Kit 固定为 0.7.0，兼容 GPUI snapshot 固定为 0.3.7，依赖记录在 `Cargo.lock`。运行时 shader 路径已在只有 Xcode Command Line Tools 的 M5 Pro 上编译和启动，不需要本阶段安装完整 Xcode。

在项目目录执行：

```sh
export CARGO_HOME="$PWD/target/cargo-home"
cargo build --release --locked
./target/release/workspace-editor --windows 3
```

每个根目录对应一个窗口，默认至少开三个窗口；无根目录时，可在欢迎页或资源管理器里点「打开文件夹」选择工作区，或用「打开文件」单独编辑文件。可用 `--windows 1`、`--windows 3`、`--windows 5` 对照基础占用。原型最多五个窗口。

生成临时功能夹具后启动三个工作区：

```sh
python3 tools/fixtures.py /tmp/zj-fixture-f
./target/release/workspace-editor /tmp/zj-fixture-f/workspace-1 /tmp/zj-fixture-f/workspace-2 /tmp/zj-fixture-f/workspace-3
```

夹具路径必须尚不存在。脚本拒绝覆盖旧目录，不访问真实仓库，不推送，也不联网。

本地 `.app` 原型包由 `tools/package.sh` 生成，位置为 `target/workspace-editor.app`。这是本机开发用的应用包，未签名公证，也不是 v0.1 发布产物。

## 已实现的交互

- 「打开文件夹」（`⌘⇧O`）和「打开文件」（`⌘O`）使用 macOS 原生选择窗口，欢迎页也有对应入口。取消选择保留当前内容。空白窗口加载所选工作区；已有工作区时，新开窗口并保留原标签，达到五窗口上限时提示先关闭窗口。
- 单独选择文件会在当前窗口打开，也支持工作区外的文件；文件树和搜索范围保持当前工作区。已有文件仍按身份和路径定位到原标签，所有大小、编码与只读限制继续生效。
- 侧栏顶部切换资源管理器、文件名搜索和源代码管理（角标显示变更数）；`⌘B` 显示或隐藏侧栏，拖动分隔线可调整宽度。
- `⌘P` 或点击标题栏的命令中心打开「转到文件」：对全工作区路径做模糊匹配，↑↓ 选择，回车打开；查询为空时列出已打开的文件。
- 资源管理器按 VS Code 规则显示 Git 装饰：文件名按状态着色，文件右侧显示状态字母，文件夹右侧显示圆点。文件类型图标来自 vscode-icons 子集。
- 目录按展开层级异步枚举，列表只绘制可见行；搜索打开文件后自动展开祖先目录并定位当前文件。手动折叠优先于后台定位。
- 搜索对文件名和相对路径做模糊（子序列）匹配，文件名命中、连续命中和词首命中的结果排在前面；点击结果会打开文件，或定位到已有标签。打开工作区时在后台建一次路径索引：Git 仓库（含嵌套仓库和已初始化的 submodule）用 `git ls-files --cached --others --exclude-standard`，遵循 `.gitignore`；非 Git 目录做受限遍历，跳过 `core::EXCLUDED_DIRS`（`.git`、`target`、`node_modules`、`.venv`、`__pycache__`）。两种方式都不跟随目录链接。结果最多显示 300 项，被截断或索引不完整时会提示「部分结果」。没有文件监听，点击「刷新目录」会重建索引。
- 标签页保留各自的缓冲区与撤销历史；未保存的标签显示圆点，悬停时变成关闭按钮，关闭时可继续编辑或放弃。状态栏显示分支、行列、编码、换行符和语言；有未保存修改时显示「仅内存」。相同文件按设备 / inode 与规范路径去重；从另一窗口打开会定位到原窗口。外部原子替换同一路径后再次打开仍保留原缓冲区。
- 文件仅作内存编辑，`⌘S` 明确提示尚未支持保存。每窗口最多打开 20 个文件、合计 20 MiB 读入原文；单文件超过 8 MiB、单行超过 256 KiB、非 UTF-8 或二进制内容会拒绝打开，多硬链接文件只读。编辑增长与撤销的字节预算仍待 P4。
- 全部仓库显示在同一份可滚动列表中，可同时展开多个分组；只渲染可见行。
- 同一文件同时有 index 与工作树修改时，分别出现在「已暂存」和「Changes（磁盘）」中；点击行加载对应 diff。
- 未跟踪、重命名、删除、二进制和冲突状态可列出；未跟踪内容和冲突编辑视图尚未实现。
- 「刷新 Changes」重新扫描与查询；「取消扫描」取消任务并保留待确认或陈旧状态。当前没有自动文件监听，外部修改后需要手动刷新。
- 编辑器复用 Kit 的真实 Editor，带行号、Rust / Markdown / Diff 高亮、选择、复制粘贴、撤销重做及组件内置查找替换。中文组合输入仍需要实机专项验收。
- Markdown 预览（原型样例已移除）将在 P6 接入。

后台 Git 采用独立 argv、porcelain v2 NUL 格式、全局最多两个命令和每仓库串行；状态和 diff 结果带 generation，旧任务不能覆盖新刷新。Git 命令清除继承的 `GIT_*` 定位与配置变量，避免命令被导向其他仓库。查询禁用 optional locks 和 fsmonitor，避免原型隐式启动常驻进程；不执行暂存、提交或任何网络 Git 操作。

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

G1 尚未通过。初步三窗口 F 夹具的 30 秒观测约 182 MB 来自早期构建，不能代表当前布局、正常 N / R 工作集、完整操作峰值或长期使用结果。仍需正常 20 文档工作集、1 / 3 / 5 窗口回收、中文 IME、空闲绘制与 GPU 诊断，以及正式时长的资源测量。本轮按用户要求补充布局与文件打开原型；监听、保存、恢复等后续功能仍等待门禁验证。
