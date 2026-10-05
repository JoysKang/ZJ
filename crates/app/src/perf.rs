//! Timing marks for the resource budget (CLAUDE.md), read by `tools/measure_budget.py`:
//! - `event=first_frame ms=…`: launch (start of `main`) to the first window's first frame;
//! - `event=key_latency us=…` (only with `ZJ_LATENCY_LOG=1`): a keystroke, before dispatch,
//!   to the end of the frame that follows it (drawn and handed to the GPU; the display's own
//!   scan-out is not included).
//!
//! Off by default apart from the one first-frame line; nothing here runs on a timer.

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static FIRST_FRAME: AtomicBool = AtomicBool::new(false);

/// The start of `main`.
pub fn mark_start() {
    START.get_or_init(Instant::now);
}

/// Logs the first frame of the first window (once per process).
pub fn first_frame(window: &gpui_kit::Window) {
    if FIRST_FRAME.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(start) = START.get().copied() else {
        return;
    };
    window.on_next_frame(move |_, _| {
        eprintln!("event=first_frame ms={}", start.elapsed().as_millis());
    });
}

/// `ZJ_LATENCY_LOG=1`: log every keystroke's latency to the next frame.
pub fn watch_key_latency(cx: &mut gpui_kit::App) {
    if std::env::var_os("ZJ_LATENCY_LOG").is_none_or(|v| v == "0") {
        return;
    }
    cx.intercept_keystrokes(|_, window, _| {
        let pressed = Instant::now();
        window.on_next_frame(move |_, _| {
            eprintln!("event=key_latency us={}", pressed.elapsed().as_micros());
        });
    })
    .detach();
}
