use gpui::{Context, Pixels, px};

// On Windows, Linux, we should use integer to avoid blurry cursor.
#[cfg(not(target_os = "macos"))]
pub(super) const CURSOR_WIDTH: Pixels = px(2.);
#[cfg(target_os = "macos")]
pub(super) const CURSOR_WIDTH: Pixels = px(1.5);

/// The Input cursor's visibility.
///
/// ZJ patch: the cursor is steady (CLAUDE.md: an idle editor draws nothing). Upstream blinks
/// it every 500 ms, which repaints the whole window twice a second, keeps the CPU busy and
/// keeps the renderer from giving back its spare drawable. Here it shows while the input is
/// focused, with no timer; the blink loop and its pause delay are gone.
///
/// The input painter checks [`Self::visible`] before drawing the cursor.
pub(crate) struct BlinkCursor {
    visible: bool,
    /// Zero before the first [`Self::start`] and after [`Self::stop`]: not focused.
    epoch: usize,
}

impl BlinkCursor {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            epoch: 0,
        }
    }

    /// The input got focus: show the cursor.
    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        self.epoch += 1;
        if !self.visible {
            self.visible = true;
            cx.notify();
        }
    }

    /// The input lost focus: hide the cursor, so the next [`Self::start`] shows it again.
    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        self.epoch = 0;
        self.visible = false;
        cx.notify();
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    /// A keystroke: the cursor stays visible. A no-op on an input that is not focused (a
    /// programmatic `set_value` on an unfocused input must not show a cursor).
    pub(crate) fn pause(&mut self, cx: &mut Context<Self>) {
        if self.epoch == 0 {
            return;
        }
        if !self.visible {
            self.visible = true;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};
    use std::{cell::Cell, rc::Rc, time::Duration};

    #[gpui::test]
    fn a_focused_cursor_stays_visible_without_repainting(cx: &mut TestAppContext) {
        let cursor = cx.new(|_| BlinkCursor::new());
        assert!(!cursor.read_with(cx, |cursor, _| cursor.visible()));
        cursor.update(cx, |cursor, cx| cursor.start(cx));
        cx.run_until_parked();
        let notifies = Rc::new(Cell::new(0usize));
        let counter = notifies.clone();
        let _observer =
            cx.update(|cx| cx.observe(&cursor, move |_, _| counter.set(counter.get() + 1)));
        for _ in 0..5 {
            cursor.update(cx, |cursor, cx| cursor.pause(cx));
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));
        }
        assert_eq!(notifies.get(), 0, "a steady cursor repainted");
    }

    #[gpui::test]
    fn an_unfocused_cursor_stays_hidden(cx: &mut TestAppContext) {
        let cursor = cx.new(|_| BlinkCursor::new());
        cursor.update(cx, |cursor, cx| cursor.pause(cx));
        cx.run_until_parked();
        assert!(!cursor.read_with(cx, |cursor, _| cursor.visible()));
        cursor.update(cx, |cursor, cx| {
            cursor.start(cx);
            cursor.stop(cx);
            cursor.pause(cx);
        });
        cx.run_until_parked();
        assert!(!cursor.read_with(cx, |cursor, _| cursor.visible()));
        cursor.update(cx, |cursor, cx| cursor.start(cx));
        assert!(cursor.read_with(cx, |cursor, _| cursor.visible()));
    }
}
