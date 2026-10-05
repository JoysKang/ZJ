//! An idle window schedules nothing: once a Git folder is loaded and an edited file has
//! settled, no timer is left waiting. A blinking caret (Kit's, without the `vendor/gpui-base`
//! patch), a spinner or a polling loop would keep one queued forever, and on macOS keep the
//! window drawing frames (CLAUDE.md: 空闲 CPU 0%，光标不闪).

use super::test_support::{TICK, open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

/// More one-shot timers than an idle window can have queued (debounces, deferred refreshes).
const MAX_ONE_SHOT_TIMERS: usize = 100;

#[gpui_kit::test]
async fn an_idle_window_with_an_edited_file_has_no_timer_left(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-idle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("main.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(init.status.success());

    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.focus(window, cx);
            state.replace_text_in_range(Some(0..0), "// idle\n", window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    // Let the edit's background work (Git status, highlighting) finish in real time.
    for _ in 0..20 {
        cx.executor().advance_clock(TICK);
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let dispatcher = cx.executor().dispatcher().clone();
    let dispatcher = dispatcher.as_test().unwrap();
    let mut fired = 0;
    while dispatcher.advance_clock_to_next_timer() {
        fired += 1;
        assert!(
            fired <= MAX_ONE_SHOT_TIMERS,
            "an idle window keeps scheduling timers (a blinking caret, a spinner or polling?)"
        );
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    let _ = std::fs::remove_dir_all(&root);
}
