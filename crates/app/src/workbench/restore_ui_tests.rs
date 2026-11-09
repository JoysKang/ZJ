//! Restoring the last session's window: the first frame already shows its saved sidebar.

use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, WindowBounds, base::Root, test::TestWindowExt};

/// The saved sidebar must be in place for the first frame, not after a second one.
#[gpui_kit::test]
async fn a_restored_window_renders_its_saved_sidebar_first(cx: &mut TestAppContext) {
    let handle = cx.update(|cx| {
        let documents = test_support::install_globals(cx, crate::settings::Settings::default());
        crate::session::track_at(None, cx);
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let saved = crate::session::SavedWindow {
            sidebar: crate::session::SidebarLayout {
                width: Some(333.),
                visible: Some(true),
                view: Some("search".into()),
            },
            ..Default::default()
        };
        crate::open_workspace(
            None,
            service,
            documents,
            0,
            None,
            crate::Startup {
                saved: Some(saved),
                ..Default::default()
            },
            cx,
        )
        .unwrap();
        cx.windows()
            .last()
            .copied()
            .unwrap()
            .downcast::<Root>()
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // 333 px panel with the 2 px inset on each side.
        assert_eq!(
            window.find("workbench-sidebar").bounds().size.width,
            px(329.)
        );
        window.find("search-scope-details");
    })
    .unwrap();
}

/// A preference wider than the restored window shrinks on the first frame, then comes back
/// when the window grows again.
#[gpui_kit::test]
async fn a_restored_sidebar_returns_to_its_width_when_the_window_grows(cx: &mut TestAppContext) {
    let handle = cx.update(|cx| {
        let documents = test_support::install_globals(cx, crate::settings::Settings::default());
        crate::session::track_at(None, cx);
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let saved = crate::session::SavedWindow {
            sidebar: crate::session::SidebarLayout {
                width: Some(520.),
                visible: Some(true),
                view: Some("explorer".into()),
            },
            ..Default::default()
        };
        let bounds =
            WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(760.), px(900.))));
        crate::open_workspace(
            None,
            service,
            documents,
            0,
            Some(bounds),
            crate::Startup {
                saved: Some(saved),
                ..Default::default()
            },
            cx,
        )
        .unwrap();
        cx.windows()
            .last()
            .copied()
            .unwrap()
            .downcast::<Root>()
            .unwrap()
    });
    let widths = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            (
                window.find("workbench-sidebar").bounds().size.width,
                window.find("workbench-editor").bounds().size.width,
            )
        })
        .unwrap()
    };
    // At the window minimum the 520 px preference cannot fit beside the editor's minimum.
    assert_eq!(widths(cx), (px(432.), px(320.)));
    cx.simulate_window_resize(handle.into(), size(px(1400.), px(900.)));
    cx.run_until_parked();
    // With room again the sidebar returns to the saved preference and the editor takes the
    // difference.
    assert_eq!(widths(cx), (px(516.), px(876.)));
}
