# Web 组件与 GPUI 原生方案调研

检查日期：2026-10-11。问题：beUI、Rare UI、Transitions 和 shadcn/ui 是否能直接用于 ZJ，是否有美观、实用的 GPUI 原生替代方案。

## 结论

**四个项目均未查到官方 GPUI 实现，其现有 Web 组件不能直接作为 ZJ 的原生组件使用。最适合 ZJ 的原生方案是已经接入的 GPUI Kit 0.7.1／`gpui-component`。** 后者明确借鉴 shadcn/ui，并提供统一样式的桌面组件。[GPUI Kit][kit-readme]、[组件目录][kit-catalog]、[本地依赖](../../Cargo.toml)

这是对官方项目、文档和源码的核查结论，不代表已经穷尽所有非官方移植。推荐沿用现有原生组件，统一整个编辑器的主题、布局和控件状态，再选择性借鉴 Web 项目的设计。

## 四个项目的适配情况

| 项目 | 官方实现与定位 | 能否直接用于 GPUI | 许可要点 |
| --- | --- | --- | --- |
| [beUI][beui-site] | React、TypeScript、Tailwind CSS、Motion；支持 shadcn 注册表和复制源码 | 不能；可以参考外观与交互后用 Rust 实现 | MIT，复制／分发源码时保留许可声明 |
| [Rare UI][rare-site] | React 动效组件，使用 Next.js、Tailwind、TypeScript、Motion 和 shadcn 注册表 | 不能；移植代码还需考虑其分发限制 | MIT 加 Commons Clause 与署名要求，不能当作普通 MIT |
| [Transitions][transitions-site] | CSS／React 过渡动效片段与相关工具 | 不能；可参考动效参数和状态切换，不是完整的桌面控件库 | 动效／skills 为自定义许可；工具目录为 MIT |
| [shadcn/ui][shadcn-site] | Web 组件源码与注册表；官方安装依赖 Tailwind 和 React 生态 | 不能；GPUI Kit 有类似设计理念的原生实现 | MIT，复制／分发源码时保留许可声明 |

依据：[beUI README][beui-readme]、[beUI LICENSE][beui-license]、[Rare UI 仓库][rare-repo]、[Rare UI LICENSE][rare-license]、[Transitions 仓库][transitions-repo]、[Transitions LICENSE][transitions-license]、[shadcn 手动安装][shadcn-install]、[shadcn LICENSE][shadcn-license]。官网和 main 分支资料按检查日读取，后续可能更新。

Rare UI 的许可允许软件作为应用、网站或产品的一部分使用、修改和分发，但要求提供用户可找到的 Rare UI 署名链接，并保留版权和许可。许可同时禁止销售、再许可或再分发组件本身，包括单独、打包或移植后的组件。不能因此认定所有应用集成都被禁止，也不能把它当作可以自由发布移植组件库的普通 MIT 项目。[许可原文][rare-license]

Transitions 将 `transitions/`、`skills/` 和 CSS／React／TypeScript 动效片段放在自定义许可下，允许在个人及商业产品中使用、修改并交付，但禁止将集合或其实质部分重新发布为竞争性动效库。`agent/`、`cli/`、`cli-legacy/`、`refine/`、`build/`、`scripts/` 才适用 MIT。付费动效和模板另有条款；网站设计、品牌和标志不在上述授权范围内。[许可原文][transitions-license]

## 可用的 GPUI 原生方案

### 首选：现有 GPUI Kit／gpui-component

这是一套原生 Rust 组件系统，官方将 `gpui-component` 对应到 shadcn/ui 的样式组件层，将 `gpui-base` 对应到 Base UI 的基础交互层。它借鉴 shadcn/ui 和 macOS／Windows 原生控件；不是 shadcn Web 源码的直接编译版本。[项目说明][kit-readme]

ZJ 已固定 `gpui-kit = "=0.7.1"`，锁定的 `gpui-component`、`gpui-base` 同为 0.7.1，底层为 `gpui-pre` 0.3.8。现有版本已包含以下模块，无需为了这些组件另接入 Web 框架：[Cargo.toml](../../Cargo.toml)、[Cargo.lock](../../Cargo.lock)、[升级记录](../adr/0008-gpui-kit-071.md)、[0.7.1 组件源码][kit-component]

- 通用控件：按钮、输入、选择器、菜单、Popover、Dialog、Tooltip、通知和命令面板。
- 编辑器布局：Sidebar、Toolbar、Tab、Tree、可调整面板与虚拟列表。
- Agent 界面：Attachment、Bubble、Message、MessageScroller、Questionnaire 和 Collapsible。

可先从[官方组件目录][kit-catalog]查看整体设计。官网和主分支可能比已安装版本更新；具体可用 API 应以 ZJ 锁定版本为准。这里列出的模块已核对本地 0.7.1 源码，对应发布源码提交为 `87d10ae5e1299d1be18670a37570c5402f952e34`。

ZJ 的 `theme.rs` 会覆盖 Kit 的主题颜色和语法样式。因此，使用这套库并不自动获得官网展示效果：面板层次、文字、间距、控件尺寸和状态仍需在 ZJ 中统一。这个方向通常比换另一套组件库更直接。[主题实现](../../crates/app/src/theme.rs)

### 更自由的外观：gpui-base

优先调整现有 Component 主题和布局；只有现成组件难以表达特定设计时，再使用 Base 的基础交互能力实现外观。Base 是同一 Kit 的底层，并非另一套独立 UI 框架。采用前需检查具体基础控件接口及 ZJ 的本地补丁，避免重写已有焦点、输入和滚动行为。[Kit 分层][kit-readme]、[本地补丁](../../vendor/README.md)

本次仅确认组件存在，没有提出替换 Agent 滚动实现；后续样式调整仍应保留已有滚动、图片引用、运行中追加输入和审批行为。

### 编辑器设计参考：Zed UI

Zed 的界面本身基于 GPUI，适合作为紧凑编辑器的布局、图标、排版和状态设计参考。但其 `crates/ui` 依赖 Zed 工作区中的 `theme`、`menu`、`icons` 等模块，以及工作区 GPUI，不能作为 ZJ 的独立即插即用组件包。[UI manifest][zed-ui]

该 UI crate 标注 `GPL-3.0-or-later`；GPUI 运行时的 Apache-2.0 许可不能代替 Zed UI 代码的许可。建议参考设计，不直接整包搬入。[UI manifest][zed-ui]

原 Longbridge `gpui-component` 项目已迁移到 `longbridge/gpui-kit`，不应把两个名称当成两套独立候选库。[迁移入口][old-component]

## 对 ZJ 的建议

1. 以现有 GPUI Kit 为基础，先统一全局主题：背景、侧栏、编辑区、输入和浮层的层次，以及文字、边框、圆角和强调色。
2. 统一按钮、菜单、下拉、输入和对话框的尺寸与状态，再覆盖标题栏、标签栏、文件树、搜索和源码管理器。下拉内容、宽度和触发控件的关系也属于这一步。
3. 用 Beautiful UI／beUI 借鉴少量 Agent 信息布局，用 shadcn 和 Zed 参考整体克制、紧凑的编辑器风格。需要动效时只添加短暂、有限的反馈。

这些是基于兼容性和 ZJ 轻量目标的工程判断，不是实机视觉或资源测试结果。沿用已安装 Kit 不需要为此新增一套 Web 运行时，但新增组件或动效的内存和 CPU 影响仍需按实际改动测量。

## 验证与限制

主会话复核了四个项目的官方资料和许可、Zed UI manifest、本地 Cargo 版本、0.7.1 组件模块和主题覆盖逻辑；独立资料检索由只读研究子代理协助。本轮仅新增此报告，未修改应用源码、安装依赖、构建或打包，也未改变运行中的 ZJ。

没有运行四个 Web 项目的演示，没有测量内存／CPU，没有验证所有组件在 ZJ 中的运行效果，也没有穷尽非官方 GPUI 移植。组件可用性、视觉质量和行为兼容性应在后续具体实现时验收。

[beui-site]: https://beui.dev/
[beui-readme]: https://github.com/starc007/ui-components/blob/main/README.md
[beui-license]: https://github.com/starc007/ui-components/blob/main/LICENSE
[rare-site]: https://rareui.com/
[rare-repo]: https://github.com/swamimalode07/rare-ui
[rare-license]: https://github.com/swamimalode07/rare-ui/blob/main/LICENSE
[transitions-site]: https://transitions.dev/detail.html?doc=introduction
[transitions-repo]: https://github.com/Jakubantalik/transitions.dev
[transitions-license]: https://github.com/Jakubantalik/transitions.dev/blob/main/LICENSE
[shadcn-site]: https://ui.shadcn.com/
[shadcn-install]: https://ui.shadcn.com/docs/installation/manual
[shadcn-license]: https://github.com/shadcn-ui/ui/blob/main/LICENSE.md
[kit-readme]: https://github.com/longbridge/gpui-kit/blob/87d10ae5e1299d1be18670a37570c5402f952e34/README.md
[kit-catalog]: https://gpui-kit.com/component/
[kit-component]: https://github.com/longbridge/gpui-kit/blob/87d10ae5e1299d1be18670a37570c5402f952e34/crates/component/src/lib.rs
[zed-ui]: https://github.com/zed-industries/zed/blob/main/crates/ui/Cargo.toml
[old-component]: https://github.com/longbridge/gpui-component
