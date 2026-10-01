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

- CI 的体积预算提高到 45,000,000 字节；`CLAUDE.md` 的体积上限也相应调整。
- 如果以后要压体积，优先考虑去掉 bash（1.3 MB）；或者把 TS 和 TSX 合并为只用 TSX 语法（JSX 的文件能正常解析，普通 TS 文件里的 `<T>` 类型断言会出现解析错误）。
