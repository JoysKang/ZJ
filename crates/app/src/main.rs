mod agent_model;
mod assets;
mod diff_doc;
mod diff_syntax;
mod file_icons;
mod file_ops;
mod files;
mod fuzzy;
mod languages;
mod markdown;
mod partial_patch;
mod platform;
mod prototype;
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
use gpui_kit::component::input::GoToDefinition;
use gpui_kit::*;
use prototype::navigation as nav;
use prototype::{DocumentOwners, Prototype};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};
use workspace_editor_git::GitService;

impl Global for watch::WatchService {}

/// `bounds`: a frame restored from the last session; `None` cascades from the default place.
fn open_workspace(
    root: Option<PathBuf>,
    service: GitService,
    documents: DocumentOwners,
    index: usize,
    bounds: Option<WindowBounds>,
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
        cx.new(|cx| Prototype::new(root, service, documents, index + 1, window, cx))
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
    let documents = cx.global::<prototype::OpenDocuments>().0.clone();
    let index = cx.windows().len();
    if let Err(e) = open_workspace(None, service, documents, index, None, cx) {
        eprintln!("无法创建窗口: {e}");
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
    let documents = cx.global::<prototype::OpenDocuments>().0.clone();
    // Not from inside the close notification, which runs while GPUI removes the window.
    cx.defer(move |cx| {
        if let Err(e) = open_workspace(None, service, documents, 0, bounds, cx) {
            eprintln!("无法创建窗口: {e}");
        }
    });
}

#[allow(clippy::print_stdout)] // --help output belongs on stdout.
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut roots = Vec::new();
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
                "ZJ [--windows 1..5] [工作区根目录 ...]\n轻量代码编辑器：多仓库源代码管理、并排 / 内联 Diff、文件编辑与保存。"
            );
            return Ok(());
        } else {
            let root = std::fs::canonicalize(PathBuf::from(arg))?;
            if !root.is_dir() {
                return Err("工作区根必须是目录".into());
            }
            roots.push(root);
        }
    }
    // A launch without arguments (Dock, Finder) reopens the windows open at the last quit.
    let restored = if roots.is_empty() && windows.is_none() {
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
        if cx.windows().is_empty() && cx.has_global::<prototype::OpenDocuments>() {
            open_empty_window(reopen.clone(), cx);
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        cx.set_global(settings::Settings::load());
        cx.set_global(watch::WatchService::default());
        prototype::init_agent_store(cx);
        diff_syntax::register();
        platform::DockBlink::apply(cx);
        cx.observe_global::<settings::Settings>(platform::DockBlink::apply)
            .detach();
        theme::follow_appearance(None, cx);
        cx.bind_keys([
            KeyBinding::new("cmd-shift-n", prototype::NewWindow, Some("WorkspaceEditor")),
            KeyBinding::new("secondary-s", prototype::Save, Some("WorkspaceEditor")),
            KeyBinding::new(
                "secondary-shift-s",
                prototype::SaveAs,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "alt-secondary-s",
                prototype::SaveAll,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-n",
                prototype::NewUntitled,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-w",
                prototype::CloseEditor,
                Some("WorkspaceEditor"),
            ),
            // The tab menu's commands (VS Code's macOS keys), acting on the active tab.
            KeyBinding::new(
                "alt-cmd-t",
                prototype::CloseOtherEditors,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k cmd-w",
                prototype::CloseAllEditors,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k p",
                prototype::CopyActivePath,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k alt-cmd-c",
                prototype::CopyActiveRelativePath,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "cmd-k r",
                prototype::RevealActiveInFinder,
                Some("WorkspaceEditor"),
            ),
            // The terminal panel (VS Code's macOS keys).
            KeyBinding::new("ctrl-`", prototype::ToggleTerminal, Some("WorkspaceEditor")),
            KeyBinding::new(
                "ctrl-shift-`",
                prototype::NewTerminal,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("cmd-\\", prototype::SplitTerminal, Some("Terminal")),
            KeyBinding::new("secondary-q", prototype::Quit, None),
            KeyBinding::new("cmd-o", prototype::OpenFile, Some("WorkspaceEditor")),
            // VS Code: ⌘⇧O is go to symbol; open folder moves to ⌘K ⌘O.
            KeyBinding::new(
                "cmd-k cmd-o",
                prototype::OpenFolder,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-shift-o",
                nav::GoToSymbol,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("f12", GoToDefinition, Some("Input")),
            KeyBinding::new("shift-f12", nav::FindReferences, Some("WorkspaceEditor")),
            KeyBinding::new("secondary-c", prototype::CopyDiff, Some("DiffEditor")),
            KeyBinding::new("secondary-a", prototype::SelectAllDiff, Some("DiffEditor")),
            // Agent reviews (design): accept / reject the current change.
            KeyBinding::new(
                "secondary-y",
                prototype::AcceptAgentChange,
                Some("DiffEditor"),
            ),
            KeyBinding::new(
                "secondary-backspace",
                prototype::RejectAgentChange,
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
                prototype::QuickOpenFile,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-b",
                prototype::ToggleSidebar,
                Some("WorkspaceEditor"),
            ),
            // Explorer (VS Code's macOS bindings), acting on the selected row.
            KeyBinding::new("f2", prototype::RenameFile, Some("Explorer")),
            KeyBinding::new("enter", prototype::RenameFile, Some("Explorer")),
            KeyBinding::new("delete", prototype::DeleteFile, Some("Explorer")),
            KeyBinding::new("cmd-backspace", prototype::DeleteFile, Some("Explorer")),
            KeyBinding::new("secondary-c", prototype::CopyFiles, Some("Explorer")),
            KeyBinding::new("secondary-x", prototype::CutFiles, Some("Explorer")),
            KeyBinding::new("secondary-v", prototype::PasteFiles, Some("Explorer")),
            KeyBinding::new("alt-cmd-c", prototype::CopyPath, Some("Explorer")),
            KeyBinding::new(
                "alt-shift-cmd-c",
                prototype::CopyRelativePath,
                Some("Explorer"),
            ),
            KeyBinding::new("alt-cmd-r", prototype::RevealInFinder, Some("Explorer")),
            KeyBinding::new(
                "secondary-shift-f",
                prototype::FindInFiles,
                Some("WorkspaceEditor"),
            ),
            // ⌘⇧F inside an editor is find in files (Kit binds it to its replace panel).
            KeyBinding::new("secondary-shift-f", prototype::FindInFiles, Some("Input")),
            // The editor's find widget, with VS Code's macOS keys.
            KeyBinding::new(
                "secondary-f",
                prototype::FindInFile,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "alt-secondary-f",
                prototype::FindReplace,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("secondary-g", prototype::FindNext, Some("WorkspaceEditor")),
            KeyBinding::new(
                "secondary-shift-g",
                prototype::FindPrevious,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("f3", prototype::FindNext, Some("WorkspaceEditor")),
            KeyBinding::new("shift-f3", prototype::FindPrevious, Some("WorkspaceEditor")),
            KeyBinding::new(
                "secondary-shift-1",
                prototype::ReplaceOne,
                Some("FindWidget"),
            ),
            // Some keyboards report ⇧1 as "!".
            KeyBinding::new(
                "secondary-shift-!",
                prototype::ReplaceOne,
                Some("FindWidget"),
            ),
            KeyBinding::new("secondary-!", prototype::ReplaceOne, Some("FindWidget")),
            KeyBinding::new(
                "secondary-alt-enter",
                prototype::ReplaceAll,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-c",
                prototype::ToggleFindCase,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-w",
                prototype::ToggleFindWord,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-r",
                prototype::ToggleFindRegex,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-p",
                prototype::TogglePreserveCase,
                Some("FindWidget"),
            ),
            KeyBinding::new(
                "alt-secondary-l",
                prototype::ToggleFindInSelection,
                Some("FindWidget"),
            ),
            KeyBinding::new("cmd-=", prototype::ZoomIn, Some("WorkspaceEditor")),
            KeyBinding::new("cmd-+", prototype::ZoomIn, Some("WorkspaceEditor")),
            KeyBinding::new("cmd--", prototype::ZoomOut, Some("WorkspaceEditor")),
            KeyBinding::new("cmd-0", prototype::ZoomReset, Some("WorkspaceEditor")),
            // Agent panel: ⌥⌘B as VS Code's secondary side bar; ⌘J / ⌘L / ⌘⇧A as in the
            // design (session search, add the selection, next approval).
            KeyBinding::new(
                "alt-cmd-b",
                prototype::ToggleAgentPanel,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-j",
                prototype::SearchSessions,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-l",
                prototype::AddSelectionToAgent,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-shift-a",
                prototype::NextApproval,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new(
                "secondary-n",
                prototype::NewAgentSession,
                Some("AgentPanel"),
            ),
            // Finder's shortcut for hidden files.
            KeyBinding::new(
                "cmd-shift-.",
                prototype::ToggleHiddenFiles,
                Some("WorkspaceEditor"),
            ),
        ]);
        cx.set_menus([
            Menu::new("ZJ").items([
                MenuItem::os_submenu("服务", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("退出 ZJ", prototype::Quit),
            ]),
            Menu::new("文件").items([
                MenuItem::action("新建文件", prototype::NewUntitled),
                MenuItem::action("新建窗口", prototype::NewWindow),
                MenuItem::separator(),
                MenuItem::action("打开文件…", prototype::OpenFile),
                MenuItem::action("打开文件夹…", prototype::OpenFolder),
                MenuItem::separator(),
                MenuItem::action("保存", prototype::Save),
                MenuItem::action("另存为…", prototype::SaveAs),
                MenuItem::action("全部保存", prototype::SaveAll),
                MenuItem::separator(),
                MenuItem::action("关闭编辑器", prototype::CloseEditor),
                MenuItem::separator(),
                MenuItem::submenu(Menu::new("自动保存").items([
                    MenuItem::action("关闭", prototype::AutoSaveOff),
                    MenuItem::action("编辑后 1 秒", prototype::AutoSaveAfterDelay),
                    MenuItem::action("失去焦点时", prototype::AutoSaveOnFocusChange),
                ])),
            ]),
            Menu::new("查看").items([
                MenuItem::action("放大", prototype::ZoomIn),
                MenuItem::action("缩小", prototype::ZoomOut),
                MenuItem::action("重置缩放", prototype::ZoomReset),
                MenuItem::separator(),
                MenuItem::action("显示 / 隐藏点文件", prototype::ToggleHiddenFiles),
                MenuItem::separator(),
                MenuItem::action("Agent 面板", prototype::ToggleAgentPanel),
                MenuItem::action("搜索 Agent 会话…", prototype::SearchSessions),
            ]),
            Menu::new("转到").items([
                MenuItem::action("返回", nav::NavigateBack),
                MenuItem::action("前进", nav::NavigateForward),
                MenuItem::separator(),
                MenuItem::action("转到定义", GoToDefinition),
                MenuItem::action("查找所有引用", nav::FindReferences),
                MenuItem::action("转到文件中的符号…", nav::GoToSymbol),
            ]),
        ]);
        let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
        cx.set_global(prototype::OpenDocuments(documents.clone()));
        // ⌘Q asks about unsaved changes window by window before quitting.
        cx.on_action(|_: &prototype::Quit, cx| prototype::quit(cx));
        session::track(cx);
        // 新建窗口 from the menu bar when no window is open to handle it.
        let empty = service.clone();
        cx.on_action(move |_: &prototype::NewWindow, cx| open_empty_window(empty.clone(), cx));
        let displays = display_bounds(cx);
        for index in 0..count {
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
            if let Err(e) = open_workspace(root, service, documents, index, bounds, cx) {
                eprintln!("无法创建窗口: {e}");
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
