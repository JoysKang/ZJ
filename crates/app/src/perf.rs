//! Timing marks for the resource budget (CLAUDE.md), read by `tools/measure_budget.py`:
//! - `event=first_frame ms=…`: launch to the first window's pre-draw callback (not submission).
//! - `event=key_cpu_submit us=…` (`ZJ_LATENCY_LOG=1`): emitted by the vendored macOS
//!   renderer, from deduplicated native keyDown before IME / GPUI dispatch to the same
//!   window's next successful Metal commit return. CPU submission, not GPU completion or
//!   display scan-out; no additional frame callback / interval. No key content is logged.
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

/// Logs the first window's pre-draw callback (once per process), not a completed frame.
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
