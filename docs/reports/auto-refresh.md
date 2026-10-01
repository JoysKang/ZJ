# 工作区自动刷新：依赖与资源对比

记录日期：2026 年 10 月 1 日，Asia/Shanghai。基准 commit 为 `e2f0613fa5c95d48c0ee71152cb03620f036d777`；修改后是未提交的工作区实现，该批源码清单见 [auto-refresh-source-manifest.json](auto-refresh-source-manifest.json)。本报告是依赖准入的短测记录，不是 G1 报告。

## 构建与依赖

两版均以 ARM64 `--profile dist --locked --offline` 构建。GUI 版本和 feature 未变；新增直接依赖 notify 7.0.0、async-channel 2.5.0 已存在于 GPUI Kit 依赖树，Cargo.lock 仅增加 app 的两条依赖名称，没有新增包或 feature。

规定命令在两版锁文件下各执行一次：

```sh
CARGO_HOME="$PWD/target/cargo-home" cargo tree -e normal --target aarch64-apple-darwin --prefix none --locked --offline | sort -u | wc -l
```

| 指标 | 修改前 | 修改后 | 差值 |
| --- | --- | --- | --- |
| cargo tree 去重行数 | 567 | 568 | +1 |
| dist 二进制字节 | 16,953,552 | 17,055,088 | +101,536 |
| footprint 中位数字节 | 93,996,184 | 89,539,712 | −4,456,472 |
| footprint p95 / 观测最大值 | 93,996,184 | 89,539,712 | −4,456,472 |
| 空闲 CPU p95，单核口径 | 0.018465% | 0.000363% | −0.018102 个百分点 |

去重行数包含直接依赖重复边的打印形式，不等于独立 crate 数。二进制增量低于 200 KB，未观测到 footprint 增加超过 5 MB，不触发本次依赖决策记录阈值。短测中的 footprint 下降可能受进程初始化与系统状态影响，不能据此宣称节省内存。

修改前二进制 SHA-256：`8efffaea5ec32dca9d4d3feaf9a17c678994a0793c21a4e0a5508a2cb4ae070e`。

修改后二进制及本地 `.app` 中二进制 SHA-256：`d0c907e6569e42abf53c9bb9cd0c345724bd978c1f759658b975df5cf32df202`。

## 采样条件与限制

机器沿用开发记录中的 M5 Pro / ARM64 / macOS 环境。两次均为一个 1280 × 800 逻辑像素窗口，使用同一个三文件临时 Git 工作区 `/private/tmp/zj-layout-ui`、欢迎页、零打开文档。前后应用分别单独运行；在修改夹具或打开文档之前采样。工具对应用与子进程同一时点的 physical footprint 求和，目标间隔 200 ms。

| 采样信息 | 修改前 | 修改后 |
| --- | --- | --- |
| 应用 PID | 68516 | 73592 |
| 样本数 | 144 | 147 |
| 观测时长 | 29.829 秒 | 29.883 秒 |
| 缺失子进程样本 | 0 | 0 |
| 采样自身耗时 p95 | 28.599 ms | 26.736 ms |
| 实际间隔 p95 | 210.144 ms | 205.119 ms |

原始记录：[修改前 CSV](auto-refresh-before.csv)、[修改前 JSON](auto-refresh-before.json)、[修改后 CSV](auto-refresh-after.csv)、[修改后 JSON](auto-refresh-after.json)。

当前 17.06 MB 二进制低于 25 MB 目标；89.54 MB footprint 高于 70 MB 目标、低于 100 MB 上限。CPU 短测低于 p95 0.1% 上限。上述比较不等于正式门禁通过：未按两分钟稳定、十分钟稳态、三轮重复执行；未包含正常 N / R、三窗口与 20 文档、编辑操作峰值、冷启动、GPU 或窗口回收。采样可能漏掉短命子进程。G1 保持未通过。
