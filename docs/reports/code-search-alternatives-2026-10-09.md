# zvec-grep 的 Rust 内存预期与替代工具

检查日期：2026-10-09。问题：后续 Rust 实现能否明显降低内存，以及什么工具适合替代 Agent 使用的 zvec-grep。场景为 macOS、多个私有代码库、多个 Agent，关注常驻内存、代码语义召回、增量更新和 Agent 接入。

此次只读核对本机既有实测、官方发布记录及候选源码；未安装候选、切换配置、创建或重建索引，也未运行模型性能对比。候选源码按下文固定 main 快照核对，不代表所有功能均已进入正式安装包。

## Rust 能降低什么

**Rust 有望明显减少 Node、JavaScript 依赖及协议层的启动开销，但尚不能保证整个检索服务的内存大幅下降。** 模型运行时、原生向量索引、项目缓存和并发任务仍需要内存。zvec-grep 当前已通过原生 `zvec_node_binding.node` 使用向量库，不能将搜索核心的占用全部归因于 JavaScript。

已有本机证据见 [Agent 内存报告](agent-memory-2026-10-09.md)：

| 测试范围 | physical footprint | 含义 |
| --- | ---: | --- |
| 空 Node 进程，两轮 | 14.5–14.6 MB | Node 基线 |
| 仅导入 STDIO bridge，两轮 | 60.7–61.3 MB | 尚未创建模型或打开索引，启动依赖已有明显成本 |
| 导入 CLI commands 与 bridge，两轮 | 71.6–72.6 MB | CLI 额外启动依赖 |
| 真实 STDIO 两会话 | 约 105.3、105.1 MB | 两份转发进程合计约 210.5 MB |
| 同一守护进程直接 HTTP 两会话 | 无 Node 转发进程 | 工具发现、只读检索和两会话卸载通过 |

导入探针与真实协议会话的工作负载不同，不能用差额推断泄漏。HTTP 取消转发进程后，共享守护进程仍存在。若先切 HTTP，这约 210.5 MB 的转发层就已去掉，不能再计入后续 Rust 升级收益。

本机 code、5g、prod 索引均使用 `potion-code-16m-v2`，256 维，权重文件约 32.49 MB。文件大小不等于内存；当前未量化模型、worker、索引和缓存各自的贡献。

截至检查时，本机及 npm latest 均为 **0.2.2**。维护者表示当前 TypeScript 实现不计划大幅重构 lazy loading，后续 Rust 将移除该启动依赖结构；这属于路线说明，未给出同负载内存降幅。Rust 实现位于独立的 `rust/` 目录，CLI 和索引格式有差异，不宜将 main 直接当成已发布的兼容替换版本。

Rust 文档描述的共享模型 runtime、15 分钟模型空闲回收、60 秒读会话缓存以及 4 小时项目回收，在当前 Node 0.2.2 中已有对应机制，不能算作 Rust 独有收益。

来源：[npm registry](https://registry.npmjs.org/@zvec%2fzvec-grep)、[维护者回复](https://github.com/zvec-ai/zvec-grep/issues/72#issuecomment-5586481736)、[固定快照 Rust README](https://github.com/zvec-ai/zvec-grep/blob/a09cd1236eee003feb699a17c5bf63d76f83f06c/rust/README.md)。

## 替代的是完整 Agent 检索工具

zvec-grep 同时承担文件切块、embedding、索引维护、排序、检索和 Agent 接入。单换 Qdrant、LanceDB 等向量存储，不能直接替代这些功能，也不会自动消除每个 Agent 的转发进程。

| 方案 | 适用位置 | 内存与能力取舍 | 建议 |
| --- | --- | --- | --- |
| ripgrep + ast-grep CLI | 精确、正则及代码结构检索 | 按需执行、结束退出，无需常驻模型或持久语义索引；无法等价提供自然语言语义召回 | 常驻内存优先时首选 |
| CocoIndex Code | 代码语义检索 | 共享 daemon、AST 切块、增量更新；Python 前端加 Rust 内核，本地 full 版使用 PyTorch，尚无更省内存的实测 | 保留语义能力的优先试点候选 |
| QMD | 本地文档与代码资料的混合检索 | 共享 HTTP MCP；默认 embedding、rerank、query expansion 模型文件较大，不能据此认定省内存 | 文档知识库优先，当前减内存目标下不优先替换 |
| mgrep | 云端代码语义检索 | 文件上传 Mixedbread，避免本地模型但依赖云端；本机占用收益未测 | 仅在接受代码上传时考虑 |

### ripgrep + ast-grep

[ripgrep](https://github.com/BurntSushi/ripgrep) 提供精确字符串和正则检索，遵循 `.gitignore`；[ast-grep](https://ast-grep.github.io/guide/quick-start.html) 使用 AST 模式做结构匹配。Agent 可直接调用 CLI，无需另加 Node/Python MCP 包装。两者适合已知符号、错误文本、配置名或代码模式；「某个业务行为由哪些模块负责」这类未知措辞问题仍需要语义检索或多次人工组织查询。

### CocoIndex Code

核对快照：[b883be0b5d762c9e2d7d82afdedeade5df21d5be](https://github.com/cocoindex-io/cocoindex-code/tree/b883be0b5d762c9e2d7d82afdedeade5df21d5be)，2026-10-06。

支持 AST 代码语义检索、changed-files 增量、语言及路径过滤、`.gitignore`。CLI 和 STDIO MCP 为客户端，通过全局 Unix socket daemon 共享 embedder 与多个项目。本次源码核对未发现官方 HTTP MCP 入口。

Python >=3.11 前端、Rust CocoIndex 内核、sqlite-vec；full 版使用 SentenceTransformers/PyTorch，slim 版通过 LiteLLM 接 embedding provider，可用云端或本地 Ollama。README 中约 1 GB torch + transformers 指依赖体积，不是内存实测。

默认 daemon idle 为 180 分钟，`keep_alive_with_mcp=true` 时 MCP heartbeat 持续保温，可改为 false 按真实请求空闲退出。新 `ccc grep` 结构检索依赖尚未正式发布的 CocoIndex `code_match`，不能当作现成稳定能力。Rust 内核并不能证明它比本机 Potion 小模型更省内存。

### QMD

核对快照：[93d211f9ef4a869a9aed0d075ca767dda552627f](https://github.com/tobi/qmd/tree/93d211f9ef4a869a9aed0d075ca767dda552627f)，2026-10-06。

Node/Bun 与 node-llama-cpp，提供本地 BM25、向量及重排，支持共享 HTTP MCP。main 已有 TS/JS、Python、Go、Rust 等 AST chunking，不能说完全不支持代码。

默认模型文件约为 embedding 300 MB、reranker 640 MB、query expansion 1.1 GB；这些不是 RSS 或 physical footprint。HTTP 模式空闲 5 分钟仅释放 contexts，模型仍加载。索引更新与 embedding 分别由 `qmd update`、`qmd embed` 维护，本次未建立 zvec 式自动 watcher 刷新证据。BM25 排序搜索不等同于 rg 的穷尽精确匹配。

### mgrep

核对快照：[5c1ba628c62d9f3cf96cb98b3616f2ac23698aad](https://github.com/mixedbread-ai/mgrep/tree/5c1ba628c62d9f3cf96cb98b3616f2ac23698aad)，2026-04-25，快照 package 0.1.13。

Node/TypeScript CLI，语义搜索由 Mixedbread 提供。官方 README 明示文件推送到云端 Store，`mgrep watch` 监听变化增量上传，支持忽略规则。它不满足全本地私有代码检索条件。

核对快照的 MCP 实现 `ListTools` 为空、`CallTool` 未实现，主要负责后台同步；不能将它描述为完整共享 HTTP MCP 搜索服务。官方同样建议保留 grep 做精确查询。

## 当前推荐

以下选型、MCP 传输和检索策略属于 Agent 配置及外部工具。ZJ 保持 Agent 会话接口，不直接适配这些工具，不管理其模型、索引或共享守护进程。Codex 子会话订阅回收仍保留在 Codex 桥接内部。

1. **在 Agent／工具配置层优先评估 zvec-grep 的直接 HTTP 连接。** 这是已有本机验证的收益，适合多 Agent；实际切换仍需核实工具支持的服务启动、鉴权和交互请求，不能转为 ZJ 的工具专属功能。当前全局配置仍是 `zg server --stdio`，此次未改动。
2. **已知符号和文本用 rg，需要结构匹配时用 ast-grep。** 自然语言语义发现继续使用共享 zvec-grep，避免把精确检索都交给模型索引。
3. **需要完整替换语义检索时，优先试 CocoIndex Code。** 对同一代码库测检索召回、增量正确性、冷/热请求、单/多 Agent 和空闲后的 physical footprint，再决定；目前没有候选被证明比本机 zvec-grep 更省内存。
4. **Rust 等正式发布后做同负载对照。** 重点看共享守护进程的收益、CLI/MCP 兼容和索引迁移，避免重复计算 HTTP 已去除的转发开销。

替换通常落在 Agent 的 MCP 配置及检索指引，例如 Codex 的 `~/.codex/config.toml`，无需据此重写 ZJ 编辑器核心。此次仅新增研究报告；验证为来源核对和文档差异检查，无源码改动，因此未运行项目测试。
