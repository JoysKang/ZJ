//! The bottom panel's terminals, with VS Code's keys: ⌃` shows / hides the panel, ⌃⇧` adds a
//! terminal, ⌘\ splits the focused one side by side, the trash button kills it. Terminals
//! keep running while the panel is hidden; a shell that exits closes its terminal.

use super::Workbench;
use super::terminal_view::{TerminalEvent, TerminalView};
use crate::{terminal::Shell, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    prelude::FluentBuilder,
    *,
};

gpui_kit::actions!(
    terminal,
    [ToggleTerminal, NewTerminal, SplitTerminal, KillTerminal]
);

/// The panel as the workbench holds it.
pub(super) type Terminals = TerminalPanel<Entity<TerminalView>>;

/// Terminals side by side in the panel; the panel shows one group at a time.
struct Group<T> {
    panes: Vec<T>,
    /// The pane that last had focus.
    focused: usize,
}

/// The panel's groups, active group and visibility. Generic over the pane so that every
/// index rule (clamping, group removal, falling back to a neighbour) lives here and is unit
/// tested with plain ids; the workbench only adds spawning, focus and repainting.
pub(super) struct TerminalPanel<T> {
    visible: bool,
    groups: Vec<Group<T>>,
    active: usize,
    /// The shell to start; `None` is the user's login shell (tests use `/bin/sh`).
    shell: Option<Shell>,
    subscriptions: Vec<(EntityId, Subscription)>,
}

impl<T> Default for TerminalPanel<T> {
    fn default() -> Self {
        Self {
            visible: false,
            groups: Vec::new(),
            active: 0,
            shell: None,
            subscriptions: Vec::new(),
        }
    }
}

/// What [`TerminalPanel::remove`] did, from the smallest change to the largest.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RemoveOutcome {
    /// No pane matched.
    Missing,
    /// The pane's group still has other panes.
    Pane,
    /// The pane was its group's last, so the group is gone too.
    Group,
    /// That was the last terminal; the panel is hidden.
    Last,
}

impl<T> TerminalPanel<T> {
    pub(super) fn is_visible(&self) -> bool {
        self.visible
    }

    /// Whether the panel takes room under the editor: visible and holding a terminal.
    pub(super) fn is_shown(&self) -> bool {
        self.visible && !self.groups.is_empty()
    }

    pub(super) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub(super) fn active(&self) -> usize {
        self.active
    }

    pub(super) fn shell(&self) -> Option<Shell> {
        self.shell.clone()
    }

    #[cfg(test)]
    pub(super) fn set_shell(&mut self, shell: Option<Shell>) {
        self.shell = shell;
    }

    /// Each group's panes, left to right.
    pub(super) fn groups(&self) -> impl ExactSizeIterator<Item = &[T]> {
        self.groups.iter().map(|group| group.panes.as_slice())
    }

    /// The panes the panel shows; empty when there is no active group.
    pub(super) fn active_panes(&self) -> &[T] {
        self.groups
            .get(self.active)
            .map_or(&[], |group| group.panes.as_slice())
    }

    pub(super) fn pane(&self, group: usize, pane: usize) -> Option<&T> {
        self.groups.get(group)?.panes.get(pane)
    }

    /// The pane that last had focus in `group`, as recorded (not clamped).
    pub(super) fn last_focused(&self, group: usize) -> Option<usize> {
        self.groups.get(group).map(|group| group.focused)
    }

    /// Where the first pane matching `matches` is, as (group, pane).
    pub(super) fn position(&self, mut matches: impl FnMut(&T) -> bool) -> Option<(usize, usize)> {
        self.groups
            .iter()
            .enumerate()
            .find_map(|(g, group)| group.panes.iter().position(&mut matches).map(|p| (g, p)))
    }

    /// The pane commands act on: `focused` (the pane holding keyboard focus, which only the
    /// window knows), else the active group's last focused one, clamped to its panes.
    pub(super) fn target(&self, focused: Option<(usize, usize)>) -> Option<(usize, usize)> {
        focused.or_else(|| {
            let group = self.groups.get(self.active)?;
            Some((
                self.active,
                group.focused.min(group.panes.len().saturating_sub(1)),
            ))
        })
    }

    /// Makes `group` active and remembers `pane` as its focus; returns the pane to give
    /// keyboard focus to, if both exist.
    pub(super) fn focus(&mut self, group: usize, pane: usize) -> Option<&T> {
        self.active = group;
        let target = self.groups.get_mut(group)?;
        target.focused = pane;
        target.panes.get(pane)
    }

    /// Records a click on a pane without switching groups; focus itself follows the click.
    pub(super) fn remember_focus(&mut self, group: usize, pane: usize) {
        if let Some(group) = self.groups.get_mut(group) {
            group.focused = pane;
        }
    }

    /// Adds `pane` as a new group of its own and shows the panel; returns the group's index.
    pub(super) fn push_group(&mut self, pane: T) -> usize {
        self.groups.push(Group {
            panes: vec![pane],
            focused: 0,
        });
        self.visible = true;
        self.groups.len() - 1
    }

    /// Inserts `new` right of `pane` in `group` and shows the panel; returns where it went.
    /// `pane` is clamped so a stale index appends instead of panicking; a missing group drops
    /// `new` and returns `None`.
    pub(super) fn insert_split(&mut self, group: usize, pane: usize, new: T) -> Option<usize> {
        let target = self.groups.get_mut(group)?;
        let at = pane.saturating_add(1).min(target.panes.len());
        target.panes.insert(at, new);
        self.visible = true;
        Some(at)
    }

    /// Drops the first pane matching `matches`. An emptied group goes away; an active group
    /// after it steps left with it, and when the active group itself goes, the next one
    /// slides into its place (the previous one if it was the last). The last terminal hides
    /// the panel.
    pub(super) fn remove(&mut self, matches: impl FnMut(&T) -> bool) -> RemoveOutcome {
        let mut outcome = RemoveOutcome::Missing;
        if let Some((g, index)) = self.position(matches) {
            let group = &mut self.groups[g];
            group.panes.remove(index);
            group.focused = group.focused.min(group.panes.len().saturating_sub(1));
            outcome = RemoveOutcome::Pane;
            if group.panes.is_empty() {
                self.groups.remove(g);
                if self.active > g || self.active >= self.groups.len() {
                    self.active = self.active.saturating_sub(1);
                }
                outcome = RemoveOutcome::Group;
            }
        }
        if self.groups.is_empty() {
            self.visible = false;
            self.active = 0;
            if outcome == RemoveOutcome::Group {
                outcome = RemoveOutcome::Last;
            }
        }
        outcome
    }

    fn subscribe(&mut self, id: EntityId, subscription: Subscription) {
        self.subscriptions.push((id, subscription));
    }

    fn unsubscribe(&mut self, id: EntityId) {
        self.subscriptions.retain(|(pane, _)| *pane != id);
    }
}

impl Workbench {
    /// Whether a terminal holds keyboard focus.
    pub(super) fn focused_terminal_any(&self, window: &Window, cx: &App) -> bool {
        self.focused_terminal(window, cx).is_some()
    }

    /// The terminal holding keyboard focus, if any.
    fn focused_terminal(&self, window: &Window, cx: &App) -> Option<(usize, usize)> {
        self.terminals
            .position(|pane| pane.read(cx).focus_handle(cx).contains_focused(window, cx))
    }

    fn spawn_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let cwd = self
            .root
            .clone()
            .or_else(|| std::env::var_os("HOME").map(Into::into));
        match TerminalView::spawn(cwd, self.terminals.shell(), cx) {
            Ok(pane) => {
                let subscription =
                    cx.subscribe_in(&pane, window, |this, pane, event, window, cx| match event {
                        TerminalEvent::Exited => {
                            let focused = pane.read(cx).focus_handle(cx).is_focused(window);
                            this.remove_terminal(pane, cx);
                            if focused {
                                this.refocus_after_close(window, cx);
                            }
                        }
                        TerminalEvent::TitleChanged => cx.notify(),
                    });
                self.terminals.subscribe(pane.entity_id(), subscription);
                let focus = pane.read(cx).focus_handle(cx);
                let entered = cx.on_focus_in(&focus, window, |this, window, cx| {
                    this.input_area_entered(crate::input_switch::Area::Terminal, window, cx)
                });
                self.terminals.subscribe(pane.entity_id(), entered);
                Some(pane)
            }
            Err(error) => {
                eprintln!("event=terminal_spawn_failed error={error}");
                self.message = format!("无法启动终端：{error}");
                cx.notify();
                None
            }
        }
    }

    fn focus_terminal(&mut self, group: usize, pane: usize, window: &mut Window, cx: &mut App) {
        if let Some(pane) = self.terminals.focus(group, pane) {
            pane.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// ⌃`: opens the panel (with a terminal if there is none), focuses it, or hides it when a
    /// terminal already has focus.
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.is_empty() {
            return self.new_terminal(window, cx);
        }
        let focused = self.focused_terminal(window, cx);
        if self.terminals.is_visible() && focused.is_some() {
            self.terminals.set_visible(false);
            self.focus_active_editor(window, cx);
        } else if let Some((group, pane)) = self.terminals.target(focused) {
            self.terminals.set_visible(true);
            self.focus_terminal(group, pane, window, cx);
        }
        cx.notify();
    }

    /// The title bar's button: shows or hides the panel without the focus rule of ⌃`.
    pub(super) fn toggle_terminal_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.is_visible() {
            self.hide_terminal_panel(window, cx);
        } else {
            self.toggle_terminal(window, cx);
        }
    }

    fn hide_terminal_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminals.set_visible(false);
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    /// ⌃⇧`: a new terminal in its own group.
    pub(super) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.spawn_terminal(window, cx) else {
            return;
        };
        let group = self.terminals.push_group(pane);
        self.focus_terminal(group, 0, window, cx);
        cx.notify();
    }

    /// ⌘\: a new terminal to the right of the focused one.
    pub(super) fn split_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focused_terminal(window, cx);
        let Some((group, pane)) = self.terminals.target(focused) else {
            return self.new_terminal(window, cx);
        };
        let Some(new) = self.spawn_terminal(window, cx) else {
            return;
        };
        let Some(at) = self.terminals.insert_split(group, pane, new) else {
            return;
        };
        self.focus_terminal(group, at, window, cx);
        cx.notify();
    }

    pub(super) fn kill_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focused_terminal(window, cx);
        let Some(pane) = self
            .terminals
            .target(focused)
            .and_then(|(group, pane)| self.terminals.pane(group, pane))
            .cloned()
        else {
            return;
        };
        self.remove_terminal(&pane, cx);
        self.refocus_after_close(window, cx);
    }

    /// Focus moves to the group's next terminal, or to the editor when none is left.
    fn refocus_after_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.focused_terminal(window, cx);
        if let Some((group, pane)) = self.terminals.target(focused) {
            self.focus_terminal(group, pane, window, cx);
        } else {
            self.focus_active_editor(window, cx);
        }
    }

    /// Drops a terminal (its shell is hung up); the last one hides the panel.
    fn remove_terminal(&mut self, pane: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let id = pane.entity_id();
        self.terminals.unsubscribe(id);
        self.terminals.remove(|pane| pane.entity_id() == id);
        cx.notify();
    }

    pub(super) fn select_terminal_group(
        &mut self,
        group: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.terminals.last_focused(group).unwrap_or(0);
        self.focus_terminal(group, pane, window, cx);
        cx.notify();
    }

    pub(super) fn render_terminal_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let terminals = &self.terminals;
        let label = |panes: &[Entity<TerminalView>], cx: &App| {
            panes
                .iter()
                .map(|pane| pane.read(cx).title().to_string())
                .collect::<Vec<_>>()
                .join("、")
        };
        let chips = (terminals.groups().len() > 1).then(|| {
            h_flex()
                .gap_1()
                .children(terminals.groups().enumerate().map(|(g, panes)| {
                    let active = g == terminals.active();
                    div()
                        .id(("terminal-group", g))
                        .px_2()
                        .rounded(theme::RADIUS)
                        .cursor_pointer()
                        .text_size(theme::TEXT_CAPTION)
                        .map(|chip| {
                            if active {
                                chip.bg(colors.selected).text_color(colors.selected_fg)
                            } else {
                                chip.text_color(colors.muted)
                                    .hover(|chip| chip.bg(colors.hover))
                            }
                        })
                        .child(format!("{}: {}", g + 1, label(panes, cx)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_terminal_group(g, window, cx)
                        }))
                }))
        });
        let button = |id: &'static str, icon: IconName, tooltip: &'static str| {
            Button::new(id).xsmall().ghost().icon(icon).tooltip(tooltip)
        };
        let header = h_flex()
            .h(theme::SECTION_HEIGHT)
            .flex_shrink_0()
            .px_2()
            .gap_2()
            .child(
                div()
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.foreground)
                    .child("终端"),
            )
            .children(chips)
            .child(div().flex_1())
            .child(
                button("terminal-new", IconName::Plus, "新建终端（⌃⇧`）")
                    .on_click(cx.listener(|this, _, window, cx| this.new_terminal(window, cx))),
            )
            .child(
                button(
                    "terminal-split",
                    IconName::SquareSplitHorizontal,
                    "拆分终端（⌘\\）",
                )
                .on_click(cx.listener(|this, _, window, cx| this.split_terminal(window, cx))),
            )
            .child(
                button("terminal-kill", IconName::Trash, "终止终端")
                    .on_click(cx.listener(|this, _, window, cx| this.kill_terminal(window, cx))),
            )
            .child(
                button("terminal-hide", IconName::Close, "隐藏面板（⌃`）").on_click(
                    cx.listener(|this, _, window, cx| this.hide_terminal_panel(window, cx)),
                ),
            );
        let group = terminals.active();
        let panes = terminals.active_panes().to_vec();
        v_flex()
            .size_full()
            .bg(colors.editor)
            .border_t_1()
            .border_color(colors.border)
            .child(header)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .children(panes.into_iter().enumerate().map(|(p, pane)| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .when(p > 0, |pane| pane.border_l_1().border_color(colors.border))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, _| {
                                    this.terminals.remember_focus(group, p)
                                }),
                            )
                            .child(pane)
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{RemoveOutcome, TerminalPanel};

    /// A panel whose groups hold the given ids, left to right; the last group is active.
    fn with_groups(groups: &[&[u32]]) -> TerminalPanel<u32> {
        let mut panel = TerminalPanel::default();
        for panes in groups {
            let group = panel.push_group(panes[0]);
            for (p, &pane) in panes.iter().enumerate().skip(1) {
                panel.insert_split(group, p - 1, pane);
            }
        }
        panel
    }

    fn layout(panel: &TerminalPanel<u32>) -> Vec<Vec<u32>> {
        panel.groups().map(<[u32]>::to_vec).collect()
    }

    #[test]
    fn push_group_appends_and_shows_the_panel() {
        let mut panel = TerminalPanel::default();
        assert!(panel.is_empty() && !panel.is_visible() && !panel.is_shown());
        assert_eq!(panel.push_group(1), 0);
        assert_eq!(panel.push_group(2), 1);
        assert!(panel.is_visible() && panel.is_shown());
        assert_eq!(layout(&panel), [vec![1], vec![2]]);
        // Pushing alone does not move the active group; focusing does.
        assert_eq!(panel.active(), 0);
        assert_eq!(panel.focus(1, 0), Some(&2));
        assert_eq!(panel.active(), 1);
        assert_eq!(panel.active_panes(), [2]);
    }

    #[test]
    fn insert_split_goes_right_of_the_pane_and_clamps_stale_indices() {
        let mut panel = with_groups(&[&[1, 2]]);
        assert_eq!(panel.insert_split(0, 0, 3), Some(1));
        assert_eq!(layout(&panel), [vec![1, 3, 2]]);
        // A pane index past the end appends instead of panicking.
        assert_eq!(panel.insert_split(0, 99, 4), Some(3));
        assert_eq!(panel.insert_split(0, usize::MAX, 5), Some(4));
        assert_eq!(layout(&panel), [vec![1, 3, 2, 4, 5]]);
        // A missing group changes nothing.
        assert_eq!(panel.insert_split(7, 0, 6), None);
        assert_eq!(layout(&panel), [vec![1, 3, 2, 4, 5]]);

        // Splitting re-shows a hidden panel.
        panel.set_visible(false);
        assert_eq!(panel.insert_split(0, 0, 7), Some(1));
        assert!(panel.is_visible());
    }

    #[test]
    fn remove_keeps_the_group_while_it_has_panes() {
        let mut panel = with_groups(&[&[1, 2, 3]]);
        panel.focus(0, 2);
        assert_eq!(panel.remove(|&pane| pane == 3), RemoveOutcome::Pane);
        assert_eq!(layout(&panel), [vec![1, 2]]);
        // The group's focus is clamped onto a pane that still exists.
        assert_eq!(panel.last_focused(0), Some(1));
        assert_eq!(panel.remove(|&pane| pane == 9), RemoveOutcome::Missing);
        assert_eq!(layout(&panel), [vec![1, 2]]);
        assert!(panel.is_visible());
    }

    #[test]
    fn remove_of_a_middle_group_keeps_the_active_group_in_range() {
        // Active after the removed group: it steps left with its group.
        let mut panel = with_groups(&[&[1], &[2], &[3]]);
        panel.focus(2, 0);
        assert_eq!(panel.remove(|&pane| pane == 2), RemoveOutcome::Group);
        assert_eq!(layout(&panel), [vec![1], vec![3]]);
        assert_eq!(panel.active(), 1);
        assert_eq!(panel.active_panes(), [3]);

        // Active before the removed group: unchanged.
        let mut panel = with_groups(&[&[1], &[2], &[3]]);
        panel.focus(0, 0);
        assert_eq!(panel.remove(|&pane| pane == 2), RemoveOutcome::Group);
        assert_eq!(panel.active(), 0);
        assert_eq!(panel.active_panes(), [1]);

        // Active is the removed group: its right neighbour slides into its place.
        let mut panel = with_groups(&[&[1], &[2], &[3]]);
        panel.focus(1, 0);
        assert_eq!(panel.remove(|&pane| pane == 2), RemoveOutcome::Group);
        assert_eq!(panel.active(), 1);
        assert_eq!(panel.active_panes(), [3]);
        assert!(panel.is_visible());
    }

    #[test]
    fn remove_of_the_last_group_falls_back_to_its_left_neighbour() {
        let mut panel = with_groups(&[&[1], &[2, 3], &[4]]);
        panel.focus(2, 0);
        assert_eq!(panel.remove(|&pane| pane == 4), RemoveOutcome::Group);
        assert_eq!(layout(&panel), [vec![1], vec![2, 3]]);
        assert_eq!(panel.active(), 1);
        assert_eq!(panel.active_panes(), [2, 3]);

        // The first group, while active, leaves the next one active at index 0.
        let mut panel = with_groups(&[&[1], &[2]]);
        panel.focus(0, 0);
        assert_eq!(panel.remove(|&pane| pane == 1), RemoveOutcome::Group);
        assert_eq!(panel.active(), 0);
        assert_eq!(panel.active_panes(), [2]);
    }

    #[test]
    fn remove_of_the_last_terminal_hides_the_panel() {
        let mut panel = with_groups(&[&[1], &[2]]);
        panel.focus(1, 0);
        assert_eq!(panel.remove(|&pane| pane == 2), RemoveOutcome::Group);
        assert_eq!(panel.remove(|&pane| pane == 1), RemoveOutcome::Last);
        assert!(panel.is_empty() && !panel.is_visible() && !panel.is_shown());
        assert_eq!(panel.active(), 0);
        assert_eq!(panel.active_panes(), [] as [u32; 0]);
        assert_eq!(panel.target(None), None);
        assert_eq!(panel.remove(|_| true), RemoveOutcome::Missing);
    }

    #[test]
    fn target_prefers_keyboard_focus_then_the_active_groups_last_focus() {
        let mut panel = with_groups(&[&[1, 2], &[3, 4, 5]]);
        panel.focus(1, 2);
        assert_eq!(panel.target(Some((0, 1))), Some((0, 1)));
        assert_eq!(panel.target(None), Some((1, 2)));

        // A recorded focus past the end is clamped onto the group's last pane.
        panel.remember_focus(1, 9);
        assert_eq!(panel.last_focused(1), Some(9));
        assert_eq!(panel.target(None), Some((1, 2)));
        assert_eq!(panel.pane(1, 2), Some(&5));

        // An active group that does not exist has no fallback.
        assert_eq!(panel.focus(5, 0), None);
        assert_eq!(panel.target(None), None);
        assert_eq!(panel.active_panes(), [] as [u32; 0]);
    }

    #[test]
    fn position_finds_panes_across_groups() {
        let panel = with_groups(&[&[1, 2], &[3]]);
        assert_eq!(panel.position(|&pane| pane == 2), Some((0, 1)));
        assert_eq!(panel.position(|&pane| pane == 3), Some((1, 0)));
        assert_eq!(panel.position(|&pane| pane == 4), None);
    }
}
