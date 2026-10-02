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
5. `ZJ_GPU_LOWMEM=0` 在启动时关闭以上全部改动，恢复上游行为，方便 A/B 对比。

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
