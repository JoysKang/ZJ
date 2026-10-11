# Beautiful UI 对 ZJ 的适配调研

检查日期：2026-10-11。问题：Beautiful UI 是否可以直接用于 ZJ，如何帮助整个编辑器统一视觉与交互，同时保留轻量目标。

源码检查快照：`slev12397/beautiful-ui`，`44a274e598395ab61e7c96c26fda2758780253b7`。官网可能独立更新；下列源码链接固定到该提交，官网组件与安装说明按检查日读取。

## 结论

**适合借鉴设计并在现有 GPUI 组件上实现，不能作为 Rust/GPUI 组件直接引入。** Beautiful UI 有真实的 React 组件源码和 shadcn 注册表，不只是图片展示。Web 项目可以按官方方式拉取或复制组件；ZJ 使用 Rust 原生 GPUI Kit 0.7.1，没有承载这些 React/DOM/CSS 组件的现有界面运行时。[官网][site]、[README][readme]、[ZJ Cargo.toml](../../Cargo.toml)

建议先统一整个编辑器的基础视觉规则，再选择性借鉴 Prompt Bar、Sidebar Nav、Search、Thinking、Tool Chips、Task Rows 和 Approval Card。现有 GPUI Kit 已有按钮、输入、菜单、弹层、对话框、通知、列表和标签等组件，无需为外观增加 WebView 或替换界面框架。[GPUI Kit][kit]、[主题实现](../../crates/app/src/theme.rs)、[Agent 输入区](../../crates/app/src/workbench/agent_panel/composer.rs)

本轮只调研并保存报告，没有修改应用源码、安装依赖、启动本地演示或改变运行中的 ZJ。

## 已确认的事实

### 项目形态与技术栈

官网列出 21 个 AI 界面组件，并提供 Ice Cream Harness 演示。官方 README 的技术栈为 Next.js App Router、React、TypeScript、Tailwind CSS v4。组件主要位于 `components/primitives/`；部分组件另依赖内部基础组件和 npm 包，不能将「一个组件文件」理解为完全独立。[官网][site]、[README][readme]、[package.json][package]

官方提供 `npx shadcn add https://www.beautifului.dev/r/approval-card.json` 形式的安装入口，也支持手动复制源码。注册表声明 `registryDependencies`、`dependencies` 和 `files`；CLI 可以按需拉取基础样式及内部依赖。手动复制时需同时处理共享 CSS 基础层；仅复制颜色变量不足以生成 Tailwind utility 类。[README][readme]、[注册表索引][registry]、[Prompt Bar 注册条目][prompt-registry]

Harness 使用脚本场景与模拟数据。接入真实 Agent 仍需将流式输出、工具事件、审批和会话状态映射到应用的后端；它不提供替代 ZJ 当前 Agent 协议层的能力。[README][readme]、[Harness 源码][harness-source]

### 许可证与图标

项目采用 MIT 许可证，版权人为 Shane Levine（2026）。复制或分发源码及其实质部分时，需保留版权声明和许可声明。[LICENSE][license]

README 明确说明 SidebarNav 使用付费的 `@central-icons-react` 图标库，安装需要授权密钥或替换相应图标。项目的 MIT 许可证不能代替该图标库的商业授权。ZJ 已使用 Lucide，可以用现有图标表达相同操作，本轮无需引入此图标依赖。[README][readme]、[package.json][package]、[ZJ 视觉约定](../../CLAUDE.md)

### 值得借鉴的视觉规则

Beautiful UI 的全局样式使用低饱和冷色底色、分层表面、主次文字、细边框、单一强调色与语义颜色，并为明暗两种主题定义对应 token。圆角按 chip、control、card、window 区分，参考值为 6、8、10、14 px；阴影也按按钮、卡片、浮层分级。界面字体采用 Inter，等宽字体采用 JetBrains Mono。[README][readme]、[globals.css][css]、[layout.tsx][layout]

这些是设计参考，不是 ZJ 应逐个照抄的数值。ZJ 当前有 4 px 间距网格、正文与次要文字对比度要求，以及 Solarized Dark／Nord Light 配色。若采用更中性的底色，应先做明暗主题对照；应保留高密度文件列表与代码阅读所需的可读性。[主题实现](../../crates/app/src/theme.rs)、[ZJ 视觉约定](../../CLAUDE.md)

### 动效与资源边界

源码存在 CSS 循环动画、打字光标闪烁和状态脉冲；Prompt Bar 依赖 `glimm`，切换特定演示模型会触发一次彩虹 Shader 扫光，并有减少动态效果的处理。不能将这些效果全部视为常驻动画，也没有证据据此量化网页的内存或 CPU。[globals.css][css]、[PromptBar.tsx][prompt-source]、[package.json][package]

ZJ 的本地测量已表明，持续整窗重画会阻止渲染器归还备用缓冲并停止 display link。设计迁移应优先使用静态层次、边框、排版和交互状态；必要动效应有结束时间，仅在可见且正在交互时发生。流光、视频加载器、循环闪烁与多层动态阴影不属于本次推荐范围。[资源预算与视觉约定](../../CLAUDE.md)、[ADR 0005](../adr/0005-gpu-memory.md)

## 三种接入方式

| 方式 | 能否直接使用 Beautiful UI 源码 | 对 ZJ 的成本与判断 |
| --- | --- | --- |
| 在现有 GPUI 上实现视觉和交互 | 不能直接编译 TSX；设计 token、状态组织和布局可以参考 | 推荐。基础交互继续由现有原生组件处理，新增依赖不是必要条件 |
| 在应用中嵌入 WebView | 可以承载 Web 组件，但仍需 Web 构建与状态桥接 | 技术上可行，增加一套渲染与焦点、输入法、快捷键、滚动的整合工作；资源增量需实测，当前没有采用理由 |
| 将应用界面整体改为 Web 技术 | Web 组件适用 | 改造范围远大于统一 UI，不建议作为本次任务的路线 |

后两项为基于技术栈差异的工程判断，本轮没有实现或测量 WebView，不能给出确定的内存增加值。

GPUI Kit 官方明确区分带统一样式的 `gpui-component` 和可自定义外观的 `gpui-base`。ZJ 可以优先调整组件主题和外部布局，仅在确有必要时使用 Base 的基础交互实现自有样式，无需重写按钮焦点、弹层定位、文本选择等行为。[GPUI Kit][kit]、[Kit 在 ZJ 的使用](../../crates/app/src/workbench/agent_panel.rs)

## 对整个编辑器的映射

| 参考 | ZJ 对应区域 | 推荐借鉴的部分 | 适配边界 |
| --- | --- | --- | --- |
| 全局 token 与 Sidebar Nav | 标题栏、活动栏、文件树、源码管理器 | 面板层次、分区标题、主次文字、选中与悬停状态、工具按钮对齐 | 保留紧凑树结构和多仓库列表；不照搬聊天导航的数据模型 |
| Search、菜单与弹层 | 命令中心、快速打开、全文搜索、模型菜单、额度卡片 | 搜索框与结果层次、快捷键提示、统一边框与阴影、锚点和屏幕边界规则 | 用现有 GPUI 菜单与弹层，保留键盘操作和搜索功能 |
| 统一 control 与 card 圆角 | 文件标签、按钮、输入框、设置、Git 对话框 | 少量明确的尺寸与圆角等级、一致的 hover／active／focus／disabled 状态 | 列表密度和表单密度可以不同；无需所有控件等高 |
| Prompt Bar | Agent 输入区 | 附件、输入正文、模型选择和发送按钮的层次；菜单贴近触发控件 | 中文输入法、图片引用、运行中追加指令沿用现有行为；不引入演示模型、语音或 Shader 功能 |
| Thinking、Tool Chips、Task Rows | Agent 执行过程、工具输出、修改文件列表 | 清楚的摘要、状态与详情入口，减少视觉噪声 | 保留运行过程默认展开、完成后折叠；修改文件默认折叠；保持虚拟化和用户滚动位置 |
| Approval Card | Agent 审批与提问、Git 操作结果 | 清晰的动作对象、主次按钮、错误与等待状态 | 官网页面示例包含问答选择，不等于命令审批协议；实际命令、工作目录和审批选项必须继续显示 |
| Code Block | Agent 代码回复、代码块复制 | 标题、复制动作、边框与代码背景 | 继续使用现有高亮和可选中文本；网站 Diff Table 是表格数据编辑示例，不能代替源码 Diff 和逐处审阅 |

映射依据：[组件目录][site]、[ZJ 主题](../../crates/app/src/theme.rs)、[标题栏](../../crates/app/src/workbench/chrome.rs)、[文件树](../../crates/app/src/workbench/sidebar.rs)、[标签栏](../../crates/app/src/workbench/editor_area.rs)、[源码管理器](../../crates/app/src/workbench/scm.rs)、[Agent 面板](../../crates/app/src/workbench/agent_panel.rs)。表格中的推荐属于设计判断，不是对现有各区域缺陷的审计结论。

## 推荐的后续顺序

1. **先确定全局样式。** 对照现有 Nord／Solarized 与中性冷色方向，统一背景、面板、编辑区、输入和浮层的层次，以及文字、边框、圆角、强调色规则；调整应进入现有 `theme.rs`。
2. **再统一公共控件。** 按钮、输入框、菜单、提示、对话框和通知共用状态规则，覆盖主操作、次操作、危险操作、悬停、按下、键盘焦点、禁用、错误与空状态。少量复用即可，不需要先建设新的通用组件框架。
3. **覆盖整个工作台，再完善 Agent 细节。** 先看标题栏／标签栏／左右侧栏／搜索／Git 表单是否一致，再借鉴 Agent 组件的信息层次。仅换 Agent 卡片无法完成整个编辑器的统一。
4. **用真实密度与状态验收。** 明暗主题、宽窄窗口、长路径、多标签、多文件变更、中文输入、流式输出、键盘操作和滚动位置都应覆盖。若后续增加动画或新依赖，再按项目约定测二进制、空闲 CPU 与 physical footprint。

## 验证与限制

已亲自复核官方 README、LICENSE、package.json、全局 CSS、Prompt Bar 源码和注册条目，并核对 ZJ 的 GPUI 版本、主题 token 与基础控件使用。调研由只读后台研究子代理协助，最终取舍与本地适配由主会话确认。

本轮未安装或执行 shadcn、未检查全部第三方依赖的独立许可证、未运行 Harness、未测 Beautiful UI 或 WebView 的内存／CPU。浏览器预览未建立成功，因此关于视觉层次与动效的描述来自官方样式和说明，不声称已完成动态效果或实机截图验收。未为调研重建本地检索索引，也未提交或推送代码。

[site]: https://www.beautifului.dev/
[readme]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/README.md
[license]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/LICENSE
[package]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/package.json
[css]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/app/globals.css
[layout]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/app/layout.tsx
[registry]: https://www.beautifului.dev/r/registry.json
[prompt-registry]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/public/r/prompt-bar.json
[prompt-source]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/components/primitives/PromptBar.tsx
[harness-source]: https://github.com/slev12397/beautiful-ui/blob/44a274e598395ab61e7c96c26fda2758780253b7/components/site/IceCreamHarness.tsx
[kit]: https://gpui-kit.com/
