//! Git write results through real controls on temporary repositories.
use super::*;

#[gpui_kit::test]
async fn a_finished_git_write_is_one_status_bar_line(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("feedback-status", 0);
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
            !p.groups[0].write_pending && !p.message.is_empty()
        })
    });
    this.read_with(cx, |p, _| {
        assert!(p.notice.is_none(), "成功不打开失败对话框");
        assert!(
            p.message.ends_with("：同步完成") || p.message.ends_with("：远端已是最新"),
            "{}",
            p.message
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("status-message").visible());
        assert!(window.try_find("scm-notice").is_none());
        // The pinned strip's own ids: one of them coming back means the old surface did.
        assert!(window.try_find(("git-operation-result", 0usize)).is_none());
        assert!(window.try_find(("git-result-detail", 0usize)).is_none());
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn a_failed_git_write_is_one_dialog_with_selectable_output(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("feedback-failure", 0);
    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    let (handle, this) = open(cx, repo.clone());
    std::fs::write(repo.join(".git/index.lock"), "").unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.sidebar = Sidebar::SourceControl;
            let group = &p.groups[0];
            let request = WriteRequest {
                repo: group.repo.clone(),
                generation: 0,
                expected: group.status.as_ref().unwrap().as_ref().unwrap().clone(),
                operation: WriteOperation::Stage {
                    paths: vec!["a.txt".into()],
                },
            };
            p.request_git_write(request, window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.groups[0].write_pending && p.notice.is_some())
    });
    let full = this.read_with(cx, |p, _| {
        let notice = p.notice.as_ref().unwrap();
        assert!(notice.title.ends_with("：暂存失败"), "{}", notice.title);
        notice.detail.clone()
    });
    assert!(full.contains("index.lock"), "{full}");
    cx.write_to_clipboard(ClipboardItem::new_string("sentinel".into()));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let detail = window.find("scm-notice-detail").bounds();
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
        !selected.is_empty() && selected.len() < full.len() && full.contains(&selected),
        "must copy the selected characters: {selected:?}"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.notice.is_none(), "Escape closes it"));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("scm-notice").is_none());
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn repository_errors_open_the_full_list_from_the_status_bar(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-scm-issues-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    let base = std::fs::canonicalize(base).unwrap();
    let missing: Vec<String> = ["gone-a", "gone-b"]
        .into_iter()
        .map(|name| base.join(name).to_string_lossy().into_owned())
        .collect();
    let settings = crate::settings::Settings {
        extra_repos: [(base.to_string_lossy().into_owned(), missing)].into(),
        ..Default::default()
    };
    let (window, this) =
        crate::workbench::test_support::open_window(cx, Some(base.clone()), settings);
    settle(cx, None, |cx| {
        crate::workbench::test_support::loaded(cx, &this)
    });
    this.read_with(cx, |p, _| {
        assert_eq!(p.issues.len(), 2, "{:?}", p.issues);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("status-issues", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let detail = this.read_with(cx, |p, _| {
        let notice = p.notice.as_ref().expect("the count must open the list");
        assert_eq!(notice.title, "2 项仓库错误");
        notice.detail.clone()
    });
    assert!(
        detail.contains("gone-a") && detail.contains("gone-b"),
        "{detail}"
    );
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("scm-notice").visible());
        window.click("scm-notice-copy", cx);
    })
    .unwrap();
    let copied = cx.read_from_clipboard().unwrap().text().unwrap();
    assert_eq!(copied, detail);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.notice.is_none(), "Escape closes it"));
    // The count is a real button: Return reopens the list without a mouse, and Return
    // again answers its primary button (关闭).
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(p.notice.is_some(), "Return on the count opens it");
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.notice.is_none(), "Enter closes it"));
    let _ = std::fs::remove_dir_all(&base);
}

#[gpui_kit::test]
async fn closing_a_failure_dialog_gives_focus_back(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("feedback-focus", 0);
    let (handle, this) = open(cx, repo.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            // Somewhere specific, neither the root handle nor the sidebar: closing must put
            // the keyboard back exactly here.
            p.scm_focus.clone().focus(window, cx);
            p.report_git_failure("repo：推送失败".into(), "fatal: nope".into(), window, cx);
        });
    })
    .unwrap();
    this.read_with(cx, |p, _| assert!(p.notice.is_some()));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        let p = this.read(cx);
        assert!(p.notice.is_none());
        assert!(
            p.scm_focus.is_focused(window),
            "focus must go back to where the user was, not to the sidebar"
        );
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn a_second_failure_waits_for_the_first_dialog(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("feedback-queue", 0);
    let (handle, this) = open(cx, repo.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.report_git_failure("a：暂存失败".into(), "first".into(), window, cx);
            p.report_git_failure("b：推送失败".into(), "second".into(), window, cx);
            // The same reason again (a retrying auto-save) adds nothing to the queue.
            p.report_git_failure("a：暂存失败".into(), "first".into(), window, cx);
            assert_eq!(p.queued_notices.len(), 1);
        });
    })
    .unwrap();
    let first = this.read_with(cx, |p, _| p.notice.as_ref().unwrap().title.clone());
    assert_eq!(first, "a：暂存失败");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let second = this.read_with(cx, |p, _| {
        p.notice
            .as_ref()
            .map(|notice| notice.title.clone())
            .expect("the second failure follows the first")
    });
    assert_eq!(second, "b：推送失败");
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
