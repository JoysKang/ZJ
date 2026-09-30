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
| 二进制体积（aarch64，stripped）| ≤ 25 MB | 35 MB（CI 会检查）|
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

- `crates/core`：身份模型与共享常量（`RepoId`、`DocumentId`），**不依赖 GPUI**。
- `crates/git_service`：调用系统 git，负责有界输出、超时、取消、全局限流，**不依赖 GPUI**。
- `crates/app`：GPUI 界面。
  - `files.rs`：受限的只读文件访问。
  - `prototype.rs`：工作台。
- UI 的 render 回调里不做 IO、不跑 Git 命令、不做全文解析；这些都通过 `background_spawn` 执行，结果带 generation 校验。

## 视觉规则

- 尺寸和颜色集中定义，不要在各处散写 `px(数字)` 或色值（设计 token 模块待引入）。
- 图标只用 Lucide（`IconName`），不要用文本符号充当图标。
- 文字对比度：正文 ≥ 7:1，次要文字 ≥ 4.5:1。

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

- 错误要让用户看得见，不能被当成"干净"或"零变更"；Git 查询失败要显示为错误或陈旧状态。
- 每个 `unsafe` 块都要写 `// SAFETY:` 注释（lint 会强制检查）。
- 日志用 `eprintln!("event=... key=value")` 这种格式，不输出文件内容和凭据。
- 提交信息用 Conventional Commits 格式（例如 `feat(scm): …`），正文可以写中文；一个提交只做一件事，每个提交都要能构建。
- 测试只在临时夹具里进行，不在真实仓库上做写入、重置或提交。
