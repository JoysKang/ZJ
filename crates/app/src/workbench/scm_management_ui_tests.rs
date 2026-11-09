use super::*;
#[gpui_kit::test]
async fn git_management_publish_requires_remote_target_and_keeps_copyable_result(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let repo = fixture("management-publish", 0);
    git(&repo, &["branch", "--unset-upstream"]);
    let (handle, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        p.hide_clean_repos = false;
        p.groups[0].expanded = true;
        p.rebuild_rows();
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("scm-publish", 0usize), cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.quick_open.is_some())
    });
    this.read_with(cx, |p, _| {
        let items = &p.quick_open.as_ref().unwrap().items.as_ref().unwrap().0;
        assert_eq!(items[0].label, "origin");
        assert!(items[0].detail.contains("remote.git"));
    });
    assert!(!cx.has_pending_prompt());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("published-ui", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    settle(cx, None, |cx| cx.has_pending_prompt());
    let (_, detail) = cx.pending_prompt().unwrap();
    assert!(detail.contains("origin/refs/heads/published-ui"));
    cx.simulate_prompt_answer("发布分支");
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            !p.groups[0].write_pending && p.groups[0].write_message.contains("上游配置已回读")
        })
    });
    this.read_with(cx, |p, _| {
        assert!(p.groups[0].write_message.contains("远程引用已更新"));
        assert!(p.groups[0].write_message.contains("published-ui"));
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("git-copy-result", 0usize), cx);
    })
    .unwrap();
    let copied = cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.contains("published-ui"));
    assert!(copied.contains("refs/heads/"));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
#[gpui_kit::test]
async fn git_management_remote_view_copies_real_push_address(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("management-remotes", 0);
    let push = repo.parent().unwrap().join("separate-push.git");
    git(
        &repo,
        &["config", "remote.origin.pushurl", push.to_str().unwrap()],
    );
    let (handle, this) = open(cx, repo.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_git_management(0, "remote", window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.quick_open.is_some())
    });
    let pick = this.read_with(cx, |p, _| {
        p.quick_open
            .as_ref()
            .unwrap()
            .items
            .as_ref()
            .unwrap()
            .0
            .iter()
            .find(|i| i.label.starts_with("查看／复制"))
            .unwrap()
            .pick
            .clone()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            if let super::super::super::quick_open::Pick::GitFlow { repo, step } = pick {
                p.scm_git_flow(&repo, step, String::new(), window, cx);
            }
        });
    })
    .unwrap();
    let copied = cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.contains("separate-push.git"));
    assert!(copied.contains("Fetch："));
    assert!(copied.contains("Push："));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
