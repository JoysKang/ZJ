# fsearch 对 ZJ 搜索的适配评估

检查日期：2026-10-11。问题：ZJ 是否已经使用 [noahdunnagan/fsearch](https://github.com/noahdunnagan/fsearch)，以及它宣称的微秒级查询是否值得替换现有搜索。场景为 macOS 原生编辑器、多工作区，优先考虑低内存、文件变更后的正确性和交互延迟。此项目是 Rust 的 macOS 搜索引擎，与 Linux 图形应用 FSearch 不是同一个项目。

**ZJ 当前没有使用 fsearch，现阶段不建议整体替换。** 当前文件名索引在常见工程规模已经是亚毫秒查询，内存也很小；fsearch 的主要优势是把百万级、全盘查询的计算和磁盘读取成本提前放到持久索引中。它适合未来单独评估超大工程或全盘找文件，暂不适合直接替换项目全文搜索与安全替换流程。

本次仅调研、读取源码，并运行现有 ZJ 文件名查询的临时微基准；没有接入 fsearch、创建其持久索引、运行全盘扫描或测量其本机内存。下文区分作者基准、源码核对和本机实测，不将不同硬件、数据集、返回限额的数字解释为性能倍数。

## 核对版本与成熟度

上游快照为 [b88f4e9a90703eb7338085b325c2852fcaf131a9][commit]，提交时间为 2026-10-10 18:41:05 UTC，即北京时间 2026-10-11 02:41:05。Cargo 包版本为 **0.1.0**，许可证为 **MIT**。[GitHub API][repository-api] 显示仓库创建于 2026-10-08 17:50:33 UTC；此次观察时尚无 [GitHub releases][releases-api]。因此按固定提交评估，不将 main 当成稳定版本承诺。[Cargo.toml][manifest]、[LICENSE][license]

维护者在 [issue #9][platform-issue] 明确表示保持 macOS 专用。Linux 支持 [PR #8][linux-pr] 已关闭但未合并；带 `FSEARCH_ROOT` 和 Engine／daemon／CLI 测试的 [PR #14][root-pr] 仍未合并。不能将分支能力算到当前主线。固定快照没有常规 `tests/` 或 GitHub CI 工作流，提供的性能脚本及示例不等于自动回归测试体系。[文件树][tree-api]、[e2e.rs][e2e]

## ZJ 当前搜索实现

依赖清单、锁文件和源码没有 fsearch 依赖或引用。几种搜索分别承担不同任务：

| 范围 | 当前实现 | 主要约束 |
| --- | --- | --- |
| 文件名快速打开 | [PathIndex](../../crates/app/src/files.rs) + [模糊评分](../../crates/app/src/fuzzy.rs) | 路径紧凑存储；Unicode 小写、子序列评分；最多 200,000 条路径，查询最多 300 条结果 |
| 项目全文搜索 | [text_search.rs](../../crates/app/src/text_search.rs) | 按查询读取磁盘当前内容，使用 `regex::bytes`；后台并行、流式结果；大小写／整词／正则／包含排除规则；跳过二进制及超过 2 MiB 的文件 |
| 文件变更 | [共用原生监听](../../crates/app/src/watch.rs) + [工作区刷新](../../crates/app/src/workbench/workspace_refresh.rs) | 共享 FSEvents，增量更新文件名索引；忽略规则变化时按需重建 |
| 搜索替换 | [search_replace.rs](../../crates/app/src/workbench/search_replace.rs) | 依赖文件校验与匹配字节范围，替换前检查文件是否变化 |
| Agent 历史搜索 | [agent_history](../../crates/agent_history/src/lib.rs) | SQLite FTS5 与 CJK 处理，搜索会话历史，不是项目文件内容 |
| Agent 外部检索工具 | Agent 自身配置 | ZJ 不管理工具专属索引和 daemon，见 [既有检索工具报告](code-search-alternatives-2026-10-09.md) |

文件名索引通过 Git 跟踪及未忽略的未跟踪路径，或有界目录遍历构建。索引保存路径及少量元数据，没有预先存储项目文件全文。当前全文搜索也不是调用 ripgrep 子进程，而是 ZJ 内部的 Rust 实现。

文件名查询有 16 ms 防抖；全文搜索有 250 ms 防抖及 100 ms 结果刷新间隔。优化微秒级引擎耗时，也不会按同样比例缩短界面响应时间。[快速打开](../../crates/app/src/workbench/quick_open.rs)、[搜索视图](../../crates/app/src/workbench/search_view.rs)

## 「微秒级」具体指什么

[README][readme] 的主要数字来自作者 M4 Max、8.3 百万磁盘文件测试，本次未复测：

| 作者声明 | 数值 | 解读 |
| --- | ---: | --- |
| 文件名查询 | 0.13 ms，即 130 µs | 已有文件名索引上的查询，不含首次扫描 |
| 文件内容查询 | 2.3 ms | 已有 trigram／过滤索引上的内容查询 |
| CLI 启动到答案 | 3.3 ms | 含进程及协议开销，已经不是 130 µs |
| 首次完全可搜索 | 49 s | 全盘预处理成本 |
| 首次构建峰值内存 | 0.9 GB | 构建峰值，不是常驻内存 |
| Chromium 内容索引磁盘大小 | 0.76 GB | 磁盘体积，不是 RSS 或 physical footprint |

README 将该版本与旧版本比较，并说明结果相同；不能据此证明它在 ZJ 的相同项目和规则下更快或更省内存。官方脚本也限定了测量范围：

- [examples/rig.rs][rig] 载入已有索引并预热，交替查询避免重复命中上一条查询缓存；不包含 GUI、首次建索引或进程启动。
- [demo/vs_fff.py][vs-fff] 对比 fff 时，fsearch 使用常驻 daemon socket，fff 使用 Python 包的进程内接口。双方预热；文件名取 50 条结果。样本限定唯一、6–32 字节、ASCII 文件名，拼写错误不改首字母，不能外推到所有名字或中文输入。
- 该脚本全文计时返回前 50 个匹配文件，每文件最多 1 行；另跑不限时的大限额查询核对集合。结果并不完全相等：作者也承认 fsearch 因文件类型和 `build/`、`vendor/` 过滤少读部分文件。[Chromium 原始结果][chromium-results]
- 对比表的「63 MB」由 macOS `footprint` 读取 daemon physical footprint；Chromium 原始结果同时记录 9,009,581 个文件名索引项。这是该次全盘 daemon 的占用，不是「ZJ 增加这项功能只需要 63 MB」，也不是同工作区对照。[脚本][vs-fff]、[原始结果][chromium-results]

源码确实具备实现高查询速度的机制：文件名按目录组织在 mmap 文件内，字符位图先排除大部分候选；内容索引保存 trigram 倒排和每份内容的过滤器，重复内容只索引一次，筛出候选后再读取磁盘验证。[index.rs][index]、[query.rs][query]、[content.rs][content]

## 现有 ZJ 文件名查询的本机微基准

在 Apple M5 Pro、arm64、`rustc 1.99.0`、`-C opt-level=3` 下，直接复用当前 `PathIndex::search`、打包存储逻辑和原 `fuzzy.rs`。临时 harness 排除目录索引构建，但计入评分、排名、300 条限额及返回 `PathBuf` 的成本；每查询预热 10 次，再计时 100 次。

| 路径集 | `main` 中位数 | `main` p95 | `rs` 中位数 | 路径索引 heap |
| --- | ---: | ---: | ---: | ---: |
| ZJ 当前 596 个 Git 跟踪路径 | 52.79 µs | 70.71 µs | 153.17 µs | 31,589 B |
| 20,000 个合成路径 | 584.33 µs | 687.04 µs | 887.08 µs | 721,290 B，约 0.72 MB |
| 200,000 个合成路径 | 5.084 ms | — | 8.370 ms | 7,214,890 B，约 7.21 MB |

合成路径为 `src/mxxxx/file_xxxxxx.rs` 形式，4% 带 `main`，1% 为中文文档路径；`说明` 查询在 20,000 条数据上中位数为 518 µs。这证明当前索引在这组负载下已经很快，大规模时仍有线性扫描成本。

复核材料为本次会话的临时文件 `/private/tmp/zj-fsearch-path-bench.rs` 和 `/private/tmp/zj-fsearch-path-bench.log`，不是长期仓库基准。未测 Git 扫描、磁盘 IO、GUI、端到端延迟或整个应用 physical footprint，也未运行相同数据集的 fsearch 对照。**不能用本表与作者 M4 Max／8.3 百万文件数据计算胜负或倍数。**

## 功能适配与正确性差异

| 能力 | fsearch 当前快照 | 对 ZJ 的影响 |
| --- | --- | --- |
| 文件名与路径 | 子序列评分；多词可分别匹配名字及祖先目录；名字至少匹配一个正 token | 排序、跨目录匹配和空白语义不同，不能只换依赖而保持结果相同 |
| 拼写容错 | 至少 5 字节的 fuzzy token 可接受一次插入／删除／替换／相邻交换；首字节须匹配，数字不纠错，主要在名字及空格词首尝试 | 是当前子序列匹配没有的增益，但不是任意位置的通用纠错 |
| 文件名 Unicode／中文 | 保留 UTF-8 字节；查询小写与 `fold` 仅处理 ASCII，评分及纠错按字节 | 中文原文可参与匹配，不能称为完整字符感知、中文纠错或 Unicode 大小写匹配 |
| 正则／内容 | Rust `regex::bytes`，字面量、正则、smart-case；`sym:` 为声明模式启发式 | 当前独立大小写和整词开关需适配；符号模式不是 LSP／AST 语义分析 |
| Git 忽略 | 没有读取项目 `.gitignore`；内容用硬编码路径、扩展名过滤 | 可能包含当前不展示的路径，也可能遗漏应搜索源码，需复用 ZJ 路径集合和规则 |
| 内容覆盖 | 以 home 为索引范围；跳过 `vendor`、`build`、`Library` 等；扩展名白名单，排除 `.min.js`、`package-lock.json` | ZJ 自身有 `vendor/gpui-base` 等源码，不能按目录名直接全局跳过 |
| 文件大小 | 索引及未覆盖目录的备用扫描均限制到 1 MiB | 不能直接覆盖 ZJ 当前 2 MiB 上限的全部文件 |
| 增量更新 | 文件名跟随 FSEvents，作者称约 0.1 s；内容等 2 s 安静期，持续变化最多等 300 s 再安排处理 | 候选可能滞后；读取最新文件防止展示旧文本，却不能保证新匹配立即召回 |
| 持久化 | `index.bin` + mmap 内容 segments；单写者锁、follower；重启回放事件 | 支持共享及复用，但新增索引目录、格式升级和恢复策略 |
| 结果限额 | 默认 50 个文件、每文件 5 个匹配行、250 ms 候选读取预算，均可调整 | 需与当前匹配上限及流式呈现对齐，不能把前几行当完整搜索 |
| 搜索替换 | `FileMatches` 为路径及行号／文本，无匹配 byte span 或文件 stamp | 高亮、单个匹配替换、捕获组替换和文件变更校验需保留现有匹配阶段 |
| 未保存缓冲区 | 内容验证读取磁盘文件 | 不会自动解决未保存编辑内容搜索 |
| 语义检索 | 未发现 embedding／语义模型检索 | 不等价替换 Agent 的 zvec-grep |

来源：[Query 解析与评分][query]、[内容过滤、Grep、FileMatches、scan_paths][content]、[Engine 与内容更新][engine]、[FSEvents][fsevents]、[README][readme]。

`GrepResult.complete` 不是「所有文件所有匹配已穷尽」标记：处理完候选或满足结果限额都会置为 true。批量替换需独立判断是否截断并补全精确范围。[verify_from][verify-from]

中文内容匹配的最终正则支持 Unicode 行为，但候选索引及若干快速路径使用字节和 ASCII 折叠。本次没有实测中文、非 ASCII 大小写、短模式或任意正则的召回一致性，因此不作等价覆盖承诺。

## 内存、CPU 与嵌入成本

Rust 实现不依赖 Node、Python 或 embedding 模型，直接依赖为 `libc`、`rayon`、`memchr`、`memmap2`、`regex`、`regex-syntax`、`serde`、`serde_json`。原生实现不等于已满足 ZJ 预算。上游 release 使用 fat LTO，并配置 Apple Silicon `target-cpu=native`；成为依赖后，最终优化由 ZJ workspace 控制，不能假设复现上游构建结果。本次未测二进制增量或编译时间。[Cargo.toml][manifest]、[构建配置][build-config]

官方有两种接入方式：[README][readme]、[lib.rs][lib]

1. **进程内 Rust Engine。** 无需协议转发，但 `Options` 只有索引目录、home、排除目录；当前首次构建固定 `walk::scan(b"/", 8)`，不是传项目根目录即可使用的工作区引擎。它自己管理 FSEvents、内容线程、锁及合并；没有公开 shutdown，线程持有共享状态持续运行，按工作区打开／关闭需改造回收。自行用公开 `walk`、`Index`、`Searcher` 组装，则由 ZJ 负责忽略、增量及生命周期，成本高于只替换 scorer。[engine]、[walk]、[index]
2. **JSON-lines socket／stdio。** 可复用用户已有共享 daemon，避免 ZJ 管理索引；新用户仍需独立安装、扫描、权限及版本处理。Full Disk Access 与启动方式有关，后台服务占用应计入总成本。当前整条 JSON 回复还需衔接现有流式匹配、取消及替换。[readme]、[server]

默认 daemon 对持续连接、等待下一次按键的客户端调用 `keep_warm(250 ms)`：高 QoS 线程使用 `spin_loop` 保持 CPU 主频，减少下一查询延迟。这有时间边界，不是永久忙等；直接 Rust API 可以不调用。它体现了延迟与能耗的交换，本次未测实际 CPU 或电池影响。[server 的 WARM／handle][server-warm]、[engine 的 warmer][engine-warm]

上游 [issue #16][memory-issue] 报告 FSEvents callback 上下文和 overlay 目录记录可能增长；当前源码仍明确写出 callback sender 保留不释放，以防回调仍在执行。这是**开放问题与源码线索，不是本次复现的大内存泄漏**。接入前需验证频繁临时目录增删、长时间运行及退出回收。[fsevents]

ZJ 准入要求新增依赖测依赖数、dist 二进制及空闲 footprint；体积增加超过 200 KB 或 footprint 增加超过 5 MB 需决策记录。公开 63 MB daemon 数据不能代替准入测量。[CLAUDE.md](../../CLAUDE.md)

## 推荐范围

**保留当前文件名搜索及全文搜索。** 常见工程已有低成本、亚毫秒查询，尚无同负载证据证明值得改变。全文搜索／替换依赖项目规则、当前磁盘内容及文件校验；预索引引入的候选滞后和覆盖差异需要先解决。

有价值的后续范围是：

- **百万级路径、跨工作区或全盘找文件。** 在现有上限或排名耗时成为真实瓶颈后，用相同路径集合、返回数量及硬件对照；已有共享服务时可考虑可选连接，让工具管理索引。
- **借鉴位图预过滤、按目录组织候选。** 可用于超大工作区文件名加速，先验证现有 200,000 条负载是否值得增加复杂度。
- **拼写容错作为独立需求。** 先在现有 scorer 上做小型可回退验证，无需同时引入全盘 Engine 和内容索引。

若决定试点，先验证项目根目录与忽略规则、中文／正则召回、频繁增删及新内容召回、完整匹配和替换数据，再测冷启动、预热查询、首次构建、多窗口和长期空闲资源。证据充分前，不建议设为默认后端。

此次仅新增报告，保留此前未提交滚动修复，未修改生产代码或外部配置。验证为固定提交源码、官方脚本及 API 核对、现有 ZJ 路径查询微基准；未运行项目构建或测试。

[commit]: https://github.com/noahdunnagan/fsearch/commit/b88f4e9a90703eb7338085b325c2852fcaf131a9
[repository-api]: https://api.github.com/repos/noahdunnagan/fsearch
[releases-api]: https://api.github.com/repos/noahdunnagan/fsearch/releases
[tree-api]: https://api.github.com/repos/noahdunnagan/fsearch/git/trees/b88f4e9a90703eb7338085b325c2852fcaf131a9?recursive=1
[readme]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/README.md
[manifest]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/Cargo.toml
[license]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/LICENSE
[lib]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/lib.rs
[engine]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/engine.rs
[engine-warm]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/engine.rs#L130
[index]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/index.rs
[query]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/query.rs
[content]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/content.rs
[verify-from]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/content.rs#L2444
[walk]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/walk.rs
[fsevents]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/fsevents.rs
[server]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/server.rs
[server-warm]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/src/server.rs#L73
[rig]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/examples/rig.rs
[e2e]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/examples/e2e.rs
[vs-fff]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/demo/vs_fff.py
[chromium-results]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/demo/vs_fff_chromium.json
[build-config]: https://github.com/noahdunnagan/fsearch/blob/b88f4e9a90703eb7338085b325c2852fcaf131a9/.cargo/config.toml
[platform-issue]: https://github.com/noahdunnagan/fsearch/issues/9
[linux-pr]: https://github.com/noahdunnagan/fsearch/pull/8
[root-pr]: https://github.com/noahdunnagan/fsearch/pull/14
[memory-issue]: https://github.com/noahdunnagan/fsearch/issues/16
