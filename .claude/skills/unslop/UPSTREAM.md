# 来源与 ZJ 适配说明

引入日期：2026-11-04。上游：[unslop by heydamir](https://github.com/heydamir/unslop)，作者为 [heydamir](https://github.com/heydamir)。本副本及适配采用 [CC BY 4.0](LICENSE)，保留原作者署名与来源；这是 ZJ 项目适配，不代表上游提供或验证了 GPUI 支持。

固定上游版本：[`a6f121eee5f29c2f74976b75456feae8b66afb3a`](https://github.com/heydamir/unslop/tree/a6f121eee5f29c2f74976b75456feae8b66afb3a)。仅读取公开源文本和参考图，没有执行上游脚本或安装依赖。

| 本地文件 | 来源／处理 |
| --- | --- |
| `SKILL.md` | 中文 GPUI 适配入口，保留审阅、五行方向与九项打磨顺序，接入 GPUI 原生指南 |
| [UPSTREAM-SKILL.md](UPSTREAM-SKILL.md) | 完整原 skill 原样副本，重命名避免重复发现入口；其中 Web 工具流程不自动执行 |
| `references/signs.md`、`tokens.md`、`refs.md`、`visuals.md`、`effects.md`（均在 `references/`） | 五份完整原样参考，保留原署名；网页 token、CSS 与生成工具仅作资料 |
| `references/refs/*.webp` | 五张原版参考图，包括八帧故事分解图；只供离线设计参考，不进入应用 assets 或运行包 |
| [LICENSE](LICENSE) | 上游许可证完整原样副本 |
| [references/gpui.md](references/gpui.md) | 本地新增：主题、组件、异步边界、交互与验收映射 |
| [references/combined-workflow.md](references/combined-workflow.md) | 本地新增：与官方 GPUI 指南协作的一次设计流程 |
| [SOURCES.json](SOURCES.json) | 12 份上游原样副本的源路径、本地路径、字节数与 SHA-256 |

## 对原 skill 的改动

- 从 Web／营销页适配为 ZJ 原生工作台的审阅与打磨，保留实质性的方向、视觉参考与九项流程。授权区域内可重组信息、比例、对齐和留白；不重建 HTML 或为套模板添加装饰。
- 原版建议先确定方向并等待批准；本适配服从用户当前授权，已有明确修复要求时直接执行，仅在必要决定缺失时澄清。
- 项目 `theme.rs` 的配色、尺寸、字体、400 px 侧栏和图标规则覆盖原版 `tokens.md` 的建议数值。需要的新语义 token 仍定义在 `theme.rs`；「每屏一个亮点」不变成额外装饰要求。
- 新增紧凑操作反馈、详情渐进展开、原文复制、错误持久性、焦点与只读交互规则。这些是 ZJ 的本地适配，不宣称上游有 Git 专项规范。
- 与 [gpui-kit-design-guides](../gpui-kit-design-guides/SKILL.md) 共用联合流程；原生任务、组件、键盘与验证由 GPUI 规则承接，视觉方向、层级和细节由 unslop 打磨。项目规则和用户授权优先。
- 保留上游参考文本和图片，省略 `agents/openai.yaml`、网页快照／故事视频脚本及示例 HTML，没有安装生成图片／视频工具。原入口中指向这些未安装资源的路径属于上游资料，不是本地可用功能；不能据此自动联网、安装或执行。

## 来源校验

[SOURCES.json](SOURCES.json) 是 12 份上游原样副本的校验清单，不包含本地改编文件。SHA-256 用于确认固定来源，不是脚本执行或自动更新授权。升级时人工核对上游差异，更新固定版本、许可和清单；不要自动覆盖 GPUI 适配或项目规则。
