//! The agent panel in a headless window: typing and sending from the composer.

use super::*;
// `super::*` brings in GPUI's `test` macro through `gpui_kit::*`; `#[gpui_kit::test]` expands
// to the built-in one.
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions, base::Root};
use workspace_editor_agent::registry::UserAgentConfig;

fn fake_agent() -> UserAgentConfig {
    let exe = std::env::current_exe().unwrap();
    let program = exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("zj-fake-acp-agent");
    assert!(
        program.exists(),
        "{} is missing: run `cargo test --workspace` (it builds the agent crate's test binary)",
        program.display()
    );
    UserAgentConfig {
        id: "fake".into(),
        name: Some("Fake".into()),
        command: program.display().to_string(),
        args: Vec::new(),
        env: Default::default(),
    }
}

fn open(cx: &mut TestAppContext, root: Option<PathBuf>) -> (WindowHandle<Root>, Entity<Prototype>) {
    open_with(
        cx,
        root,
        AgentStore {
            history: None,
            default_workspace: None,
        },
    )
}

fn open_with(
    cx: &mut TestAppContext,
    root: Option<PathBuf>,
    store: AgentStore,
) -> (WindowHandle<Root>, Entity<Prototype>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let mut settings = crate::settings::Settings::default();
        settings.agent.panel_visible = true;
        settings.agent.custom = vec![fake_agent()];
        settings.agent.default_agent = "fake".into();
        cx.set_global(settings);
        cx.set_global(crate::watch::WatchService::default());
        cx.set_global(store);
        let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
        cx.set_global(OpenDocuments(documents.clone()));
        let service = GitService::new(1, std::time::Duration::from_secs(5)).unwrap();
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        let (window, content) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Prototype::new(root, service, documents, 1, window, cx)),
        )
        .unwrap();
        (window.downcast::<Root>().unwrap(), content)
    })
}

fn send(cx: &mut TestAppContext, handle: WindowHandle<Root>, this: &Entity<Prototype>, text: &str) {
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |this, cx| this.agent_focus_composer(window, cx));
        window.render_frame(cx);
        window.input(text, cx);
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
}

/// Waits (real time: the agent is a child process) until the current session is idle again.
fn settle(cx: &mut TestAppContext, this: &Entity<Prototype>) {
    for _ in 0..200 {
        cx.run_until_parked();
        let idle = this.read_with(cx, |p, _| {
            p.agent
                .current()
                .is_some_and(|s| !s.busy() && s.client.is_some())
        });
        if idle {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the agent never finished the turn");
}

/// The user messages of the current session, and what is left in the composer.
fn sent(cx: &mut TestAppContext, this: &Entity<Prototype>) -> (Vec<String>, String) {
    this.read_with(cx, |p, cx| {
        let users = p
            .agent
            .current()
            .map(|s| {
                s.thread
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        Item::User { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        (users, p.agent.composer.read(cx).value().to_string())
    })
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("zj-ui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::canonicalize(root).unwrap()
}

#[gpui_kit::test]
async fn mention_without_a_folder_says_so(cx: &mut TestAppContext) {
    let (handle, this) = open(cx, None);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |this, cx| this.agent_focus_composer(window, cx));
        window.input("@ma", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        let mention = p.agent.mention.as_ref().expect("the @ picker is open");
        assert_eq!(mention.query, "ma");
        assert!(mention.results.is_empty());
        assert_eq!(
            p.agent_mention_hint(),
            "没有打开文件夹，也没有打开的文件可以引用"
        );
    });
}

#[gpui_kit::test]
async fn enter_sends_every_turn(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("send");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "echo one");
    settle(cx, &this);
    send(cx, handle, &this, "echo two");
    settle(cx, &this);
    assert_eq!(
        sent(cx, &this),
        (
            vec!["echo one".to_string(), "echo two".to_string()],
            String::new()
        )
    );
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn without_a_folder_the_session_runs_in_the_default_workspace(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let data = temp_root("default");
    let workspace = data.join("workspace");
    let (handle, this) = open_with(
        cx,
        None,
        AgentStore {
            history: Some(Arc::new(History::new(data.join("history.sqlite")))),
            default_workspace: Some(workspace.clone()),
        },
    );
    send(cx, handle, &this, "echo hi");
    settle(cx, &this);
    assert!(!cx.did_prompt_for_paths());
    assert!(workspace.is_dir(), "the default workspace is created");
    assert_eq!(
        sent(cx, &this),
        (vec!["echo hi".to_string()], String::new())
    );
    this.read_with(cx, |p, _| {
        assert_eq!(p.root, None);
        assert_eq!(p.agent.current().unwrap().root, Some(workspace.clone()));
    });

    // The history list of a window without a folder is the default workspace's.
    let mut rows = Vec::new();
    for _ in 0..100 {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |this, cx| this.agent_reload_history(window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        rows = this.read_with(cx, |p, _| p.agent.history.rows.clone());
        if !rows.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].workspace_root, workspace);
    this.read_with(cx, |p, cx| {
        assert_eq!(p.agent.history.counts.0, 1);
        assert_eq!(p.agent_scope_label(false, cx), "默认工作区");
        assert_eq!(workspace_label(&workspace, cx), "默认工作区");
    });
    let _ = std::fs::remove_dir_all(data);
}
