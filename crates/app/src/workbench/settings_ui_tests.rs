//! ⌘, opens the settings file (created if missing; in tests a temporary file); saving it puts
//! it into effect, and a file that does not parse leaves the settings in use.

use super::test_support::{empty_store, open_window, settle};
use super::*;
use crate::settings::Settings;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn the_settings_file_opens_and_applies_when_saved(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    assert!(path.exists());

    let save = |cx: &mut TestAppContext, text: &str| {
        let (id, editor) = this.read_with(cx, |p, _| {
            (p.documents[0].id, p.documents[0].editor.clone())
        });
        let text = text.to_string();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| state.set_value(text, window, cx));
            this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap()
    };
    assert!(save(cx, r#"{ "editor_font_size": 19 }"#).await);
    cx.run_until_parked();
    cx.read(|cx| assert_eq!(cx.global::<Settings>().editor_font_size, 19.));
    this.read_with(cx, |p, _| assert_eq!(p.message, "设置已生效"));

    // A typo is saved as typed, but the settings in use stay.
    assert!(save(cx, r#"{ "editor_font_size": 12, "#).await);
    cx.run_until_parked();
    cx.read(|cx| assert_eq!(cx.global::<Settings>().editor_font_size, 19.));
    this.read_with(cx, |p, _| {
        assert!(p.message.contains("不是有效的 JSON"), "{}", p.message)
    });
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let _ = std::fs::remove_file(&path);
}
