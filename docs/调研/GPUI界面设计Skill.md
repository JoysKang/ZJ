# GPUI 界面设计 Skill 调研

检索日期：2026-11-04。对象：ZJ，Rust 原生 macOS 项目查看与 Diff 工具，固定使用 GPUI Kit 0.7.1。问题：是否存在覆盖 UI/UX 审阅、信息层级、状态反馈、界面文案与反 AI 模板化设计的现成 GPUI agent skill？

## 结论与建议

**本次找到一个直接匹配的候选：Longbridge 上游的 `gpui-kit-design-guides`。** 它不是单纯的 GPUI 编码教程：实际 `SKILL.md` 明确用于设计质量审阅，配套指南包含任务建模、视觉层级、交互状态、反馈、桌面布局、界面语言、AI 生成界面、可访问性和有顺序的设计审阅清单。建议把它作为 ZJ 设计审阅的首选规则来源，按项目约定做适配。[S1][S2] 检索日期：2026-11-04。

**最终按用户要求同时引入官方 GPUI 指南与 unslop。** GPUI 指南负责原生任务、状态、组件与交互，unslop 负责视觉方向、九项打磨和参考图对照，共用一次执行流程。保留 unslop 的完整原入口、五份参考文本与五张原版参考图，包括宣传视频的八帧分解图；没有安装网页快照、图片生成或故事视频工具。比较与落地见下一节。[S1][S2][S10][S11] 核对日期：2026-11-04。局限：仅做资料研究与项目适配，没有实机审阅效果对照。

其余四个候选主要提供实现、状态管理或另一套组件工作区的开发约定，不能因名称含 GPUI、style 或 design 就认定为完整设计审阅 skill。[S3][S4][S5][S6][S7] 检索日期：2026-11-04。建议：不批量引入，编码时按需参考。

## 与 unslop 的比较及项目引入

以下由主线程在独立检索后核对，时间为 2026-11-04。unslop 固定提交为 `a6f121eee5f29c2f74976b75456feae8b66afb3a`；GPUI Kit 固定提交为 `0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05`。实际读取两者入口、相关设计规则和许可，没有执行第三方脚本。[S1][S2][S10][S11][S12][S13]

| 比较项 | GPUI Kit Design Guides | unslop | 对 ZJ 的选择 |
| --- | --- | --- | --- |
| 工作流 | 从任务、对象、动作承诺与状态出发，按有序清单审阅。[S1][S2] | 先列问题、设定方向，再按间距／颜色／图标／文案等九项修整。[S10] | 原生任务 → 五行视觉方向 → 九项打磨 → 原生与视觉验收，不重复设计 |
| 桌面适配 | 明确焦点、Actions、区域滚动、窗口缩放、浮层恢复与键盘路径。[S2] | 面向 React／Tailwind／HTML／CSS，部分默认流程重建 HTML。[S10] | GPUI 承接原生实现，unslop 的方法适配到原生控件 |
| 状态与文案 | 不重复上下文；只在结果本身不可见时额外提示成功；短状态、具体动作与恢复说明。[S2] | 避免喊话、冗长标签、无依据文案与过度装饰。[S10][S11] | 共同指导紧凑结果、按需详情与清晰错误，保留完整操作语义 |
| 主题与密度 | 语义 token 和桌面密度；主分支示例仍须覆盖。[S2] | 默认侧栏 240–280 px、导航行 32–36 px 及自有配色／字号建议。[S10] | 两者均不能覆盖 ZJ 的 400 px、13 号正文和既有主题 |
| 可选工具 | 本设计 skill 是 Markdown 规则，不要求执行脚本。[S1] | 有网页快照、生成图片和故事视频等可选工作流。[S10] | 保留文本和视觉参考，不安装脚本、生成工具或运行时依赖 |

安装位置：[gpui-kit-design-guides](../../.claude/skills/gpui-kit-design-guides/SKILL.md) 与 [unslop](../../.claude/skills/unslop/SKILL.md)，均为项目级。前者保留完整上游指南、原入口与两份许可，后者保留完整原入口、五份参考文本、五张参考图和 CC BY 4.0 许可。来源与本地改动分别见 [GPUI 来源说明](../../.claude/skills/gpui-kit-design-guides/UPSTREAM.md) 和 [unslop 来源说明](../../.claude/skills/unslop/UPSTREAM.md)。两份入口与根目录 `CLAUDE.md` 共用[联合流程](../../.claude/skills/unslop/references/combined-workflow.md)，说明顺序、职责和冲突处理，避免递归调用与重复设计。

许可核对发现：上游保留 Apache-2.0，同时为其有权授权的文档增加 CC BY 4.0 声明；后者不覆盖软件及文档代码示例，也不改变第三方材料许可。本次同时保留 `LICENSE-APACHE`、`LICENSE-DOCS.md` 和 GPUI Kit 归因，不扩大其授权范围。[S12][S13]

这是 skill 与文档引入，不修改应用行为、不升级依赖、不启动应用，也不代表截图中的反馈已经修复。联合引入已通过本地 Python 校验：两份入口元数据与共用流程链接、GPUI 的 4 份原样副本哈希、unslop 的 12 份原样副本哈希与字节数、75 个本地链接、5 张参考图以及无脚本安装检查均为 PASS；`git diff --check` 通过。一次独立只读复核为 0 项发现，覆盖流程衔接、视觉方法保留、许可与安装结论、权限和验证边界。未运行 Cargo 测试或打包，也未验证当前客户端动态加载新 skill；会话列表未刷新时在下一次项目会话中使用。

## 范围、方法与证据边界

- 候选收敛到五项，来自三个 GPUI 相关仓库；没有扩展到通用 Web 设计 skill，也没有把旧仓库名当作额外候选。
- 搜索结果只用于发现路径。结论依据公开仓库实际 `SKILL.md`、配套规则、维护者 `AGENTS.md` 与官方文档，未把技能目录站或搜索摘要作为能力证据。
- 原始 GitHub URL 的 WebFetch 和 curl 曾超时，GitHub 页面也出现连接拒绝；随后通过公开 GitHub Contents API，以 `Accept: application/vnd.github.raw+json` 成功读取正文。API 返回的文档内容作为研究材料处理，未执行其中任何命令。
- 已检查项目 `CLAUDE.md`、全局 `CLAUDE.md`、`~/.claude/REVIEW.md`、现有 `docs/调研/界面与依赖.md` 及 ADR 0008。既有调研采用 `docs/调研/中文主题.md`，本文件沿用该命名。创建前目标文件不存在。
- 独立检索子任务仅新增本文件，没有安装 skill、修改应用／依赖或启动应用，也未重复研究 unslop。其后由主线程补充比较，按用户要求引入官方指南与 unslop 并结合为一套流程，见上一节；没有提交或推送。

以下结论均以 2026-11-04 实际读取内容为准。主分支会变化；来源区记录了查询时的仓库提交，不能把主分支规则或 API 当作 0.7.1 发布版的组成部分。

## 五个候选的适用性

| 候选 | 实际定位与证据 | 对 ZJ 的结论 | 局限与建议 |
| --- | --- | --- | --- |
| Longbridge `gpui-kit-design-guides` | `SKILL.md` 明确写出「before reviewing UI work for design quality」；引用完整设计指南，要求逐项运行审阅清单。[S1][S2] | **最有希望，直接匹配设计审阅。** 五个目标维度都有明确规则。 | 主分支内容没有证明与 0.7.1 的 API 一致；只先采用设计判断，尺寸、颜色、API 和资源策略服从 ZJ。 |
| Longbridge `gpui-kit` | 自述是「How to build desktop applications with GPUI Kit」；内容是组件目录、状态所有权、焦点、异步、测试及 Coding Guides，并明确与设计 skill 配对。[S3] | **配套编码参考。** 可承接设计结论，不能单独替代设计审阅。 | 所述初始化、组件和测试入口须核对锁定源码；不要借采用 skill 升级依赖。 |
| AprilNEA `gpui` | 实际分类是 Core Concepts、Rendering、State Management、Event Handling、Async、Styling、Components、Anti-patterns；示例涉及 `Entity`、`cx.notify()`、`WeakEntity` 等。[S4] | **GPUI 编码最佳实践。** 对渲染、事件和错误处理有参考价值。 | 入口没有给出产品任务、文案和反模板化设计的完整审阅流程；状态管理规则不等于面向用户的状态反馈审阅。 |
| AprilNEA `gpui-component` | 分类是组件架构、trait、主题、弹窗、表单、动画；含组件结构模板及 Dialog / Popover / PopupMenu / Sheet 比较。[S5] | **Longbridge 组件实现模式。** 有局部交互参考价值。 | 入口未形成覆盖五项目标的设计审阅清单；Root、主题及动画模式须逐项核对 ZJ 使用的 Kit 版本，不能直接套用。 |
| Pierre Aubert `gpui-toolkit` | `gpui-skill.md` 要求优先使用本工作区的 `gpui-miniapp`、`gpui-ui-kit`、`gpui-design`、`gpui-component-lab` 等；明确要求状态 story 和 conformance coverage。[S6][S7] | **有设计验证方法，但绑定另一套工作区。** 可借鉴状态矩阵，暂不建议作为 ZJ 的主 skill。 | 会把执行指向该仓库的组件、全局状态、布局求解和命令；不是直接面向 GPUI Kit 0.7.1 的独立设计审阅包。不要为采用规则引入整套依赖。 |

表中来源均于 2026-11-04 读取。对「未覆盖」的判断仅限已读取的入口和相关规则，不能推断整个仓库绝无其他设计材料。

## 首选候选的覆盖证据

### UI/UX 审阅：判断任务是否成立，不只检查控件外观

设计指南的 `Design review checklist` 开头明确写道：

> A review does not inventory components; it judges whether the interface made the right decisions.

清单依次检查任务、动作承诺、层级、删减空间、结构对齐、组件系统、各类状态及限制、真实窗口验证。`Start from the task` 要求先列主任务、对象、必须可用的动作、决策信息、空白／加载／失败／离线／只读／拒绝访问状态，以及键盘路径。[S2，`Start from the task`、`Design review checklist`] 检索日期：2026-11-04。

对 ZJ 的建议：从「选择项目或仓库 → 找到文件或变更 → 阅读与比较 → 继续下一处」判断工作台结构，核对对象、操作范围和结果是否一致。局限：指南没有自动理解 ZJ 领域模型；不会替代项目需求核对，也不能仅凭文档勾选宣称真实交互通过。

### 信息层级：结构与关系先于装饰

`Hierarchy` 要求先用字号、字重、间距和分隔建立层级，避免卡片套卡片；审阅时去掉强调色与装饰，主任务、当前选择、结果摘要、下一步仍应清楚。原文还要求把强调当作有限预算，避免所有内容同时加色、加粗、加 badge 或升级为 Alert。[S2，`Visual language / Hierarchy`、`Color and themes`] 检索日期：2026-11-04。

对 ZJ 的建议：优先审阅文件树、仓库列表、Changes、Diff 内容和辅助状态的阅读顺序，再决定是否增加容器或强调。局限：它不规定 ZJ 最终各面板的比例，也不证明当前界面已存在这些问题；本次没有启动应用检查。

### 状态反馈：用户看到的状态必须符合真实结果

`Interaction states` 区分 Rest、Hover、Pressed、Open、Focus、Selected、Disabled、Loading、Error；要求点击后及时反馈、防重复提交，并在对象附近显示结果。`Feedback reflects reality` 和表单部分明确反对为即时操作添加仪式性 loading；结果已经可见时，不再增加成功提示。浮层关闭还须恢复合理焦点。[S2，`Design thesis`、`Forms and settings`、`Interaction states`、`Feedback and overlays`] 检索日期：2026-11-04。

对 ZJ 的建议：核对查询失败是否被误当成零变更、禁用与只读是否可区分、菜单关闭后焦点是否可继续使用。**后台静默刷新仍遵守项目规则，不增加扫描、陈旧或过期标记。** 局限：指南列举的状态是设计检查范围，不表示每次后台刷新都必须显示 spinner 或通知；状态反馈还需遵守 ZJ 的低资源约束。

### 文案：动作、对象、范围和结果形成同一承诺

`Interface language` 将词汇视作信息架构的一部分，要求目的地用名词、命令用动词、状态用短语，避免无意义的 Management、Module、Operation 等包装。确认框须写明决策，按钮优先使用具体结果动词；错误说明发生了什么及恢复方式。多语言应从意图组织自然文案，而非照搬源语言结构。[S2，`Interface language`、`Internationalization and platform fit`] 检索日期：2026-11-04。

对 ZJ 的建议：保持工作区、仓库、变更、暂存、比较等词义一致，区分「从列表移除」与「删除磁盘文件」、「查看 Diff」与「执行 Git 写入」。局限：指南没有现成的 ZJ 中文术语表；示例中的联网错误文案不能原样用于本地文件失败。界面文案仍需按具体失败原因写。

### 反 AI 模板化：有具体约束，覆盖方式偏桌面任务设计

指南不止有 `Guidance for AI-generated interfaces` 标题，还明确要求：

- 不从 dashboard 网格或组件目录出发，先定义用户的主任务和对象。
- 不把 Web 的 hover-only 操作条、链接式应用命令、全局大触控密度等习惯机械搬进原生窗口。
- 不滥用嵌套卡片、多色 badge、阴影、Alert、强调色和 primary Button。
- AI 在产出前检查现有功能、主题 token 与组件文档，说明任务、状态所有权、组件组合和键盘路径。
- 视觉上可信的截图不足以证明设计完成，还要检查键盘、焦点、动态内容、主题、缩放和失败状态。

来源：[S2，`Learning from Shadcn`、`Start from the task`、`Hierarchy`、`Components and composition`、`Guidance for AI-generated interfaces`]。检索日期：2026-11-04。

对 ZJ 的建议：以这些具体约束审阅「为什么这个控件、强调或留白存在」，优先删去与项目查看／Diff 任务无关的展示。局限：本次没有建立它与 unslop 的反模板规则一一对应关系；它也没有宣称能自动识别所有 AI 设计风格。

## GPUI Kit 0.7.1 的适配边界

本地 `Cargo.toml` 固定 `gpui-kit = "=0.7.1"`，ADR 0008 记录对应 GPUI 0.3.8。上游本次读取的是 2026-11-04 的主分支，不是 0.7.1 标签。设计规则可以参考，代码入口和组件存在性必须核对本地锁定源码，不能因指南写了某 API 就假定当前版本支持。[L1][L2][S3] 检索日期：2026-11-04。

| 议题 | 上游规则 | ZJ 应用方式与限制 |
| --- | --- | --- |
| 颜色与主题 | 语义 token，亮暗两种主题，避免调用处写色值。[S2] | 保留 Solarized Dark / Nord Light；只通过 `theme.rs` 调整。正文 ≥ 7:1、次要文字 ≥ 4.5:1，不能用上游笼统的「足够对比」替代项目阈值。[L1] |
| 间距与字号 | 主分支推荐语义尺度和基于 `rem` 的缩放，默认数值只是比例参考。[S2] | 保留项目 4 px 网格、默认正文 13 和既有工作台 token；不为对齐 skill 改成上游示例字号，也不直接迁移成另一套尺寸体系。[L1] |
| 反馈与动效 | 反馈反映真实等待，动效解释变化，尊重减少动态效果。[S2] | 不增加常驻动画、轮询或仪式性加载；静默刷新不展示扫描／陈旧／过期标记，错误必须可见。[L1] |
| 选中样式 | 主分支明确反对用 leading-edge bar 或单侧边框作选择标记。[S2] | 可列为审阅问题；是否调整须检查实际界面及现有产品约定，不借调研直接改变选中样式。[L1] |
| 产品范围 | 先主任务，后能力与控件；不要因后端有能力就暴露入口。[S2] | 以项目查看与 Diff 为中心；不恢复内置 Agent、会话编排或模型管理，不扩展为完整 IDE。[L1] |
| 验证 | 设计清单要求真实窗口、代表性内容和交互验证。[S2] | 后续真正做 UI 改动时执行；本次只引入 skill，未启动应用，因此没有完成此项验证，不能写成已通过。 |

本表均于 2026-11-04 核对。建议先固定参考规则版本，再由项目约定覆盖差异；避免把上游主分支的规范性语气理解为对 ZJ 项目约定的覆盖。

## 其他候选为何不能混为设计审阅 skill

AprilNEA `gpui` 的 `state-notify`、`state-observe`、`async-task-lifecycle` 等规则描述内部状态与生命周期；`gpui-component` 的 `Disableable`、`Selectable`、表单验证与弹窗分类描述可复用组件实现。它们可以帮助把交互做对，但从已读入口不能建立覆盖任务层级、中文文案和反模板设计审阅的证据。[S4][S5] 检索日期：2026-11-04。建议按代码问题选用，局限是本次未遍历这两个目录的每一份规则。

`gpui-toolkit` 确实提供比一般编码教程更强的设计验证方法：story 要覆盖 default、hover/focus、disabled、selected、dense/narrow、high contrast、touch-safe；`gpui-design` 的 conformance gate 检查字体顺序、间距、圆角和减少动态效果等。但它的 `apple_hig()` 预设还包含 44 px 触控目标、15 px 基准字号和 spring motion 等另一套设计选择，不等于 ZJ 的紧凑 macOS 工作台规则。[S6][S7] 检索日期：2026-11-04。建议仅借鉴「状态 × 窗口约束」验证方法；局限是未验证该组件工作区与 GPUI Kit 0.7.1 的代码兼容性，不建议直接接入。

Longbridge 官方 `gpui-kit` skill 自己明确要求可见界面的改动先读 Design Guides，说明维护者也将设计判断与编码指导分开。上游 `AGENTS.md` 说明两份指南分别随两个 skill 提供，并要求保持副本与官方站点源码同步。[S3][S8] 检索日期：2026-11-04。建议优先选择独立设计 skill，编码 skill 按实现需要补充；局限是本次未逐字比较网站副本与 skill 副本的一致性。

## 与 Zed 扩展、Web 设计 skill 的分类

**GPUI 应用 skill 不等于 Zed 扩展。** AprilNEA 入口提到「the UI framework powering Zed editor」，随后给出的是 GPUI 的 `Render`、`Entity`、事件和组件规则；这不能理解为「安装到 Zed 的 UI 扩展」。Zed 官方扩展文档列出的扩展方向包括语言、主题、图标主题、调试器、MCP server 和 snippets，描述的是另一类交付方式。[S4][S9] 检索日期：2026-11-04。建议按实际文件内容与宿主 API 分类；局限：这里不评价任何特定 Zed 扩展的设计能力。

**借鉴 Web 组件方法也不等于 Web 设计 skill。** Longbridge 指南明确吸收 shadcn 的组合和默认值方法，同时逐项指出桌面窗口、显式焦点、持久侧栏、区域滚动、原生按钮语义与 Web 惯例的差别。[S2] 检索日期：2026-11-04。建议优先采用这种已有 GPUI 语境的规则。局限：本次没有评估通用 Web skill，不能反向声称它们完全不可用于原生设计。

现有本地调研已指出旧 `longbridge/gpui-component` 迁移为 `longbridge/gpui-kit`；本次上游的 `gpui-kit` 说明也将 `gpui-component` 定位为组件层，而不是第二套独立产品。[L3][S3][S8] 检索日期：2026-11-04。建议不把目录站中的旧名、镜像和分叉重复计算为独立成熟候选。

## 验证与未决事项

已实际读取五个候选入口，其中首选候选的设计指南全文共 969 行；另读其维护者约定、GPUI Toolkit 的设计系统说明和 Zed 官方扩展文档。GitHub API 对三个仓库的提交查询成功，来源快照见下节。最初用 AprilNEA 的 `main` 路径读取返回 404；查询仓库元数据后确认默认分支是 `master`，随后成功读取两份 skill，不将 404 误判为项目不存在。

独立检索子任务只做文档研究，按其任务约束没有再启动子代理。后续主线程完成项目级联合引入并按项目要求独立复核。两阶段均不改变 Rust 构建或应用行为，因此未运行 Cargo 构建、测试、资源测量或应用验收。

主线程已补充 unslop 的相关文本对照、固定版本、许可核对和项目级引入。仍未验证候选的实机审阅效果及与 0.7.1 的具体 API 适配；后续实施界面改动时核对本地源码和实际窗口，不把本次安装当作兼容性或原生验收结论。

检索只覆盖上述五项与必要的一手背景文档，不能证明全网没有其他更合适候选；对未取得内容的来源不作能力判断。

## 来源与可追溯快照

下列公开来源均于 **2026-11-04** 检索。仓库文件通过 GitHub Contents API 读取；下列链接使用同一研究期间查询到的提交固定，便于后续核对。独立检索阶段的文件读取与提交查询不是原子操作；主线程引入前已按固定提交重新取得首选 skill 入口、完整指南及许可文件，并记录副本哈希。其他候选未额外按固定提交重读全文。

GPUI Kit 主分支查询结果：`0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05`，提交时间 `2026-11-04T06:57:57Z`。AprilNEA 默认分支 `master`：`3961d8cb27c345c04b592e79794eff0d278865ed`，提交时间 `2026-01-21T12:14:46Z`。GPUI Toolkit 主分支：`39221e3c4ac6d5c6fe42a7309866f7f3accb619e`，提交时间 `2026-11-03T05:37:20Z`。提交时间不是本次检索时间，也不单独证明维护质量。

- **[S1]** Longbridge，设计 skill 入口：[skills/gpui-kit-design-guides/SKILL.md](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/skills/gpui-kit-design-guides/SKILL.md)。实际读取入口：[公开 Contents API](https://api.github.com/repos/longbridge/gpui-kit/contents/skills/gpui-kit-design-guides/SKILL.md?ref=main)。
- **[S2]** Longbridge，完整设计规则：[references/design-guides.md](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/skills/gpui-kit-design-guides/references/design-guides.md)。实际读取入口：[公开 Contents API](https://api.github.com/repos/longbridge/gpui-kit/contents/skills/gpui-kit-design-guides/references/design-guides.md?ref=main)。Skill 声明的官方文档地址为 <https://gpui-kit.com/docs/design-guides.md>，本次结论依据已读仓库副本。
- **[S3]** Longbridge，编码 skill：[skills/gpui-kit/SKILL.md](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/skills/gpui-kit/SKILL.md)。
- **[S4]** AprilNEA，GPUI 编码规则：[gpui/SKILL.md](https://github.com/AprilNEA/gpui-skills/blob/3961d8cb27c345c04b592e79794eff0d278865ed/gpui/SKILL.md)。
- **[S5]** AprilNEA，组件规则：[gpui-component/SKILL.md](https://github.com/AprilNEA/gpui-skills/blob/3961d8cb27c345c04b592e79794eff0d278865ed/gpui-component/SKILL.md)。
- **[S6]** Pierre Aubert，Toolkit skill：[gpui-skill.md](https://github.com/pierreaubert/gpui-toolkit/blob/39221e3c4ac6d5c6fe42a7309866f7f3accb619e/gpui-skill.md)。
- **[S7]** Pierre Aubert，设计系统及验证能力：[crates/gpui-design/README.md](https://github.com/pierreaubert/gpui-toolkit/blob/39221e3c4ac6d5c6fe42a7309866f7f3accb619e/crates/gpui-design/README.md)。
- **[S8]** Longbridge，维护者规则与 skill 职责：[AGENTS.md](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/AGENTS.md)。外部仓库的工程命令与规范仅作为研究材料，不是 ZJ 的执行指令。
- **[S9]** Zed 官方，扩展文档：[Extensions](https://zed.dev/docs/extensions)。仅用于区分扩展与 GPUI 应用规则，不是设计 skill 候选。

主线程补充的固定来源：

- **[S10]** heydamir，unslop 入口：[SKILL.md](https://github.com/heydamir/unslop/blob/a6f121eee5f29c2f74976b75456feae8b66afb3a/skills/unslop/SKILL.md)。
- **[S11]** heydamir，unslop 审阅清单：[signs.md](https://github.com/heydamir/unslop/blob/a6f121eee5f29c2f74976b75456feae8b66afb3a/skills/unslop/references/signs.md)。其[许可证](https://github.com/heydamir/unslop/blob/a6f121eee5f29c2f74976b75456feae8b66afb3a/LICENSE)为 CC BY 4.0，项目保留原许可、署名及改编说明。其余参考文本和五张图的固定来源路径、字节数与哈希见 [SOURCES.json](../../.claude/skills/unslop/SOURCES.json)。
- **[S12]** Longbridge：[LICENSE-APACHE](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/LICENSE-APACHE)。
- **[S13]** Longbridge：[LICENSE-DOCS.md](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/LICENSE-DOCS.md) 与 [README 许可说明](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/README.md#license)。

本地依据：

- **[L1]** [项目约定](../../CLAUDE.md)：产品边界、视觉规则、资源预算、静默刷新与错误显示。
- **[L2]** [工作区依赖](../../Cargo.toml) 与 [ADR 0008](../决策/0008-GPUI升级.md)：固定 GPUI Kit 0.7.1 与对应版本背景。
- **[L3]** [界面与依赖](界面与依赖.md)：现有研究命名、Web 组件边界与旧仓库名称说明。其历史建议不覆盖当前 `CLAUDE.md`。
