# Rust／GPUI 升级补充核验

日期：2026-10-22。用户要求继续完成未完成项。本次为有界只读核验，未下载依赖、修改锁文件或工具链，也未绕过官方页面的域名访问阻断。

## 结果

当前仍不足以安全实施「升级到最新」。项目锁定 Rust 1.99.0、GPUI Kit／base／component 0.7.1、GPUI／apple／macos 0.3.8；这些是当前项目版本，不能证明官方最新版本。

本地缓存的 `gpui-kit-0.7.1/Cargo.toml` 明确依赖 `gpui-pre =0.3.8`，三个 vendor manifest 与该快照配套。默认 Cargo registry 的六个相关包精确目录、归档及 sparse-index 检查没有找到候选 0.7.3／0.3.9 的源码或记录；这个范围有限，不能据此断言版本不存在。随后补查构建实际采用的 `target/cargo-home`，发现 GPUI 0.3.9 的索引记录，但所有候选源码及归档仍缺失；具体增量如下。

| 缓存记录 | 版本 | 缓存发布日期 | 缓存 yanked |
| --- | --- | --- | --- |
| gpui-kit | 0.7.1 | 2026-10-05T02:38:26Z | false |
| gpui-component | 0.7.1 | 2026-10-05T02:36:16Z | false |
| gpui-pre | 0.3.8 | 2026-10-05T00:43:40Z | false |

实际构建缓存补查结果：Kit／base／component 0.7.3 没有源码、归档或候选索引；GPUI／apple／macos 0.3.9 没有源码和归档，但有 sparse-index 记录，均标示 `yanked: false`、`rust_version: null`。其缓存发布日期分别为 2026-10-12T00:44:57Z、00:44:59Z、00:45:03Z。索引确认 apple 精确依赖 GPUI `=0.3.9`，macos 精确依赖 GPUI／apple `=0.3.9`。这是候选发布及内部配套的缓存证据；仍缺 Kit 0.7.3 的配套依据和实际源码，不能迁移 vendor。

这些记录仅代表缓存生成时的数据，`rust_version` 为空不能证明 MSRV。当前 Rust 安装版本沿用本任务此前实际版本命令结果；默认 rustup 目录未找到 channel manifest，不能由目录缺失推断未安装。

官方域名限定的搜索摘要提到 Kit 0.7.3 与 GPUI 0.3.9，与旧报告一致，但未取得发布正文、当前 registry 响应及固定版本 manifest。搜索摘要不足以确认当前 non-yanked 状态、checksum、MSRV 和完整兼容性。没有把候选版本写入 Cargo.lock。

## 实施前尚缺的证据

1. Rust stable channel 当前版本、日期及对应官方公告。
2. Kit 关联包的当前版本列表、non-yanked 状态、checksum 和 MSRV。
3. 固定版本的 manifest、源码和 release notes，用于迁移 vendor 补丁。
4. 升级后的功能、构建和资源验证；当前构建通过不代表新版兼容。

迁移范围沿用 [vendor/README.md](../../vendor/README.md) 与[原升级报告](2026-10-22-upgrades-and-diff-research.md)：编辑器焦点／选区／IME／行控件扩展、常亮光标、纹理闲置回收、共享图集、display link 暂停唤醒和通知生命周期均须保留。新 CPU 上限为 p95 ≤ 1%，其余资源门槛不变。

## 来源

- [项目工具链](../../rust-toolchain.toml)、[锁文件](../../Cargo.lock)、[vendor 补丁清单](../../vendor/README.md)：本地已读取证据。
- [GPUI Kit 官方 Releases](https://github.com/longbridge/gpui-kit/releases)、[官方发布页](https://gpui-kit.com/releases/)：本次搜索发现入口；未取得正文作为升级依据。
- [Rust stable channel](https://static.rust-lang.org/dist/channel-rust-stable.toml)：尚缺当前有效响应。

首次本地缓存探针因 `tomllib` 不可用失败，改用标准库 `json`／`re` 后成功；没有为核验安装工具。升级保持阻塞，现有功能修复和验证可独立推进。
