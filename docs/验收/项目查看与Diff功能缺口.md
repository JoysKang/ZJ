# ZJ 功能与交互核查

日期：2026-11-04。目标：ZJ 作为轻量的项目查看与 Diff 工具，保留顺手的基础编辑能力，能够配合外部编程工具完成浏览、编辑、检查改动和必要的 Git 操作。

核查同时覆盖功能是否存在和已有功能是否好用，包含搜索、查找替换、快捷键、焦点、选区、标签、文件操作、保存、恢复、代码导航和 Diff。每项尽量说明触发步骤、实际行为、通行交互预期与证据，不能只统计菜单数量。

本报告按本地源码、现有功能入口、无头界面键盘测试和临时本地 Git 仓库验证整理。比较基线是成熟桌面编辑器常见的工作流程，未联网核对其他产品的最新版本，也未把完整 IDE 的功能数量作为目标。本报告保留最初发现时的行为与证据；用户随后要求全部实施，最新实现状态、专项结果和仍待原生验收的项目以[功能补齐进度](../功能补齐进度.md)为准，不再用本报告的历史状态判断当前源码。

## 当前完整界面与交互审阅（2026-11-04）

本节对应用户「结合 unslop 与 GPUI skill 优化完整界面、交互」的要求，范围是当前应用的全部主要界面与跨区域行为。以下旧章节保留最初核查时的状态；本节和[功能补齐进度](../功能补齐进度.md)用于判断当前实现。源码基线是本轮开始前的 116 个文件副本，保留此前所有未提交修改；界面实施完成后，已按用户后续「打包、替换」要求生成新包并替换已安装应用，具体哈希与验证见本节末尾。

### 联合设计方向

- **Product：** 快速浏览项目、连续阅读代码、理解和核查改动，必要时执行既有编辑及 Git 操作。
- **Character：** 安静的导航、紧凑的操作、清晰的内容层级；减少重复背景、圆形计数和过重的状态块。
- **World：** 保留 Solarized Dark／Nord Light、默认正文 13、400 px 可拖动侧栏及现有资源预算。
- **Moments：** 正文、实际变更与错误承担主要视觉重量；普通成功先给一句结果，诊断按需展开。
- **Signature：** 仓库名称、分支和提交区对齐；同类操作使用有名称、有键盘行为的原生按钮，切换和关闭后焦点回到可见对象。

按联合流程的九项顺序处理：间距统一 SCM 两层标题和输入缩进；颜色复用主题语义色；图标去掉重复装饰并补操作名称；文案分离摘要和原始详情；字体减轻导航计数；布局减少嵌套表面、限制长分支宽度；细节补原生焦点和选择复制；没有增加插画或其他视觉素材；没有增加常驻动效或刷新定时器。参考图用于层级和节奏，不复制网页 dashboard。

### 覆盖范围与结论

| 区域 | 本轮检查与处理 | 验证边界 |
| --- | --- | --- |
| 工作台、欢迎页、标题栏和状态栏 | 命令中心、分支、同步及行尾操作使用原生按钮；标题栏图标具名，长分支限宽，非动作状态不暗示点击 | 无头布局、现有命令与窗口回归；实机视觉待验收 |
| 侧栏导航 | 文件／变更／搜索使用原生可聚焦 Tab；保留默认 400 px 和拖动范围，减轻计数、背景及字重 | 实际 click → Tab → Space 切换回归 |
| 文件树及菜单 | 新建时展开树并滚到命名行；标题图标补名称；检查已有选择、多选、改名、移动与菜单路径 | 折叠／展开状态下真实右键新建及输入焦点；文件操作沿用临时夹具 |
| 标签与关闭 | 文本、图片、Diff、固定比较、Git 图和受限查看共用剩余标签选择入口；标签栏依据真实标签集合存在；关闭按钮具名且可键盘激活 | 最后文本关闭后保留图片；全部关闭不重新激活固定 Diff；已有取消和批量关闭回归 |
| 文本、历史快照和受限查看 | 历史全文切回后聚焦实际快照编辑器，大文件切回后聚焦查看器 | 真实切换／焦点断言；保存、恢复和只读保护保留 |
| Markdown | 检查源码／预览切换、分块编辑、本地图片与链接；工作区外文档的相对链接使用其实际文件归属 | 真实异步打开同目录文档；远程图片仍不加载 |
| 图片 | 适应窗口后的放大／缩小从实际画布比例计算；保留显式 100%、解码预算、刷新与关闭 | 2000 × 1000 完整 PNG，适应比例约 50% 时缩小到约 40%，同时验证放大 |
| 文内查找与替换 | 只读文档有非空查询时请求替换，聚焦可见查询框并保持只读 | 焦点、查找结果和缓冲区／磁盘不变断言 |
| 全文搜索与替换 | 旧替换完成不回填新查询结果；磁盘写入、文档刷新和撤销仍完成，必要时重查当前查询 | 完成屏障交错 A 写盘 → B 搜索完成 → A 回调，验证结果和撤销 |
| 快速打开、命令、跳转 | 检查关闭、恢复、查询输入和目标选择；首次 Shift+F12 等索引，复用 F12 等待逻辑，离开源编辑器使请求失效 | 首次跨文件引用、取消和标签切换后迟到结果不抢焦点；仍有 10 秒索引等待上限 |
| Diff 与比较 | 布局切换清除旧选区、拖动及光标定位，避免复制不可见旧选区；检查字符／跨行复制、只读、固定比较与历史 | 切换后直接 ⌘C 保留剪贴板，再全选复制可见侧；保留现有比较和写入保护测试 |
| Git、提交与操作反馈 | 仓库标题分为名称／操作、分支／同步两层；普通结果一行显示，详情默认收起，错误默认展开并保留失败确认及刷新入口 | 37 项 SCM 专项已通过；详情拖选与 ⌘C、整段复制、关闭结果后隐藏干净仓库 |
| 终端与输入焦点 | 已有分组和拆分窗格增加前后切换 Actions、快捷键和命令项；分组使用原生 Tab 按钮 | 真实本地 `/bin/sh`，切换复用进程、恢复各组窗格；4 项终端专项已通过 |
| 弹窗、菜单及跨区域状态 | 检查确认、取消、恢复焦点、异步读取／查询迟到和关闭衔接；本轮不取消既有数据保护 | 全量测试及两阶段复核结果补记于下；系统面板和 VoiceOver 待验收 |

### 已复现问题与修复证据

第一阶段完整应用审阅提出 12 项：混合标签关闭、历史／大文件焦点、适应图片缩放、Diff 隐藏选区、外部 Markdown 相对链接、标签关闭语义、搜索替换迟到、折叠树新建、只读替换焦点、首次引用查询、终端键盘切换、文件树按钮名称。前 11 项涉及行为／原生控件，第 12 项为图标名称遗漏。逐项核对源码，并给行为修复增加或加强回归；不是仅对本轮新增功能做审阅。

真实红测包括：图片适应比例 `0.498` 时缩小得到 `0.8`；切换 Diff 布局后复制得到旧 `x =`；只读查找焦点断言失败；折叠树新建输入未揭示；历史全文切回焦点错误；最后文本关闭后未返回图片；外部 Markdown 链接报「链接目标位于工作区之外，未读取」；首次引用遗漏其他文件；旧替换回调覆盖查询 B；终端 ⌃PageUp 后仍在原组。对应修复后专项均已通过。标签关闭和导航的原生控件语义也有真实控件断言。

统一关闭入口另补回归，捕获「全部关闭」过程中固定比较被重新激活的问题；批量关闭先移除固定比较，再关闭其他视图，回归已通过。普通 SCM 结果原先高 100 px，新回归要求收起时不超过 24 px，详情仍可展开、拖选、复制与关闭。

### 第二阶段独立复核

`code-review` 的两轴分别审查本轮快照补丁和实际调用链，没有把此前整个未提交工作区当作本轮修改。

- **Standards：** 0 项硬性标准违例，1 项 Nit 判断项：成功、进行中和失败文案分散在两个操作枚举映射中。已合并到一次 UI 层映射，避免同类文案分别维护。
- **Spec：** 1 项 Important、1 项 Nit。Important 是保留固定比较 A 并关闭其他标签时，丢弃唯一脏文本可能先激活 B，导致 A 被关掉；Nit 是键盘关闭后台标签后焦点没有回到可见内容。两项都以真实确认／键盘路径复现后修复：批量关闭复用版本化批准，重新核对后先恢复保留视图再移除文档；标签关闭按钮统一先归还内容焦点。标签专项 12 项及图片／混合标签专项 6 项通过。主线程重新追踪批准、移除和后继激活路径，不扩大复审到此前未改源码。

### 最终检查与未验收范围

最终 `GIT_ALLOW_PROTOCOL=file CARGO_HOME=target/cargo-home cargo test --workspace --offline --locked` 输出 **504 passed、0 failed、3 ignored**：应用 456、core 2、Git 单元 15、Git 集成 31；应用全量耗时 102.00 秒。比本轮开始前增加 13 项测试，既有专项另补焦点／选区断言。当前专项结果均为全量的子集，不累加。fmt、差异空白、Git 超时／父进程退出／取消检查通过；新增行空白和更新文档 156 个本地链接检查通过。严格 Clippy 首次指出测试中多余的 `as_deref()`，去掉后 `cargo clippy --workspace --all-targets --offline --locked -- -D warnings` exit 0；fmt 再查 exit 0，受影响图片／混合标签专项再次 **6 passed、0 failed**。

首次全量的应用测试 454 项通过、3 项 ignored；Git 子模块克隆夹具因默认拒绝本地 file 传输而失败。显式设置 `GIT_ALLOW_PROTOCOL=file` 后该项通过，最终全量使用相同的仅本地传输环境，未修改生产 Git 策略或用户配置。第三方 `block 0.1.6` 的未来兼容提示保留。

本轮没有当前原生窗口截图，未完成亮／暗主题的屏幕观感、真实键鼠、IME、VoiceOver、系统文件面板或资源预算复测。历史辅助功能／屏幕权限阻塞记录见[原生验收](原生验收.md)，本轮不绕过权限；无头控件和布局断言不等于视觉验收通过。没有新增依赖、连接真实远端、提交、推送或重启用户应用。

后续用户授权打包替换后，离线 dist 构建 exit 0；应用元数据及 arm64 架构检查通过。新 `target/ZJ.app` 与已安装 `/Applications/ZJ.app` 全部 3 个包文件一致，可执行文件与 dist 的 SHA-256 均为 `6c834a357e78c1fc6c44e45f37335fe4856f09e78f3ff00192ee3675afd77e1d`，大小 **47,269,536 bytes**，比上一版增加 **16,672 bytes**。安装以原子替换可执行文件完成，没有关闭或重启当前实例；本次替换不代表完成原生视觉验收或资源复测。

## 结论与建议顺序（最初核查记录）

优先保证搜索与替换的目标正确、保存不覆盖外部内容、焦点和快捷键可预测，再补常用查看能力。建议顺序：搜索／替换模式与键盘导航、保存和另存为一致性 → 推送结果与首次发布分支 → 图片查看 → 标签与文件树交互 → Diff 内查找、任意文件及分支比较、单文件历史。搜索旧目标、已删除文件重建后的保存冲突，以及 U02／U04／U05 的搜索入口和替换模式问题已修复。后续已补齐 U03／U08／U13／U14／U15 的搜索导航、侧栏焦点、恢复标签和另存为缩进，以及 U11／U17 的文件树键盘导航与原生编辑菜单；U07 已修复打开文档的最近激活排序，已关闭文件的完整最近历史仍未实现。其余改进尚未实施。

状态含义：

- **缺失**：已核查相关界面入口和服务接口，没有完整的产品流程。
- **受限**：已有能力，但存在明确的支持范围或工作流程限制。
- **待复现**：用户报告了异常，当前验证没有复现，不能作为已确认缺陷。

优先级含义：P0 保证操作目标、数据保存和 Git 结果可靠；P1 补齐日常浏览、编辑和比较的主要流程；P2 根据实际使用需求补充。

## 搜索与常见交互核查

重新调用搜索时，合理的基础行为是全选原查询，让新输入直接替换；直接清空不是唯一方案。通过鼠标返回输入框继续编辑原查询，也不应一律强制清空。

| 编号 | 优先级与状态 | 触发步骤与当前行为 | 期望与最小改进 | 证据 |
| --- | --- | --- | --- | --- |
| U01 | P1，用户报告待复现 | 文内或全文搜索一次后，再按对应快捷键并输入新查询。当前无头测试覆盖焦点仍在搜索框、先回到编辑区两种情况，均替换原查询，未出现追加 | 保留用户报告；继续核对其他入口、实际应用构建与输入法路径，不能仅因有 `select_all` 就宣称问题不存在，也不能把未复现写成已修复 | [`find_ui_tests.rs`](../../crates/app/src/workbench/find_ui_tests.rs)：`repeated_find_shortcuts_replace_the_previous_query`；[`find_widget.rs`](../../crates/app/src/workbench/find_widget.rs)：`open_find`；[`search_view.rs`](../../crates/app/src/workbench/search_view.rs)：`find_in_files` |
| U02 | P1，已复现并修复 | 已有全文查询，切到文件树再点侧栏「搜索」后输入新查询，修复前实际得到 `oldnew` | 侧栏与快捷键现共用 `open_search`，聚焦并全选旧查询，新输入直接替换；直接点击查询框仍可继续编辑旧查询。侧栏入口保留旧查询，不从编辑器选区另取文字 | [`sidebar.rs`](../../crates/app/src/workbench/sidebar.rs)：`activity_item`；[`search_view.rs`](../../crates/app/src/workbench/search_view.rs)：`open_search`；[`search_ui_tests.rs`](../../crates/app/src/workbench/search_ui_tests.rs)：`search_interaction_sidebar_replaces_the_previous_query` |
| U03 | P1，已补齐并验证 | 原先结果列表只有鼠标操作；回归复现查询框按 ↓ 不进入结果 | 查询框 ↓ 进入列表，↑↓ 选择，←→ 折叠／展开或转到父项／子项，Enter 打开结果，Esc 返回查询框。选中项按路径和行号保留，移除前面的文件不改变原目标；新查询清除旧选中项 | [`search_view.rs`](../../crates/app/src/workbench/search_view.rs)：`key_bindings`、`SearchSelection`、`activate_search_result`；[`search_ui_tests.rs`](../../crates/app/src/workbench/search_ui_tests.rs)：键盘导航与目标稳定性回归 |
| U04 | P1，已复现并修复 | 保留非空旧替换词后调用普通全文搜索，修复前点击结果实际打开了替换 Diff | 新增明确的展开／收起替换按钮，以 `ReplaceState.expanded` 判断模式。重新调用普通搜索会收起替换，点击结果打开源码；保留旧替换词供下次展开使用，模式切换后聚焦可见输入框 | [`search_replace.rs`](../../crates/app/src/workbench/search_replace.rs)：`ReplaceState.expanded`；[`search_view.rs`](../../crates/app/src/workbench/search_view.rs)：`open_search`、`render_search`、`search_row`；[`search_ui_tests.rs`](../../crates/app/src/workbench/search_ui_tests.rs)：普通搜索打开源码与模式切换测试 |
| U05 | P1，已补齐并验证 | 原先空替换词不能进入替换预览，也没有逐行删除按钮；批量替换可以传空字符串，但缺少明确模式 | 显式展开替换后，空字符串现在显示删除式预览，支持逐行、整文件和全部删除匹配；批量操作保留范围确认。回归实际点击逐行替换，仅删除选中行命中，并验证撤销恢复文件 | [`search_replace.rs`](../../crates/app/src/workbench/search_replace.rs)：`replace_preview`、`replace_line`、`undo_replace`；[`search_view.rs`](../../crates/app/src/workbench/search_view.rs)：`search_row`；[`search_ui_tests.rs`](../../crates/app/src/workbench/search_ui_tests.rs)：`search_interaction_empty_replacement_deletes_only_the_chosen_line` |
| U06 | P2，静态确认缺失 | 切换查询 A、B 后想找回 A。文内／全文查询与替换没有独立的历史入口，不能方便复用此前输入 | 提供有限条数的查询／替换历史和键盘调用，不必先做跨设备或永久保存 | [`find_widget.rs`](../../crates/app/src/workbench/find_widget.rs)：`FindState`；[`search_view.rs`](../../crates/app/src/workbench/search_view.rs)：`SearchState`；普通输入撤销不等同于查询历史 |
| U07 | P2，激活排序已修复；关闭历史仍受限 | 修复前打开 A、B、C 再切回 A，空查询仍显示 C、B、A；回归复现后改为 A、C、B。当前列表只覆盖已打开文件，不包含 Untitled 或已关闭文件的完整最近历史 | 以当前文档 ID 记录最近激活顺序，受现有文档数量上限约束；关闭清理、另存为／移动跟随路径，不改变标签顺序。关闭 Diff／Git 图／大文件查看后返回文档也记录激活；最近关闭栈独立保留 | [`quick_open.rs`](../../crates/app/src/workbench/quick_open.rs)：`update_quick_open`；[`workbench.rs`](../../crates/app/src/workbench.rs)：`recent_documents`、`show_pane`、`remove_document`；[`commands_ui_tests.rs`](../../crates/app/src/workbench/commands_ui_tests.rs)：`quick_open_orders_open_files_by_activation_without_reordering_tabs` |
| U08 | P1，无头复现并修复 | 回归复现隐藏侧栏后搜索输入框仍持有键盘焦点 | 侧栏根节点跟踪其子区域焦点。隐藏时只有侧栏持有焦点才交还当前编辑区；原焦点在其他区域时保留，重新显示不抢焦点。原生输入法路径仍待实机验证 | [`workbench.rs`](../../crates/app/src/workbench.rs)：`toggle_sidebar`；[`sidebar.rs`](../../crates/app/src/workbench/sidebar.rs)：侧栏 `track_focus`；[`search_ui_tests.rs`](../../crates/app/src/workbench/search_ui_tests.rs)：`hiding_the_sidebar_returns_focus_only_when_it_owned_focus` |

**U09：P0，已复现并在本轮修复。** 先搜索有效查询，再改为无效正则表达式时，旧结果、旧计数和上一查询的替换匹配器没有失效。结果列表与替换操作仍可能针对上一条查询，界面却已显示新查询的错误。新查询等待后台结果期间也有同类状态混用窗口。

最小复现测试 `changing_to_an_invalid_query_cannot_keep_old_replace_targets` 首次运行输出 `0 passed; 1 failed`，失败断言为 `old search results remain actionable for a different, invalid query`。修复在 [`search_view.rs`](../../crates/app/src/workbench/search_view.rs) 的 `schedule_search` 统一入口完成：每次新查询开始时，先清空旧结果、计数和匹配器，再校验和执行新查询；已有 generation 校验继续阻止旧后台任务覆盖新结果。测试随后输出 `cargo test: 1 passed, 368 filtered out (1 suite, 0.05s)`，回归保留在 [`search_ui_tests.rs`](../../crates/app/src/workbench/search_ui_tests.rs)。

重复搜索的实际验证命令：

```sh
CARGO_HOME="$PWD/target/cargo-home" cargo test -p workspace-editor --release \
  --offline --locked repeated_find_shortcuts_replace_the_previous_query -- --nocapture
```

实际输出：`cargo test: 1 passed, 367 filtered out (1 suite, 0.09s)`。测试通过真实键绑定和输入事件驱动无头窗口，没有直接调用 `set_value` 来代替用户输入。该结果只覆盖当前源码的文内／全文快捷键路径，不覆盖真实输入法、正在使用的应用包或所有鼠标入口。

后续搜索交互修复先通过实际界面事件复现 U02 的 `oldnew` 与 U04 的 `ordinary search opened a replacement diff`，再统一修复入口和模式。`cargo test -p workspace-editor --offline --locked search_ui_tests -- --nocapture` 实际输出 `11 passed, 362 filtered out (1 suite, 6.20s)`；覆盖侧栏重开、新旧查询、普通搜索点击、空替换词逐行删除与撤销、模式切换的输入焦点和旧词保留，以及现有范围／布局行为。该结果仍不等同于 U01 的真实输入法路径已验收。

## 文件树、标签与基础编辑核查

以下 U11–U18 记录源码确认的限制或设计取舍，其中 U11／U13／U14／U15／U17 已经修复并通过无头回归，其余尚未实施。尚未逐项实机验收；这些日常流程问题不是拆除 Agent 新引入的回归。

| 编号 | 优先级与状态 | 触发步骤与当前行为 | 期望与最小改进 | 证据 |
| --- | --- | --- | --- | --- |
| U11 | P1，已补齐并验证 | 原先保持树焦点却不能用方向键浏览，回归按 ↓ 后选择仍在根目录。现支持 ↑↓ 可见行选择、←→ 父子导航及折叠／展开、滚入视口；`⌘↓` 打开并进入编辑器，Enter / F2 保留 macOS 重命名习惯 | 复用已有异步加载与路径选择；行内命名不受树导航影响。打开请求复用可取消入口，后续标签或树选择统一使旧请求失效，避免抢回焦点；两种时序均有失败后通过的回归 | [`explorer_ops.rs`](../../crates/app/src/workbench/explorer_ops.rs)：`key_bindings`、`focus_tree_selection`、`open_tree_selection`；[`workbench.rs`](../../crates/app/src/workbench.rs)：`cancel_file_open`；[`explorer_ui_tests.rs`](../../crates/app/src/workbench/explorer_ui_tests.rs)：导航与晚到读取取消回归 |
| U12 | P1，静态确认受限 | 连按 ⌘D 只把单个选区移到下一处；注释、移动行和复制行只处理主选区，并丢弃额外光标 | 明确当前行为；如继续沿用多选区编辑习惯，应先补齐 Kit 的选区接口，再让编辑命令统一处理全部选区 | [`edit_commands.rs`](../../crates/app/src/workbench/edit_commands.rs)：文件说明、`select_next_occurrence`、`edit_active`；[`edit_commands_ui_tests.rs`](../../crates/app/src/workbench/edit_commands_ui_tests.rs)：现有单选区测试 |
| U13 | P1，已复现并修复 | 原先单独关闭或批量关闭未加载标签后，⇧⌘T 无法重开；回归实际未能打开预期文件 | 单独与批量关闭共用 `remove_pending_tab`，将实际移除的路径记入有界最近关闭栈。重开复用现有文件检查和后台读取；关闭取消及版本检查继续有效 | [`workbench.rs`](../../crates/app/src/workbench.rs)：`remove_pending_tab`；[`tab_menu.rs`](../../crates/app/src/workbench/tab_menu.rs)：`close_panes_except`；[`tab_menu_ui_tests.rs`](../../crates/app/src/workbench/tab_menu_ui_tests.rs)：单独／批量关闭后重开回归 |
| U14 | P1，已复现并修复 | 原先未加载标签的复制路径菜单不可用，回归剪贴板实际为空 | 菜单优先使用 pending 路径，支持复制绝对／相对路径、在 Finder 或资源管理器显示，无需先载入文档。无头测试验证两种复制与资源管理器定位，Finder 系统窗口未实机验证 | [`tab_menu.rs`](../../crates/app/src/workbench/tab_menu.rs)：`tab_menu`、`copy_tab_path`、`reveal_tab_in_explorer`；[`tab_menu_ui_tests.rs`](../../crates/app/src/workbench/tab_menu_ui_tests.rs)：`unloaded_tab_path_actions_do_not_read_the_file` |
| U15 | P1，已复现并修复 | 原先 Untitled 首次保存只用语言默认，另存到其他目录也不重读规则；两条保存回归均失败 | 写入成功后在后台复用 `indent::resolve` 读取目标规则，应用时校验路径、文本版本和缩进版本；保存期间的手动选择优先。回归覆盖首次保存、跨目录另存和保存期间选择与原值相同的缩进 | [`documents.rs`](../../crates/app/src/workbench/documents.rs)：`write_as`、`set_indent`；[`indent.rs`](../../crates/app/src/indent.rs)：`resolve`；[`indent_ui_tests.rs`](../../crates/app/src/workbench/indent_ui_tests.rs)：另存为规则与并发手动选择回归 |
| U16 | P2，静态确认受限 | 连续单击不同文件进行浏览，每次都打开永久标签，没有可复用的预览标签；达到 20 个文件／20 MiB 上限后需要手动清理 | 可选单击复用一个预览标签，双击或编辑后固定；与明确打开、恢复标签区分，不扩大读取预算 | [`sidebar.rs`](../../crates/app/src/workbench/sidebar.rs)：文件树行的 `on_click`；[`workbench.rs`](../../crates/app/src/workbench.rs)：`open_file`、`install_loaded` 和文档预算 |
| U17 | P2，已补齐并验证 | 原先原生菜单缺少「编辑」。现增加撤销、重做、剪切、复制、粘贴和全选，复用组件标准动作及原有快捷键，按当前焦点派发 | 搜索框和文档编辑器使用组件动作；文件树、Diff、大文件查看、终端接入各自已有能力，不增加不支持的编辑动作。回归逐项验证查询框操作、切到文档后不改查询、树文件剪贴板及真实临时 shell 的菜单粘贴 | [`main.rs`](../../crates/app/src/main.rs)：`app_menus`；[`commands_ui_tests.rs`](../../crates/app/src/workbench/commands_ui_tests.rs)：`edit_menu_actions_follow_the_focused_input`；[`terminal_ui_tests.rs`](../../crates/app/src/workbench/terminal_ui_tests.rs)：`terminal_accepts_the_native_menu_paste_action` |
| U18 | 已补齐自动化，待原生验收 | 历史问题：标签不能拖动排序，较多时换行占用高度 | 当前为单行横向滚动，提供已打开文件菜单及窗口内鼠标拖动排序，文件顺序持久保存；窄窗口与真实桌面视觉仍待验收 | [`editor_area.rs`](../../crates/app/src/workbench/editor_area.rs)、[`tab_interactions.rs`](../../crates/app/src/workbench/tab_interactions.rs)、[`feature_ui_tests.rs`](../../crates/app/src/workbench/feature_ui_tests.rs) |

**U10：P0，已复现并在本轮修复。** 已打开且编辑过的文件被外部删除后，监听将缓冲区标为已删除。若另一个程序在原路径重建文件，而监听尚未更新标记，此时手动保存原来会跳过磁盘版本检查，静默覆盖新文件。

回归测试 `saving_a_deleted_buffer_does_not_overwrite_an_external_recreation` 使用真实临时文件，在同一个前台事件中重建文件并立即保存。修复前输出 `0 passed; 1 failed`，文件实际内容从 `external replacement\n` 变为 `twooriginal\n`。修复仅调整 [`documents.rs`](../../crates/app/src/workbench/documents.rs) 的 `save_document`：只有明确授权覆盖时才跳过检查，已删除标记不再使旧磁盘状态失效。底层 [`save.rs`](../../crates/app/src/save.rs) 继续允许目标不存在时重新创建。

```sh
CARGO_HOME="$PWD/target/cargo-home" cargo test -p workspace-editor --offline --locked \
  saving_a_deleted_buffer_does_not_overwrite_an_external_recreation -- --nocapture
```

修复后实际输出 `cargo test: 1 passed, 369 filtered out (1 suite, 1.34s)`。测试同时检查外部内容保留、出现冲突提示、取消后未保存缓冲区保留，以及目标仍不存在时可以正常重建。回归保留在 [`auto_save_ui_tests.rs`](../../crates/app/src/workbench/auto_save_ui_tests.rs)。这不代表已解决底层所有外部并发写入竞态；本项覆盖的是已确认的删除标记绕过检查问题。

## 推送问题的验证

用户反馈：「远端没有这个分支，显示成功但实际上没有推送成功。」当前证据需要区分以下场景：

| 场景 | 当前实现与验证结果 | 判定 |
| --- | --- | --- |
| 新本地分支尚未配置上游，远端也没有该分支 | `push_args` 拒绝执行，错误提示要求在终端运行 `git push -u`；临时远端未创建分支 | 首次发布分支的功能缺失已确认；此场景未复现假成功 |
| 已有上游，远端分支被删除，本地远端跟踪引用仍在 | 推送成功，并校验临时远端分支确实重新创建且 OID 等于本地 HEAD | 此场景当前实现可用 |
| 已有上游，远端分支被删除，本地跟踪引用也被 `fetch --prune` 清理 | 返回错误，临时远端没有分支 | 恢复或重新发布流程受限；未复现假成功 |
| 远端通过 `pre-receive` hook 拒绝新提交 | 返回错误，校验临时远端 OID 保持原值 | 服务层能报告远端拒绝；未复现假成功 |

验证入口：[`crates/git_service/tests/writes.rs`](../../crates/git_service/tests/writes.rs) 的 `push_distinguishes_unpublished_pruned_recreated_and_rejected_branches`。

```sh
CARGO_HOME="$PWD/target/cargo-home" CARGO_TARGET_DIR=/tmp/zj-gap-audit-target \
  cargo test -p workspace-editor-git --test writes \
  push_distinguishes_unpublished_pruned_recreated_and_rejected_branches \
  --offline --locked -- --exact --nocapture
```

实际输出：`cargo test: 1 passed, 17 filtered out (1 suite, 0.89s)`。所有仓库、远端和拒绝 hook 都位于测试生成的临时目录，没有连接真实远端。

相关实现：[`write.rs`](../../crates/git_service/src/write.rs) 的 `push_args`、`GitService::write`；[`scm_actions.rs`](../../crates/app/src/workbench/scm_actions.rs) 的 `start_git_write`。界面目前丢弃服务返回的命令输出，只按成功结果显示「已推送」，不能直接看到目标 URL、目标引用或逐引用结果。这是诊断能力不足的证据，并不证明当前代码必然把失败判为成功。

还需要用户遇到问题时所用的应用构建、选中的仓库与分支、脱敏后的分支上游和 push URL、Git 实际输出，才能继续定位该次假成功。当前未核对正在使用的应用包是否与本次源码一致。

## P0：Git 结果与发布流程

| 编号 | 状态 | 缺口与实际影响 | 最小补齐方式 | 代码依据 |
| --- | --- | --- | --- | --- |
| G01 | 缺失 | 没有首次「发布分支」流程。新分支无上游时必须离开 ZJ 去终端；跟踪引用消失后也没有重新发布入口 | 明确选择远程与远端分支，确认后执行精确 refspec 的 `push --set-upstream`，回读配置并刷新状态；不默认强推 | [`write.rs`](../../crates/git_service/src/write.rs)：`push_args`；[`scm_actions.rs`](../../crates/app/src/workbench/scm_actions.rs)：`scm_push`；新集成测试 |
| G02 | 受限；假成功待复现 | 成功只显示「已推送」，服务输出没有进入界面。难以确认推到了哪个目标、是新建还是已是最新，也难排查用户报告的假成功 | 保留脱敏的操作结果详情：仓库、分支、推送目标、逐引用结果；区分已更新、无变化、失败、部分完成，错误可复制。按实际推送配置展示目标，不能只显示 fetch URL | [`scm_actions.rs`](../../crates/app/src/workbench/scm_actions.rs)：`start_git_write` 将 `Reply` 转成 `Ok(())`；[`write.rs`](../../crates/git_service/src/write.rs)：成功回复携带 `output` |

## P1：查看与 Diff 的主要缺口

| 编号 | 状态 | 缺口与实际影响 | 最小补齐方式 | 代码依据 |
| --- | --- | --- | --- | --- |
| V01 | 缺失 | 不能把 PNG、JPEG、WebP、GIF 等图片作为项目文件直接打开；有图片图标不代表有查看器 | 图片标签页，支持适应窗口、原始大小、缩放、尺寸和大小信息，限制解码预算；SVG 同时提供源码与预览入口 | [`files.rs`](../../crates/app/src/files.rs)：`text_file` 拒绝含 NUL 的二进制文件；[`file_icons.rs`](../../crates/app/src/file_icons.rs) 只有图片类型图标；[`workbench.rs`](../../crates/app/src/workbench.rs)：`Pane` 无图片视图 |
| V02 | 缺失 | 图片修改只能看到 Git 的二进制差异说明，无法检查图像改动 | 至少并排显示旧图与新图，标明版本和尺寸；滑动叠加等高级模式后置 | [`diff_doc.rs`](../../crates/app/src/diff_doc.rs)：`DiffDoc::parse` 对 binary patch 返回 `None`；[`diff_view.rs`](../../crates/app/src/workbench/diff_view.rs)：`fallback` |
| D01 | 缺失 | 没有「选择用于比较／与所选文件比较」或剪贴板比较入口，不能方便比较任意两个文件 | 文件树或标签页选择 A，再与 B 比较，复用现有 Diff 渲染；剪贴板比较可后置 | [`tab_menu.rs`](../../crates/app/src/workbench/tab_menu.rs)：`tab_menu`；[`explorer_ops.rs`](../../crates/app/src/workbench/explorer_ops.rs)；[`search_replace.rs`](../../crates/app/src/workbench/search_replace.rs)：现有本地 Diff 入口用于替换预览 |
| D02 | 缺失 | 没有分支对分支、提交对提交、基于共同祖先的整组文件比较入口，审阅一个功能分支不方便 | 选择两个引用和比较方式，先列完整变更文件，再逐个打开 Diff；明确直接比较与 merge-base 比较的差别 | [`lib.rs`](../../crates/git_service/src/lib.rs)：`Operation`；[`graph_view.rs`](../../crates/app/src/workbench/graph_view.rs)：`graph_open_diff` 仅打开所选提交相对第一父提交的单文件 Diff |
| H01 | 缺失 | 无单文件历史与跟随重命名的时间线；也没有从历史版本直接打开完整只读文件的通用入口 | 文件右键「查看历史」，选择提交后查看该版完整内容、与当前版本比较 | [`lib.rs`](../../crates/git_service/src/lib.rs)：`graph` 无路径过滤；[`graph_view.rs`](../../crates/app/src/workbench/graph_view.rs)：提交详情和单文件 Diff；[`tab_menu.rs`](../../crates/app/src/workbench/tab_menu.rs)：无文件历史入口 |
| D03 | 缺失 | Diff 内没有查找。长补丁里不能用 ⌘F 定位函数名或字符串 | 两侧和内联视图共用查找状态，显示命中计数、上一个／下一个，并滚动到匹配位置 | [`find_widget.rs`](../../crates/app/src/workbench/find_widget.rs)：`find_document` 仅接受 `Pane::Document`；`open_find` 只另行处理大文件 |
| D04 | 缺失 | 没有忽略空白差异、折叠未改动区域等审阅选项；格式化改动容易淹没有效修改 | 添加忽略行尾空格或全部空白的查看选项、折叠长段未变更内容；查看选项不能悄悄改变暂存补丁语义 | [`settings.rs`](../../crates/app/src/settings.rs)：Diff 设置仅布局；[`diff_view.rs`](../../crates/app/src/workbench/diff_view.rs)：`render_diff_actions`；[`lib.rs`](../../crates/git_service/src/lib.rs)：Diff 参数 |
| D05 | 受限 | 每窗口只有一份 Diff 状态，新比较覆盖前一个；没有固定多个 Diff 或左右分组查看多个文件的流程 | 先允许保留多个 Diff 标签及各自滚动位置，再按需补编辑区分组 | [`workbench.rs`](../../crates/app/src/workbench.rs)：`Pane::Diff`、单个 `diff`；[`diff_view.rs`](../../crates/app/src/workbench/diff_view.rs)：`DiffPane.tab`；[`search_replace.rs`](../../crates/app/src/workbench/search_replace.rs)：`show_diff_tab` |
| H02 | 受限 | 提交图只能按全部／本地／单分支筛选，无法按提交信息、作者、日期、哈希或路径快速查历史 | 提交搜索输入和必要筛选；搜索应覆盖仓库历史，不只筛已加载的 500 条 | [`lib.rs`](../../crates/git_service/src/lib.rs)：`GraphScope`、`graph`；[`graph_view.rs`](../../crates/app/src/workbench/graph_view.rs)：`graph_header` |
| V03 | 受限 | 非 UTF-8 文件只能按 UTF-8 有损近似查看，GBK 中文可能乱码；UTF-16 因含 NUL 可能直接被当成二进制 | 至少识别 BOM，支持显式「以编码重新打开」；先满足正确只读查看，编码转换写回后置 | [`files.rs`](../../crates/app/src/files.rs)：`Restricted::NotUtf8`、`text_file`；[`large_file.rs`](../../crates/app/src/large_file.rs)：`Line` 的有损解码 |
| R01 | 缺失 | 缺少独立于 Agent 的审阅进度或基线。查看过哪些文件、外部工具之后又改了哪些，不能作为持久的审阅状态保存 | 按仓库记录本地审阅检查点，文件版本变化时重新标为待审；标记已读不应自动暂存或还原 | [`workbench.rs`](../../crates/app/src/workbench.rs)：`Group` 的状态与变更列表；[`session.rs`](../../crates/app/src/session.rs)：`SavedWindow` 不保存审阅检查点 |
| W01 | 受限 | 重启主要恢复文件夹、窗口和文件标签，不能恢复多个 Diff、比较双方、阅读位置与审阅上下文 | 先持久化文件阅读位置和固定 Diff 的比较来源；磁盘与引用变化后重新校验，旧快照不能被当成当前版本 | [`session.rs`](../../crates/app/src/session.rs)：`SavedWindow` 只保存 `root`、`frame`、`tabs`、`active` |

## P2：按需补充的辅助流程

| 编号 | 状态 | 缺口与实际影响 | 最小补齐方式 | 代码依据 |
| --- | --- | --- | --- | --- |
| G03 | 缺失 | 无克隆、初始化仓库、添加／修改远程地址的完整图形流程；「添加仓库」只是把已有仓库纳入列表 | 先补远程列表与地址查看、复制，写入配置的操作按需提供；克隆和初始化可继续由外部工具完成 | [`write.rs`](../../crates/git_service/src/write.rs)：`WriteOperation`；[`scm.rs`](../../crates/app/src/workbench/scm.rs)：仓库菜单和添加仓库入口 |
| G04 | 受限 | 能创建和切换分支，但没有分支重命名、删除或显式设置／修复上游的完整流程 | 优先提供上游状态与修复入口；分支删除需区分本地／远程并确认目标 | [`write.rs`](../../crates/git_service/src/write.rs)：`Checkout`、`CreateBranch`；[`scm_actions.rs`](../../crates/app/src/workbench/scm_actions.rs)：分支选择器 |
| G05 | 受限 | 已有冲突标记识别和采用当前／传入／双方，但没有三方合并视图、merge/rebase/cherry-pick 状态及继续／中止流程的完整产品入口 | 先清楚显示仓库正在进行的操作和恢复办法，提供外部工具入口；是否内置三方合并再按真实需求决定 | [`conflict_bar.rs`](../../crates/app/src/workbench/conflict_bar.rs)；[`conflicts.rs`](../../crates/app/src/conflicts.rs)；[`write.rs`](../../crates/git_service/src/write.rs)：操作枚举 |
| D06 | 受限 | 能复制 Git 原文、对所选行生成内部补丁，但没有打开／导出／验证后应用 `.patch` 的文件流程 | 先提供导出当前比较为补丁；应用补丁需预检目标仓库、路径和失败结果，可继续留给外部工具 | [`diff_ops.rs`](../../crates/app/src/workbench/diff_ops.rs)：复制与块操作；[`partial_patch.rs`](../../crates/app/src/partial_patch.rs)；[`write.rs`](../../crates/git_service/src/write.rs)：`ApplyPatch` |
| G06 | 受限 | 能发现已初始化的 submodule 和 linked worktree，但没有专门的 submodule 初始化／更新、worktree 管理或 LFS 内容查看流程 | 明确显示对象类型、未初始化状态和 LFS 指针，支持跳转已有工作树；管理操作是否内置另行取舍 | [`lib.rs`](../../crates/git_service/src/lib.rs)：仓库发现与 `--no-textconv` Diff；[`write.rs`](../../crates/git_service/src/write.rs)：操作枚举 |
| S01 | 受限 | 全文搜索跳过大于 2 MiB 的文件、无法打开／读取的文件和二进制内容；资源上限合理，但没有逐类汇总跳过原因，容易把局部搜索结果理解成完整搜索 | 保留预算，补充「搜索范围与跳过原因」详情；大文件可单独按需搜索，无需取消所有限制 | [`text_search.rs`](../../crates/app/src/text_search.rs)：`MAX_FILE_BYTES`、`MAX_MATCHES`、`run` 的跳过路径；[`files.rs`](../../crates/app/src/files.rs)：路径搜索结果上限 |

## 已有能力，不应重复列为缺失

已存在多仓库 Changes、已暂存／未暂存区分、并排／内联 Diff、行内字符差异、高亮与同步滚动、上一个／下一个改动、按块和行暂存／取消暂存／还原、Git 图和提交详情、标签管理、stash、单行 blame、基本冲突处理、文件搜索与全文搜索、代码跳转、Markdown 预览、基本文件操作、未保存内容恢复和外部修改保护。具体支持范围仍应按相关实现和验收记录理解。

LSP、调试器、插件市场、Agent 对话与编排不建议纳入本次补齐清单。PDF、音视频、十六进制查看和高级 Git 历史改写可继续交给外部工具，除非出现明确的高频需求。

## 当前自动化验证

最新源码执行 `CARGO_HOME="$PWD/target/cargo-home" cargo test --workspace --offline --locked`，实际输出为 `cargo test: 421 passed, 3 ignored (10 suites, 78.81s)`。保留此前 Agent 移除、搜索、保存保护、焦点、恢复标签与另存为回归；本批增加五项测试，覆盖 U07／U11／U17 的最近激活排序、目标路径、关闭与返回路径、树键盘导航、读取取消、菜单焦点派发和终端粘贴。取消回归同时覆盖打开后切标签与继续选择树项目，两种时序均先复现失败再验证修复。文件树专项最终 `24 passed`，命令面板与菜单专项 `10 passed`，终端专项 `3 passed`，均属于工作区测试子集，不累加。严格全目标 Clippy、fmt、`git diff --check` 和离线 dist 构建通过；产物大小与哈希见[开发记录](../开发记录.md)。

`cargo fmt --all --check`、严格 `cargo clippy --workspace --all-targets --offline --locked -- -D warnings` 与 `cargo build --profile dist --offline --locked` 均通过，仅有第三方 `block 0.1.6` 的 future-incompat 提示。审查按 `ponytail-review` → `code-review` 顺序完成：第一阶段未发现拆除回归；新增保存修复复查了调用方与底层写入路径；第二阶段 Standards、Spec 两轴独立只读审查均为 0 项发现。后续 U02／U04／U05 修复也按同一顺序对搜索相关四个文件单独审查，三个审查结果均为 0 项发现。U03／U08／U13／U14／U15 按批次源码快照单独审查，主线程验证补丁与最终九个源码文件一致；第一阶段与第二阶段两轴也均为 0 项发现。未提供 issue tracker，依赖其内容的核对未执行。

曾尝试对全量套件加 `--release`，结果为 `365 passed; 1 failed; 3 ignored`，失败测试为 `the_ignored_files_switch_stays_in_the_column_and_toggles`。已确认 GPUI 的 `debug_bounds` 在未启用 `debug_assertions` 时返回 `None`，因此该套件应使用默认 test profile。README 的测试命令和 CLAUDE.md 的测试约定已同步修正；未通过删除布局断言来规避失败。

## 验证边界

本报告包含代码核查、无头界面键盘验证和真实 Git 的本地临时仓库验证，未进行当前应用的逐项实机验收；静态缺口应在实施前补充对应的用户流程验收。没有连接真实远端，未验证用户账号的认证、SSH、代理、签名或远端 hooks，也没有复现用户当时看到「成功」的完整界面路径。

资源限制属于受控设计，不应为追求功能数量直接删除。验收文档仍记录四小时回放和完整屏幕呈现延迟等未完成项；这类质量门禁应与新增功能分别跟踪，不能用功能数量替代。
