# 白天补充验收（2026-10-23）

后续追加：Agent 核心忙碌态与微信拼音扩展场景已通过；VoiceOver 后续已按用户要求取消，专项脚本已删除，不再作为待验收项。最新产物与证据见 [Agent、IME 与 VoiceOver 验收](2026-10-23-agent-ime-voiceover.md)。下文保留上一批验收产物与结果。

用户要求继续四小时回放之外的未完成项，长时回放留到晚上。本文只记录本轮新增证据；不会重复已通过的全量检查，也不把未执行的计划写为通过。

## 固定范围

当前产物 `target/dist/workspace-editor`，49,461,504 bytes，SHA-256 `b46b04b3ae8136ab2f5efe5b8745dd4d2428e3edab4c17fb7dd26a1492ace12d`。保留全部未提交修改，不提交或推送。真实 GUI 操作串行，使用临时文件和隔离设置／会话／恢复／历史目录，每次输入前检查前台 PID。

优先补齐：脏文档关闭的取消／保存、Untitled 系统另存为、终端新建／拆分／输入／关闭、中文组合输入，以及运行中 Agent 面板。Agent UI 使用已有本地 ACP 测试程序，不调用外部模型，不把本地协议夹具当作真实提供商端到端验证。四小时回放尚未启动，也未安排具体开始时间。

## 升级资料核验

本轮再次请求官方 Rust stable manifest：`https://static.rust-lang.org/dist/channel-rust-stable.toml`。WebFetch 仍返回 `Unable to verify if domain static.rust-lang.org is safe to fetch. This may be due to network restrictions or enterprise security policies blocking claude.ai.`。这不是 HTTP 403，也不能据此断定用户网络无法访问；没有改用其他通道绕过工具的域名检查。升级仍缺官方候选版本和兼容证据，现有依赖保持锁定。

## 新增验证结果

| 场景 | 当前状态 | 证据边界 |
| --- | --- | --- |
| 脏文档关闭：取消／保存 | 通过 | `dialogs3/result.json`：取消保留缓冲区及磁盘，保存写盘后关闭 |
| Untitled 系统另存为 | 通过 | 原生 Save 面板写入临时路径，磁盘内容精确一致 |
| 终端新建／拆分／输入／关闭 | 核心路径通过 | `terminal4/result.json`：ABC 命令输出与工作目录正确；shell 数 1→2→3→0，关闭后回收；新建快捷键注入未验证成功，按钮成功 |
| 中文候选、提交、撤销 | 本场景通过 | 微信拼音 `nihao`→空格选择「你好」，Escape 保留已提交文本，撤销回原文；不扩展为全输入法覆盖 |
| 实际编辑后的异常恢复 | 通过 | `recovery1/result.json`：真实快照出现后仅 kill 测试 PID，再启动精确恢复，原文件未改变 |
| Diff 双状态与外部刷新、标签详情、Markdown | 部分通过 | 两个 Diff 入口及各自块操作控件、Markdown 源码／预览往返通过；外部修改有监听和刷新日志。AX 不提供正文与图片内容，画面和标签详情未验证 |
| Agent 忙碌、追加、审批与待审入口 | 未完成 | 本地 stub 配置可用，但驱动未建立稳定输入框定位，未发送首条 prompt；不能判为产品提交缺陷，也未验证动态审批卡 |

以上已执行记录对应固定 `b46b04b3…` 二进制，GUI 串行。保存、终端与恢复证据已复制到主工作区 `target/health-check/daytime-followup-20261023/native-save-terminal/{dialogs3,terminal4,recovery1}/`。详细步骤和失败尝试见[保存、终端、中文输入与异常恢复验收](2026-10-23-save-terminal-validation.md)及 [Diff 与 Markdown 原生交互验收](2026-10-23-diff-markdown-validation.md)，整体缺口见 [A01–A19 证据矩阵](2026-10-23-acceptance-status.md)。

实机确认终端四个图标按钮在 AX 中没有可访问名称。已在 `terminal_panel.rs` 共用按钮构造处补 `.accessibility_label(tooltip)`，覆盖新建、拆分、终止和隐藏，不改变动作或样式。产品改动独立窄审 0 findings，fmt 与差异检查通过。终端驱动改按名称定位，断言四个名称唯一，并验证隐藏／重开不会结束 shell；驱动审阅发现 AX 可能晚一帧的检查时序问题，已改为有界等待。快捷键 helper 3 项、输入源 helper 4 项测试均 OK，三个新驱动语法检查通过。`cargo clippy --workspace --all-targets --locked -- -D warnings` 退出码 0，31.27 s 完成；`cargo test -p workspace-editor terminal --locked` 为 14 passed、0 failed、446 filtered out。发布构建退出码 0，新产物 49,461,504 bytes，SHA-256 `9da85a2d4bc710374e93de4bf7a354bfcc21d553d96ddd82a75290b6ec7905aa`，与修复前大小相同。旧产物副本保存在 `before-terminal-labels`，不把旧结果混称新改动验证。间接依赖 `block 0.1.6` 的未来兼容性警告仍在。

新产物执行 `python3 -B -I target/health-check/daytime-followup-20261023/run-terminal-labels.py` 退出码 0：四个 AX 名称唯一、按名称点击、隐藏／重开保留 shell、ABC 输入与工作目录、新建／拆分以及 shell 全部回收均通过。测试 PID 49033 退出码 -15，清理前后子进程列表均为空，输入源恢复微信拼音；证据在 `terminal-labels/result.json`。此次新增产品改动仅为按钮可访问名称，未重复全量 638 项或资源／延迟测量。

Agent 验收的驱动限制与两次复核见 [Agent 原生验收边界](2026-10-23-agent-native-validation.md)。未实际生成的审批卡和待审状态不能用空闲 AX 树推断；没有据此改动 Agent 产品代码。

## 晚间回放安排边界

尚未启动或预约四小时任务。晚间取得具体开始指示后，先冻结当时产物、夹具和源文件状态，复用白天已验证的交互步骤；只操作隔离数据。保留完整时序和失败记录，观察前后半段内存／对象／CPU 趋势；丢失前台焦点即停止输入，不能继续向其他应用发送按键。四小时内避免编译与其他 GUI 自动化，空闲间隔必须保留，不能用持续繁忙的循环代替真实使用。到时另行记录实际持续时长，未满四小时不得宣称长时验收通过。
