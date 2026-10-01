mod assets;
mod diff_doc;
mod diff_syntax;
mod file_icons;
mod files;
mod fuzzy;
mod languages;
mod prototype;
mod refresh_plan;
mod theme;
mod watch;
use gpui_kit::*;
use prototype::{DocumentOwners, Prototype};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};
use workspace_editor_git::GitService;

impl Global for watch::WatchService {}

fn open_workspace(
    root: Option<PathBuf>,
    service: GitService,
    documents: DocumentOwners,
    index: usize,
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
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(offset, offset),
            size(theme::WINDOW_WIDTH, theme::WINDOW_HEIGHT),
        ))),
        window_min_size: Some(size(theme::WINDOW_MIN_WIDTH, theme::WINDOW_MIN_HEIGHT)),
        ..gpui_kit::component::TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Prototype::new(root, service, documents, index + 1, window, cx))
    })?;
    Ok(())
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
                "ZJ [--windows 1..5] [工作区根目录 ...]\n轻量代码编辑器：多仓库源代码管理、并排 Diff、临时文件编辑。编辑不写磁盘，退出不保留。"
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
    let count = windows.unwrap_or(roots.len().max(1));
    if count > 5 || roots.len() > count {
        return Err("原型最多支持 5 个窗口，每个根目录一个窗口".into());
    }
    let service = GitService::new(2, Duration::from_secs(30))?;
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_global(watch::WatchService::default());
            diff_syntax::register();
            theme::follow_appearance(None, cx);
            cx.bind_keys([
                KeyBinding::new("cmd-shift-n", prototype::NewWindow, Some("WorkspaceEditor")),
                KeyBinding::new("cmd-s", prototype::SaveUnavailable, Some("WorkspaceEditor")),
                KeyBinding::new("cmd-o", prototype::OpenFile, Some("WorkspaceEditor")),
                KeyBinding::new(
                    "cmd-shift-o",
                    prototype::OpenFolder,
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
            ]);
            cx.set_menus([
                Menu::new("ZJ").items([MenuItem::os_submenu("服务", SystemMenuType::Services)]),
                Menu::new("文件").items([
                    MenuItem::action("新建窗口", prototype::NewWindow),
                    MenuItem::separator(),
                    MenuItem::action("打开文件…", prototype::OpenFile),
                    MenuItem::action("打开文件夹…", prototype::OpenFolder),
                ]),
            ]);
            let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
            for index in 0..count {
                let root = roots.get(index).cloned();
                let service = service.clone();
                let documents = documents.clone();
                if let Err(e) = open_workspace(root, service, documents, index, cx) {
                    eprintln!("无法创建窗口: {e}");
                }
            }
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    Ok(())
}
