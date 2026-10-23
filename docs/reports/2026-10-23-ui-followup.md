# 未完成交互补充

日期：2026-10-23。用户要求继续完成未完成项；本次范围是已有调研提出的轻量 Agent 交互，以及自动保存剩余入口。保留上一轮审计改动，不提交或推送。

## 范围与基线

- 待审修改会话保留活动入口，不因回复已读而消失。
- 运行中的输入区直接说明追加当前轮的语义，沿用 Agent 能力判断与审批。
- 命令面板复用现有审阅入口。
- 检查 Markdown 块编辑失焦保存与系统菜单的当前自动保存模式反馈。

本轮前产物保存在 `target/health-check/ui-followup-20261023/before-workspace-editor`，49,461,456 bytes，SHA-256 `259486e114e85d48ca5c3bab91ebc1cbb3eec424100bfccaf9fc1f25e8e1657c`。上一轮工作区测试为 629 passed、0 failed、4 ignored。这些是基线，不代表本轮修改已验证。

## 原生资源对照

全量检查与构建结束后，使用同一临时目录和同一 6781-byte Rust 文件，加载后等 10 秒，各采样 30 秒。数据为 `target/health-check/ui-native-20261023/ui-compare.json`，含两个产物的完整 SHA、原始样本及进程退出记录。

| 指标 | 修改前 | 修改后 |
| --- | ---: | ---: |
| footprint 中位 | 82.27 MB | 64.80 MB |
| footprint 最大 | 90.90 MB | 73.43 MB |
| CPU p95 | 0.9048% | 0.01496% |
| CPU 最大单样本 | 1.0512% | 0.0497% |
| 样本数／时长 | 114／30.07 s | 114／30.19 s |
| 出帧／display link 唤醒 | 0／0 | 0／0 |

两组 CPU p95 均低于 1% 上限，新产物单窗口中位内存低于 70 MB 目标。两组都经 SIGTERM 约 0.004 s 退出，无跟踪进程遗留；信号退出不代表正常关闭交互验收。该组没有启动 Agent 会话，未覆盖真实 busy Agent UI，单轮差值未归因，不能宣称这次界面改动节省了 17 MB 内存或改善 CPU。

执行偏差：测量 worker 在正式开始前的脚本检查中意外启动过两个隔离实例，未遵守等待全部检查结束再启动的协调要求。该组数据未删除，另存 `probe-run-not-used.json`；正式表格仅使用全量检查结束后的固定对照，未按数值选择结果。没有重复五分钟或多窗口测量。

## 重新核验的阻塞项

2026-10-23 对 GPUI Kit 发布页与 Rust stable channel 的 WebFetch 均返回 `Unable to verify if domain … is safe to fetch`。没有通过其他下载方式绕过该限制；依赖升级仍缺一手发布与源码兼容证据。

本次实现时只读查询 `AXIsProcessTrusted: False`；`cargo-deny` 未安装。当时未更改系统权限，也未将无头测试写作实机验收。随后用户完成授权，辅助功能与 System Events 已验证可用，并进行了真实输入、保存、关闭与恢复测试；实际通过项及新发现见[真实原生交互验收](2026-10-23-native-interaction-validation.md)。中文 IME、VoiceOver 和完整画面延迟仍未验收。

## 实现与验证

实现已完成，最终全工作区验证通过：

- `LiveSession::row_status` 在现有状态为空且仍有修改时显示「待审阅」，活动条保留这些会话；最后修改处理完成后自然退出待审状态。原有审批、运行、错误与未读优先。
- 输入区忙时显示「追加到当前轮」；不能追加时显示原因。复用原发送逻辑，不增加队列。
- 命令面板新增「Agent: 审阅当前会话修改」，调用现有审阅路径；无会话或无修改时显示真实空状态。
- Markdown 块失焦先冲刷最后输入，再等源缓冲区 Change 记入版本与 dirty 后走现有自动保存入口。
- 原生自动保存菜单显示当前勾选，全局设置生效后同步；只有模式改变才重建菜单。外部编辑器修改设置文件的既有热载限制未在本轮扩展。

没有新增依赖、轮询或服务，未修改 Agent 协议和审批规则。格式与差异空白检查退出码 0。自动保存增量按顺序完成第一阶段 `ponytail-review`、第二阶段 `code-review` Standards／Spec，各为 0 findings。Agent 增量按相同顺序完成第一阶段和第二阶段 Standards／Spec，各为 0 findings。

首次测试与 Clippy 编译均因新增 Markdown 测试两处 E0502 借用冲突失败；已改为先克隆焦点句柄再调用 focus。原失败日志保留在 `target/health-check/ui-followup-20261023/{test,clippy}.log`。第二次测试进入执行，App 为 452 passed、1 failed、3 ignored：新增的最终审阅状态断言遇到夹具回复仍为 Unread，不能把未读当作已读。已调整夹具先经现有会话选择路径标为已读，保留产品未读语义；该聚焦测试随后 1 passed、455 filtered out，耗时 2.01 s，夹具修正窄复核 0 findings。随后独立 Clippy 退出码 0，日志为 `clippy-final.log`。最终 `cargo test --workspace --locked` 退出码 0：**635 passed、0 failed、4 ignored**，20 个 suite；App 为 453 passed、3 ignored，耗时 91.15 s。最终 `cargo clippy --workspace --all-targets --locked -- -D warnings` 退出码 0，耗时 3.18 s。日志为 `test-verified.log`、`clippy-verified.log`；新增六项测试通过，原有四项忽略仍为三个性能基准和真实 CLI 握手。`cargo build --profile dist --locked` 退出码 0，耗时 1 分 26 秒；产物 49,461,488 bytes，比基线增加 32 bytes，SHA-256 `ca4f0b630618fb6cccf32df66bdae95e142e5c62a691b547438038bd6261a682`。唯一构建提示仍为上游 `block v0.1.6` 的未来 Rust 兼容性警告。CPU 换算 Python 回归实际执行：1 test、OK。Git deadline 检查 PASS：timeout 1.002 s、parent_exits 1.011 s、cancel 1.006 s，无遗留子进程。
