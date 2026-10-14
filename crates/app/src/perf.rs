//! Timing marks for the resource budget (CLAUDE.md), read by `tools/measure_budget.py`:
//! - `event=first_frame ms=…`: launch (start of `main`) to the first window's first frame;
//! - `event=key_latency us=…` (only with `ZJ_LATENCY_LOG=1`): a keystroke, before dispatch,
//!   to the end of the frame that follows it (drawn and handed to the GPU; the display's own
//!   scan-out is not included).
//!
//! - `event=diff_timing` (only with `ZJ_DIFF_TIMING_LOG=1`): cumulative Diff load stages;
//!   `frame_upper` includes a frame interval after draw/present, not GPU completion or scan-out.
//!
//! Off by default apart from the one first-frame line; nothing here runs on a timer.

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
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

/// Optional, process-wide IDs keep concurrent windows' Diff timings distinct.
#[derive(Clone, Copy)]
pub(crate) struct DiffTiming {
    started: Option<(u64, Instant)>,
    reason: &'static str,
}

impl DiffTiming {
    pub(crate) fn start(reason: &'static str) -> Self {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let started = ENABLED
            .get_or_init(|| std::env::var_os("ZJ_DIFF_TIMING_LOG").is_some_and(|v| v == "1"))
            .then(|| (NEXT_ID.fetch_add(1, Ordering::Relaxed), Instant::now()));
        let timing = Self { started, reason };
        timing.mark("start");
        timing
    }

    pub(crate) fn mark(self, stage: &'static str) {
        if let Some((id, start)) = self.started {
            eprintln!(
                "event=diff_timing id={id} reason={} stage={stage} ms={}",
                self.reason,
                start.elapsed().as_millis()
            );
        }
    }

    pub(crate) fn frame_upper(self, window: &gpui_kit::Window) {
        if self.started.is_none() {
            return;
        }
        // GPUI runs callbacks before draw/present; the following callback is an upper bound.
        window.on_next_frame(move |window, _| {
            window.on_next_frame(move |_, _| self.mark("frame_upper"));
            window.refresh();
        });
    }
}
