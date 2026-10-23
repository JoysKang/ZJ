# 保存、终端、中文输入与异常恢复验收（2026-10-23）

使用主工作区冻结的最终产物，SHA-256 为 `b46b04b3ae8136ab2f5efe5b8745dd4d2428e3edab4c17fb7dd26a1492ace12d`。本轮没有修改产品代码、提交或推送。未重跑旧快捷键、内存测试，也未做四小时回放。

## 隔离与协调

每轮新建临时工作区和设置、session、Agent 历史及 recovery 目录。设置明确为 `auto_save: off`、`auto_input_source: false`、`hidden_files: []`。发送按键、点击和 AX 写入前均通过已有 Driver 核验目标 PID 是前台进程；退出时仅终止本轮 Popen 进程并 wait。

开始前与主会话协调独占 GUI。全部结束后，最后的恢复实例 PID 43966 已退出，GUI 交给 Diff／Markdown 验收会话。没有改系统权限、启动 VoiceOver、申请录屏或调用 Agent。

## 实际通过的项目

| 项目 | 断言与结果 | 证据 |
| --- | --- | --- |
| 关闭脏文档 → 取消 | 已提交标记仍在缓冲区，磁盘仍为原始内容，确认 sheet 消失 | `target/health-check/daytime-followup-20261023/native-save-terminal/dialogs3/result.json` |
| 关闭脏文档 → 保存 | 磁盘全文等于关闭前缓冲区，随后编辑器标签关闭 | 同上 |
| Untitled → 系统另存为 | 真正的 macOS Save 独立窗口；通过 Go to folder 指向临时 workspace，设置新文件名后保存；文件全文一致 | 同上 |
| 终端输入与 cwd | ABC 输入下执行 `printf TERMINALOK > terminal-marker.txt`，标记文件在临时工作区内且内容精确 | `target/health-check/daytime-followup-20261023/native-save-terminal/terminal4/result.json` |
| 新建、拆分终端 | 点击原生界面按钮，直属 shell 数量从 1→2→3 | 同上 |
| 关闭终端 | 依次点击终止按钮，shell 数量从 3→2→1→0；退出应用前已无子进程 | 同上 |
| 微信拼音候选、提交、撤销 | `nihao` 后按空格选首候选，结果为「你好」；Escape 保留文字，Command-Z 恢复原文 | `target/health-check/daytime-followup-20261023/native-save-terminal/recovery1/result.json` |
| 真实异常恢复 | 真实输入并提交标记，等待 recovery JSON 出现，SIGKILL 自有测试 PID；同数据目录重新启动后恢复全文一致，原文件始终未改 | 同上 |

恢复测试首进程 PID 43888，明确以 `-9` 退出；恢复进程 PID 43966 正常完成断言后由脚本清理。没有使用人工构造 snapshot。

中文输入与恢复前后，实时 TIS 输入源均为 `com.tencent.inputmethod.wetype.pinyin`。终端测试单独借用已有 `native_input_source.abc_input_source()` 切到已启用 ABC，`finally` 恢复微信拼音；不是修改默认系统设置。

## 失败尝试与边界

- `dialogs2`：向 Go to folder 发送字符串按键后，没有写出目标文件。该轮仍受拼音输入影响，不能据此认定 Save 面板缺陷。改为 AX 精确设置文件夹路径与文件名，并读回确认后，`dialogs3` 通过。应用日志没有该轮 Untitled 保存成功事件。
- `terminal1`：关闭自动输入源切换、保持微信拼音时，含空格及符号的 shell 命令未生成目标文件；ABC 对照通过。这不是终端自动英文化失败，因为夹具明确关闭了该功能。
- `terminal2`：`key code 50 using {control down, shift down}` 没有创建第二个 shell。按钮路径正常；本轮未追踪该快捷键的原生转换与上下文，不将它直接定为产品 bug，也不声称新建终端快捷键已通过。
- `terminal3`：Kit 图标按钮的 AX `name` 和 `help` 均为 missing value，按 tooltip 查找失败。最终使用当前固定单文档布局中 11／12／13 号按钮，并用实际 shell 数校验结果。这是驱动的明确布局依赖，不是通用 AX 定位方案。
- 终端未验收长输出、复杂全屏 TUI、颜色及输入法候选窗几何。中文只覆盖微信拼音首候选「你好」、提交和一次撤销；未覆盖候选翻页、多候选选择或其他输入法。
- 未验收关闭应用时的保存分支、另存为覆盖现有文件确认；本轮覆盖的是关闭文档和新建 Untitled 文件。

## 可复用脚本

三个脚本都依赖同目录 `check_native_shortcuts.py` 的 PID guard、AX 读取和输入提交检查。证据目录必须不存在；运行前必须协调 GUI 时段。

```bash
python3 -I tools/check_native_dialogs.py /absolute/path/to/workspace-editor /absolute/path/to/new-dialog-evidence
```

```bash
python3 -I tools/check_native_recovery.py /absolute/path/to/workspace-editor /absolute/path/to/new-recovery-evidence
```

终端脚本要求实时输入源为 ABC。可由协调过 GUI 的调用者置于项目现有 `native_input_source.abc_input_source()` 上下文中执行 `check_native_terminal.run(binary, output)`，上下文会恢复原输入源；脚本自己不会静默切换。

```bash
python3 -I tools/check_native_terminal.py /absolute/path/to/workspace-editor /absolute/path/to/new-terminal-evidence
```

通过后的只读审查收紧了驱动：删除 Save 面板完整 AX 树输出，面板出现改为有界等待；保存夹具改为唯一目录名，Where 精确比较，避免通用 `ws` 子串检查；终端要求 ABC，命令输出改为绝对临时路径并单独核验 `pwd`，清理后检查已记录的 shell PID；中文未测时不再给出整组通过。上述收紧没有再次占用已交接的 GUI，实际通过证据仍对应前述运行版本，最终脚本的这些新增约束尚未原生重跑。

三个脚本在交付时的 `py_compile` 与 `git diff --check` 退出码均为 0。该组验收没有 Rust 变更，没有为此重复全量构建或 Rust 测试。集成后终端脚本已改成按 AX 名称定位，并增加隐藏／重新打开时保留 shell 的检查；主会话另行修复按钮 AX 标签，后续产物与验证见[白天补充验收](2026-10-23-daytime-followup.md)，不包含在本报告的旧产物结果中。
