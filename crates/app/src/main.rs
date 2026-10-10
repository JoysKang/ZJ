mod agent_images;
mod agent_model;
mod assets;
mod conflicts;
mod diff_doc;
mod diff_syntax;
mod editing;
mod file_icons;
mod file_ops;
mod files;
mod fuzzy;
mod indent;
mod input_switch;
mod languages;
mod large_file;
mod markdown;
mod markdown_blocks;
mod md_images;
mod partial_patch;
mod perf;
mod platform;
mod quota;
mod recovery;
mod refresh_plan;
mod replace;
mod save;
mod secrets;
mod session;
mod settings;
mod symbol_index;
mod symbols;
mod terminal;
mod text_search;
mod theme;
mod watch;
mod workbench;
use gpui_kit::component::input::GoToDefinition;
use gpui_kit::*;
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};
use workbench::navigation as nav;
use workbench::{DocumentOwners, Workbench};
use workspace_editor_git::GitService;

impl Global for watch::WatchService {}

/// What a new window opens besides its folder.
#[derive(Default)]
pub(crate) struct Startup {
    /// The last session's tabs: `active` is read now, the others when chosen.
    tabs: Vec<PathBuf>,
    active: Option<PathBuf>,
    /// Files from the command line, read now.
    open: Vec<PathBuf>,
}

/// `bounds`: a frame restored from the last session; `None` cascades from the default place.
fn open_workspace(
    root: Option<PathBuf>,
    service: GitService,
    documents: DocumentOwners,
    index: usize,
    bounds: Option<WindowBounds>,
    startup: Startup,
    cx: &mut App,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if cx.windows().len() >= 5 {
        return Err("原型最多打开 5 个窗口，请先关闭一个窗口".into());
    }
    let offset = theme::WINDOW_ORIGIN + theme::WINDOW_CASCADE * index as f32;
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(theme::TRAFFIC_LIGHT_X, theme::TRAFFIC_LIGHT_Y)),
        }),
        window_bounds: Some(bounds.unwrap_or(WindowBounds::Windowed(Bounds::new(
            point(offset, offset),
            size(theme::WINDOW_WIDTH, theme::WINDOW_HEIGHT),
        )))),
        window_min_size: Some(size(theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT)),
        ..gpui_kit::component::TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let mut workbench = Workbench::new(root, service, documents, index + 1, window, cx);
            if !startup.tabs.is_empty() || startup.active.is_some() {
                workbench.restore_tabs(startup.tabs, startup.active, window, cx);
            }
            for path in startup.open {
                workbench.open_now(path, window, cx);
            }
            workbench
        })
    })?;
    Ok(())
}

fn display_bounds(cx: &App) -> Vec<Bounds<Pixels>> {
    cx.displays()
        .iter()
        .map(|display| display.bounds())
        .collect()
}

/// An empty window: the Dock icon or 新建窗口 with no window open.
fn open_empty_window(service: GitService, cx: &mut App) {
    let documents = cx.global::<workbench::OpenDocuments>().0.clone();
    let index = cx.windows().len();
    if let Err(e) = open_workspace(
        None,
        service,
        documents,
        index,
        None,
        Default::default(),
        cx,
    ) {
        eprintln!("event=window_open_failed error={e}");
    }
}

/// Closing a window never quits ZJ. The last workspace window gives way to an empty window
/// in its place; closing that one leaves ZJ in the Dock, whose icon opens a window again.
fn window_closed(id: WindowId, service: GitService, cx: &mut App) {
    let closed = session::closed(id, cx);
    if !cx.windows().is_empty() {
        return;
    }
    let Some(closed) = closed.filter(|closed| closed.root.is_some()) else {
        return;
    };
    let bounds = closed
        .frame
        .and_then(|frame| frame.window_bounds(&display_bounds(cx)));
    let documents = cx.global::<workbench::OpenDocuments>().0.clone();
    // Not from inside the close notification, which runs while GPUI removes the window.
    cx.defer(move |cx| {
        if let Err(e) = open_workspace(None, service, documents, 0, bounds, Default::default(), cx)
        {
            eprintln!("event=window_open_failed error={e}");
        }
    });
}

#[allow(clippy::print_stdout)] // --help output belongs on stdout.
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // The Claude Code / Codex bridge runs as this executable, before anything GPUI (ADR 0009).
    let mut raw = std::env::args_os().skip(1);
    if raw
        .next()
        .is_some_and(|a| a == workspace_editor_agent_bridge::FLAG)
    {
        let args: Vec<String> = raw.map(|a| a.to_string_lossy().into_owned()).collect();
        std::process::exit(workspace_editor_agent_bridge::run(&args));
    }
    perf::mark_start();
    let mut roots = Vec::new();
    let mut files = Vec::new();
    let mut windows = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--windows" {
            let count: usize = args
                .next()
                .ok_or("--windows 缺少数量")?
                .to_str()
                .ok_or("无效窗口数量")?
                .parse()?;
            if !(1..=5).contains(&count) {
                return Err("窗口数量必须为 1 至 5".into());
            }
            windows = Some(count);
        } else if arg == "--help" {
            println!(
                "ZJ [--windows 1..5] [工作区根目录 | 文件 ...]\n轻量代码编辑器：多仓库源代码管理、并排 / 内联 Diff、文件编辑与保存。\n文件在包含它的工作区窗口里打开，否则在第一个窗口。"
            );
            return Ok(());
        } else {
            let path = std::fs::canonicalize(PathBuf::from(arg))?;
            if path.is_dir() {
                roots.push(path);
            } else if path.is_file() {
                files.push(path);
            } else {
                return Err("参数必须是目录或文件".into());
            }
        }
    }
    // A launch without arguments (Dock, Finder) reopens the windows open at the last quit.
    let restored = if roots.is_empty() && files.is_empty() && windows.is_none() {
        session::path()
            .map(|path| session::restorable(session::load_from(&path), 5))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let count = windows.unwrap_or(roots.len().max(restored.len()).max(1));
    if count > 5 || roots.len() > count {
        return Err("原型最多支持 5 个窗口，每个根目录一个窗口".into());
    }
    let service = GitService::new(2, Duration::from_secs(30))?;
    let app = gpui_kit::application().with_assets(assets::AppAssets);
    let reopen = service.clone();
    // The Dock icon with no window open (the last one was closed) brings up an empty window.
    app.on_reopen(move |cx| {
        if cx.windows().is_empty() && cx.has_global::<workbench::OpenDocuments>() {
            open_empty_window(reopen.clone(), cx);
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        cx.set_app_identity("local.zj.editor", "ZJ");
        workbench::init_agent_notifications(cx);
        cx.set_global(settings::Settings::load());
        cx.set_global(watch::WatchService::default());
        workbench::init_agent_store(cx);
        diff_syntax::register();
        platform::DockBlink::apply(cx);
        cx.observe_global::<settings::Settings>(platform::DockBlink::apply)
            .detach();
        theme::follow_appearance(None, cx);
        cx.bind_keys([
            KeyBinding::new("cmd-shift-n", workbench::NewWindow, Some("WorkspaceEditor")),
            KeyBinding::new("secondary-c", workbench::CopyLargeLines, Some("LargeView")),
            KeyBinding::new(
                "cmd-,",
                workbench::OpenSettingsFile,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("secondary-s", workbench::Save, Some("WorkspaceEditor")),
            KeyBinding::new(
                "secondary-shift-s",
                workbench::SaveAs,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "alt-secondary-s",
                workbench::SaveAll,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-n",
                workbench::NewUntitled,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-w",
                workbench::CloseEditor,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-shift-t",
                workbench::ReopenClosedEditor,
                Some("WorkspaceEditor"),
            ),
            // The tab menu's commands (VS Code's macOS keys), acting on the active tab.
            KeyBinding::new(
                "alt-cmd-t",
                workbench::CloseOtherEditors,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k cmd-w",
                workbench::CloseAllEditors,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k p",
                workbench::CopyActivePath,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k alt-cmd-c",
                workbench::CopyActiveRelativePath,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k r",
                workbench::RevealActiveInFinder,
                Some("WorkspaceEditor"),
            ),
            // The terminal panel (VS Code's macOS keys).
            KeyBinding::new("ctrl-`", workbench::ToggleTerminal, Some("WorkspaceEditor")),
            KeyBinding::new(
                "ctrl-shift-`",
                workbench::NewTerminal,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("cmd-\\", workbench::SplitTerminal, Some("Terminal")),
            // VS Code's Markdown: Open Preview, here a toggle in place.
            KeyBinding::new(
                "shift-cmd-v",
                workbench::ToggleMarkdownPreview,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("alt-z", workbench::ToggleSoftWrap, Some("WorkspaceEditor")),
            KeyBinding::new("secondary-q", workbench::Quit, None),
            KeyBinding::new("cmd-o", workbench::OpenFile, Some("WorkspaceEditor")),
            // VS Code: ⌘⇧O is go to symbol; open folder moves to ⌘K ⌘O.
            KeyBinding::new(
                "cmd-k cmd-o",
                workbench::OpenFolder,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-shift-o",
                nav::GoToSymbol,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("ctrl-g", nav::GoToLine, Some("WorkspaceEditor")),
            KeyBinding::new("f12", GoToDefinition, Some("Input")),
            KeyBinding::new("shift-f12", nav::FindReferences, Some("WorkspaceEditor")),
            KeyBinding::new("secondary-c", workbench::CopyDiff, Some("DiffEditor")),
            KeyBinding::new("secondary-a", workbench::SelectAllDiff, Some("DiffEditor")),
            // Agent reviews (design): accept / reject the current change.
            KeyBinding::new(
                "secondary-y",
                workbench::AcceptAgentChange,
                Some("DiffEditor"),
            ),
            KeyBinding::new(
                "secondary-backspace",
                workbench::RejectAgentChange,
                Some("DiffEditor"),
            ),
            KeyBinding::new("ctrl--", nav::NavigateBack, Some("WorkspaceEditor")),
            KeyBinding::new(
                "ctrl-shift--",
                nav::NavigateForward,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-p",
                workbench::QuickOpenFile,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-shift-p",
                workbench::ShowAllCommands,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-b",
                workbench::ToggleSidebar,
                Some("WorkspaceEditor"),
            ),
            // Explorer (VS Code's macOS bindings), acting on the selected row.
            KeyBinding::new("f2", workbench::RenameFile, Some("Explorer")),
            KeyBinding::new("enter", workbench::RenameFile, Some("Explorer")),
            KeyBinding::new("delete", workbench::DeleteFile, Some("Explorer")),
            KeyBinding::new("cmd-backspace", workbench::DeleteFile, Some("Explorer")),
            KeyBinding::new("secondary-a", workbench::SelectAllFiles, Some("Explorer")),
            KeyBinding::new("secondary-c", workbench::CopyFiles, Some("Explorer")),
            KeyBinding::new("secondary-x", workbench::CutFiles, Some("Explorer")),
            KeyBinding::new("secondary-v", workbench::PasteFiles, Some("Explorer")),
            KeyBinding::new("alt-cmd-c", workbench::CopyPath, Some("Explorer")),
            KeyBinding::new(
                "alt-shift-cmd-c",
                workbench::CopyRelativePath,
                Some("Explorer"),
            ),
            KeyBinding::new("alt-cmd-r", workbench::RevealInFinder, Some("Explorer")),
            KeyBinding::new("alt-shift-f", workbench::FindInFolder, Some("Explorer")),
            KeyBinding::new(
                "secondary-shift-f",
                workbench::FindInFiles,
                Some("WorkspaceEditor"),
            ),
            // ⌘⇧F inside an editor is find in files (Kit binds it to its replace panel).
            KeyBinding::new("secondary-shift-f", workbench::FindInFiles, Some("Input")),
            // The editor's find widget, with VS Code's macOS keys.
            KeyBinding::new(
                "secondary-f",
                workbench::FindInFile,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "alt-secondary-f",
                workbench::FindReplace,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("secondary-g", workbench::FindNext, Some("WorkspaceEditor")),
            KeyBinding::new(
                "secondary-shift-g",
                workbench::FindPrevious,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("f3", workbench::FindNext, Some("WorkspaceEditor")),
            KeyBinding::new("shift-f3", workbench::FindPrevious, Some("WorkspaceEditor")),
            KeyBinding::new(
                "secondary-shift-1",
                workbench::ReplaceOne,
                Some("FindWidget"),
            ),
            // Some keyboards report ⇧1 as "!".
            KeyBinding::new(
                "secondary-shift-!",
                workbench::ReplaceOne,
                Some("FindWidget"),
            ),
            KeyBinding::new("secondary-!", workbench::ReplaceOne, Some("FindWidget")),
            KeyBinding::new(
                "secondary-alt-enter",
                workbench::ReplaceAll,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-c",
                workbench::ToggleFindCase,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-w",
                workbench::ToggleFindWord,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-r",
                workbench::ToggleFindRegex,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-p",
                workbench::TogglePreserveCase,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-l",
                workbench::ToggleFindInSelection,
                Some("FindWidget"),
            ),
            KeyBinding::new("cmd-=", workbench::ZoomIn, Some("WorkspaceEditor")),
            KeyBinding::new("cmd-+", workbench::ZoomIn, Some("WorkspaceEditor")),
            KeyBinding::new("cmd--", workbench::ZoomOut, Some("WorkspaceEditor")),
            KeyBinding::new("cmd-0", workbench::ZoomReset, Some("WorkspaceEditor")),
            // Agent panel: ⌥⌘B as VS Code's secondary side bar; ⌘J / ⌘L / ⌘⇧A as in the
            // design (session search, add the selection, next approval).
            KeyBinding::new(
                "alt-cmd-b",
                workbench::ToggleAgentPanel,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-j",
                workbench::SearchSessions,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-l",
                workbench::AddSelectionToAgent,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-shift-a",
                workbench::NextApproval,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-n",
                workbench::NewAgentSession,
                Some("AgentPanel"),
            ),
            // Finder's shortcut for hidden files.
            KeyBinding::new(
                "cmd-shift-.",
                workbench::ToggleHiddenFiles,
                Some("WorkspaceEditor"),
            ),
        ]);
        // ⌘/ and the other line editing keys on document editors (workbench/edit_commands.rs).
        cx.bind_keys(workbench::edit_key_bindings());
        cx.set_menus([
            Menu::new("ZJ").items([
                MenuItem::action("设置…", workbench::OpenSettingsFile),
                MenuItem::separator(),
                MenuItem::os_submenu("服务", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("退出 ZJ", workbench::Quit),
            ]),
            Menu::new("文件").items([
                MenuItem::action("新建文件", workbench::NewUntitled),
                MenuItem::action("新建窗口", workbench::NewWindow),
                MenuItem::separator(),
                MenuItem::action("打开文件…", workbench::OpenFile),
                MenuItem::action("打开文件夹…", workbench::OpenFolder),
                MenuItem::separator(),
                MenuItem::action("保存", workbench::Save),
                MenuItem::action("另存为…", workbench::SaveAs),
                MenuItem::action("全部保存", workbench::SaveAll),
                MenuItem::separator(),
                MenuItem::action("关闭编辑器", workbench::CloseEditor),
                MenuItem::separator(),
                MenuItem::submenu(Menu::new("自动保存").items([
                    MenuItem::action("关闭", workbench::AutoSaveOff),
                    MenuItem::action("编辑后 1 秒", workbench::AutoSaveAfterDelay),
                    MenuItem::action("失去焦点时", workbench::AutoSaveOnFocusChange),
                ])),
            ]),
            Menu::new("查看").items([
                MenuItem::action("命令面板…", workbench::ShowAllCommands),
                MenuItem::separator(),
                MenuItem::action("放大", workbench::ZoomIn),
                MenuItem::action("缩小", workbench::ZoomOut),
                MenuItem::action("重置缩放", workbench::ZoomReset),
                MenuItem::separator(),
                MenuItem::action("自动换行", workbench::ToggleSoftWrap),
                MenuItem::separator(),
                MenuItem::action("显示 / 隐藏点文件", workbench::ToggleHiddenFiles),
                MenuItem::separator(),
                MenuItem::action("Agent 面板", workbench::ToggleAgentPanel),
                MenuItem::action("搜索 Agent 会话…", workbench::SearchSessions),
            ]),
            Menu::new("转到").items([
                MenuItem::action("返回", nav::NavigateBack),
                MenuItem::action("前进", nav::NavigateForward),
                MenuItem::separator(),
                MenuItem::action("转到定义", GoToDefinition),
                MenuItem::action("查找所有引用", nav::FindReferences),
                MenuItem::action("转到文件中的符号…", nav::GoToSymbol),
                MenuItem::action("转到行/列…", nav::GoToLine),
            ]),
        ]);
        let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
        cx.set_global(workbench::OpenDocuments(documents.clone()));
        // ⌘Q asks about unsaved changes window by window before quitting.
        cx.on_action(|_: &workbench::Quit, cx| workbench::quit(cx));
        session::track(cx);
        perf::watch_key_latency(cx);
        // Before the windows open: they restore what an abnormal exit left unsaved.
        if let Some(dir) = recovery::dir() {
            workbench::recovery::install(dir, cx);
        }
        // 新建窗口 from the menu bar when no window is open to handle it.
        let empty = service.clone();
        cx.on_action(move |_: &workbench::NewWindow, cx| open_empty_window(empty.clone(), cx));
        let displays = display_bounds(cx);
        // Command-line files go to the window whose folder holds them, else the first.
        let mut open: Vec<Vec<PathBuf>> = vec![Vec::new(); count];
        for file in files {
            let window = roots
                .iter()
                .position(|root| file.starts_with(root))
                .unwrap_or(0);
            open[window].push(file);
        }
        for (index, open) in open.into_iter().enumerate() {
            let saved = restored.get(index);
            let root = roots
                .get(index)
                .cloned()
                .or_else(|| saved.and_then(|saved| saved.root.clone()));
            let bounds = saved
                .and_then(|saved| saved.frame)
                .and_then(|frame| frame.window_bounds(&displays));
            let service = service.clone();
            let documents = documents.clone();
            // Tabs come back only for the folder they were open in.
            let (tabs, active) = saved
                .filter(|saved| {
                    roots.get(index).is_none() || saved.root.as_ref() == roots.get(index)
                })
                .map(|saved| (saved.tabs.clone(), saved.active.clone()))
                .unwrap_or_default();
            let startup = Startup { tabs, active, open };
            if let Err(e) = open_workspace(root, service, documents, index, bounds, startup, cx) {
                eprintln!("event=window_open_failed error={e}");
            }
        }
        let closing = service.clone();
        cx.on_window_closed(move |cx, id| window_closed(id, closing.clone(), cx))
            .detach();
        cx.activate(true);
    });
    Ok(())
}

// The vendored macOS renderer's low-memory policy is plain Rust; test it here so it runs on
// every platform (the renderer itself only builds on macOS).
#[cfg(test)]
#[path = "../../../vendor/gpui-pre-apple/src/zj_low_memory.rs"]
#[allow(dead_code)] // `enabled()` reads the environment for the renderer, not for the tests.
mod gpu_low_memory_policy;
