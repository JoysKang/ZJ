//! A double click on the welcome page opens a new untitled file (as in VS Code); a single
//! click does nothing.

use super::test_support::{empty_store, open_window};
use super::*;
use crate::settings::Settings;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{
    MouseDownEvent, MouseUpEvent, TestAppContext, VisualTestContext, test::TestWindowExt,
};

fn click(cx: &mut VisualTestContext, position: Point<Pixels>, click_count: usize) {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
    });
}

#[gpui_kit::test]
async fn a_double_click_on_the_welcome_page_opens_an_untitled_file(cx: &mut TestAppContext) {
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    // Below the welcome page's list, in the editor area.
    let blank = point(px(1000.), px(820.));
    click(cx, blank, 1);
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.documents.is_empty()));
    click(cx, blank, 2);
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert_eq!(p.documents.len(), 1);
        assert!(p.documents[0].untitled);
        assert_eq!(p.active, Pane::Document(p.documents[0].id));
    });
}
