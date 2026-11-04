//! Entering a terminal selects English while its window is active. No polling.

use super::*;

impl Workbench {
    pub(super) fn input_terminal_entered(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.global::<crate::settings::Settings>().auto_input_source || !window.is_window_active()
        {
            return;
        }
        let current = crate::platform::input_source();
        if crate::input_switch::terminal_needs_ascii(current.as_ref())
            && !crate::platform::select_input_source(None)
        {
            eprintln!("event=input_source_switch_failed area=Terminal");
        }
    }

    pub(super) fn input_area_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused_terminal_any(window, cx) {
            self.input_terminal_entered(window, cx);
        }
    }
}
