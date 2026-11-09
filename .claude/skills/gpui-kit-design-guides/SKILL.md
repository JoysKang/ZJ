---
name: gpui-kit-design-guides
description: ZJ 的原生 GPUI 界面设计与交互审阅规范，基于 Longbridge 官方 Design Guides。用于新增或修整布局、信息层级、间距、文案、状态反馈、浮层、密集列表和键盘交互，以及审阅界面是否自然、克制、符合桌面习惯。采用项目现有主题与 GPUI Kit 0.7.1，不能照搬网页设计或上游主分支 API。
license: Apache-2.0
metadata:
  upstream: https://github.com/longbridge/gpui-kit
  upstream_commit: 0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05
  adaptation: ZJ native desktop
---

# GPUI Kit Design Guides：ZJ 项目适配

基于 [GPUI Kit Design Guides](https://gpui-kit.com/docs/design-guides.md)，版权归 Longbridge／GPUI Kit。完整上游指南保存在 [references/design-guides.md](references/design-guides.md)，原入口保存在 [UPSTREAM-SKILL.md](UPSTREAM-SKILL.md)。本入口加入了 ZJ 的产品、版本与验证约束，上游指南与原入口保持原样。上游保留 [Apache-2.0](LICENSE-APACHE)，并对具备授权权利的指定文档提供 [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)；适用范围见 [LICENSE-DOCS.md](LICENSE-DOCS.md)。来源及变更说明见 [UPSTREAM.md](UPSTREAM.md)。

## 先读什么

1. 先读项目根目录 `CLAUDE.md`，用户当前要求和项目规则优先。
2. 新屏幕或重设计：阅读完整上游指南。局部改动：先读 `Design thesis`、`Start from the task`，再读相关章节。
3. Agent 产出界面前读 `Guidance for AI-generated interfaces`；交付前核对 `Accessibility checklist` 和 `Design review checklist`。清单用来判断设计，不等于已完成原生验收。
4. 不按记忆编造 GPUI API。实现涉及的组件、方法和主题字段必须在项目锁定的 GPUI Kit 0.7.1／vendor 源码核实。

指南中的网站相对链接和插图路径属于 `https://gpui-kit.com/docs/`／该站点，不是要求在项目里创建同名文件。`./coding-guides.md` 指官方 [Coding Guides](https://gpui-kit.com/docs/coding-guides.md)，本次未安装配套编码 skill。设计指南副本已完整提供，不需为使用本 skill 自动联网、安装工具或升级依赖。

## ZJ 覆盖规则

- **范围。** 项目查看、代码阅读、搜索和 Diff 是主任务。默认局部打磨，保留未保存内容、标签、选区、滚动与焦点；不恢复内置 Agent，不扩张为完整 IDE。
- **主题。** 保留 Solarized Dark／Nord Light。尺寸、颜色、间距、行高和圆角只从 `crates/app/src/theme.rs` 取；上游 `cx.theme()`／`rem` 示例是参考，不能据此替换 ZJ 的 token 体系或引入未支持的 API。
- **密度。** 保留默认正文 13、默认侧栏 400 px、200–520 px 拖动范围与既有工作台行高。使用现有 4 px 网格及项目已记录的例外，不把上游默认字号或间距当作新要求。
- **图标与可访问性。** 使用现有 Lucide 和文件类型图标，核实已内嵌；图标按钮必须有 `accessibility_label`。正文对比度 ≥ 7:1、次要文字 ≥ 4.5:1；tooltip 不能替代名称、关键说明或键盘入口。
- **状态。** 上游「状态须可辨识」不代表每次后台刷新都显示提示。保留静默刷新；Git 查询失败不能显示为零变更或成功。只读内容通常仍应允许选择与复制。
- **资源。** 不新增常驻动画、光标闪烁、轮询和持续重绘；保留低内存 vendor 补丁。必要的临时反馈使用有结束条件、身份校验的一次性任务，不能让旧任务清掉新结果。
- **实现。** render 不做 IO、Git 命令或全文解析；后台结果校验 generation。优先复用现有组件，不为审美调整引入依赖。
- **权限。** 本 skill 不增加权限。不擅自连接用户服务、真实远端，不提交、推送、部署或替换应用；这些操作按已有用户授权和项目规则执行。测试只使用临时本地夹具。

## 与 unslop 一起使用

视觉方向与逐项打磨使用项目级 [unslop](../unslop/SKILL.md)，保留其九项流程、完整原入口和视觉参考。共同执行顺序见[联合流程](../unslop/references/combined-workflow.md)：先明确原生任务与状态，再确定视觉方向并打磨，最后核对原生实现、交互及视觉结果。下列步骤是原生侧的检查要点，合入同一次任务，不再独立重设计或递归调用另一份 skill。

## 一次改动怎么做

1. **明确任务与对象。** 写清当前用户要完成什么、动作作用于哪个对象、结果是什么、键盘如何走通，以及状态由谁持有。
2. **建立证据。** 从用户截图／步骤定位相关渲染、事件和状态路径。列出「位置 → 具体障碍 → 最小改法」，不凭控件名称猜测行为。
3. **执行已授权的范围。** 仅审阅请求则只报告；已明确要求修复时按既有方向继续，不重复索要批准。只有缺失决定确实改变范围或结果时才询问。
4. **逐项核对设计。** 动作承诺、信息主次、上下文复用、状态语义、组件、键盘、焦点、窄窗口及亮暗主题。没有具体问题时不为套用清单而改动。
5. **验证并交付。** 交互问题先复现，再补正确事件入口的回归；按项目规定审查和检查。说明行为变化、真实结果与无法验证的范围，不把截图、静态代码或无头测试等同于完整原生验收。

## 操作反馈：本项目的具体应用

这些是从官方 `Interaction states`、`Feedback and overlays`、`Interface language` 推导的 **ZJ 本地规则**，不是上游逐字规定。

- 无变化如「已是最新」：一行紧凑反馈即可，诊断信息按需展开。结果已清楚可见时，不再叠加成功通知。
- 成功有变化：用真实结果说明发生什么，如「已拉取 3 个提交」；没有可靠数量时不编造。
- 失败／部分完成：保留发生的问题、受影响范围与恢复动作，直到用户处理或明确关闭；不能自动消失成成功。
- 详情：保留有界、脱敏、可选择和复制的原始输出及截断说明。收起正文前必须有可访问的详情入口；主区域不重复堆放已有仓库、分支和完整路径。
- 安全信息：首次发布、实际远端目标确认、危险写入确认、外部修改保护与错误信息不能以简化为由移除。

## 验证边界

- 复用 `crates/app/src/workbench/test_support.rs` 和相关 UI 测试，通过实际鼠标／键盘／动作断言焦点、剪贴板及最终结果，不靠直接设置状态证明入口可用。
- 无头布局测试用默认 test profile；窄窗口通过 `simulate_window_resize` 并断言真实视口或面板尺寸，不能仅调用 `Window::resize`。
- 原生窗口检查须使用当前已授权、可用的验收方法；权限阻塞时如实说明，不绕过，也不以重建 HTML、浏览器截图或无头结果冒充原生通过。
- 纯 skill／文档引入验证元数据、路径、引用、来源哈希、许可证、独立复核及差异空白即可，不因此运行 Rust 全量测试或打包。

客户端加载本项目 skill 后，可用 `/gpui-kit-design-guides 审阅 SCM 操作反馈`，或要求「按 GPUI 设计指南修整这个界面」。当前会话列表尚未刷新时，在下一次项目会话中使用；不能声称未调用的 skill 已成功加载。
