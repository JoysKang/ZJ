# ZJ 的 GPUI 适配约定

这是项目适配要求，使用前核对根目录 `CLAUDE.md` 和当前代码；冲突时服从项目规则和用户要求。它不增加执行权限，也不把静态审阅当作原生验收。原生任务、组件与交互依据 [GPUI 官方指南](../../gpui-kit-design-guides/SKILL.md)，与 unslop 共用[联合流程](combined-workflow.md)，不重复启动两轮设计。

## 映射到项目现有实现

| 上游 Web 概念 | ZJ 的对应做法 |
| --- | --- |
| CSS／Tailwind 间距、颜色和圆角 | 只在 `crates/app/src/theme.rs` 定义 token，GPUI 元素引用；不在渲染代码散落字面尺寸和色值 |
| 组件库 | 复用当前 GPUI Kit 组件及项目已有组件，不导入针对 Zed 版 GPUI 的不兼容示例 |
| 网页响应式布局 | 检查桌面窗口与窄窗口的真实 GPUI 布局，保留可读内容与关键操作；默认侧栏 400 px、可拖动 200–520 px |
| 字体和主题重设 | 保留默认字号 13、现有行高与字体；Solarized Dark／Nord Light，不使用上游 token 表替换主题 |
| 图标 | 现有 Lucide／vscode-icons；新增 Lucide 先核对是否内嵌；图标按钮设置 `accessibility_label` |
| DOM 点击／键盘自动化 | GPUI 的真实鼠标、快捷键与动作入口，验证焦点和最终可见结果／剪贴板；复用 `workbench/test_support.rs` |
| Playwright 截图 | 使用已获授权、当前环境支持的原生验收方法；不安装浏览器来冒充原生窗口测试 |
| CSS 动效 | 优先静态；需要时使用有结束条件的一次性任务；不破坏空闲不重绘与光标常亮约束 |
| HTML 故事页／生成图片 | 本适配不包含；不能替代原生代码交付，也不自动连接外部生成服务 |

## 相关实现入口

先按问题用精确检索定位，只读需要的代码。这些路径是定位线索，使用时确认仍存在。

- `crates/app/src/workbench/scm.rs`：SCM 布局、仓库行和结果呈现。
- `crates/app/src/workbench/scm_actions.rs`：Git 操作、确认与结果状态。
- `crates/app/src/workbench/scm_management.rs`：首次发布及仓库管理入口。
- `crates/app/src/workbench/diff_view.rs`、`diff_ops.rs`：Diff 展示、选区、复制与行操作。
- `crates/app/src/workbench/chrome.rs`、`sidebar.rs`、`editor_area.rs`：工作台与面板布局。
- `crates/app/src/workbench/test_support.rs` 和相关 `*_ui_tests.rs`：无头窗口、真实事件与布局断言。

## 实现边界

- render 不做 IO、Git 命令或全文解析。后台任务结果校验 generation；迟到结果不能覆盖新选择或新请求。
- 保留焦点、选区、标签、滚动、未保存内容及关闭保护。只读与禁止复制不是同一个概念。
- 反馈收起、关闭和自动消失都须绑定对应操作／状态，避免旧定时任务清掉新结果。只在真实操作完成后显示成功；无变化与失败不能混淆。
- 详细输出须有界、可访问、可复制且已脱敏；只在明确用户动作下展开，不为布局美化丢弃诊断信息。
- 不新增依赖解决能用现有组件完成的局部调整；不启动外部连接或真实 Git 写入验证。Git 写测试只使用临时本地夹具。

## 验证与交付

- 行为回归使用实际事件触发同一条用户路径，断言具体结果。用户已指出的交互问题应先有能捕获该问题的失败回归。
- 布局测试使用默认 test profile；通过 `simulate_window_resize` 改变视口并断言真实尺寸。仅调用 `Window::resize` 不算窄窗口验证。
- 对照亮／暗、正常／窄窗口、键盘焦点、长路径／长错误、进行中／成功／无变化／失败／部分完成等与本次改动有关的情况，不为无关状态扩大测试。
- 按项目要求运行检查与独立审查。纯 skill／文档引入只需核对元数据、路径、引用、许可证、改动范围和空白，不因此重跑 Rust 全量或打包。
- 原生截图及真实交互无法验证时，写明限制。测试通过不等于原生外观、中文输入法或完整屏幕呈现已验收。

## 调用方式

项目 skill 被客户端加载后，可用 `/unslop 修整 SCM 的操作反馈`，或要求「按 unslop 审阅这个界面」。如果当前会话的 skill 列表尚未刷新，在下一次项目会话中使用；不要声称本会话已成功调用新 skill，除非工具确实完成加载。
