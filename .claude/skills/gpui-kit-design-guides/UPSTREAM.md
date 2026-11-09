# 来源、许可与适配

引入日期：2026-11-04。来源：[Longbridge / GPUI Kit](https://github.com/longbridge/gpui-kit)，固定提交 [`0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05`](https://github.com/longbridge/gpui-kit/tree/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05)。

设计指南来自 **GPUI Kit**：[Design Guides](https://gpui-kit.com/docs/design-guides.md)，保留完整原文。上游仓库保留 [Apache-2.0](LICENSE-APACHE) 许可；其[文档许可声明](LICENSE-DOCS.md)还就声明范围内、具备授权权利的文档提供 [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)。本副本同时保留这两份声明、版权和来源，不扩大文档声明的覆盖范围，也不改变第三方材料原有许可。

## 安装内容

| 文件 | 处理 |
| --- | --- |
| `SKILL.md` | ZJ 编写的项目入口；保留上游按任务审阅与按章节读取方法，加入现有产品、主题、版本、状态反馈和验证约束 |
| `UPSTREAM-SKILL.md` | [上游入口](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/skills/gpui-kit-design-guides/SKILL.md)原样保存，只更改本地文件名以避免重复发现 |
| `references/design-guides.md` | [上游完整设计指南](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/skills/gpui-kit-design-guides/references/design-guides.md)，原样保存 |
| `LICENSE-APACHE` | [上游 Apache 许可证](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/LICENSE-APACHE)，原样保存 |
| `LICENSE-DOCS.md` | [上游文档许可声明](https://github.com/longbridge/gpui-kit/blob/0ab9cf4ed6d237457e66efc56c8ffcf915e8ce05/LICENSE-DOCS.md)，原样保存 |

本 skill 没有安装配套编码 skill、应用依赖、网页截图／视频脚本或素材，也没有修改应用代码。另行引入的 [unslop](../unslop/SKILL.md) 保留其原版视觉参考图，两份本地入口按[联合流程](../unslop/references/combined-workflow.md)协作；这些本地接续说明不修改上游原样副本。上游主分支的设计规范不证明相应 API 已存在于 ZJ 锁定的 GPUI Kit 0.7.1；代码实现必须核对本地源码。项目覆盖规则不表示上游已经对 ZJ 做过适配或验收。

## 校验值

| 本地原样副本 | SHA-256 |
| --- | --- |
| `UPSTREAM-SKILL.md` | `de874a5a8db458fc82c3300d5484afe1e0f5352bdae54db923e1f6dde64ea06a` |
| `references/design-guides.md` | `44fbafdacc89bd5c250db23afd18f1b427b5a1babd3454122b5600e6fd146635` |
| `LICENSE-APACHE` | `d1b0449e5478c574ba4f686c2656df7fe77d66821a61f8b6ed3378a58ed9a811` |
| `LICENSE-DOCS.md` | `8a9ccf144b2d5bf5aae56abdc6e723e17e1ba4de1f5d515b3b644288fd4fc16f` |

升级时人工比较固定提交与新版本，再更新来源、许可和哈希；不要自动覆盖本地适配。研究比较见 [GPUI 界面设计 Skill 调研](../../../docs/调研/GPUI界面设计Skill.md)。
