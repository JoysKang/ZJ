//! Feedback and navigation through real controls on temporary repositories.
use super::*;

#[gpui_kit::test]
async fn git_feedback_is_compact_until_details_are_requested(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("feedback-disclosure", 0);
    let (handle, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        p.hide_clean_repos = false;
        p.rebuild_rows();
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("scm-repo-sync", 0usize), cx);
    })
    .unwrap();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("同步");
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            !p.groups[0].write_pending && !p.groups[0].write_message.is_empty()
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let notice = window.find(("git-operation-result", 0usize));
        assert!(
            notice.bounds().size.height <= theme::ROW_HEIGHT,
            "ordinary results must fit one row: {:?}",
            notice.bounds()
        );
        assert!(window.try_find(("git-result-detail", 0usize)).is_none());
        window.click(("git-result-toggle", 0usize), cx);
        window.render_frame(cx);
        assert!(window.find(("git-result-detail", 0usize)).visible());
        window.click(("git-copy-result", 0usize), cx);
    })
    .unwrap();
    let copied = cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.contains(repo.to_str().unwrap()));
    assert!(copied.contains("main"));
    cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into()));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let detail = window.find(("git-result-detail", 0usize)).bounds();
        let start = detail.origin
            + point(
                theme::ROW_INSET * 2.,
                theme::ROW_INSET * 2. + theme::SCM_DETAIL_LINE / 2.,
            );
        window.drag(start, start + point(theme::TEXT_BODY * 2., px(0.)), cx);
        window.press("cmd-c", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let selected = cx.read_from_clipboard().unwrap().text().unwrap();
    assert_ne!(selected, "sentinel");
    assert!(
        !selected.is_empty() && selected.len() < copied.len() && copied.contains(&selected),
        "must copy the selected characters: {selected:?}"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(("git-result-toggle", 0usize), cx);
        window.render_frame(cx);
        assert!(window.try_find(("git-result-detail", 0usize)).is_none());
        window.click(("git-result-dismiss", 0usize), cx);
        window.render_frame(cx);
        assert!(window.try_find(("git-operation-result", 0usize)).is_none());
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn sidebar_destinations_are_keyboard_operable_native_tabs(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("sidebar-keyboard", 0);
    let (handle, this) = open(cx, repo.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("资源管理器", cx);
        window.render_frame(cx);
        assert_eq!(window.find("资源管理器").focused(), Some(true));
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("源代码管理").focused(), Some(true));
        window.press("space", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.sidebar == Sidebar::SourceControl));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
