# ADR 0006：用 alacritty_terminal 做内置终端

日期：2026-10-03。状态：已采纳。

## 背景

用户要求像 VS Code 一样在窗口底部开终端，能新建、左右拆分、关闭，快捷键和 VS Code 一致。终端仿真（VT 解析、网格、滚动区、备用屏、鼠标上报、括号粘贴、宽字符）很大，也很难写对；GPUI Kit 没有终端组件，Zed 的终端依赖 Zed 版 `gpui`，和这里的 `gpui-pre` 不兼容。

## 决定

- 仿真和 pty 用 `alacritty_terminal =0.26.0`（`default-features = false`，不带 serde）：Alacritty 自己在用、持续维护；Zed 的终端也是基于它。
- 自己只写界面衔接：`crates/app/src/terminal.rs` 起 shell、转发事件、按键编码成 xterm 序列、把 ANSI 颜色映射到主题；`workbench/terminal_view.rs` 用 canvas 画网格（同色同样式的连续字符合并成一段排版，背景块合并成矩形）、光标、选区、输入法预编辑，处理鼠标选择和滚轮；`workbench/terminal_panel.rs` 是底部面板（分组、拆分、关闭）。
- 默认 shell 是 `$SHELL`（login shell），工作目录是窗口打开的文件夹；只给 shell 设 `TERM=xterm-256color`、`COLORTERM=truecolor`、`TERM_PROGRAM=ZJ`，不改 ZJ 自己的环境。回滚 1000 行（VS Code 默认值）。
- 没有定时器：光标不闪，读线程有输出时通过 channel 唤醒视图，一批事件只重画一次。面板隐藏时终端进程保留，不重画。
- 快捷键：⌃` 显示 / 隐藏面板（没有终端时新建一个），⌃⇧` 新建，⌘\ 在终端里向右拆分；面板按钮还有关闭当前终端。shell 退出时自动关掉对应的终端，最后一个关掉时面板隐藏。

## 代价

在 M5 Pro 上测量（`--profile dist`，aarch64）：

- 依赖：`cargo tree -e normal --target aarch64-apple-darwin --prefix none | sort -u | wc -l` 从 601 到 609（`alacritty_terminal`、`vte`、`rustix-openpty`、`signal-hook`、`miow`、`home`、`cursor-icon` 等）。
- 二进制：34,944,016 B → 35,374,784 B，+430,768 B（约 0.41 MiB，含界面代码）。
- 内存：同一个空文件夹、窗口在前台，不开终端 165 MB，开一个终端后 ZJ 进程 168 MB（+3 MB，主要是网格和读线程）；另有 shell 进程本身（这台机器上 zsh 4.8 MB，加上 macOS 下 alacritty 用来起 login shell 的 `/usr/bin/login`）。shell 的占用取决于用户自己的配置。
- 空闲 CPU：开着一个终端、shell 在等输入时，p95 0.08%（`tools/sample_resources.py`，10 s）。

## 后果

- 只做基本功能：没有标签重命名、上下拆分、查找、链接点击、shell 集成和终端设置项；需要时再加。
- 依赖升级要跟 Alacritty 的发版节奏走；`alacritty_terminal` 的 API 每个小版本都可能变，所以锁定精确版本。
- 起 shell 走 alacritty 的 `tty::new`，macOS 下经过 `/usr/bin/login -flp`，和 Terminal.app / Alacritty 一样会读 login 配置。
