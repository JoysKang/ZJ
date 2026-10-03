//! Closing windows and the window record, in headless windows.

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

fn close(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn closing_windows_falls_back_to_an_empty_window_and_never_quits(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-session-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (a, b) = (base.join("a"), base.join("b"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let path = base.join("session.json");
    let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
    let service = GitService::new(1, Duration::from_secs(5)).unwrap();
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::settings::Settings::default());
        cx.set_global(crate::watch::WatchService::default());
        cx.set_global(AgentStore {
            history: None,
            default_workspace: None,
        });
        cx.set_global(OpenDocuments(documents.clone()));
        session::track_at(Some(path.clone()), cx);
        let service = service.clone();
        cx.on_window_closed(move |cx, id| crate::window_closed(id, service.clone(), cx))
            .detach();
    });
    let windows = |cx: &mut TestAppContext| cx.update(|cx| cx.windows().len());

    // Opening writes the window with its folder and frame.
    let frame = Bounds::new(point(px(40.), px(60.)), size(px(1200.), px(800.)));
    let first = open(cx, Some(a.clone()), frame, &documents);
    let second = open(cx, Some(b.clone()), frame, &documents);
    cx.run_until_parked();
    assert_eq!(roots(&path), [Some(a.clone()), Some(b.clone())]);
    let restored = session::load_from(&path)[0].frame.unwrap();
    assert_eq!((restored.x, restored.width), (40., 1200.));

    // Closing one of two only closes that window.
    close(cx, first);
    assert_eq!(windows(cx), 1);
    assert_eq!(roots(&path), [Some(b.clone())]);

    // Closing the last workspace window brings up an empty window in its place.
    close(cx, second);
    assert_eq!(windows(cx), 1);
    assert_eq!(roots(&path), [None]);
    let empty = cx.update(|cx| cx.windows()[0]);
    let placed = session::load_from(&path)[0].frame.unwrap();
    assert_eq!((placed.x, placed.y), (40., 60.));

    // Closing that one leaves no window, and ZJ keeps running with the empty window recorded.
    close(cx, empty);
    assert_eq!(windows(cx), 0);
    assert_eq!(roots(&path), [None]);

    // The Dock icon opens an empty window again.
    cx.update(|cx| crate::open_empty_window(service.clone(), cx));
    cx.run_until_parked();
    assert_eq!(windows(cx), 1);
    assert_eq!(roots(&path), [None]);

    // Quitting writes every open window.
    open(cx, Some(a.clone()), frame, &documents);
    cx.run_until_parked();
    std::fs::remove_file(&path).unwrap();
    cx.quit();
    assert_eq!(roots(&path), [None, Some(a)]);
    let _ = std::fs::remove_dir_all(base);
}
