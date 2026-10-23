# Diff 与 Markdown 原生交互验收（2026-10-23）

本轮使用主工作区冻结产物 `b46b04b3ae8136ab2f5efe5b8745dd4d2428e3edab4c17fb7dd26a1492ace12d`。仅操作新建的临时 Git 仓库，设置、会话和历史目录隔离；每次输入前核对前台 PID。未修改产品、构建、调用外部服务或截图。

## 实际结果

| 场景 | 结果 | 证据边界 |
| --- | --- | --- |
| Markdown 源码／预览切换 | 通过 | ⇧⌘V 切到源码，AXTextArea 保留标题、表格、Rust 代码、本地图片和相对链接标记；切回后恢复「显示源码」按钮 |
| 同文件的暂存／未暂存入口 | 通过 | 两个命名入口都存在；暂存侧出现「取消暂存块」，工作区侧出现「还原块」「暂存块」；未执行这些写操作 |
| 两个 Diff 的正文版本 | 未验证 | AX 不提供正文，无法从界面核对 `BASE_ONLY → STAGED_ONLY` 和 `STAGED_ONLY → WORKTREE_ONLY` |
| 外部修改自动刷新 | 部分通过 | 修改临时文件后出现 `watch_flush` 与 generation=3 的 `refresh_finished`；无法核对刷新后的界面正文 |
| Markdown 表格／代码／图片及相对链接 | 未验证 | AX 未提供渲染内容或链接入口；源码完整不能代替视觉与点击验收 |
| annotated tag 多段中文详情 | 未验证 | 临时标签已创建，但通过命名控件尝试打开 Git 图未成功，未进入标签详情 |

末尾断言实际输出：

```text
PASS: captured source markers, distinct Diff controls, unchanged HEAD/index, synthetic external write
These checks do not verify rendered Diff or Markdown content.
```

Git HEAD 和 index 保持夹具初始内容，仅临时工作区文件被写为外部刷新标记。未对真实仓库做暂存、提交、标签或重置。

## 证据与清理

原始报告、AX 快照、合成夹具状态、驱动和日志已复制到 `target/health-check/daytime-followup-20261023/native-content-validation/`，共 16 个条目。测试 PID 44137 已退出，临时运行夹具已删除，GUI 随后交给本地 Agent 面板验收。

没有屏幕录制权限，未绕过权限获取图像，也未猜测不可观察控件的坐标。AX 未暴露内容只是证据限制，不能据此报告产品渲染错误；上述未验证项继续保留。
