# 自主迭代补充记录

日期：2026-10-22。用户要求将 CPU 上限放宽到 1%，继续未完成项，并研究 mygo、t3code、tern、herdr。项目仍以原生、简单、低资源和美观为约束，不因借鉴增加常驻框架。

## 已完成的预算调整

`CLAUDE.md` 与 `tools/measure_budget.py` 的空闲 CPU 上限从 p95 0.1% 调为 1%；目标仍接近 0%，其他门槛不变。README、体检报告的当前结论及升级验收要求同步更新。历史报告与原始 JSON 保留原数值和当时判断。

使用既有最终 JSON 重判：文件窗口 0.723829%、欢迎页 0.721763%，均在新上限内。纯逻辑边界自检通过：1.0% 不超上限，1.001% 超上限。独立新上下文审阅 0 findings，另以紧邻 1.0 的浮点数验证边界。门槛调整不等于性能优化，也不补齐尚未执行的按键延迟或长期验收。

## 并行工作范围

- 原生验证：只启动隔离配置／会话／工作区夹具中的现有 dist，检查可执行的多窗口、恢复与进程回收路径；补一次固定 5 分钟 CPU 采样。与 Cargo 构建错开。
- 自动保存：补齐「失去焦点时」从编辑器到同窗其他输入控件的触发，沿用现有 Blur 事件和保存保护，不新增轮询。
- 同类项目：[研究报告](2026-10-22-peer-product-research.md)已完成，建议保留待审修改会话的入口、显式说明追加当前轮，以及可选的审阅命令。MyGo 暂为 `egoist/mygo` 框架候选，身份未最终确认；外部来源是官方搜索摘要，不能当作已读源码或实机体验。外部项目内存未经同机实测，不能据架构或热度断言超过 ZJ 预算。上述界面建议本轮未实施。
- 升级核验：[补充报告](2026-10-22-upgrade-followup.md)。候选源码、发布状态与兼容性证据仍不足，未修改工具链或锁定版本。

## 边界

原生辅助功能权限未开放；没有更改系统安全设置。真实桌面点击、中文 IME、VoiceOver、系统保存面板和按键延迟不能用无头测试冒充。`cargo-deny` 本机不可用，本轮没有安装，因此不补报依赖公告检查通过。没有运行真实付费 Agent 会话、推送、部署或发出外部消息；没有提交 Git。

## 自动保存修复与最终构建

源码编辑器现在通过既有 `InputEvent::Blur` 调用自动保存入口；焦点转到同窗 Agent、终端等控件时，`onFocusChange` 能保存。关闭／延时模式不会因失焦提前保存，已删除文件不会被自动重建。没有新增定时器、依赖或后台服务；Markdown 块编辑器的独立编辑路径未扩展，静态系统菜单的模式勾选反馈仍未补齐。

回归测试使用激活后的真实 GPUI 焦点转移。移除新增 Blur 分支后，回归 **0 passed、1 failed**；恢复分支后专项 **22 passed**。此前未激活测试窗口的两次失败属于夹具错误，不作为有效 RED/GREEN 证据。有效日志为 `target/health-check/overnight-focus-active-{red,green}.log`。

全部 Cargo 命令使用 `CARGO_HOME=/Users/joys/work/code/ZJ/target/cargo-home`，夹具位于系统临时目录：

| 命令 | 实际结果 |
| --- | --- |
| `cargo test --workspace --locked` | 退出码 0；629 passed、0 failed、4 ignored，20 个 suite；App 为 447 passed、3 ignored |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 退出码 0 |
| `cargo build --profile dist --locked` | 退出码 0，1 分 19 秒 |
| `cargo fmt --all --check`、`git diff --check` | 退出码 0 |
| `python3 tools/check_git_deadlines.py` | PASS；timeout 1.007 s、parent_exits 1.011 s、cancel 1.012 s，无遗留子进程 |
| `python3 -m unittest discover -s tools -p 'test_sample_resources.py' -v` | 1 test passed |

全量日志：`target/health-check/overnight-{test,clippy,dist}.log`。4 个 ignored 沿用原配置：3 个性能测试和真实付费 CLI 握手。构建仍提示上游 `block v0.1.6` 的未来 Rust 兼容性警告。

新 dist 为 **49,461,456 bytes**，与本次失焦修复前相同；SHA-256 为 `259486e114e85d48ca5c3bab91ebc1cbb3eec424100bfccaf9fc1f25e8e1657c`。体积相同不代表二进制相同，旧资源数据不能归属新 SHA。

## 资源采样工具修复

补充验证发现 `sample_resources.py` 直接把 `proc_pid_rusage` 的 Mach ticks 当成纳秒。本机 timebase 为 125/3，原值因此低估约 41.67 倍。现统一读取 `mach_timebase_info`，两个测量脚本共用 helper；按每个进程的读取完成时刻计算间隔，继续以 `(pid, proc_start_abstime)` 区分进程。

回归覆盖 125/3、1:1 和零值转换，1 项通过。实际 CPU 自校准中，`time.process_time` 为 0.200002 s，换算后的 rusage 为 0.200019 s，差值小于 0.02 s。两个 CLI 的 `--help` 正常；失败读取和零分母均明确报错。

旧 5 分钟记录保留不覆盖。未舍入 JSON 的 p95 从 0.0005520425% 按 timebase 修正为 **0.0230017707%**；CSV 已舍入，按 CSV 算出的 0.025% 精度较低。修正记录为 `target/health-check/native-followup-20261022/cpu-5min-unit-correction.json`，仅修单位，无法追溯修正旧采样时刻。新构建已使用修好的工具完成独立采样，结果见[原生运行时补充验收](2026-10-22-native-validation-followup.md)。

## 新构建原生资源实测

测量对象是上述 `259486e1…1657c` 新产物，所有 Cargo 构建结束后运行；隔离设置、会话、历史库与工作区，无真实 Agent 会话。每组加载后等 10 秒，固定采样 30 秒，111–112 个样本。原始记录在 `target/health-check/native-final-20261022/`。

| 用例 | footprint 中位 | 区间最大 | CPU p95 | 出帧／display link 唤醒 |
| --- | ---: | ---: | ---: | --- |
| 1 窗口欢迎页 | 54.95 MB | 59.15 MB | 0.0164% | 0／0 |
| 1 窗口 20 文档 | 77.14 MB | 85.77 MB | 0.7006% | 0／0 |
| 3 窗口 20 文档 | 130.02 MB | 151.24 MB | 0.0121% | 0／0 |
| 5 窗口 20 文档 | 189.43 MB | 227.43 MB | 0.7249% | 0／0 |

四组 CPU p95 均低于 1% 上限；单窗口 20 文档 footprint 高于 70 MB 目标、低于 100 MB 上限。三窗口 20 文档低于 220 MB 目标，五窗口没有独立的项目硬门槛。与历史运行的波动未归因，不能据此宣称失焦修复改善了性能。

各组 SIGTERM 后约 0.005 秒退出，返回码 -15，无跟踪进程遗留；会话恢复得到 5 个窗口，合成恢复记录被读取。SIGTERM 只验证信号退出与进程回收，不等同于 ⌘Q／⌘W 的保存提示、窗口顺序和关闭生命周期。固定 5 分钟单文件采样已完成：1446 个样本，299.993 秒；CPU p95 **0.0233724%**、最大单样本 0.866%，footprint 中位 **65.08 MB**、最大 73.65 MB，出帧／display link 唤醒均为 0。该组使用修正后的工具直接测得，无需手工换算；一次运行不代表长期分布。

## 分阶段审阅

- CPU 门槛调整：独立新上下文复核 0 findings。
- 失焦保存：`ponytail-review` 0；随后 `code-review` Standards 0、Spec 0。夹具补 `activate_window` 后，两轴窄复核均为 0，不把静态审阅代替执行验证。
- CPU 单位换算：`ponytail-review` 0；第二阶段 Standards 0、Spec 0。

2026-10-23 后续：轻量 Agent 交互与自动保存菜单／Markdown 失焦保存已继续实现，当前验证见[未完成交互补充](2026-10-23-ui-followup.md)。以下保留本报告完成时的边界。

## 尚未完成

原生实机权限相关项目、长期回收与回放仍受本报告「边界」限制。Rust／GPUI 升级没有足够的候选源码与兼容证据，保持现有锁定版本。研究提出的待审会话入口、追加语义提示和审阅命令仍是建议，未宣称已经实现。
