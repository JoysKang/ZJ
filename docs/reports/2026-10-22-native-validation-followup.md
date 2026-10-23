# 原生运行时补充验收（2026-10-22）

本报告只补 [健康检查](2026-10-22-health-check.md) 中未完成、且**不需要用户在场**的原生运行时项目。结论先说：

- 可自动完成的部分：1／3／5 窗口空闲 footprint、SIGTERM 后的进程回收、5 窗口会话恢复、合成恢复快照的读取，均已在隔离临时目录对真实 dist 程序跑通。
- 受权限阻塞：中文 IME、VoiceOver、真实鼠标键盘、按键延迟、通过 ⌘Q 的窗口关闭／恢复交互。辅助功能未授权（`AXIsProcessTrusted` = `false`），Automation 授权查询返回 `-1744`（需要用户确认的弹窗）。没有更改系统安全设置，也没有触发授权弹窗。
- 测量分两组产物：**旧产物**（49,461,456 bytes，mtime `2026-10-22 22:43:17`，SHA-256 前缀 `20d7a333`）与**重建产物**（49,461,456 bytes，mtime `2026-10-22 23:28:07`，SHA-256 `259486e114e85d48ca5c3bab91ebc1cbb3eec424100bfccaf9fc1f25e8e1657c`）。两组数字分开列，不合并、不互相替代。

原始数据分目录保存，均未覆盖：

- 旧产物：`target/health-check/native-followup-20261022/`
- 重建产物：`target/health-check/native-final-20261022/`

每个目录含 `lifecycle.json`（每 0.2 s 一条原始样本：footprint、CPU、进程树大小、CoreGraphics 窗口数）、`lifecycle.log`、`cpu-5min.csv`（`tools/sample_resources.py` 原始 CSV）、`cpu-5min.json`、`cpu-5min-meta.json`、`cpu-5min.log`。旧目录另有 `session5.log`（一个失败用例的真实错误）与 `cpu-5min-unit-correction.json`（CPU 单位换算，主代理写入）。

## 权限检查（只读）

```bash
python3 -I - <<'PY'
import ctypes
lib = ctypes.CDLL('/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices')
lib.AXIsProcessTrusted.restype = ctypes.c_bool
print('AXIsProcessTrusted ->', lib.AXIsProcessTrusted())
lib.AEDeterminePermissionToAutomateTarget.restype = ctypes.c_int
# ……构造 'bund' AEDesc 后，对 local.zj.editor 用 askUserIfNeeded=false 查询
PY
```

实际输出：

```text
local.zj.editor -> -1744 errAEEventWouldRequireUserConsent (would prompt)
com.apple.systemevents -> -600 procNotFound/not running
AXIsProcessTrusted -> False
```

| 项目 | 状态 | 原因 |
| --- | --- | --- |
| 真实鼠标键盘、按键延迟 | 阻塞 | 需要 System Events 打字，需辅助功能＋自动化授权 |
| 中文 IME 实机输入 | 阻塞 | 需要把焦点交给真实窗口并注入按键；系统当前启用 ABC 与拼音（`defaults read com.apple.HIToolbox`，只读确认），但未授权操作 |
| VoiceOver | 阻塞 | 未运行；启动与朗读需要辅助功能授权 |
| ⌘Q／关闭窗口后的恢复交互 | 未做 | 需要向真实窗口发送关闭／退出事件；未授权时不改用信号模拟为等价证据 |

未授权，因此没有更改「系统设置 → 隐私与安全性」，也没有申请或点击授权弹窗。

## 方法

夹具：每个用例一个 `tempfile.TemporaryDirectory(prefix="zj-native-")`，工作区是该目录下的 `ws0..ws4`，文档从 `crates/app/src/*.rs` 复制 20 个。应用数据同样隔离：

```bash
ZJ_SETTINGS=<tmp>/settings.json ZJ_SESSION=<tmp>/session.json \
ZJ_AGENT_DB=<tmp>/agent.db ZJ_FRAME_LOG=1 \
target/dist/workspace-editor <folders...> <files...>
```

没有读写 `~/Library/Application Support/ZJ`，没有真实工作区文件，没有 Agent 会话、网络写入或付费调用。footprint 与 CPU 复用 `tools/measure_budget.py` 的 `tree` / `footprint_mb` / `CpuMeter` / `p95`（含 Git 子进程，按实测采样间隔折算百分比）；窗口数用只读的 CoreGraphics `CGWindowListCopyWindowInfo` 按 pid 计数，不经过辅助功能。

测量窗口与预算脚本一致：加载完成后再等 10 秒，连续采样 30 秒，不裁剪、不挑选、不重测换低值。

脚本（临时目录，不在仓库内）：`/tmp/zj-native-harness/harness.py`（生命周期用例）、`/tmp/zj-native-harness/cpu5min.py`（固定 5 分钟单文件 CPU）、`/tmp/zj-native-harness/session5.py`（旧产物上的失败用例）。

### 5 分钟 CPU 的单位

旧产物那一轮用的是当时的 `tools/sample_resources.py`：它把 Mach absolute time 直接 `/ 1e9`，漏了 `mach_timebase_info` 换算，CSV 与 JSON 里的 CPU 数值都小了 41.667 倍（本机 numer/denom = 125/3）。主代理已修正该工具（抽出 `mach_timebase()` 并改为 `cpu × timebase / 1e9`），本报告旧数据的 CPU 采用主代理写入 `cpu-5min-unit-correction.json` 的换算值：用未取整的 JSON p95 `0.00055204249775429 × 41.6666666667 = 0.02300177074`。重建产物那一轮直接使用修好的工具，无需再换算。

## 结果：旧产物（`20d7a333`）

命令：

```bash
cd /tmp/zj-native-harness && python3 -I harness.py \
  /Users/joys/work/code/ZJ/target/health-check/native-followup-20261022
```

实际退出码 0，耗时 2 分 52 秒（`time` 输出：`7.68s user 21.50s system 16% cpu 2:52.51 total`）。每个用例 112 个样本、采样间隔中位约 0.27 s、实测时长 29.98–30.16 s。

| 用例 | 窗口（日志／CG） | footprint 中位 | 最大值 | CPU p95 | CPU 中位 | CPU 最大 | 出帧／唤醒 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 窗口，欢迎页 | 1／1（CG 首次查询为 0，采样期间稳定为 1） | 54.99 MB | 59.18 MB | 0.694% | 0.0% | 0.732% | 0／0 |
| 1 窗口，20 文档 | 1／1 | 78.51 MB | 87.15 MB | 0.712% | 0.0% | 0.761% | 0／0 |
| 3 窗口，20 文档 | 3／3 | 126.72 MB | 147.95 MB | 0.0154% | 0.0% | 0.066% | 0／0 |
| 5 窗口，20 文档 | 5／5 | 186.37 MB | 224.38 MB | 0.0132% | 0.0% | 0.051% | 0／0 |

固定 5 分钟单文件空闲（1443 样本，299.893 s）：CPU p95 `0.02300177074%`，中位 0%，最大 0.75%（换算后）、footprint 中位 65,045,656 bytes、最大 73,663,640 bytes（仅开头样本）、出帧／唤醒 0／0。

## 结果：重建产物（`259486e1`）

先确认产物：

```bash
stat -f '%z bytes %Sm' -t '%Y-%m-%d %H:%M:%S' target/dist/workspace-editor   # 49461456 bytes 2026-10-22 23:28:07
shasum -a 256 target/dist/workspace-editor                                  # 259486e1…1657c
```

命令与旧产物相同，输出写入新目录：

```bash
cd /tmp/zj-native-harness && python3 -I harness.py \
  /Users/joys/work/code/ZJ/target/health-check/native-final-20261022
cd /tmp/zj-native-harness && python3 -I cpu5min.py \
  /Users/joys/work/code/ZJ/target/health-check/native-final-20261022
```

两条命令实际退出码均为 0；`lifecycle.json` 记录 `2026-10-22T23:29:55`–`23:32:48`，5 分钟采样 `2026-10-22T23:32:51`–`23:38:01`。

| 用例 | 窗口（日志／CG） | footprint 中位 | 最大值 | CPU p95 | CPU 中位 | CPU 最大 | 出帧／唤醒 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| 1 窗口，欢迎页 | 1／1 | 54.95 MB | 59.15 MB | 0.0164% | 0.0% | 0.052% | 0／0 |
| 1 窗口，20 文档 | 1／1 | 77.14 MB | 85.77 MB | 0.7006% | 0.0% | 0.850% | 0／0 |
| 3 窗口，20 文档 | 3／3 | 130.02 MB | 151.24 MB | 0.0121% | 0.0% | 0.043% | 0／0 |
| 5 窗口，20 文档 | 5／5 | 189.43 MB | 227.43 MB | 0.7249% | 0.0% | 0.789% | 0／0 |

每个用例 111–112 个样本，实测时长 30.02–30.14 s。footprint 最大值都出现在测量窗口开头（1 窗口 t≈0.24 s；3／5 窗口在 t≈0.24–0.51 s 的相邻样本），之后回落到中位值附近，没有再次出现同等峰值。这一点只描述观测到的形状，未定位原因。

CPU 非零样本的形状：1 文件窗口在 t≈19.6–24.6 s 有一组连续样本（峰值 0.850%）；5 窗口在 t≈1.6–3.7 s 有一组（峰值 0.789%）；3 窗口没有越过 0.05% 的样本；欢迎页只有 t≈25.5 s 一个 0.052% 的样本。四组中位均为 0%，测量期间出帧与 display link 唤醒均为 0。这些活动没有采样栈，未定位原因。

固定 5 分钟单文件空闲（修好的 `sample_resources.py`，1446 样本，299.993 s，间隔 p95 0.210 s，采样开销 p95 0.029 s，`unavailable_child_samples` 0）：

| 指标 | 值 |
| --- | --- |
| CPU p95（一个逻辑核心 = 100%） | 0.0234% |
| CPU 中位 | 0.0% |
| CPU 最大单样本 | 0.866% |
| 非零样本 | 143／1446 |
| footprint 中位 | 65,078,448 bytes |
| footprint p95 | 65,078,448 bytes |
| footprint 最大 | 73,647,256 bytes（仅开头 2 个样本，t≤0.2 s） |
| 出帧／display link 唤醒 | 0／0 |

非零 CPU 以约 3 秒一次的单个样本为主（约 0.02%），另有 4 组约 1–3 秒的连续样本（t≈8.5–12.9、27.0–30.3、66.4–69.3、83.3–85.3 s），峰值 0.866%。中位 0%、无出帧、无唤醒：窗口空闲时不重绘。按开发说明 297 的「无文件变更且光标静止后的 5 分钟、汇总本应用相关进程、100% = 一个逻辑核心」，这一轮 p95 0.0234%、最大 0.866%，在 1% 门槛内；它是一次运行，不建立长期分布。

## 结果：生命周期与回收

两组产物上的结果相同：

| 检查 | 实际结果 |
| --- | --- |
| SIGTERM 到主进程退出 | 4 个用例均为 0.005 s，返回码 `-15` |
| 进程树回收 | 停止后 `members_alive_after_stop` 全为 `[]`；`pgrep -fl workspace-editor` 输出为空 |
| 测量期子进程 | 进程树成员恒为 1（临时工作区没有 Git 仓库，未启动 git 子进程） |
| 会话记录 | 每个用例加载后 `session.json` 已写入对应窗口数（1／1／3／5），带 root 与 frame |
| 无参数重启恢复 | 先 `--windows 5` 写入 5 个窗口记录，再以空参数启动：`event=window_opened` 5 次，CG 窗口数 5 |
| 恢复快照读取 | 在隔离的 `recovery/` 放一条合法记录：`event=recovery_found count=1`，无 `recovery_unreadable`／`recovery_write_failed`；SIGTERM 后快照仍在磁盘 |

旧产物上另做了一次「5 窗口各带 4 个标签」的会话记录用例，实际 exit 1：

```text
TimeoutError: no 20× 'event=document_opened' within 60 s
```

原因是设计上只读入 active 标签，其余进 `pending_tabs` 延迟到点开时再读（`restore_tabs`：只有 active 进 `open_in_background`）。记录下来的是失败事实；该用例没有取得 footprint／CPU，也没有据此宣称通过。重建产物上按指示没有重跑这个用例。

会话恢复用例是**直接写会话记录**，不是走完 ⌘Q 的退出流程；它与空参数启动的读取路径相同，但不验证退出时的窗口顺序、保存提示或「会话在退出时写入」本身。

## 两组产物对照

| 指标 | 旧 `20d7a333` | 新 `259486e1` | 目标／上限 |
| --- | ---: | ---: | --- |
| 二进制 | 49,461,456 bytes | 49,461,456 bytes | 50／52 MB |
| 1 窗口欢迎页 footprint | 54.99 MB | 54.95 MB | 70／100 MB |
| 1 窗口 20 文档 footprint | 78.51 MB | 77.14 MB | 70／100 MB |
| 3 窗口 20 文档 footprint | 126.72 MB | 130.02 MB | 220／300 MB |
| 5 窗口 20 文档 footprint | 186.37 MB | 189.43 MB | 无单列上限 |
| 5 分钟 CPU p95 | 0.0230%（换算） | 0.0234% | ≤ 1% |

1 窗口 20 文档的 77–79 MB 高于健康检查报告里 `resources-*-json` 的 65.4–65.5 MB；3 窗口的 127–130 MB 低于那两轮的 152.65／153.39 MB。这些差异未归因，本报告不改写既有数字。

## 没有被验证的

1. 真实 macOS 鼠标／键盘事件、按键到画面 p99。
2. 中文 IME 组合输入、候选窗与光标位置。
3. VoiceOver 朗读顺序、可访问性标签。
4. 通过 ⌘Q、⌘W 或红绿灯按钮关闭窗口后的恢复／空窗口替换。
5. 窗口最小化、遮挡、后台可见时的绘制停止。
6. 20 次开关窗口、预览的缓存回收趋势，以及 4 小时回放。

## 复现

```bash
cd /tmp/zj-native-harness
python3 -I harness.py /Users/joys/work/code/ZJ/target/health-check/native-final-20261022
python3 -I cpu5min.py /Users/joys/work/code/ZJ/target/health-check/native-final-20261022
```

脚本依赖仓库里的 `tools/measure_budget.py` 与 `tools/sample_resources.py`（只读）；夹具全部在系统 `TMPDIR` 下，运行结束即被 `TemporaryDirectory` 删除。`/tmp/zj-native-harness/` 是本次的临时脚本目录，不属于产品代码，也未提交。

没有修改任何生产代码、设置或既有报告；这是本次新增的唯一文档。
