mod files;
mod prototype;
use gpui_kit::*;
use prototype::{DocumentOwners, Prototype};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};
use workspace_editor_git::GitService;

fn open_workspace(
    root: Option<PathBuf>,
    service: GitService,
    documents: DocumentOwners,
    index: usize,
    cx: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(60. + index as f32 * 28.), px(60. + index as f32 * 28.)),
            size(px(1100.), px(720.)),
        ))),
        window_min_size: Some(size(px(760.), px(480.))),
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Prototype::new(root, service, documents, index + 1, window, cx))
    })?;
    Ok(())
}

#[allow(clippy::print_stdout)] // --help output belongs on stdout.
fn main() -> Result<(), Box<dyn std::error::Error>> {
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
                "workspace-editor [--windows 1..5] [工作区根目录 ...]\nP1 原型：目录、文件名搜索、临时文件编辑、只读 Changes、双状态 diff。编辑不写磁盘，退出不保留。"
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
    let count = windows.unwrap_or(roots.len().max(3));
    if count > 5 || roots.len() > count {
        return Err("原型最多支持 5 个窗口，每个根目录一个窗口".into());
    }
    let service = GitService::new(2, Duration::from_secs(30))?;
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-s", prototype::SaveUnavailable, Some("WorkspaceEditor")),
                KeyBinding::new("cmd-o", prototype::OpenFile, Some("WorkspaceEditor")),
                KeyBinding::new(
                    "cmd-shift-o",
                    prototype::OpenFolder,
                    Some("WorkspaceEditor"),
                ),
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
