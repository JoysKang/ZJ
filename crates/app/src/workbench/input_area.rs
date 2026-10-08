//! The input source follows the focus (`crate::input_switch`): entering a terminal, the
//! Agent composer, or coming back to a window with one of them focused. Nothing happens
//! while the window is in the background, and nothing is polled.

use super::*;
use crate::input_switch::{Area, InputSwitch, Switch};

impl Workbench {
    pub(super) fn input_area_entered(
        &mut self,
        area: Area,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cx.global::<crate::settings::Settings>().auto_input_source || !window.is_window_active()
        {
            return;
        }
        let current = crate::platform::input_source();
        let switch = cx.default_global::<InputSwitch>().enter(
            area,
            current.as_ref(),
            crate::platform::non_ascii_input_source,
        );
        let selected = match &switch {
            None => return,
            Some(Switch::Ascii) => crate::platform::select_input_source(None),
            Some(Switch::Select(id)) => crate::platform::select_input_source(Some(id)),
        };
        if !selected {
            eprintln!("event=input_source_switch_failed area={area:?}");
        }
    }

    /// The composer lost focus: the source in use is the one it gets back next time.
    pub(super) fn input_composer_left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cx.global::<crate::settings::Settings>().auto_input_source && window.is_window_active() {
            let current = crate::platform::input_source();
            cx.default_global::<InputSwitch>()
                .leave_composer(current.as_ref());
        }
    }

    /// The window came to the front with a terminal or the composer focused: as entering
    /// it. Going to the back with the composer focused: remembered as leaving it.
    pub(super) fn input_area_activation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = self
            .agent
            .composer
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx);
        let area = if self.focused_terminal_any(window, cx) {
            Area::Terminal
        } else if composer {
            Area::Composer
        } else {
            return;
        };
        if window.is_window_active() {
            self.input_area_entered(area, window, cx);
        } else if area == Area::Composer
            && cx.global::<crate::settings::Settings>().auto_input_source
        {
            let current = crate::platform::input_source();
            cx.default_global::<InputSwitch>()
                .leave_composer(current.as_ref());
        }
    }
}
