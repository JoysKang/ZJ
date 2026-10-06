//! Switching between ZJ windows flashes the workspace's name a little above the middle of the
//! window, gray on a feathered halo: it grows in, holds and fades out, about a second, to tell
//! the windows apart. Not when coming
//! back from another app, with a single window, or when a new window first shows. One-off: no
//! frames once it is gone; with
//! "reduce motion" it shows still and goes away at once.

use super::*;
use gpui_kit::{Animation, component::v_flex};
use std::time::Instant;

const SHOWN: Duration = Duration::from_millis(1000);
/// Shares of [`SHOWN`]: growing in at the start, fading out at the end.
const GROW: f32 = 0.12;
const FADE: f32 = 0.3;
/// Smallest size while growing in.
const FROM_SCALE: f32 = 0.92;
/// Another ZJ window lost the focus this recently: the activation is a switch between them.
const SWITCH_GAP: Duration = Duration::from_millis(500);

/// The active ZJ window, and the last one to lose the focus.
#[derive(Default)]
struct ActiveWindow {
    active: Option<WindowId>,
    left: Option<(WindowId, Instant)>,
}

impl Global for ActiveWindow {}

#[derive(Default)]
pub(super) struct SwitchHud {
    /// The window has been active before (a new window needs no reminder).
    seen: bool,
    /// The showing's number (the animation's id), while the name is on screen.
    shown: Option<u64>,
    next: u64,
    task: Option<Task<()>>,
}

/// (scale, opacity) at `t` (0..=1) of the showing.
fn frame(t: f32) -> (f32, f32) {
    let grown = (t / GROW).min(1.0);
    let eased = 1.0 - (1.0 - grown).powi(3);
    let opacity = if t < GROW {
        eased
    } else {
        ((1.0 - t) / FADE).clamp(0.0, 1.0)
    };
    (FROM_SCALE + (1.0 - FROM_SCALE) * eased, opacity)
}

impl Workbench {
    /// This window gained or lost the focus.
    pub(super) fn switch_hud_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = window.window_handle().window_id();
        let tracker = cx.default_global::<ActiveWindow>();
        if !window.is_window_active() {
            if tracker.active == Some(id) {
                tracker.active = None;
            }
            tracker.left = Some((id, Instant::now()));
            return;
        }
        // The other window's loss of focus may come before or after this one's gain.
        let from_zj = match tracker.active {
            Some(other) => other != id,
            None => tracker
                .left
                .is_some_and(|(other, at)| other != id && at.elapsed() < SWITCH_GAP),
        };
        tracker.active = Some(id);
        let seen = std::mem::replace(&mut self.switch_hud.seen, true);
        if from_zj && seen {
            self.show_switch_hud(cx);
        }
    }

    fn show_switch_hud(&mut self, cx: &mut Context<Self>) {
        self.switch_hud.next += 1;
        let shown = self.switch_hud.next;
        self.switch_hud.shown = Some(shown);
        self.switch_hud.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOWN).await;
            let _ = this.update(cx, |this, cx| {
                if this.switch_hud.shown == Some(shown) {
                    this.switch_hud.shown = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    pub(super) fn render_switch_hud(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shown = self.switch_hud.shown?;
        let colors = theme::colors(cx);
        let name = self.workspace_name();
        // No box: gray text on a halo of the editor's color, blurred out at the edges, so the
        // code under it fades instead of competing with the name.
        let label = move |scale: f32, opacity: f32| {
            div()
                .px(theme::SWITCH_HUD_PAD_X * scale)
                .py(theme::SWITCH_HUD_PAD_Y * scale)
                .rounded_full()
                .shadow(vec![BoxShadow {
                    color: colors.switch_halo,
                    offset: Point::default(),
                    blur_radius: theme::SWITCH_HUD_BLUR * scale,
                    spread_radius: theme::SWITCH_HUD_SPREAD * scale,
                    inset: false,
                }])
                .max_w(relative(theme::SWITCH_HUD_MAX_WIDTH))
                .opacity(opacity * theme::SWITCH_HUD_OPACITY)
                .text_size(theme::SWITCH_HUD_TEXT * scale)
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.muted)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name.clone())
        };
        let label = if crate::platform::reduce_motion() {
            label(1.0, 1.0).into_any_element()
        } else {
            let (scale, opacity) = frame(0.0);
            label(scale, opacity)
                .with_animation(("switch-hud", shown), Animation::new(SHOWN), move |_, t| {
                    let (scale, opacity) = frame(t);
                    label(scale, opacity)
                })
                .into_any_element()
        };
        // Not a hit target: the window can be used at once.
        Some(
            v_flex()
                .absolute()
                .inset_0()
                .items_center()
                .child(div().h(relative(theme::SWITCH_HUD_TOP)))
                .child(
                    div()
                        .w_full()
                        .h(Pixels::ZERO)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(label),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{empty_store, new_window, open_window};
    use super::frame;
    use super::*;
    use crate::settings::Settings;
    // `super::*` brings in GPUI's `test` macro through `gpui_kit::*`; `#[gpui_kit::test]`
    // expands to the built-in one.
    #[allow(unused_imports)]
    use core::prelude::v1::test;
    use gpui_kit::{TestAppContext, base::Root};

    #[gpui_kit::test]
    async fn only_a_switch_between_zj_windows_shows_the_name(cx: &mut TestAppContext) {
        let (wa, a) = open_window(cx, None, Settings::default(), empty_store());
        let (wb, b) = cx.update(|cx| {
            let documents = cx.global::<OpenDocuments>().0.clone();
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
            let (window, this) = new_window(cx, None, documents, bounds);
            (window.downcast::<Root>().unwrap(), this)
        });
        let activate = |cx: &mut TestAppContext, window: WindowHandle<Root>| {
            cx.update_window(window.into(), |_, window, _| window.activate_window())
                .unwrap();
            cx.run_until_parked();
        };
        let shown = |cx: &mut TestAppContext, this: &Entity<Workbench>| {
            this.read_with(cx, |p, _| p.switch_hud.shown.is_some())
        };
        // Each window's first time in front: it was just opened.
        activate(cx, wa);
        activate(cx, wb);
        assert!(!shown(cx, &a) && !shown(cx, &b));
        // From one ZJ window to the other.
        activate(cx, wa);
        assert!(shown(cx, &a));
        activate(cx, wb);
        assert!(shown(cx, &b));
        // Gone for a while (another app), then back to the other window: no switch.
        cx.executor().advance_clock(SHOWN);
        cx.run_until_parked();
        assert!(!shown(cx, &a) && !shown(cx, &b));
        gpui_kit::VisualTestContext::from_window(wb.into(), cx).deactivate_window();
        cx.update(|cx| {
            let tracker = cx.default_global::<ActiveWindow>();
            assert_eq!(tracker.active, None);
            tracker.left = tracker
                .left
                .map(|(id, at)| (id, at - SWITCH_GAP - SWITCH_GAP));
        });
        activate(cx, wa);
        assert!(!shown(cx, &a));
    }

    #[test]
    fn grows_in_holds_and_fades_out() {
        assert_eq!(frame(0.0), (super::FROM_SCALE, 0.0));
        let (scale, opacity) = frame(0.5);
        assert!((scale - 1.0).abs() < 1e-6 && (opacity - 1.0).abs() < 1e-6);
        let (_, fading) = frame(0.85);
        assert!(fading > 0.0 && fading < 1.0);
        assert!(frame(1.0).1.abs() < 1e-6);
    }
}
