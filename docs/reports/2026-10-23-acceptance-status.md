# A01–A19 验收证据矩阵（2026-10-23）

本表汇总已有测试日志与本轮原生记录，不额外运行检查。口径：

- **自动化「已执行」** 仅指 `target/health-check/native-repairs-20261023/test.log` 的完整运行：`cargo test --workspace --locked` 638 passed、0 failed、4 ignored（App 456 passed、3 ignored；4 项忽略为 3 个性能探针 + 真实 CLI 握手）。测试函数存在但不在该日志中，标「有测试名/未见执行」。
- **headless 不等于真实桌面**：GPUI 测试窗口不算键鼠、IME、VoiceOver 或屏幕呈现验收。
- **规范**：只以现行 `CLAUDE.md` 为准。A11 旧规格的「陈旧状态可见」已被现行「静默刷新、不展示陈旧/扫描标记」取代；错误本身仍须可见。
- 已通过项不重复推荐重跑。多数原生记录对应 `b46b04b3…ace12d`；终端按钮名称修复后的当前产物为 `9da85a2d…7905aa`，同为 49,461,504 bytes。新产物终端名称、隐藏／重开、输入、新建／拆分和回收已单独通过，其他历史结果不改换归属。

## 矩阵

| 编号 | 场景 | 自动化覆盖（已执行） | 原生 / 未做 | 下一最小验证 |
| --- | --- | --- | --- | --- |
| A01 | 3 工作区、36 工作树 | `real_git_identities_states_diffs_and_cancellation`（F 夹具 3×12＋linked＋submodule）；`discovery_…` 深度/忽略 | 未做：3 个真实窗口互不串仓 | 用 F 夹具开 3 窗，逐仓核对分组与 RepoId |
| A02 | 多仓库分组同时展开 | `repositories_keep_name_order_after_changes_and_commits`、`change_tree_folders_are_independent_and_survive_refresh` | 未做 | 一窗展开 ≥3 仓并切文件，确认分组不退化 |
| A03 | 普通仓库与 gitfile 混放 | 同上 F 夹具（separate-git-dir、linked、submodule）；`git_markers_follow_git_rules` | 未做 | 原生发现 gitfile 仓并打开 Changes |
| A04 | 重叠根与别名指向同一工作树 | F 夹具含父目录、子目录、symlink 重叠，断言仍为 36；`aliases_share_identity…`（文件身份） | 未做：关一个工作区不影响另一引用 | 两工作区引用同一树，关其一后另一侧仍可操作 |
| A05 | 同仓两个 linked worktree 各有修改 | 夹具断言 RepoId 不同、common dir 相同；`linked_worktrees_cannot_drop_a_different_stash…` | 未做：两树 Changes 分列 | 原生开两条 linked 树，核对分支/index/Changes |
| A06 | 先暂存再改磁盘 | F 夹具 repo-01；`commit_message_diff_matches_the_next_commit_without_staging` 校验 Staged 与 Worktree 两侧 diff 文本 | 部分：原生两入口及各自块操作控件通过，未执行块写操作；AX 不提供正文 | 仍缺两侧正文的视觉核对 |
| A07 | 两仓同名相对路径 | `disk_stage_unborn_unstage_commit_hook_discard_and_local_push` 断言另一仓同路径未被触碰；按 RepoId 作用域 | 未做 | 两仓同 `a.txt`，分别暂存/提交后核对另一仓 |
| A08 | 新增/未跟踪/删除/重命名/冲突/二进制 | `preserves_raw_paths_and_dual_state`、`malformed_output_never_becomes_clean`、`conflicts::*`、`merge_conflicts_are_resolved_inline…`、F 夹具二进制与冲突 | 未做 | 原生逐类查看状态与 diff 边界 |
| A09 | 终端改文件、原子保存、checkout、commit 收敛 | `external_merge_conflicts_refresh_scm_and_an_open_clean_document`、`external_merge_refresh_does_not_wait…`、`refresh_plan` 构建忽略、`native_events_share_paths…` | 部分：终端基本输入通过；外部改临时文件出现 watch_flush 与 refresh_finished，但未观察到 Diff 正文 | 终端内 checkout／commit 后的 SCM 与正文收敛仍待验证 |
| A10 | 事件风暴与溢出 | `bursts_errors_and_rescans_use_one_bounded_wakeup`、`external_merge_overflow_updates_known_repos…`、溢出后退化为全量 | 未做 | 原生制造大批事件后确认有界队列且补查完成 |
| A11 | Git 缺失/超时/权限失败 | `git_failures_are_reported_not_read_as_absence`、`remote_check_failure_keeps_local_changes_and_reports_the_error`；`check_git_deadlines.py` PASS（timeout/parent/cancel，无后代） | 未做：原生 PATH 去掉 git | 去掉 git 的 PATH 启动，确认报真实错误而非 clean；不显示陈旧标记 |
| A12 | 中文组合输入、选区替换、粘贴、撤销 | 仅 `input_switch` 源切换；组合输入行为由 vendor 层处理，无单测 | 部分：微信拼音提交后快捷键已通过；`recovery1/result.json` 另确认 `nihao` 候选提交为「你好」、Escape 保留、撤销回原文 | 后续 `ime-expanded/result.json` 已通过中文候选替换选区、取消组合、多行粘贴及撤销／重做；其他输入法及候选窗几何未覆盖 |
| A13 | 保存权限或空间不足 | `read_only_files_are_refused`、`a_writable_file_in_a_folder_that_takes_no_new_files…`、临时文件创建失败不清理他人文件 | 未做：ENOSPC 未覆盖 | 只读目录与 `ENOSPC` 下保存，确认原文件不截断、缓冲区保留 |
| A14 | 外部修改后保存、两次并发保存 | `external_changes_are_detected`、`a_save_during_another_save_waits_and_writes_the_newer_text`、`manual_reload_keeps_edits_made_before_its_completion` | 未做 | 原生外部改文件后保存，核对冲突提示不静默覆盖 |
| A15 | CRLF/BOM/无末换行/可执行位/符号链接 | `line_endings_and_byte_order_mark`、`atomic_write_keeps_mode_and_replaces_the_file`、`symlinks_are_written_through_and_hard_links_in_place`、`keeps_crlf`、`missing_trailing_newline_is_a_change` | 未做 | 原生对 CRLF/BOM/可执行/软链各保存一次并比对字节 |
| A16 | dirty 关闭、取消与异常退出恢复 | `quitting_asks_each_window_and_stops_on_cancel`、`closing_tabs_rechecks_files_edited_while_the_prompt_is_open`、`snapshots_left_by_a_crash_come_back_and_can_be_discarded`、`quitting_removes_snapshots_whose_removal_is_still_pending` | 已通过：取消保留内容、保存后关闭；真实编辑产生快照后强退，重启恢复内容精确一致且原文件未变（`native-save-terminal/{dialogs3,recovery1}/result.json`） | 原生「放弃恢复」分支尚未覆盖 |
| A17 | 重复打开同一文件（含跨窗口） | 仅 `a_hard_link_opened_elsewhere_keeps_one_editable_owner`（硬链接）、`save_as_serializes_the_same_buffer_and_reserves_its_destination`；**同路径跨窗定位未见专测** | 未做 | 原生两窗开同一路径，确认定位唯一实例 |
| A18 | 大文件模式、取消大 diff、关闭预览 | `large_file::*`（稀疏索引、长行、非 UTF-8、前部改重建）、`large_view::*`（打开/跟随磁盘/拖目录/转到行复制查找） | 未做：取消中的可见反馈与缓存回收 | 原生打开 >8 MiB 文件，滚动/查找/关闭看内存回落 |
| A19 | Markdown 表格、代码块、相对图片、外链 | `markdown_opens_rendered_and_edits_one_block_at_a_time`、`local_images_resolve_remote_ones_do_not_and_relative_links_open_here`、`markdown_blocks::*`、`known_fences_get_syntax_runs` | 部分：原生源码／预览往返及源码保留通过；AX 未暴露表格、代码块、图片及链接入口 | 仍缺实际渲染与链接点击的视觉验证 |

## 关键行号与日志索引

- `crates/app/src/workbench/scm_ui_tests.rs:2356/2361` queued other-repo diff reload（日志中为 passed）。
- `crates/app/src/workbench/markdown_ui_tests.rs:52/140/203/247`；`recovery_ui_tests.rs:155/249/312`；`large_ui_tests.rs:20/117`（经 `large_view.rs:646` 的 `#[path]` 挂为 `large_view::ui_tests`，日志即此名）。
- `crates/git_service/tests/fixtures.rs`（F 夹具 36 工作树、别名、linked）、`failures.rs`（Git 失败）、`writes.rs:192`（同路径另一仓）。
- 完整日志：`target/health-check/native-repairs-20261023/test.log`；资源/延迟/快捷键证据同目录 `window-reclaim/`、`latency-evidence/`、`shortcut-final/`。

## 本轮待证据（由其他会话汇总，勿先定未通过）

| 项 | 状态 |
| --- | --- |
| 真实忙 Agent UI | 后续已通过本地 stub 的发送、同轮追加、停止取消、实际审批、待审入口及拒绝文件恢复；见 `2026-10-23-agent-ime-voiceover.md` |
| 保存确认框 / 系统另存为面板 | `dialogs3/result.json`：取消、保存关闭及 Untitled 系统另存为精确写盘通过 |
| 终端新建/拆分/输入/关闭 | `terminal4/result.json`：按钮路径、ABC 输入与工作目录通过，shell 数 1→2→3→0；新建快捷键未验证成功 |
| 中文 IME（候选窗、组合中按键） | `recovery1/result.json`：「你好」候选提交、Escape 保留、撤销通过；不代表所有组合中按键 |
| Diff 双状态＋外部刷新、tag 中文详情、Markdown | 原生入口／控件和源码往返通过，刷新有日志；正文、图片视觉及标签详情未验证。证据 `native-content-validation/report.md` |
| 屏幕像素呈现延迟 | 完整屏幕延迟未测；VoiceOver 已按用户要求取消，专项脚本已删除，不再作为验收缺口 |
| 升级资料（Rust/GPUI 官方页） | WebFetch 域名校验受阻，未取得一手证据，维持锁定版本 |
| 4 小时回放 | 只在晚间开放；20 次开关已有界通过，不据此声称长时稳定 |

## 结论

- 功能逻辑与异步一致性主要由已执行的 638 项覆盖；A06、A12、A19 已有部分原生证据，A16 的取消／保存及实际异常恢复已通过。A01–A05、A07–A08、A10、A13–A15、A17–A18 仍缺对应的真实桌面验收。
- 已通过的原生项：快捷键（提交后）、20 次窗口回收、ABC 按键 CPU 提交 p99 8.3–9.0 ms（不含 GPU/scanout）、单窗口原生会话恢复与退出、保存确认与系统另存为、终端基本操作。
- 不把自动化通过、headless 测试或脚本 exit 0 当作 A01–A19 完成；`docs/development.md` 的「G1 及 A01–A19 待实机」仍成立，直至上述待证据项汇总。
