use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;

#[test]
fn recent_files_keep_closed_paths_and_follow_moves_with_a_bound() {
    let mut recent = RecentFiles::default();
    recent.activate(DocumentId(1), "/project/a.rs".into());
    recent.activate(DocumentId(2), "/project/sub/b.rs".into());
    recent.activate(DocumentId(1), "/project/a.rs".into());
    recent.close(DocumentId(2), "/project/sub/saved.rs".into());
    assert_eq!(recent.entries[0].document, Some(DocumentId(1)));
    assert_eq!(recent.entries[1].document, None);
    assert!(recent.contains_under(std::path::Path::new("/project/sub")));
    recent.move_paths(
        std::path::Path::new("/project/sub"),
        std::path::Path::new("/project/new"),
    );
    assert_eq!(
        recent.entries[1].path,
        PathBuf::from("/project/new/saved.rs")
    );
    recent.activate(DocumentId(3), "/project/new/saved.rs".into());
    assert_eq!(recent.entries.len(), 2);
    for i in 4..200 {
        recent.activate(DocumentId(i), format!("/project/{i}.rs").into());
        recent.close(DocumentId(i), format!("/project/{i}.rs").into());
    }
    assert_eq!(recent.entries.len(), RecentFiles::LIMIT);
    assert_eq!(recent.entries[0].path, PathBuf::from("/project/199.rs"));
}

#[gpui_kit::test]
async fn recent_files_reopen_closed_files_and_follow_save_as(cx: &mut TestAppContext) {
    use super::super::test_support::{open, settle};
    use gpui_kit::test::TestWindowExt;
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-recent-files-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.txt");
    let moved = root.join("saved.txt");
    std::fs::write(&path, "recent file\n").unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path.clone(), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let id = p.documents[0].id;
            // The shared path-change notification is the same one used after Save As.
            std::fs::rename(&path, &moved).unwrap();
            p.documents[0].path = moved.clone();
            p.document_path_changed(id, cx);
            p.remove_document(id, window, cx);
            p.open_quick_open(window, cx);
        });
        window.render_frame(cx);
        assert_eq!(
            this.read(cx).quick_open.as_ref().unwrap().results,
            vec![moved.clone()]
        );
        window.press("enter", cx);
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(p.documents[0].path, moved);
        assert_eq!(
            p.documents[0].editor.read(cx).text().to_string(),
            "recent file\n"
        );
    });
    let _ = std::fs::remove_dir_all(root);
}
