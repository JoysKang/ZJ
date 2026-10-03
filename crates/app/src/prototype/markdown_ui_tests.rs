//! The Markdown live preview in a headless window, on a temporary folder.

use super::agent::AgentStore;
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions, base::Root, test::TestWindowExt};

fn open(cx: &mut TestAppContext, root: PathBuf) -> (WindowHandle<Root>, Entity<Prototype>) {
    let (window, this) = cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::settings::Settings::default());
        cx.set_global(crate::watch::WatchService::default());
        cx.set_global(AgentStore {
            history: None,
            default_workspace: None,
        });
        let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
        cx.set_global(OpenDocuments(documents.clone()));
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Prototype::new(Some(root), service, documents, 1, window, cx)),
        )
        .map(|(window, this)| (window.downcast::<Root>().unwrap(), this))
        .unwrap()
    });
    settle(cx, window, |cx| {
        this.read_with(cx, |p, _| p.refresh_completed && !p.loading)
    });
    (window, this)
}

fn settle(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    for _ in 0..200 {
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the operation never settled");
}

fn blocks(cx: &mut TestAppContext, this: &Entity<Prototype>) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.documents[0].markdown.as_ref().unwrap().block_texts()
    })
}

fn editing(cx: &mut TestAppContext, this: &Entity<Prototype>) -> Option<Option<usize>> {
    this.read_with(cx, |p, _| {
        p.documents[0].markdown.as_ref().unwrap().editing_block()
    })
}

fn buffer(cx: &mut TestAppContext, this: &Entity<Prototype>) -> String {
    this.read_with(cx, |p, cx| {
        p.documents[0].editor.read(cx).text().to_string()
    })
}

fn source(cx: &mut TestAppContext, this: &Entity<Prototype>) -> bool {
    this.read_with(cx, |p, _| p.documents[0].markdown.as_ref().unwrap().source)
}

fn window_do(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    f: impl FnOnce(&mut Window, &mut App),
) {
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        f(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn markdown_opens_rendered_and_edits_one_block_at_a_time(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-markdown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(root.join("README.md"), "# 标题\n\n第一段\n\n- 一\n- 二\n").unwrap();
    let (window, this) = open(cx, root.clone());
    let path = root.join("README.md");
    window_do(cx, window, |window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path, Some(root.clone()), window, cx)
        })
    });
    settle(cx, window, |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1) && blocks(cx, &this).len() == 3
    });
    assert!(!source(cx, &this));
    assert_eq!(blocks(cx, &this), ["# 标题", "第一段", "- 一\n- 二"]);

    // A click turns the block into its source; typing goes into the buffer.
    window_do(cx, window, |window, cx| {
        window.click(("md-block", 1usize), cx)
    });
    assert_eq!(editing(cx, &this), Some(Some(1)));
    window_do(cx, window, |window, cx| window.input("，加一句", cx));
    assert_eq!(
        buffer(cx, &this),
        "# 标题\n\n第一段，加一句\n\n- 一\n- 二\n"
    );
    this.read_with(cx, |p, _| assert!(p.documents[0].dirty));

    // Esc renders it again.
    window_do(cx, window, |window, cx| window.press("escape", cx));
    assert_eq!(editing(cx, &this), None);
    settle(cx, window, |cx| blocks(cx, &this)[1] == "第一段，加一句");

    // The room after the last block starts a new one, a blank line away from the list.
    window_do(cx, window, |window, cx| window.click("md-tail", cx));
    assert_eq!(editing(cx, &this), Some(None));
    window_do(cx, window, |window, cx| window.input("新段落", cx));
    assert_eq!(
        buffer(cx, &this),
        "# 标题\n\n第一段，加一句\n\n- 一\n- 二\n\n新段落"
    );

    // Clicking another block ends that edit and starts this one.
    window_do(cx, window, |window, cx| {
        window.click(("md-block", 0usize), cx)
    });
    assert_eq!(editing(cx, &this), Some(Some(0)));
    window_do(cx, window, |window, cx| window.press("escape", cx));
    settle(cx, window, |cx| blocks(cx, &this).len() == 4);
    assert_eq!(blocks(cx, &this)[3], "新段落");

    // ⇧⌘V shows the source editor, focused; edits there show in the preview.
    window_do(cx, window, |window, cx| {
        window.dispatch_action(Box::new(ToggleMarkdownPreview), cx)
    });
    assert!(source(cx, &this));
    window_do(cx, window, |window, cx| {
        let focused = this.read(cx).documents[0]
            .editor
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        assert!(focused);
        window.input("X", cx);
    });
    window_do(cx, window, |window, cx| {
        window.dispatch_action(Box::new(ToggleMarkdownPreview), cx)
    });
    assert!(!source(cx, &this));
    settle(cx, window, |cx| blocks(cx, &this).concat().contains('X'));

    // Find works on the source.
    window_do(cx, window, |window, cx| {
        this.update(cx, |p, cx| p.open_find(false, window, cx))
    });
    assert!(source(cx, &this));
    let _ = std::fs::remove_dir_all(&root);
}
