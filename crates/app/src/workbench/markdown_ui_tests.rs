//! The Markdown live preview in a headless window, on a temporary folder.

use super::test_support::{empty_store, loaded, open_window, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

/// Renders while it waits, unlike `test_support::open`.
fn open(cx: &mut TestAppContext, root: PathBuf) -> (WindowHandle<Root>, Entity<Workbench>) {
    let (window, this) = open_window(cx, Some(root), Default::default(), empty_store());
    settle(cx, Some(window), |cx| loaded(cx, &this));
    (window, this)
}

fn blocks(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.documents[0].markdown.as_ref().unwrap().block_texts()
    })
}

fn editing(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Option<Option<usize>> {
    this.read_with(cx, |p, _| {
        p.documents[0].markdown.as_ref().unwrap().editing_block()
    })
}

fn buffer(cx: &mut TestAppContext, this: &Entity<Workbench>) -> String {
    this.read_with(cx, |p, cx| {
        p.documents[0].editor.read(cx).text().to_string()
    })
}

fn source(cx: &mut TestAppContext, this: &Entity<Workbench>) -> bool {
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
    settle(cx, Some(window), |cx| {
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
    settle(cx, Some(window), |cx| {
        blocks(cx, &this)[1] == "第一段，加一句"
    });

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
    settle(cx, Some(window), |cx| blocks(cx, &this).len() == 4);
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
    settle(cx, Some(window), |cx| {
        blocks(cx, &this).concat().contains('X')
    });

    // Find works on the source.
    window_do(cx, window, |window, cx| {
        this.update(cx, |p, cx| p.open_find(false, window, cx))
    });
    assert!(source(cx, &this));
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn local_images_resolve_remote_ones_do_not_and_relative_links_open_here(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-markdown-images-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("docs/img")).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&4u32.to_be_bytes());
    png.extend_from_slice(&4u32.to_be_bytes());
    std::fs::write(root.join("docs/img/a.png"), png).unwrap();
    std::fs::write(root.join("docs/other.md"), "# 另一个\n").unwrap();
    std::fs::write(
        root.join("docs/guide.md"),
        "![本地](img/a.png)\n\n![远程](https://example.com/b.png)\n\n[另一个](other.md)\n",
    )
    .unwrap();
    let (window, this) = open(cx, root.clone());
    let path = root.join("docs/guide.md");
    let folder = Some(root.clone());
    window_do(cx, window, |window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    });
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            p.documents
                .first()
                .and_then(|d| d.markdown.as_ref())
                .is_some_and(|md| md.images.len() == 2)
        })
    });
    this.read_with(cx, |p, _| {
        let images = &p.documents[0].markdown.as_ref().unwrap().images;
        assert_eq!(
            images.get("img/a.png"),
            Some(&crate::md_images::Resolved::Local(
                root.join("docs/img/a.png")
            ))
        );
        assert!(matches!(
            images.get("https://example.com/b.png"),
            Some(crate::md_images::Resolved::Blocked(_))
        ));
    });

    let id = this.read_with(cx, |p, _| p.documents[0].id);
    window_do(cx, window, |window, cx| {
        this.update(cx, |p, cx| p.markdown_link(id, "other.md", window, cx));
    });
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 2)
    });
    let _ = std::fs::remove_dir_all(&root);
}
