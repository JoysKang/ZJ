# GPUI Kit 0.7.3 与 GPUI Fast 适配评估

调研日期：2026-10-12。问题：是否应把 ZJ 从 GPUI Kit 0.7.1 升到 0.7.3，主要评估 GPUI Fast 的性能收益及低内存约束。范围是官方发布、固定版本源码和 ZJ 当前依赖／补丁；本次没有构建 Fast 候选或进行 ZJ 的 Fast A/B 测量。

## 决定

本次保留 GPUI Kit 0.7.1，暂不默认启用 GPUI Fast。Fast 的滚动优化值得单独验证，但接入条件包括迁移现有补丁、检查缓存内存以及测量 ZJ 的实际滚动负载。当前官方基准和源码检查尚不足以证明整体升级对 ZJ 更合适。

0.7.3 自身也改变了 GPUI 快照和部分 TextView 默认排版；采用默认后端仍需要迁移平台和组件补丁。因此，本次不把它视为单纯的版本号更新。

## 官方事实

- GPUI Kit 0.7.3 于 2026-10-12 发布，替代已撤回的 0.7.2，包含该版全部改动，并将默认 GPUI 快照固定到 `gpui-pre =0.3.9`。0.7.2 的独立 Release API 返回 404，未确认其独立发布日期。[0.7.3 发布说明](https://github.com/longbridge/gpui-kit/releases/tag/v0.7.3)
- GPUI Fast 0.1.7 同日发布，与 Zed `93d7456`／gpui-pre 0.3.9 对齐。查询官方 latest Release 时，仍是 0.1.7，`prerelease` 为 false；项目 README 同时明确标注实验性状态。[Fast 0.1.7](https://github.com/longbridge/gpui-fast/releases/tag/v0.1.7) · [README](https://github.com/longbridge/gpui-fast/blob/v0.1.7/README.md)
- GPUI Fast 是独立的 GPUI 分支，主要增加保留视图、滚动层和原生合成。GPUI Kit 0.7.3 通过可选的 `gpui-fast` feature 选择它；默认后端仍是 gpui-pre。Kit 会把选择传到 base、component、assets 和 platform，并切换对应的导出类型；现有应用导入路径可保留。[Kit Cargo.toml](https://github.com/longbridge/gpui-kit/blob/v0.7.3/crates/kit/Cargo.toml) · [Kit 导出](https://github.com/longbridge/gpui-kit/blob/v0.7.3/crates/kit/src/lib.rs)
- 0.7.3 减少了文本选择、Root、输入、TextView 和可调整面板在未变化帧里的状态写入，有助于 Fast 保留视图。TextView 的默认段间距从 `1rem` 改为 `0.75rem`，标题及列表等排版也有变化。[发布说明](https://github.com/longbridge/gpui-kit/releases/tag/v0.7.3)

## 性能数据的适用范围

官方 README 记录的是 Linux release 构建的每帧 CPU 时间：

| 场景 | 原基准 | Fast |
| --- | ---: | ---: |
| 未变化窗口 | 3.31 ms | 0.19 ms |
| 侧栏滚动 | 5.95 ms | 0.79 ms |
| 行情更新 | 5.06 ms | 1.35 ms |

未变化窗口的基准是在同一构建里关闭保留视图；另外两项对比 gpui-pre 0.3.7。它们不是当前 gpui-pre 0.3.9 与 Fast 0.1.7 的同条件对比，也不是 ZJ 实测。不能把这里的下降比例直接当成 ZJ 的 CPU 或内存降幅。[官方基准及条件](https://github.com/longbridge/gpui-fast/blob/v0.1.7/README.md#performance)

滚动层文档另有 Apple Silicon 聊天记录实验，说明 macOS 也有优化潜力。但保留视图、layout nodes、GPU tiles 和 overscan 都需要缓存；焦点输入、动画、频繁内容更新等场景可能绕过或降级滚动层。外部可变状态需要正确通知，避免保留过期画面。官方资料没有证明 ZJ 的总内存会下降。[滚动层机制及测量](https://github.com/longbridge/gpui-fast/blob/v0.1.7/docs/scroll-layers.md) · [保留视图的依赖与限制](https://github.com/longbridge/gpui-fast/blob/v0.1.7/docs/retained-mode.md)

ZJ 已有空闲停帧、常亮光标和资源回收。此前的测量支持这些策略显著降低空闲 CPU 和内存；Fast 的「未变化帧更便宜」与 ZJ 的「稳定空闲时停止出帧」是不同指标。预期值得测试的负载是长对话、文件列表和编辑区滚动；具体收益仍待验证。[本地资源决策与测量](../adr/0005-gpu-memory.md)

## ZJ 的具体迁移门槛

当前依赖为 gpui-kit 0.7.1；本地 patch 是 gpui-base 0.7.1、gpui-pre-apple 0.3.8 和 gpui-pre-macos 0.3.8。Fast 的包身份以及默认后端的新快照版本都不能靠原有 patch 自动转接，需要确认最后解析和运行的实现。[补丁清单](../../vendor/README.md) · [当前依赖](../../Cargo.toml)

| 当前能力 | 固定版本源码核对 | 迁移要求 |
| --- | --- | --- |
| 双缓冲与空闲 GPU 回收 | Fast Metal renderer 仍设置 3 个 drawables，resize 时分配 path textures；运行时 shader 库在 renderer 构造中编译 | 保留 ZJ 的双缓冲、延迟纹理、空闲／遮挡回收、共享图集和 shader 策略；处理新增滚动缓存的生命周期 |
| 空闲停帧与重新唤醒 | Fast 的窗口源码存在 display-link 调度，但未接入 ZJ 的闲置表面回收配套代码 | 以真实空闲帧／唤醒计数确认等价行为，验证输入、终端和 Agent 异步输出能唤醒 |
| 常亮光标 | Kit 0.7.3 仍有 500 ms 光标闪烁定时器 | 重基常亮补丁，运行现有 idle_ui 回归 |
| 行尾 blame、接受后的视口、代码块标题 | Kit 0.7.3 对应输入及文本模块未包含 ZJ 的 LineEndAnnotation、viewport intent 和 code_block_header 扩展 | 保留或迁移这些接口及实际绘制回归，不用新 Diff 组件顺带替换现有功能 |
| Agent 通知声音与授权时序 | Fast 的通知内容未设置声音；提交仍直接跟在异步授权请求后 | 保留默认声音、前台声音、等待授权、旧回调失效和撤回逻辑 |

源码证据：[Fast Metal renderer](https://github.com/longbridge/gpui-fast/blob/v0.1.7/crates/gpui_apple/src/metal_renderer.rs#L182) · [Fast 窗口调度](https://github.com/longbridge/gpui-fast/blob/v0.1.7/crates/gpui_macos/src/window.rs#L3368) · [Fast 通知](https://github.com/longbridge/gpui-fast/blob/v0.1.7/crates/gpui_macos/src/system_notifications.rs#L164) · [Kit 光标](https://github.com/longbridge/gpui-kit/blob/v0.7.3/crates/base/src/input/base/blink_cursor.rs) · [Kit 编辑器接口](https://github.com/longbridge/gpui-kit/blob/v0.7.3/crates/base/src/input/editor/mod.rs) · [Kit 文本样式](https://github.com/longbridge/gpui-kit/blob/v0.7.3/crates/base/src/text/style.rs)

## 后续采用条件

在包含当前 Solarized Dark 改动的隔离快照里迁移补丁，先确认 Cargo 最终选择的后端、版本和路径。通过常亮／停帧、blame、审阅视口、通栏背景、代码复制与定义跳转等回归，再完成全套检查。

同机器、窗口尺寸、dist 配置和文档负载下，交错测试基线与候选并复测一次；测量滚动与长对话更新的 CPU／帧耗时，同时记录 physical footprint、空闲帧和唤醒次数。接入门槛仍是二进制不超过 32 MB、单窗口不超过 100 MB、3 窗口加 20 文档不超过 300 MB、空闲 CPU p95 不超过 0.1%。超过原有依赖准入增量时需补充 ADR。

本次静态基线：aarch64 dist 二进制 29,661,520 字节；按项目约定命令得到的正常依赖树去重输出为 615 行。没有候选数据，未声明 Fast 的体积、CPU 或内存增量。当前 Cargo.toml、Cargo.lock、vendor 和已构建应用保留原版本。
