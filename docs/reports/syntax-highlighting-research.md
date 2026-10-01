# 代码高亮方案调研

查阅日期：2026 年 10 月 1 日，Asia/Shanghai。范围：当前 Rust / GPUI 原生编辑器的语言覆盖、高亮质量、接入成本与资源约束。项目基准为 `e2f0613fa5c95d48c0ee71152cb03620f036d777`；本地 GPUI Kit / Component / Base 均为 0.7.0，发布源码对应 `0c830f4d257e69fdd17200650533ab4ca9a40cc0`。以下将锁定版本的事实与后续建议分开；外部文档的性能描述不作为本项目实测结果。

## 结论

建议继续使用现有 Tree-sitter 高亮链路，按需启用 GPUI Kit 已有的语言 feature，并完善文件类型映射。当前覆盖不足首先来自应用配置：调研时 `language_for` 只有 Rust、Markdown、Diff 使用 grammar，JSON、Python、JavaScript、TypeScript 等均返回 `plain`。其中 JSON grammar 已在现有依赖中，Kit 也已默认注册，恢复 JSON 高亮无需增加依赖或 feature。[1][2]

本轮由主任务处理 JSON / JSONC 的映射修正；本报告未修改源码或依赖。Python、TOML、YAML、Shell、Web 等新增语言仍是后续落地建议，不能据此认为当前构建已经支持。

Syntect 是值得保留的 Rust 原生备选，适合更重视 Sublime 语法与 `.tmTheme` 生态的场景。当前项目已经有增量 Tree-sitter、嵌入语言处理和统一主题接口；换引擎会增加适配与验证工作，现有证据不足以证明更换可改善本项目的质量或资源占用。[3][4]

## 当前版本的能力与限制

| 项目 | 已核验事实 | 对开发的影响 |
| --- | --- | --- |
| 实际启用的语言 | 项目只显式启用 `tree-sitter-rust`、`tree-sitter-diff`、`tree-sitter-markdown`；基础 `tree-sitter` 同时带入 JSON，Markdown 同时注册 `markdown_inline`。[1][2] | 区分「grammar 已编译」和「文件真正选择该 grammar」，状态栏显示语言名不表示已经高亮。 |
| 语言 registry | 默认由编译后的 `Language::all()` 注册；可以 `register` 自定义 grammar / query，也提供 `register_parser_factory`。[2][3] | 不必替换编辑器；缺少的语言可先通过 Kit feature 补齐。动态 grammar 接口存在不代表已经有可用的语言包加载器。 |
| 常见语言 | Kit 0.7.0 有 Python、Bash、Go、C / C++ / C#、Java、JavaScript、TypeScript、TSX、HTML、CSS、TOML、YAML、SQL 等独立 feature。[1][2] | 不要一次开启 `tree-sitter-languages` 全量合集；按工作集需要分批验证。Vue、特定模板方言等不能仅凭扩展名或近似 grammar 宣称支持。 |
| 嵌入语言 | 本地 query 支持 Markdown fenced code、Markdown inline、HTML 的 JavaScript / CSS 等；目标语言需要注册对应 grammar。Markdown 的 fenced code 可以从捕获到的语言名解析。[2][5] | Web 支持应成组验证；HTML 着色成功不能代替 `<script>` / `<style>` 着色验证。 |
| 主题 | Component 的 `SyntaxColors` 提供 41 个高亮类别及带点名称的前缀回退；`HighlightTheme` 的 JSON 结构兼容 Zed 主题格式。项目已经在 `theme.rs` 设置 Solarized Dark / Nord Light。[3] | 沿用现有主题表；逐语言检查 query capture 与颜色映射，避免只加 grammar 却仍有大段默认色。 |
| 解析与更新 | Base 已有与解析器无关的 `InputHighlighter` / `HighlightStyleResolver` 接口。Component 适配器尝试有时间预算的同步增量更新，超过大小阈值或未完成时执行可取消的后台解析。[4][5] | 应复用现有链路，不在应用的 render 中追加全文解析。后台机制存在仍需做实际输入延迟与大文件验证。 |

### JSONC 的准确边界

本地锁定的 `tree-sitter-json` 为 0.24.8，其 grammar 明确支持 `//`、`/* … */` 注释并将注释作为 `extras`；Kit 的 JSON query 包含 `(comment) @comment`，语言别名也包含 `jsonc`。因此已有直接证据支持 JSONC 的注释高亮。[2][6]

同一个 grammar 的 `commaSep` 没有尾逗号产生式。Tree-sitter 可以在错误恢复后的树上继续高亮，但这不等于完整 JSONC 校验或所有宽松 JSON 方言都已兼容。建议状态栏区分 JSON 与 JSON with Comments，并验证带注释、带尾逗号的 fixture；不要把高亮成功写成语法校验通过。[6]

## 方案比较

| 方案 | 原生接入与编辑能力 | 语言、嵌入语言与主题 | 资源、许可与维护 | 适用判断 |
| --- | --- | --- | --- | --- |
| 继续使用 Kit 的 Tree-sitter | 现有 Rust / C 链路；解析树可随编辑增量更新，语法未完成时仍可提供结果；Kit 已有后台适配器。[4][5][7] | 语言由 grammar 和 query 决定；注入 query 处理嵌入语言，主题按 capture 映射。[2][3][7] | 每个额外 grammar 都增加编译代码 / query；打开文档后还有树、缓存和注入层。实际增量需实测。Tree-sitter 核心 MIT；Kit Apache-2.0；grammar / query 的许可需分别核对。[1][7][8] | **首选**。最小改动即可增加当前所缺的常见语言。 |
| Syntect 5.3.0 | Rust 高亮库，返回样式与文本区间；可共享 `SyntaxSet`，缓存逐行解析状态，在改动后后台重算。[9] | 主要使用 Sublime `.sublime-syntax`；支持复杂语法和嵌套上下文，使用 `.tmTheme` 主题。不能把它等同于直接运行 VS Code 的全部 TextMate grammar / injection 机制。[9][10] | 默认使用 Oniguruma C 引擎；`default-fancy` 可使用纯 Rust `fancy-regex`。默认语法包、主题包、加载器均有 feature；压缩语法包与状态缓存仍有成本。库为 MIT；grammar / theme 另查许可。维护者称项目基本完成、持续维护但非高强度开发。[9][10] | 备选。需要实现 Base highlighter 适配、范围缓存、取消与主题 scope 映射，先以一个实际弱项证明收益。 |
| `vscode-textmate` / Shiki | 前者是 JavaScript TextMate 解释器，官方例子逐行传递 rule stack 并加载 Oniguruma WASM；Shiki 使用 TextMate grammar，提供 JS / Web 使用接口。[11][12] | 接近 VS Code grammar / theme 生态；支持多少语言取决于加载的包。`vscode-textmate` README 明确写有 cross-grammar injection 限制，不能把引擎与 VS Code 产品能力视为完全相同。[11] | 在当前 Rust / GPUI 项目中要增加 JS 执行或跨进程适配；Shiki 也需要按需组合 bundle。两者主库 MIT，语言包与主题另查。[11][12] | 适合将来 HTML 代码预览或 Web 内容输出；当前原生编辑区不选。 |

Tree-sitter 提供的是语法树与 query 高亮。跨文件类型推断、符号解析、语言服务器 semantic tokens 是另一层能力；增加高亮语言不需要顺带引入 LSP，也不能宣称达到其语义准确度。首版定位仍按 `CLAUDE.md` 执行。

Syntect README 中的历史耗时、Shiki 文档中的 minified / gzip bundle 数字都不适合直接换算为本应用的 ARM64 dist 二进制或 physical footprint。本轮未构建候选引擎、未开启新 grammar feature，也未测量三者的资源差值，因此不提供估计数值或速度排名。

## 建议的落地顺序

1. **先修已有覆盖**：复用 JSON grammar；保留 Rust、Markdown、Diff 的现有行为。增加文件映射与 registry 的 focused tests，验证 JSON 键名、数值、字符串、注释及 UTF-8 范围；Markdown 验证 Rust / JSON fenced code 和 inline。
2. **配置与脚本语言一组**：优先 TOML、YAML、Python、Bash。补全扩展名、常见完整文件名与明确的 shebang 规则。Shell 方言、YAML 模板不借用名称承诺完整兼容；没有合适 grammar 时保留纯文本回退。
3. **Web 语言一组**：JavaScript、TypeScript、TSX、HTML、CSS。为 `.tsx` 选择独立 TSX grammar；验证 JSX、模板字符串、HTML script / style、Markdown 中相应 fenced code。跨语言嵌套是验收项，不能只看单文件颜色。
4. **按工作集补充**：Go、C / C++、Java、SQL 等根据实际文件分布选择；仅在 Kit 当前 grammar 缺失或 query 质量确实不足时，自行注册固定版本的 grammar / query。GUI 依赖升级保持单独变更。

每组变更记录项目规定的 `cargo tree` 去重行数、dist 字节和空闲 footprint 前后值；二进制增加超过 200 KB 或 footprint 增加超过 5 MB 时写决策记录。内存检查还应包含多文档、大文件、复杂嵌入语言和关闭文档后的回收。当前单窗口短测已经高于 70 MB 目标，新增语言不能只用「可编译」作为准入依据。

高亮验收优先检查：未完成语法、长字符串 / 注释、中文 / emoji 的 UTF-8 边界、连续编辑与撤销后的过期结果、后台取消、主题切换，以及大文件下的输入延迟。若某个语言的质量仍差，先记录具体 fixture 和错色范围，修正对应 query；不要为了语言数量直接更换整条渲染链路。

## 来源与本地证据

以下来源均于 2026 年 10 月 1 日查阅。Kit / Component / Base 链接固定在本地发布版本的 commit；Syntect 固定在 v5.3.0。Tree-sitter 总体文档、Shiki 与 VS Code TextMate 文档为查阅时官方仓库内容，不代表本项目安装版本。

1. [项目 Cargo.toml](../../Cargo.toml)、[CLAUDE.md](../../CLAUDE.md)；[Kit 0.7.0 feature 定义](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/kit/Cargo.toml)、[Component 0.7.0 feature 与许可](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/component/Cargo.toml)。本地依赖证据位于 `target/cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f/` 对应 0.7.0 目录。
2. [Component 0.7.0 languages.rs](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/component/src/highlighter/languages.rs)；应用原始映射见基准版本的 `crates/app/src/prototype.rs::language_for`，当前文件由主任务修改。
3. [Component 0.7.0 registry、SyntaxColors、HighlightTheme](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/component/src/highlighter/registry.rs)；[项目 theme.rs](../../crates/app/src/theme.rs)、[Diff 注册](../../crates/app/src/diff_syntax.rs)。
4. [Base 0.7.0 InputHighlighter / HighlightStyleResolver 接口](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/base/src/input/editor/highlighting.rs)。
5. [Component 0.7.0 输入高亮适配器](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/component/src/highlighter/input_adapter.rs)、[注入层与高亮实现](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/component/src/highlighter/highlighter.rs)、[Markdown 注入 query](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/component/src/highlighter/languages/markdown/injections.scm)。
6. [tree-sitter-json 0.24.8 对应 grammar](https://github.com/tree-sitter/tree-sitter-json/blob/ee35a6ebefcef0c5c416c0d1ccec7370cfca5a24/grammar.js)、[Kit 0.7.0 JSON highlights query](https://github.com/longbridge/gpui-kit/blob/0c830f4d257e69fdd17200650533ab4ca9a40cc0/crates/component/src/highlighter/languages/json/highlights.scm)。
7. [Tree-sitter 官方 README](https://github.com/tree-sitter/tree-sitter/blob/master/README.md)、[官方语法高亮文档：queries / locals / language injection](https://github.com/tree-sitter/tree-sitter/blob/master/docs/src/3-syntax-highlighting.md)。
8. [Tree-sitter 核心 MIT 许可](https://github.com/tree-sitter/tree-sitter/blob/master/LICENSE)。
9. [Syntect v5.3.0 README：编辑器、缓存、嵌套语法、维护状态](https://github.com/trishume/syntect/blob/v5.3.0/Readme.md)。README 的历史性能数字未用于本项目结论。
10. [Syntect v5.3.0 Cargo features / 许可](https://github.com/trishume/syntect/blob/v5.3.0/Cargo.toml)、[SyntaxSet / SyntaxSetBuilder](https://github.com/trishume/syntect/blob/v5.3.0/src/parsing/syntax_set.rs)、[Theme / .tmTheme](https://github.com/trishume/syntect/blob/v5.3.0/src/highlighting/theme.rs)。
11. [Microsoft vscode-textmate 官方 README](https://github.com/microsoft/vscode-textmate/blob/main/README.md)。
12. [Shiki 官方 README](https://github.com/shikijs/shiki/blob/main/README.md)、[按需 bundle 文档](https://github.com/shikijs/shiki/blob/main/docs/guide/bundles.md)。
