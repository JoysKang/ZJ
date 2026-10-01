# CLAUDE.md

AI 辅助开发时需要遵守的约定。需求细节见 `轻量代码编辑器开发说明.md`，执行记录见 `docs/development.md`。

## 定位

自用、小巧、低资源的 Rust 原生代码编辑器，以多工作区、多 Git 仓库为组织中心。取舍顺序：

1. **颜值**：界面统一、克制、耐看；亮色和暗色都要达到可交付的质量。
2. **轻量**：内存低、CPU 空闲接近 0、二进制小，功能简单。
3. 功能完整度排在最后。LSP、终端、插件、Agent 都不进首版。

## 资源预算（M5 Pro，`--profile dist`）

| 指标 | 目标 | 上限 |
| --- | --- | --- |
| 二进制体积（aarch64，stripped）| ≤ 30 MB | 45 MB（CI 会检查；语法占用见 docs/adr/0001）|
| 冷启动到首帧 | ≤ 250 ms | 400 ms |
| 空闲 footprint，1 个窗口 | ≤ 70 MB | 100 MB |
| 3 个窗口 + 20 个文档 | ≤ 220 MB | 300 MB |
| 空闲 CPU（光标不闪）| 0% | p95 ≤ 0.1% |
| 按键到画面 p99 | ≤ 8.3 ms | 16.7 ms |

内存看 physical footprint（`footprint -p PID` 或 `tools/sample_resources.py`），并把 Git 子进程算在内。

## 依赖准入

新增依赖、或给已有依赖打开新 feature 时，PR 描述里必须写明：

- `cargo tree -e normal --target aarch64-apple-darwin --prefix none | sort -u | wc -l` 的前后变化；
- `--profile dist` 构建出的二进制体积前后差值；
- 空闲 footprint 的前后差值。

二进制增加超过 200 KB，或 footprint 增加超过 5 MB，需要单独写一条决策记录。能用标准库、系统 `git` 或 GPUI Kit 现有组件解决的，就不要引入新 crate。GUI 依赖升级单独提交，不要在功能提交里顺带升级。

## 模块边界

- `crates/core`：身份模型与共享常量（`RepoId`、`DocumentId`、`EXCLUDED_DIRS`），**不依赖 GPUI**。
- `crates/git_service`：调用系统 git，负责有界输出、超时、取消、全局限流，**不依赖 GPUI**。
- `crates/app`：GPUI 界面。
  - `theme.rs`：唯一允许写字面尺寸和颜色的地方。
  - `assets.rs`：内嵌资源；`file_icons.rs`：文件类型到图标的映射。
  - `files.rs`：受限的只读文件访问与快速打开路径索引；`fuzzy.rs`：模糊匹配打分。
  - `watch.rs`：共用原生文件监听、路径引用回收和有界事件信号（带变更路径，超出上限退化为全量刷新）；不持有界面实体。
  - `refresh_plan.rs`：把一批变更路径算成最小刷新（只刷受影响仓库的状态、只重列变化的目录、增量更新索引）；被 Git 忽略的路径（target/、node_modules）按目录缓存判定后丢弃，构建期间不刷新。
  - `prototype.rs`：工作台的状态和逻辑；`prototype/` 下是各区域的渲染（`chrome` 标题栏与状态栏、`sidebar` 侧栏与资源管理器、`scm`、`editor_area`、`quick_open`），以及 `workspace_refresh` 的事件刷新编排。
  - `diff_syntax.rs`：重新注册 diff 语法，让新增 / 删除有独立的颜色。
  - `languages.rs`：文件名 / 扩展名到语法的映射；新增语法要同时打开 Kit 的 feature 并在 `SAMPLES` 里加样例。
  - `diff_doc.rs`：在后台把全上下文补丁还原成两侧全文，做行对齐、语法高亮和字符级差异；`prototype/diff_view.rs`：只切片现成数据的虚拟化左右 / 内联 Diff 编辑器。
  - `prototype/scm_actions.rs`：Git 写操作确认与结果展示；`git_service/src/write.rs`：仓库锁内校验及有界执行。
  - `symbols.rs`：tree-sitter tags / locals 查询做定义、引用和文件大纲（查询在 `crates/app/queries/`）；`symbol_index.rs`：首次跳转时后台建立的工作区符号索引；`prototype/navigation.rs`：转到定义、符号列表、查找引用和前进后退。不跑语言服务器，见 docs/adr/0002。
- UI 的 render 回调里不做 IO、不跑 Git 命令、不做全文解析；这些都通过 `background_spawn` 执行，结果带 generation 校验。

## 视觉规则

- 尺寸、间距、行高、颜色都从 `crates/app/src/theme.rs` 的 token 取，不要在 UI 代码里新写 `px(数字)` 或色值。
- 布局参照 VS Code 工作台的尺寸：标题栏 `TITLE_HEIGHT` 38、标签栏 `TAB_HEIGHT` 35、列表行 `ROW_HEIGHT` 22、状态栏 `STATUS_HEIGHT` 22；间距用 4 px 网格。
- 配色：暗色是 Solarized Dark（按 VS Code 内置主题的映射），亮色是 Nord Light。只改 `theme.rs` 的 `DARK` / `LIGHT` / 语法表，不要在界面代码里写颜色。
- 界面图标只用 Lucide（`IconName`），不要用文本符号充当图标。Kit 默认只内嵌 101 个图标；需要额外的图标时，把 SVG 放进 `crates/app/assets/icons/`，并登记到 `assets.rs` 的 `EXTRA`。
- 文件类型图标用 vscode-icons 的一个子集（`crates/app/assets/file-icons/`，MIT 许可），映射写在 `file_icons.rs`。这些是彩色 SVG，用 `img()` 绘制：每个窗口第一次画彩色图片时，GPUI 会分配一张 1024² 的 polychrome 图集（约 4 MiB）。新增图标前先看体积（当前合计约 58 KB）。
- 文字对比度：正文 ≥ 7:1，次要文字 ≥ 4.5:1（`theme.rs` 里有测试检查）。
- 外观跟随系统亮暗；可以用环境变量 `ZJ_APPEARANCE=light|dark` 强制指定，方便截图和调试。

## 常用命令

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --profile dist --locked          # 产物在 target/dist/workspace-editor
python3 tools/check_git_deadlines.py
python3 tools/fixtures.py /tmp/zj-fixture-f  # 生成临时夹具，目标路径必须尚不存在
```

## 编码约定

- 错误要让用户看得见，不能被当成"干净"或"零变更"；Git 查询失败显示真实错误。按用户要求静默刷新，不展示扫描、陈旧或过期标记。
- 每个 `unsafe` 块都要写 `// SAFETY:` 注释（lint 会强制检查）。
- 日志用 `eprintln!("event=... key=value")` 这种格式，不输出文件内容和凭据。
- 提交信息用 Conventional Commits 格式（例如 `feat(scm): …`），正文可以写中文；一个提交只做一件事，每个提交都要能构建。
- 测试只在临时夹具里进行，不在真实仓库上做写入、重置或提交。
