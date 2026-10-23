# Rust／GPUI 升级、Diff 交互与标签正文调研

调研及访问日期：2026-10-22。目标：确定可验证的升级路线，挑选适合 ZJ 的原生 Diff 交互，并设计 annotated tag 正文读取。只写本报告，未修改源码、配置或依赖，未安装、更新、构建或进行资源实测。

## 结论与证据边界

- **已确证的本地锁定值**：Rust `1.99.0`、`gpui-kit/gpui-base/gpui-component 0.7.1`、`gpui-pre/gpui-pre-apple/gpui-pre-macos 0.3.8`。来源为 [toolchain](../../rust-toolchain.toml)、[workspace manifest](../../Cargo.toml) 和 [Cargo.lock](../../Cargo.lock)。工具链文件本身仅表示项目要求。主任务协调者随后转述 worker 已实际执行版本检查，`rustc` 与 `cargo` 均为 `1.99.0`；本报告据此记录本机已安装，但没有用该结果替代官方发行渠道核验。
- **本次未能确证任何「官方最新」版本**。crates.io API、Rust stable channel、官方 GitHub Release API、Rust 博客及 Diff 产品文档均被 WebFetch 域名安全校验阻断；返回 `Unable to verify if domain … is safe to fetch`。未改用代理、其他网络工具或安装命令绕过。系统日期不能用来推算发行号。
- [2026-10-12 旧报告](gpui-kit-0.7.3-2026-10-12.md)记录了 `gpui-kit 0.7.3 → gpui-pre 0.3.9`、`gpui-fast 0.1.7`，以及 0.7.2 撤回。这是**本地历史记录，非本次一手复核结果**；其发布日期、当前 yanked 状态与是否仍为最新均待官方核验。不可据此直接更新锁文件。
- 建议升级顺序为「核验官方版本 → 单独升级 Rust → 升级 Kit 与其指定 GPUI 快照并迁移 vendor → 验证功能和资源预算」。Fast 后端另行评估。Diff 优先做未改动区折叠与跨文件审阅队列，标签正文可作为独立小改动。

## 版本核验与迁移路线

| 项目 | 当前本地值 | 官方最新 stable／non-yanked | 复核入口与当前结果 |
| --- | --- | --- | --- |
| Rust | `1.99.0` | **未确证** | [stable channel](https://static.rust-lang.org/dist/channel-rust-stable.toml)、[官方发布博客](https://blog.rust-lang.org/)：2026-10-22 访问失败，未取得发布日期 |
| GPUI Kit | `0.7.1` | **未确证**；旧报告候选 `0.7.3` | [crates.io API](https://crates.io/api/v1/crates/gpui-kit)、[官方 latest Release API](https://api.github.com/repos/longbridge/gpui-kit/releases/latest)：2026-10-22 访问失败 |
| gpui-pre | `0.3.8` | **未确证**；旧报告候选 `0.3.9` | [crates.io API](https://crates.io/api/v1/crates/gpui-pre)：2026-10-22 访问失败；其版本必须匹配所选 Kit |
| GPUI Fast | 未启用 | 不作为本次默认升级目标 | [旧报告引用的 v0.1.7](https://github.com/longbridge/gpui-fast/releases/tag/v0.1.7)：旧报告记载发布于 2026-10-12，本次未重验 |

后续核验标准：Rust 读取 stable channel 的 `date` 与 `[pkg.rust].version`，与官方公告对应；crates.io 逐版本检查 `num`、`yanked`、`created_at`、`rust_version`，按 SemVer 取无预发布后缀且未撤回的最高版本，再核对官方 Release 与所选版本的 manifest。GitHub `latest`、主分支文档、crate 名中的 `pre` 都不能单独证明发布通道或兼容性；「未撤回的正式版本号」也不等于 API 已稳定。

**迁移成本为工程推断，尚未编译验证**：

1. **Rust：低至中。** 当前使用 edition 2024；toolchain 注释称旧 GPUI 使用 `std::hint::cold_path`，1.94 无法编译。这个注释不是完整 MSRV 证明。取得官方最新 stable 后检查所有选定依赖的 MSRV；若官方渠道实际返回值低于当前配置，先解释配置与渠道差异，不把降级称为升级。工具链单独提交，以便区分新增 lint 与 GUI 变化。
2. **Kit＋默认 GPUI：中至高。** 项目固定精确版本，三个 path patch 也有自己的版本号。必须依据新 Kit manifest 迁移整个对应快照，检查解析结果确实命中新的 vendor；不能单独把 `gpui-pre` 改为另一个「最新」。旧报告提示 0.7.3 有 TextView 默认间距和后端选择变化；这是待复核的视觉回归线索。
3. **Fast：高，暂缓默认启用。** 旧报告记录独立包身份、保留视图和滚动缓存。其性能数字不是 ZJ 实测；当前低内存策略依赖停止出帧，缓存更多画面不必然降低 footprint。只在默认后端升级完成后做独立候选和 A/B 测量。

### vendor 必须保持的行为

以下是当前 [vendor/README.md](../../vendor/README.md) 的本地事实。除非固定版本源码证明上游有等价行为，且回归和资源测试通过，否则保留；不要求机械保留旧补丁文本。

| 组件 | 必须保留／迁移的行为 | 遗失后影响 |
| --- | --- | --- |
| `gpui-pre-apple` | 双 drawable；path／MSAA 纹理按需分配与闲置释放；遮挡／隐藏释放及恢复；可见空闲 trim；同设备共享弱引用图集；shader 库进程级复用；彩色图集初始 512²；headless 例外 | 空闲与多窗口内存回升；恢复窗口时可能丢画面 |
| `gpui-pre-macos` | display link 空闲暂停、输入／异步事件唤醒；遮挡与恢复时强制 present；通知默认声音、前台声音、等待授权、同 tag 新请求替换旧回调与撤回 | 常驻唤醒、恢复空白、通知重复或丢失 |
| `gpui-base`：资源与定位 | 常亮光标且无计时器；一次性 cursor／viewport intent；⌘点击未预取定义时仍发起查询并验证文本／焦点 | 空闲不停帧；审阅后跳错位置；定义跳转失效 |
| `gpui-base`：扩展 API | `BeforeLineWidget` 行前控件及折叠／IME／选区／视口几何；保持选区的可撤销插入；行尾 blame 注释与横滚范围；代码块通栏标题；当前行背景覆盖右内边距 | 冲突操作、设置迁移、blame、代码复制和视觉行为退化 |
| vendor manifest | 当前 warnings 例外和工作区排除方式需要逐项核对；不是无条件照搬的新版本质量策略 | path 依赖的上游警告可能影响检查；宽泛抑制也可能隐藏新问题 |

`ZJ_GPU_LOWMEM=0` 只能用于相关资源策略的 A/B；不要据此假定所有编辑器 API 和通知补丁都有运行时开关。新版补丁清单必须包括最近加入的行前控件和保持选区插入，不能只复用旧报告较短的清单。

后续实施验收：项目规定的 fmt、clippy、workspace tests、dist build、`tools/check_git_deadlines.py`；vendor 文档要求的 `idle_ui` 回归和 `ZJ_FRAME_LOG=1` 停帧／唤醒检查；当前 [CLAUDE.md](../../CLAUDE.md) 的资源门禁。当前上限是二进制 52 MB、单窗口 100 MB、3 窗口＋20 文档 300 MB、空闲 CPU p95 ≤ 1%（2026-10-22 按用户要求调整）；**旧报告的 32 MB 体积门槛已经不适用**。依赖数量、dist 体积、包含 Git 子进程的 physical footprint 必须实测，不能把官方 benchmark 当作迁移结果。

## 可移植 Diff 交互：4 个候选

**已实现基线**：[diff_doc.rs](../../crates/app/src/diff_doc.rs) 持有两侧全文、语法／字符高亮和行索引；[diff_view.rs](../../crates/app/src/workbench/diff_view.rs) 使用虚拟列表，支持左右／内联和前后块导航；[diff_ops.rs](../../crates/app/src/workbench/diff_ops.rs) 支持选择和块／行操作；[agent_review.rs](../../crates/app/src/workbench/agent_review.rs) 已有 AI 逐块／整文件接受或拒绝。不要把这些再包装成新功能。

下表是**待一手核验的参考方向＋基于本地代码的设计建议**。本次无法读取外部 Diff 文档和源码，因此不把候选产品当前能力、维护状态或许可证写成已确认事实。给出官方入口以供复核，未引用未读页面中的具体 API。所有网页发布日期未知；已尝试的文档访问日期为 2026-10-22，失败，其余链接为后续核验入口。成本与内存影响均为推断，不是实测。

| 交互与参考 | ZJ 的具体增量 | 成本／内存影响 | 一手入口与许可证核验 |
| --- | --- | --- | --- |
| **未改动区折叠、按需展开上下文**；优先核对 VS Code Diff | 在现有行索引上增加显示投影，保留前后上下文和展开按钮；选择、复制、跳转和暂存始终映射原始行，不能把隐藏行误认为删除 | 中；额外存区间／索引即可。减少绘制行不等于释放当前两侧全文与高亮内存 | [VS Code 官方 SCM 文档](https://code.visualstudio.com/docs/sourcecontrol/overview)（访问失败）；[Diff 源码目录](https://github.com/microsoft/vscode/tree/main/src/vs/editor/browser/widget/diffEditor)、[LICENSE.txt](https://github.com/microsoft/vscode/blob/main/LICENSE.txt)（待核验；预期 MIT，不能据此直接复制） |
| **跨文件审阅队列、下一处未处理改动**；核对 Zed project diff／多片段交互 | 复用文件列表和单文件 Diff，一次只加载当前文件；记录本轮待审／已处理状态，跨文件跳转，操作后保持阅读位置。适合 AI 一轮改多个文件 | 中；元数据随文件数增长；避免同时构建每个文件的 `DiffDoc`。现有审阅快照也要计入总预算 | [Zed Git 文档](https://zed.dev/docs/git)（访问失败）；[project_diff 源码](https://github.com/zed-industries/zed/tree/main/crates/project_diff)、[仓库许可](https://github.com/zed-industries/zed/tree/main/licenses)（待核验）。[本地旧调研](gpui-ui-libraries-assessment-2026-10-11.md)仅确认过 UI crate 为 GPL-3.0-or-later，不能替代具体 Diff 文件许可核对，也不能把 GPUI 的 Apache-2.0 套给编辑器代码 |
| **比较来源与操作语义明确**；核对 GitButler 的工作区／暂存／提交差异呈现 | 标明「工作树 ↔ 索引」「父提交 ↔ 当前提交」「AI 修改前快照 ↔ 当前内容」；按来源提供暂存或接受／拒绝。已审阅、已接受和已暂存分别记录，不把 AI 接受等同于 Git 暂存 | 低至中；主要是元数据与命令路由。来源不变时复用现有文本，避免为标签重新生成 diff | [GitButler Diff 文档入口](https://docs.gitbutler.com/features/diff-view)（访问失败，页面存在性也未确认）；[官方仓库](https://github.com/gitbutlerapp/gitbutler)、[LICENSE.md](https://github.com/gitbutlerapp/gitbutler/blob/master/LICENSE.md)（待核验，不假定为 MIT／Apache） |
| **可见的忽略空白开关**；核对 Meld 文本过滤交互 | 仅改变阅读比较，明确显示开关状态；写操作仍使用未过滤补丁并重验原内容。先支持空白，暂不引入任意正则过滤或语义 Diff 引擎 | 中；重算可能临时同时保有两个结果，取消旧任务并有界替换。不要额外长期缓存一整份全文 | [Meld 官方文本过滤说明](https://meldmerge.org/help/text-filters.html)、[官方源码](https://gitlab.gnome.org/GNOME/meld)、[COPYING](https://gitlab.gnome.org/GNOME/meld/-/blob/main/COPYING)（均待核验；预期 GPL 系列，具体版本以拟采用提交为准） |

优先级建议：折叠 → 来源标签／跨文件审阅队列 → 忽略空白。只借鉴交互并在现有 GPUI 实现，不引入 Electron、WebView 或整套编辑器运行时。许可证未核验前不复制第三方实现；未来若移植代码，应固定 commit 并核验文件头和依赖许可证。网络恢复后只复核上述少量候选，无须穷举工具。

资源约束：[git_service](../../crates/git_service/src/lib.rs) 当前 stdout 上限为 16,000,000 字节、stderr 32,000 字节；Diff 总行数上限为 200,000，长于 4,096 字节的行跳过精细高亮，字符 LCS 与行对齐各有单元数预算。虚拟列表只限制视图对象，后台全文／高亮仍占内存。新增交互应沿用取消和 generation 校验，不在 render 中计算或启动 Git。

## annotated tag：正文与安全读取

### 已确证的语义及当前缺口

本次读取了随本机 `git version 2.54.0 (Apple Git-157)` 提供的 Git 一手手册；`git-for-each-ref.1` 标记文档日期为 **2026-04-19**，访问日期为 **2026-10-22**。在线对应入口未成功读取，以下事实来自本机手册：

- annotated tag 有独立 tag 对象、tagger、日期、消息和可选签名；lightweight tag 只是直接指向对象的引用，通常指向 commit。[git-tag](https://git-scm.com/docs/git-tag)
- `%(contents:subject)` 是第一段；`%(contents:body)` 是余下消息；`%(contents:signature)` 可读 tag 签名；带 `*` 的字段读取剥离后的目标，而不是 tag 对象。因此读取标签正文时不能误用提交消息或 peeled 字段。[git-for-each-ref](https://git-scm.com/docs/git-for-each-ref)
- `cat-file` 能查询对象类型、大小及内容；`--textconv`／`--filters` 会采用转换机制；`rev-parse --verify --end-of-options` 能避免把不可信名称当选项。[git-cat-file](https://git-scm.com/docs/git-cat-file) · [git-rev-parse](https://git-scm.com/docs/git-rev-parse)
- `GIT_NO_LAZY_FETCH=1` 禁止按需从 promisor remote 获取缺失对象；`GIT_NO_REPLACE_OBJECTS=1` 禁止 replacement refs 改写所见对象。[git](https://git-scm.com/docs/git)

本地手册路径为 `/Library/Developer/CommandLineTools/usr/share/man/man1/git-{tag,for-each-ref,cat-file,rev-parse}.1` 和同目录 `git.1`。这些是本机安装版本的证据，不用于证明官方最新 Git 版本。

当前 [graph.rs](../../crates/git_service/src/graph.rs) 的 `GraphRef` 只有种类、名字和 HEAD 标记；[graph_view.rs](../../crates/app/src/workbench/graph_view.rs) 的标签菜单提供推送、删除、复制名称，没有正文详情入口。提交详情不能替代标签消息，同一提交也可以有多个不同正文的标签。

### 建议交互与命令边界

点击标签或菜单「查看标签详情」，按需打开只读详情：名称、annotated／lightweight、tag 对象 OID、目标 OID／类型、tagger／时间、可选中复制的完整消息。单段消息也要显示，不能只显示 `contents:body` 而遗漏 subject。轻量标签显示「轻量标签，没有独立说明」；签名存在仅表示「含签名，未验证」，正文展开不自动启动签名验证工具。

推荐使用现有 `git_service` 后台执行框架，新增受限查询；以下是**设计，不是已运行或已实现的命令流程**：

1. 从完整 `refs/tags/<name>` 获取并保存未剥离 OID。引用名来自 Git 返回的记录，仍做 refname 校验；如使用 `for-each-ref` 模式匹配，返回后必须精确比对完整名称。不要接受用户输入的任意 revision 表达式。
2. 后续只把校验后的完整十六进制 OID 作为 `Command::args` 参数，先 `cat-file -t <oid>`、`cat-file -s <oid>`；只有真实类型为 `tag` 且大小在预算内才读取 `cat-file tag <oid>`。**必须先检查类型**，不能利用 `cat-file` 的隐式解引用推断类型。保留原消息；若采用 `for-each-ref` 格式字段，则同时取 subject、body，并把签名展示与验证分开。
3. 命令使用独立参数数组，不拼 shell，不使用手册示例中的 `eval`。加 `--no-pager`，禁止 lazy fetch 与 replace refs；不请求 textconv、filters 或签名验证。当前 runner 已关闭 fsmonitor、终端提示并清理仓库路由环境，但保留其他用户环境；新增只读操作要显式设置上述对象读取边界，不能假定「只读」就绝不会触发网络。
4. **建议预算**：标签对象／显示消息上限 256 KiB，按需读取一项；超限明确说明，不静默截断成完整消息。沿用超时、取消、全局限流、进程组回收和 stderr 上限。先检查大小后仍需有界读取；批量读取时按字节长度解析，不能假设任意对象正文不含 NUL。
5. tag ref 在读取中可能移动。请求绑定仓库身份、ref、OID 和 generation；详情显示已固定的对象，返回前核对是否仍对应所选标签，变化则重新查询或报错。标签可能指向 tree、blob 或另一个 tag；只在目标可确认为 commit 时提供提交 Diff，不能硬当 commit。
6. 正文按不可信纯文本显示，不执行 HTML／脚本、不自动加载图片／远程链接；控制字符安全呈现，非 UTF-8 内容明确标注解码处理。提供可复制文本，但不把签名存在冒充信任结论。

实施时在临时仓库验证：轻量标签、多段中文／空消息、签名文本、超限内容、非 commit 目标、嵌套 tag、同一提交多个 tag、读取期间删除／移动引用、缺失对象与取消。涉及新环境变量和 Git 版本差异时验证实际支持行为；未在本次真实仓库创建或修改标签。

## 本次验证与未决项

- `git -C /Users/joys/work/code/ZJ status --short` 调研开始时输出为空；检查了 manifest、lockfile、vendor 补丁清单、Diff／tag 精确位置和本机 Git 手册。
- 官方在线版本与产品资料查询均返回域名安全校验错误。报告没有把错误解释为版本不存在，也没有把历史报告或未经读取的链接当作本次一手证据。
- 本调研未运行 Cargo 检查、工具链安装或依赖解析，避免触发下载并保持只写报告的范围。主任务协调者转述另一 worker 的**现有版本基线**：603 passed、0 failed、4 ignored；dist 49,428,272 字节；GUI 空闲 52.94 MB、3 窗口＋20 文档 251.99 MB。本调研未直接读取其命令日志和测量条件，因此仅作注明来源的协作结果，不据此声称新版兼容或资源提升。
- 新增报告经独立只读复核，结论为 0 findings；锁定版本、资源限额、Git 手册语义和标签当前缺口获复核支持。没有新版本兼容性、性能或内存实测结论。
- 未决项只有两个：恢复正常许可范围内的一手网络访问后，确认官方版本／MSRV／yanked；确认候选 Diff 的当前实现、维护状态与准确许可证。此前可以独立实施标签正文或讨论 Diff 交互，但不能宣称「已升级到最新」。
