//! The window record across opening, closing and quitting, in headless windows.

use super::agent::AgentStore;
use super::*;
use crate::session::{self, SavedWindow};
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions};

fn open(
    cx: &mut TestAppContext,
    root: Option<PathBuf>,
    frame: Bounds<Pixels>,
    documents: &DocumentOwners,
) -> AnyWindowHandle {
    let documents = documents.clone();
    cx.update(|cx| {
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(frame)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Prototype::new(root, service, documents, 1, window, cx)),
        )
        .map(|(window, _)| window)
        .unwrap()
    })
}

fn roots(path: &std::path::Path) -> Vec<Option<PathBuf>> {
    session::load_from(path)
        .into_iter()
        .map(|window: SavedWindow| window.root)
        .collect()
}

#[gpui_kit::test]
async fn windows_are_recorded_until_the_last_one_and_written_on_quit(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-session-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (a, b) = (base.join("a"), base.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let path = base.join("session.json");
    let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::settings::Settings::default());
        cx.set_global(crate::watch::WatchService::default());
        cx.set_global(AgentStore {
            history: None,
            default_workspace: None,
        });
        cx.set_global(OpenDocuments(documents.clone()));
        session::track_at(path.clone(), cx);
    });

    // Opening writes the window with its folder and frame.
    let frame = Bounds::new(point(px(40.), px(60.)), size(px(1200.), px(800.)));
    let first = open(cx, Some(a.clone()), frame, &documents);
    let second = open(cx, Some(b.clone()), frame, &documents);
    cx.run_until_parked();
    assert_eq!(roots(&path), [Some(a.clone()), Some(b.clone())]);
    let saved = session::load_from(&path);
    let restored = saved[0].frame.unwrap();
    assert_eq!((restored.x, restored.width), (40., 1200.));

    // Closing one of two drops it from the record.
    cx.update_window(first, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(roots(&path), [Some(b.clone())]);

    // Closing the last window keeps it: that close quits, and the next launch reopens it.
    cx.update_window(second, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(roots(&path), [Some(b.clone())]);

    // Quitting with windows open writes all of them, including a window without a folder.
    std::fs::remove_file(&path).unwrap();
    open(cx, Some(a.clone()), frame, &documents);
    open(cx, None, frame, &documents);
    cx.run_until_parked();
    cx.quit();
    assert_eq!(roots(&path), [Some(a), None]);
    let _ = std::fs::remove_dir_all(base);
}
