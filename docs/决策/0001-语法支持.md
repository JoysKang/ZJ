# ADR 0001：启用常用语言的 tree-sitter 语法

日期：2026-10-01。状态：已采纳。

## 背景

之前只编译了 Rust、Markdown、Diff 三种语法（JSON 是 Kit 自带的）。Python、JS/TS、TOML、YAML、Go、Shell、HTML、CSS 打开后都按纯文本显示。用户把「常见语言要有高亮」列为基本需求。

## 决定

开启 GPUI Kit 0.7.0 自带的这些 feature：`tree-sitter-python`、`tree-sitter-toml`、`tree-sitter-yaml`、`tree-sitter-bash`、`tree-sitter-javascript`、`tree-sitter-typescript`、`tree-sitter-tsx`、`tree-sitter-go`、`tree-sitter-html`、`tree-sitter-css`。不更换高亮引擎，也不引入 Kit 以外的语法包。文件到语言的映射集中在 `crates/app/src/languages.rs`。

## 代价（Linux x86_64，release + strip，相对于启用前的 44.99 MB）

| 语法组 | 二进制增量 |
| --- | ---: |
| Python、TOML、YAML、Bash | +2.04 MB |
| JavaScript、TypeScript、TSX | +3.25 MB |
| Go、HTML、CSS | +0.34 MB |
| 合计 | +5.63 MB |

按单个语法的静态库体积（`.text` + `.data`）看，最大的是 typescript（同时包含 TS 与 TSX，2.86 MB）和 bash（1.32 MB）；其余依次是 python 0.47 MB、javascript 0.36 MB、go 0.21 MB、yaml 0.19 MB、css 0.09 MB、toml 0.03 MB、html 0.02 MB。依赖树只多了这 9 个语法 crate。

语法只在打开对应文件时才解析，空闲内存基本不变。实际占用随打开的文档数量和大小增长。

## 后果

- CI 的体积预算提高到 45,000,000 字节；`CLAUDE.md` 的体积上限也相应调整。（批次 4 加入 C / C++ / Java / SQL 后再提高到 52,000,000 字节，见下文。）
- 如果以后要压体积，优先考虑去掉 bash（1.3 MB）；或者把 TS 和 TSX 合并为只用 TSX 语法（JSX 的文件能正常解析，普通 TS 文件里的 `<T>` 类型断言会出现解析错误）。

## 补充（批次 4）：C、C++、Java、SQL

用户要求补上这四种语言。开启 Kit 的 `tree-sitter-c`、`tree-sitter-cpp`、`tree-sitter-java`、`tree-sitter-sql` feature；SQL 语法（tree-sitter-sequel 0.3）要求 `cc ~1.2`，构建依赖 `cc` 从 1.5.1 降到 1.2.67（只在编译期使用）。

单个语法的静态库体积（`.text` + `.data`，dist 构建）：

| 语法 | 体积 |
| --- | ---: |
| C++ | 3.40 MB |
| SQL | 2.44 MB |
| C | 0.61 MB |
| Java | 0.41 MB |

连同同一批的代码跳转（ADR 0002），Linux x86_64 `--profile dist` 从 41,347,632 字节增加到 48,698,992 字节（+7.35 MB）。CI 预算提高到 52,000,000 字节。C++ 超过 3 MB；如果要压体积，SQL（2.4 MB）是性价比最低的一个。

## 补充（批次 5）：Ruby、PHP、Zig、Make

用户要求补常见语言，并明确可以放宽体积预算。开启 Kit 的 `tree-sitter-ruby`、`tree-sitter-php`、`tree-sitter-zig`、`tree-sitter-make`；`languages.rs` 的扩展名 / 文件名映射、`SAMPLES`、`comment_for` 与 `indent.rs` 的语言默认同步补齐。

体积（macOS aarch64，`--profile dist`，同一工作区、逐个追加 feature 实测）。基线 29,727,760 字节 / 615 个包（`cargo tree -e normal --target aarch64-apple-darwin --prefix none | sort -u | wc -l`），最终 33,740,656 字节 / 619 个包：

| 语法 | 边际增量 |
| --- | ---: |
| Make | +0.13 MB |
| Zig | +0.69 MB |
| Ruby | +2.11 MB |
| PHP | +1.07 MB |
| 合计 | **+4.01 MB（+13.5%）** |

边际增量逐个实测且可加（四个边际之和与整体差值一致），所以每个语言都能单独取舍：Ruby 最贵（2.11 MB），只留 PHP + Zig + Make 是 31.6 MB，能回到原 32 MB 上限以内。Elixir 实测边际 +1.44 MB，按用户要求已去掉（同时也一并删掉了它的文件名 / 扩展名映射与样例）。这 4 个语法的许可为 MIT / Apache-2.0，都在 `deny.toml` 的允许列表内；和上文同样的结论，语法只在打开对应文件时才解析，空闲内存不变。CI 预算调整为 36,000,000 字节，`CLAUDE.md` 的目标 / 上限同步调整为 34 MB / 36 MB。

以下语言**没有**按同样方式加进来（不是体积问题）：

- **Swift、C#、CMake、Proto、GraphQL**：Kit 0.7.1 给它们的是空查询（`(language, "", "", "")`），语法注册了也不产生任何高亮。要支持必须自己写 `highlights.scm`，属于另一件事。
- **Kotlin、Lua**：Kit 自带高亮查询（`languages/kotlin`、`languages/lua`），但语法 crate 在本机离线取不到（`tree-sitter-kotlin-sg` 0.4.x 缺失；`tree-sitter-lua` 要求 `^0.4.1`，本机缓存只有 0.5.0）。联网 `cargo fetch` 后打开 `tree-sitter-kotlin`、`tree-sitter-lua` 两个 feature 即可生效，`languages.rs` 的映射也要同步加上。

## 补充（批次 6）：Swift、C#、CMake、Proto、Kotlin、Lua

上一批剩下的语言不是体积问题，而是两块不同的拦路石，这一批都解决了：

- **Kit 0.7.1 给 Swift / C# / CMake / Proto 配的是空高亮查询**（`(language, "", "", "")`），语法就算编进来也只显示纯文本。`languages::register()` 在启动时按 Kit 自己的语言名重新注册，查询用语法 crate 自带的 `HIGHLIGHTS_QUERY`；proto 的 crate 不导出查询，它的 `queries/highlights.scm` 复制为 `crates/app/queries/proto-highlights.scm`（MIT，来源见该目录的 LICENSE-QUERIES）。做法与既有的 `diff_syntax::register()` 相同；`languages::tests::every_grammar_highlights_with_app_themes` 保证这些语言在两种主题下都能出颜色。
- **Kotlin / Lua 的语法 crate 本机取不到**：本机 cargo 缓存里没有 `tree-sitter-kotlin-sg`，`tree-sitter-lua` 又只有 0.5.0（Kit 要 `^0.4.1`）。这一批从 static.crates.io 取回（CDN 很慢，kotlin 的包第一次下截断了，断点续传补齐后校验了 tarball）。Kotlin / Lua 不需要 `register()`：Kit 自带它们的查询。
- **GraphQL 没有加**：tree-sitter-graphql 0.1.0 既不带 `queries/`，也不导出查询常量，Kit 那边同样是空查询；要支持就得自己写一份 `highlights.scm`，属于另一件事。

体积（macOS aarch64，`--profile dist`，同一工作区、逐个切换 feature 实测）：

| 状态 | 二进制 | 差额 |
| --- | ---: | ---: |
| 批次 5 结束（Ruby / PHP / Zig / Make） | 33,740,656 | — |
| 加上批次 6 的六个语法 | 49,361,968 | **+15.61 MB** |
| 去掉 Kotlin | 43,516,576 | −5.85 MB |
| 去掉 SQL（对照，同一构建口径） | 46,901,536 | −2.46 MB |

Kotlin / C# / Swift 三个大语法占了这一批的几乎全部增量：Kotlin 实测 5.85 MB；C# 与 Swift 没有单独测，按编译产物大小（kotlin-sg 22.2 MB、c-sharp 20.4 MB、swift 16.2 MB 的 `.o`）与 Kotlin 同量级，各约 4–5 MB。Lua / CMake / Proto 很便宜（`.o` 分别 0.22 / 0.30 / 0.32 MB，合计约 0.5 MB）。`cargo tree` 的包数从 619 变成 629（6 个语法 crate 连同它们的编译期依赖），许可是 MIT / Apache-2.0，都在 `deny.toml` 允许列表内。

**要把体积压回去**，以下是同一构建口径下实测过的选项（可叠加）：

| 做法 | 省 | 代价 |
| --- | ---: | --- |
| `[profile.dist] opt-level = "z"`（现在是 `"s"`） | 4.14 MB | 优化等级换体积；启动与按键延迟未重测（冷启动当前约 40 ms，预算 250 ms / p99 8.3 ms） |
| 去掉 Kotlin | 5.85 MB | 没有 Kotlin 高亮 |
| 去掉 C# 或 Swift | 各约 4–5 MB（估算） | 对应语言没有高亮 |
| 去掉 SQL | 2.46 MB | ADR 0001 早就把它标成性价比最低的一个 |
| 去掉 Ruby | 2.11 MB | 批次 5 里最贵的一个 |
| 只留 TSX（去掉 TypeScript 语法） | 1.44 MB | `.ts` 里的 `<T>` 类型断言会解析错误（上文已记录这个取舍） |
| 去掉 Bash | 1.3 MB | 上文记录的另一个候选 |
| 去掉 PHP / Zig / Make | 1.07 / 0.69 / 0.13 MB | 对应语言没有高亮 |

只去掉 Kotlin + C# + Swift（保留 Lua / CMake / Proto）约回到 34 MB；再叠 `opt-level = "z"` 约 30 MB。按用户当前的选择，这一批全部保留，CI 预算调到 52,000,000 字节，`CLAUDE.md` 的目标 / 上限调整为 50 MB / 52 MB。
