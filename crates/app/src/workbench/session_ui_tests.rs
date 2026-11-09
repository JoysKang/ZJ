//! Closing windows and the window record, in headless windows.

use super::test_support::{install_globals, new_window};
use super::*;
use crate::session::{self, SavedWindow};
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

fn open(
    cx: &mut TestAppContext,
    root: Option<PathBuf>,
    frame: Bounds<Pixels>,
    documents: &DocumentOwners,
) -> AnyWindowHandle {
    let documents = documents.clone();
    cx.update(|cx| new_window(cx, root, documents, frame).0)
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
    let service = GitService::new(1, Duration::from_secs(5)).unwrap();
    let documents = cx.update(|cx| {
        let documents = install_globals(cx, crate::settings::Settings::default());
        session::track_at(Some(path.clone()), cx);
        let service = service.clone();
        cx.on_window_closed(move |cx, id| crate::window_closed(id, service.clone(), cx))
            .detach();
        documents
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

#[gpui_kit::test]
fn reading_context_is_persisted_when_file_tabs_do_not_change(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-session-reading-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let path = base.join("session.json");
    let owners = cx.update(|cx| {
        let owners = install_globals(cx, crate::settings::Settings::default());
        session::track_at(Some(path.clone()), cx);
        owners
    });
    let window = open(
        cx,
        None,
        Bounds::new(Point::default(), size(px(1400.), px(900.))),
        &owners,
    );
    cx.update_window(window, |_, window, cx| {
        let saved = SavedWindow {
            extra: serde_json::Map::from_iter([("future_window".into(), serde_json::json!(7))]),
            ..Default::default()
        };
        session::restore_context(window, &saved, cx);
        let mut reading = session::review_progress::ReadingSession::default();
        reading
            .extra
            .insert("future_reader".into(), serde_json::json!(11));
        session::remember_reading(window, reading, false, cx);
        // Pinning a Diff changes reading context while file tabs and active file stay equal.
        session::remember_tabs(window, Vec::new(), None, true, cx);
    })
    .unwrap();
    let saved = session::load_from(&path);
    assert_eq!(saved[0].extra["future_window"], 7);
    assert_eq!(saved[0].reading.extra["future_reader"], 11);
    let _ = std::fs::remove_dir_all(base);
}

#[gpui_kit::test]
fn sidebar_layout_is_remembered_and_restored(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-session-layout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let session = base.join("session.json");
    let owners = cx.update(|cx| {
        let owners = install_globals(cx, crate::settings::Settings::default());
        session::track_at(Some(session.clone()), cx);
        owners
    });
    let (window, this) = cx.update(|cx| {
        new_window(
            cx,
            Some(base.clone()),
            owners.clone(),
            Bounds::new(Point::default(), size(px(1400.), px(900.))),
        )
    });
    cx.update_window(window, |_, window, cx| {
        this.update(cx, |p, cx| {
            p.sidebar_width = px(333.);
            p.sidebar_visible = false;
            p.sidebar = Sidebar::SourceControl;
            p.remember_layout(true, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    let saved = session::load_from(&session).remove(0);
    assert_eq!(saved.sidebar.width, Some(333.));
    assert_eq!(saved.sidebar.visible, Some(false));
    assert_eq!(saved.sidebar.view.as_deref(), Some("source_control"));

    // Recording the same layout again must not rewrite the file: a save replaces it, so the
    // inode would change.
    use std::os::unix::fs::MetadataExt;
    let inode = std::fs::metadata(&session).unwrap().ino();
    cx.update_window(window, |_, _, cx| {
        this.update(cx, |p, cx| p.remember_layout(true, cx));
    })
    .unwrap();
    assert_eq!(std::fs::metadata(&session).unwrap().ino(), inode);

    cx.update_window(window, |_, _, cx| {
        this.update(cx, |p, _| {
            p.sidebar_width = theme::SIDEBAR_WIDTH;
            p.sidebar_visible = true;
            p.sidebar = Sidebar::Explorer;
            p.restore_layout(&saved);
        });
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert_eq!(p.sidebar_width, px(333.));
        assert!(!p.sidebar_visible);
        assert!(p.sidebar == Sidebar::SourceControl);
    });

    // A restored window records the layout again even if no tab reopens before quitting.
    let restored = cx.update(|cx| {
        new_window(
            cx,
            Some(base.clone()),
            owners,
            Bounds::new(Point::default(), size(px(1400.), px(900.))),
        )
        .0
    });
    cx.update_window(restored, |_, window, cx| {
        session::restore_context(window, &saved, cx);
        session::remember(window, Some(&base), true, cx);
    })
    .unwrap();
    let saved = session::load_from(&session);
    assert_eq!(saved.last().unwrap().sidebar.width, Some(333.));
    assert_eq!(saved.last().unwrap().sidebar.visible, Some(false));
    assert_eq!(
        saved.last().unwrap().sidebar.view.as_deref(),
        Some("source_control")
    );
    let _ = std::fs::remove_dir_all(base);
}

#[gpui_kit::test]
async fn save_as_records_the_new_path_without_switching_tabs(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-session-save-as-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let base = std::fs::canonicalize(base).unwrap();
    let session = base.join("session.json");
    let owners = cx.update(|cx| {
        let owners = install_globals(cx, crate::settings::Settings::default());
        session::track_at(Some(session.clone()), cx);
        owners
    });
    let (window, this) = cx.update(|cx| {
        new_window(
            cx,
            None,
            owners,
            Bounds::new(Point::default(), size(px(1400.), px(900.))),
        )
    });
    let a = base.join("a.txt");
    let b = base.join("b.txt");
    let id = cx
        .update_window(window, |_, window, cx| {
            this.update(cx, |p, cx| p.new_untitled(window, cx))
        })
        .unwrap();
    let saved = cx
        .update_window(window, |_, window, cx| {
            this.update(cx, |p, cx| p.write_as(id, a.clone(), window, cx))
        })
        .unwrap();
    assert!(saved.await);
    assert_eq!(
        session::load_from(&session)[0].tabs,
        std::slice::from_ref(&a)
    );
    let saved = cx
        .update_window(window, |_, window, cx| {
            this.update(cx, |p, cx| p.write_as(id, b.clone(), window, cx))
        })
        .unwrap();
    assert!(saved.await);
    let saved = session::load_from(&session);
    assert_eq!(saved[0].tabs, std::slice::from_ref(&b));
    assert_eq!(saved[0].active, Some(b));
    let _ = std::fs::remove_dir_all(base);
}
