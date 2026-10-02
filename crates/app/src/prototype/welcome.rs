//! The welcome page logo: the ink sprig as an image, with the red cursor drawn on top as its
//! own small view that blinks (530 ms on / 530 ms off, 120 ms fades) only while the welcome
//! page is shown in the focused window and the system does not ask for reduced motion.
//! Each step re-renders just this view; nothing runs while the page is hidden.

use crate::theme;
use gpui_kit::*;
use std::time::Duration;

/// Half a blink period, as in macOS text fields.
const HALF_PERIOD: Duration = Duration::from_millis(530);
/// The fade at each edge, in a few steps rather than per frame.
const FADE_STEPS: u32 = 3;
const FADE: Duration = Duration::from_millis(120);

pub(super) struct WelcomeCursor {
    opacity: f32,
    task: Option<Task<()>>,
}

impl WelcomeCursor {
    pub fn new() -> Self {
        Self {
            opacity: 1.,
            task: None,
        }
    }

    /// Starts or stops blinking; a stopped cursor stays fully visible.
    pub fn set_blinking(&mut self, blinking: bool, window: &mut Window, cx: &mut Context<Self>) {
        let blinking = blinking && !crate::platform::reduce_motion();
        if blinking == self.task.is_some() {
            return;
        }
        if !blinking {
            self.task = None;
            if self.opacity != 1. {
                self.opacity = 1.;
                cx.notify();
            }
            return;
        }
        let step = FADE / FADE_STEPS;
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                for (from, to) in [(1., 0.), (0., 1.)] {
                    cx.background_executor().timer(HALF_PERIOD - FADE).await;
                    for i in 1..=FADE_STEPS {
                        let opacity = from + (to - from) * i as f32 / FADE_STEPS as f32;
                        if this
                            .update(cx, |this, cx| {
                                this.opacity = opacity;
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                        cx.background_executor().timer(step).await;
                    }
                }
            }
        }));
    }
}

impl Render for WelcomeCursor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        div()
            .size_full()
            .rounded(theme::LOGO_CURSOR_RADIUS)
            .bg(colors.logo_cursor.opacity(self.opacity))
    }
}
