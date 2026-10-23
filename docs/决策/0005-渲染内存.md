# ADR 0005：给 GPUI 的 macOS 渲染器打补丁，降低每个窗口的显存

日期：2026-10-02。状态：已采纳，待 Mac 实测确认。

## 背景

用户在 M 系列 Retina Mac 上测了第 6 批（`footprint`）：

| | 4 个窗口 | 1 个窗口 | 每多一个窗口 |
| --- | ---: | ---: | ---: |
| 合计 | 532 MB（峰值 718 MB）| 209 MB | 约 108 MB |
| IOSurface | 292 MB（30 块）| 88 MB（23 块）| 68 MB |
| Owned unmapped (graphics) | 67 MB | 8.7 MB | 19.4 MB |
| IOAccelerator (graphics) | 31 MB | 14 MB | 5.7 MB |
| Owned unmapped | 32 MB | 16 MB | 5.3 MB |
| Malloc Small | 87 MB | 67 MB | 6.7 MB |

多出来的几乎都是 GPU 表面，对上了 GPUI Metal 渲染器（`gpui-pre-apple 0.3.7`）的做法：

- 每个窗口一个 `CAMetalLayer`，`maximumDrawableCount = 3`。drawable 由 IOSurface 支撑，每块 `宽 × 高 × 4` 字节（设备像素，BGRA8）。
- 每次改变窗口大小，都按窗口大小新建一张 Private 存储的路径中间纹理（同样 `宽 × 高 × 4`）。Apple GPU 上 4× MSAA 纹理是 memoryless，不占内存。这张纹理只在画矢量路径（`paint_path`）时用到；本应用只有带箭头的弹出框和虚线分隔符会画路径，大多数帧用不到它。
- 每个窗口一套 sprite 图集：单色字形 1024² A8（1 MiB），彩色图标和图片 1024² BGRA8（4 MiB），装满了再加。所有窗口里的字形和图标都一样，却要各存一份。

设一块表面为 S = 宽 × 高 × 4 × 1。窗口 1470 × 827 pt、2× 缩放时 S = 2940 × 1654 × 4 ≈ 19.4 MB，正好等于上表中每个窗口的 Owned unmapped (graphics)，也就是路径中间纹理；IOSurface 每窗口 68 MB ≈ 3 S + 约 10 MB（标题栏等系统图层）。全屏的 16 英寸 MacBook Pro（1728 × 1117 pt）上 S = 3456 × 2234 × 4 ≈ 30.9 MB。

## 决定

把 `gpui-pre-apple` 和 `gpui-pre-macos` 放进 `vendor/`，用 `[patch.crates-io]` 指向，补丁尽量小，每处都标 `ZJ patch`（见 `vendor/README.md`）：

1. **drawable 3 → 2**。保留 `displaySyncEnabled`。双缓冲时，如果一帧超过一个刷新周期，下一次 `nextDrawable` 会等显示器，而不是再排一帧，所以延迟相同或更低；只在本来就掉帧时吞吐更低。编辑器一帧只要几毫秒，60 / 120 Hz 下都有余量。不会撕裂（仍然 vsync）。
2. **路径中间纹理按需分配**：第一帧画路径时按这一帧的大小分配，连续 10 秒没有路径就释放（在下一帧检查）。缩放窗口时只丢掉旧纹理，不再每次重建。
3. **看不见时归还表面**：窗口被完全遮挡、在别的桌面、最小化或应用被隐藏时（`NSWindowOcclusionState` 不含 Visible，AppKit 对这几种情况都发同一个通知），把 layer 的 drawableSize 缩成 1 × 1，层会丢掉旧尺寸的 drawable 池，同时释放路径纹理，期间不绘制。最小化和隐藏应用时，再把 layer 的 contents 清空，连最后一帧也释放；只是被遮挡时保留最后一帧，免得切回来时先闪一下空白。重新可见时恢复尺寸，并让下一个 display-link 帧强制重新呈现上一帧的场景（`require_presentation`）。
4. **所有窗口共用一个 sprite 图集**（同一 GPU 设备）。图集用 Weak 持有，最后一个窗口关闭后释放。GPUI 删除图片时会逐个窗口调用 `remove`，按 key 删除是幂等的，共用后不会出问题；字形和图标只栅格化一次。
5. **着色器库只编译一次**：GPUI Kit 固定打开 `runtime_shaders`（不需要 Xcode 的 metal 编译器），上游每开一个窗口就把整份 Metal 源码重新编译一遍，现在每个进程只编译一次。省下每个新窗口的编译时间，以及编译器在进程内的分配。
6. `ZJ_GPU_LOWMEM=0` 在启动时关闭以上全部改动，恢复上游行为，方便 A/B 对比。

## 预计节省（S 为一块窗口大小的表面）

| 项目 | 每个可见窗口 | 每个看不见的窗口 |
| --- | ---: | ---: |
| drawable | −1 S（3 → 2）| 只留已呈现的一块（遮挡）或 0（最小化 / 隐藏）：−2 S 或 −3 S |
| 路径中间纹理 | −1 S（不画路径时）| −1 S |
| sprite 图集 | 第二个窗口起 −5 MiB 以上（CJK 字形多时更多）| 同左 |

按上表的窗口大小（S ≈ 19.4 MB）：

- 1 个窗口：约 −39 MB，209 → 约 170 MB。
- 4 个窗口全部可见：约 −39 − 3 × 44 ≈ −171 MB，532 → 约 360 MB。
- 4 个窗口、其中 3 个被遮挡：每个被遮挡的窗口再少 1 S，约 −230 MB，532 → 约 300 MB。

全屏窗口（S ≈ 31 MB）省得更多：每个可见窗口约 −62 MB。

峰值（718 MB）主要来自缩放窗口时旧表面还没释放、新表面已经分配。路径纹理不再随缩放重建，峰值也会下降。

## Malloc Small（1 个窗口 67 MB）

在 Linux 上用 heaptrack 看同类工作区（15 个仓库、2,229 个文件、1 个窗口，空闲 40 秒）的常驻堆：合计 54 MB，其中 46.2 MB 是软件渲染（lavapipe / LLVM / wgpu），只在 Linux 上有。其余约 8 MB：应用自身 2.4 MB、libc / std 2.1 MB、tree-sitter 1.3 MB、字体与排版 0.9 MB、GPUI 窗口与布局 0.9 MB、快速打开索引 0.1 MB。可见 Mac 上的 Malloc Small 大头不在应用数据里，更可能是系统框架：Metal 的运行时着色器编译（已改为只编译一次）、CoreText 的字体与中文回退缓存、AppKit。应用侧没有便宜可省的大项，本批不再动。

在 Mac 上定位的办法：`MallocStackLogging=1` 启动后执行 `heap $(pgrep -x ZJ) -s | head -60` 看各类大小，再用 `malloc_history $(pgrep -x ZJ) -allBySize | head -80` 看调用栈；或者用 Instruments 的 Allocations 模板按 Library 分组。

## 风险

- 双缓冲下，GPU 一旦卡住，主线程会在 `nextDrawable` 上多等最多一个刷新周期（`allowsNextDrawableTimeout = NO` 不变）。
- 缩小 drawableSize 能让 layer 释放 drawable 池，这是 Core Animation 的已知行为，但文档没有承诺；要以实测为准。
- 被遮挡时如果截图或屏幕共享这个窗口，看到的是最后一帧，直到它重新可见。最小化 / 隐藏后，Dock 缩略图和窗口恢复动画使用系统在最小化时保存的快照，不依赖 layer contents；如果实测发现恢复时闪白，可以只保留“缩小 drawable”这一步。
- Linux 上无法运行这些代码。只在 `aarch64-apple-darwin` 上做了类型检查和 clippy，纯逻辑部分有单元测试。

## 测量方法（请在 Mac 上执行）

```sh
cargo build --profile dist --locked && tools/package.sh
open target/ZJ.app            # 或直接运行 target/dist/workspace-editor
```

每种情况空闲 30 秒后记录 `footprint -p $(pgrep -x ZJ)` 的合计和 IOSurface、Owned unmapped (graphics)、IOAccelerator (graphics)、Malloc Small 几行。先用 `ZJ_GPU_LOWMEM=0` 启动测一遍（上游行为），再不带变量测一遍：

1. 1 个窗口，窗口模式（约 1470 × 827 pt）。
2. 1 个窗口，全屏。
3. 4 个窗口，全部并排可见。
4. 4 个窗口，叠在一起（3 个被完全遮挡）。
5. 4 个窗口，最小化其中 3 个；再 ⌘H 隐藏应用。
6. 4 个窗口来回拖动缩放 10 秒后的峰值（`footprint` 的 peak）。

同时开着

```sh
log stream --predicate 'process == "ZJ"' --level error
```

应该没有输出（尤其不应出现 `failed to retrieve next drawable`）。还要目测：切换窗口、从 Dock 恢复、切换桌面、⌘H 后再切回时，窗口内容立即正确、没有闪白；打开带箭头的弹出框（会画路径）后正常显示，10 秒后再看 Owned unmapped (graphics) 是否回落。

## 更新（2026-10-05）：空闲时归还备用 drawable

实测（1 个窗口，3456 × 2168 设备像素，接近全屏）：footprint 97–105 MB，其中 IOSurface 58 MB，是两块 29.2 MB 的 `CAMetalLayer Display Drawable`（`vmmap -v` 确认，另有一块 16 KB）；窗口缩小后 64 MB / 23 MB；`ZJ_GPU_LOWMEM=0` 时同尺寸 73 MB / 35 MB。除去 drawable，基线约 41 MB。

编辑器大部分时间是静止的，而双缓冲的第二块 drawable 只在连续出帧时有用。决定：可见窗口 3 秒没有新帧时，把 layer 的 drawableSize 缩到 1 × 1 再恢复，清空 drawable 池；已呈现的一帧留在 layer contents 里，仍在屏幕上。下一帧再取新的 drawable。预计全屏时空闲 footprint −1 S（约 29 MB），回到约 70 MB。

实测（同一窗口尺寸，空闲 10 秒）：footprint 97–105 MB → 67 MB，IOSurface 58 MB → 29 MB，`vmmap -v` 只剩一块 Display Drawable。停顿后打字、滚动、切换标签和桌面时没有看到闪烁或延迟，`log stream` 没有错误。

风险：恢复后第一帧要分配一块新的 IOSurface（预计 1–2 ms），落在停顿 3 秒后的第一次按键上；缩放 drawableSize 期间是否会让屏幕上的内容闪一下，没有文档保证，需要实测。验证：空闲 5 秒后 `vmmap -v $(pgrep -x ZJ) | grep 'Display Drawable'` 应只剩一块；停顿后连续打字、滚动、切换标签时没有闪烁或白帧；`log stream` 没有 `failed to retrieve next drawable`。不行就用 `ZJ_GPU_LOWMEM=0` 对比，或去掉 `step` 里的调用。

## 更新（2026-10-06）：编辑器光标常亮、空闲时停掉 display link

`tools/measure_budget.py --breakdown` 的实测显示，窗口在前台、编辑器有焦点时，Kit 的光标每 500 ms 闪一次并整窗重画：空闲 CPU p95 约 3%，渲染器等不到 3 秒无新帧，1 个窗口约 194 MB，3 窗口 + 20 文档 336 MB。`vendor/gpui-base` 打补丁让光标常亮后，分别降到 78 MB 和 190 MB，但空闲 CPU p95 仍有约 1.2%。

剩下的来自 display link：窗口可见时它每次刷新（ProMotion 上每秒 120 次）都回调 `step`，即使什么都不画。GPUI 支持按需出帧的平台（`PlatformWindow::frame_waker`，窗口变脏、`on_next_frame` 时调用），macOS 层原来没有实现。现在空闲 3 秒、备用 drawable 已归还后停掉 display link，`frame_waker` 再启动它。预计空闲 CPU 接近 0；风险是漏掉唤醒导致窗口不刷新，需要实测（输入、悬停、窗口切换、终端输出、Agent 回复、Git 刷新）。`ZJ_GPU_LOWMEM=0` 恢复原行为。

旧的 `tools/sample_resources.py` 把 `proc_pid_rusage` 的 CPU 时间当纳秒用；`measure_budget.py` 按 `mach_timebase_info` 换算（Apple Silicon 上约 ×41.7），之前记录的约 0.02% 空闲 CPU 应按约 0.8% 理解。

实测（停掉 display link、logo 光标常亮后）：1 个窗口打开文件空闲 76.7 MB、CPU p95 0.016%；欢迎页 53.9 MB、0.024%；3 窗口 + 20 文档 194.8 MB；3 窗口不开文件 113.7 MB。打开文件时仍有约 20 MB 的 Owned unmapped (graphics)，是路径中间纹理：它的 10 秒闲置检查只在画帧时执行，空闲窗口不再画帧，就一直留着。现在空闲归还 drawable 时一并释放。

