//! Replacing from the search view in a file whose buffer has unsaved edits: exactly the
//! results shown (ignored lines stay), and nothing when the buffer no longer matches them.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn replacing_in_an_edited_buffer_follows_the_results(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-search-replace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "x 1\nx 2\nx 3\n").unwrap();
    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    let search = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.search
                    .query
                    .update(cx, |input, cx| input.set_value("x", window, cx));
                p.search
                    .replace
                    .input
                    .update(cx, |input, cx| input.set_value("y", window, cx));
                p.schedule_search(Duration::ZERO, window, cx);
            });
        })
        .unwrap();
        settle(cx, None, |cx| {
            this.read_with(cx, |p, _| {
                p.search.results.first().is_some_and(|f| f.lines.len() == 3)
            })
        });
    };
    let buffer = |cx: &mut TestAppContext| editor.read_with(cx, |s, _| s.text().to_string());

    // An edit after the results: they still line up. Ignore "x 2", then replace the file.
    search(cx);
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |s, cx| {
            s.replace_text_in_range(Some(12..12), "x 4\n", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.ignore_line(0, 1, cx);
            p.replace_file(0, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(buffer(cx), "y 1\nx 2\ny 3\nx 4\n");
    // The file on disk is left alone.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "x 1\nx 2\nx 3\n");

    // An edit above the results moves them: nothing is replaced, and it says why.
    search(cx);
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |s, cx| {
            s.replace_text_in_range(Some(0..0), "top\n", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.replace_file(0, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.search.replace.summary.is_some())
    });
    assert_eq!(buffer(cx), "top\ny 1\nx 2\ny 3\nx 4\n");
    let summary = this.read_with(cx, |p, _| p.search.replace.summary.clone().unwrap().0);
    assert!(summary.contains("对不上"), "{summary}");
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn find_in_folder_searches_only_that_folder(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-find-in-folder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["sub", "other"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(root.join(dir).join("a.txt"), "needle\n").unwrap();
    }
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open(cx, root.clone());
    let sub = root.join("sub");
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.select_tree_path(sub.clone(), window, cx));
        window.render_frame(cx);
        window.dispatch_action(Box::new(FindInFolder), cx);
    })
    .unwrap();
    this.read_with(cx, |p, cx| {
        assert!(matches!(p.sidebar, Sidebar::Search));
        assert_eq!(p.search.include.read(cx).value().as_ref(), "./sub");
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.search
                .query
                .update(cx, |input, cx| input.set_value("needle", window, cx));
            p.schedule_search(Duration::ZERO, window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.search.results.is_empty())
    });
    this.read_with(cx, |p, _| {
        let found: Vec<_> = p.search.results.iter().map(|f| f.path.clone()).collect();
        assert_eq!(found, [sub.join("a.txt")]);
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn the_replacement_takes_effect_only_once_it_has_text(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("zj-replace-active-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open(cx, root.clone());
    let typed = |cx: &mut TestAppContext, text: &str| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.search.replace.input.update(cx, |input, cx| {
                    input.set_value(text.to_string(), window, cx)
                });
            });
        })
        .unwrap();
        cx.run_until_parked();
        this.read_with(cx, |p, cx| p.search.replace.active(cx))
    };
    // Always on screen; an empty replacement leaves results opening their files.
    assert!(!this.read_with(cx, |p, cx| p.search.replace.active(cx)));
    assert!(typed(cx, "y"));
    assert!(!typed(cx, ""));
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn the_ignored_files_switch_stays_in_the_column_and_toggles(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("zj-excludes-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    // The window where the old in-input button was pushed out of the sidebar.
    let mut settings = crate::settings::Settings::default();
    settings.agent.panel_visible = true;
    settings.agent.panel_width = 539.5;
    let (window, this) = cx.update(|cx| {
        let documents =
            super::test_support::install_globals(cx, settings, super::test_support::empty_store());
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1728.), px(1084.)));
        let (window, this) =
            super::test_support::new_window(cx, Some(root.clone()), documents, bounds);
        (window.downcast::<gpui_kit::base::Root>().unwrap(), this)
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.find_in_files(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    let cx = &mut gpui_kit::VisualTestContext::from_window(window.into(), cx);
    let exclude = cx.debug_bounds("search-exclude").unwrap();
    let query = cx.debug_bounds("search-query").unwrap();
    eprintln!("bounds: query={query:?} exclude={exclude:?}");
    assert!(
        exclude.right() <= query.right(),
        "{exclude:?} past {query:?}"
    );
    let on = |cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|_, cx| cx.global::<crate::settings::Settings>().search_use_excludes)
    };
    assert!(on(cx));
    // One line high while it fits (like the query), taller once the list wraps.
    let include = cx.debug_bounds("search-include").unwrap();
    assert_eq!(include.size.height, query.size.height);
    assert!(exclude.size.height > query.size.height);
    // The switch sits in the top right corner of the list, inside it.
    let switch = cx.debug_bounds("search-use-excludes").unwrap();
    eprintln!("bounds: switch={switch:?}");
    assert!(exclude.contains(&switch.origin) && switch.right() <= exclude.right());
    let knob = switch.center();
    cx.simulate_click(knob, Modifiers::default());
    assert!(!on(cx));
    cx.simulate_click(knob, Modifiers::default());
    assert!(on(cx));
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn the_exclude_list_starts_with_the_usual_folders_and_keeps_edits(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("zj-exclude-list-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open(cx, root.clone());
    let shown = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, cx| p.search.exclude.read(cx).value().to_string())
    };
    assert_eq!(shown(cx), crate::settings::SEARCH_EXCLUDE_DEFAULT);
    // Typed over by hand: kept for later once typing pauses.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.find_in_files(window, cx);
            p.search.exclude.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        });
        window.render_frame(cx);
        window.input("*.log", cx);
    })
    .unwrap();
    assert_eq!(shown(cx), "*.log");
    let saved = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            cx.global::<crate::settings::Settings>()
                .search_exclude
                .clone()
        })
    };
    assert_eq!(saved(cx), crate::settings::SEARCH_EXCLUDE_DEFAULT);
    cx.executor().advance_clock(Duration::from_secs(2));
    cx.run_until_parked();
    assert_eq!(saved(cx), "*.log");
    let _ = std::fs::remove_dir_all(root);
}
