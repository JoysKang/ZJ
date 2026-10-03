//! The bottom panel's terminals, with VS Code's keys: ⌃` shows / hides the panel, ⌃⇧` adds a
//! terminal, ⌘\ splits the focused one side by side, the trash button kills it. Terminals
//! keep running while the panel is hidden; a shell that exits closes its terminal.

use super::Prototype;
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

/// Terminals side by side in the panel; the panel shows one group at a time.
pub(super) struct Group {
    pub(super) panes: Vec<Entity<TerminalView>>,
    /// The pane that last had focus.
    pub(super) focused: usize,
}

#[derive(Default)]
pub(super) struct Terminals {
    pub(super) visible: bool,
    pub(super) groups: Vec<Group>,
    pub(super) active: usize,
    /// The shell to start; `None` is the user's login shell (tests use `/bin/sh`).
    pub(super) shell: Option<Shell>,
    subscriptions: Vec<(EntityId, Subscription)>,
}

impl Terminals {
    fn focused_pane(&self, window: &Window, cx: &App) -> Option<(usize, usize)> {
        self.groups.iter().enumerate().find_map(|(g, group)| {
            group
                .panes
                .iter()
                .position(|pane| pane.read(cx).focus_handle(cx).contains_focused(window, cx))
                .map(|p| (g, p))
        })
    }

    /// The pane commands act on: the focused one, else the active group's last focused.
    fn target(&self, window: &Window, cx: &App) -> Option<(usize, usize)> {
        self.focused_pane(window, cx).or_else(|| {
            let group = self.groups.get(self.active)?;
            Some((
                self.active,
                group.focused.min(group.panes.len().saturating_sub(1)),
            ))
        })
    }
}

impl Prototype {
    fn spawn_terminal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalView>> {
        let cwd = self
            .root
            .clone()
            .or_else(|| std::env::var_os("HOME").map(Into::into));
        match TerminalView::spawn(cwd, self.terminals.shell.clone(), cx) {
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
                self.terminals
                    .subscriptions
                    .push((pane.entity_id(), subscription));
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
        if let Some(target) = self.terminals.groups.get_mut(group) {
            target.focused = pane;
            if let Some(pane) = target.panes.get(pane) {
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
        }
        self.terminals.active = group;
    }

    /// ⌃`: opens the panel (with a terminal if there is none), focuses it, or hides it when a
    /// terminal already has focus.
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.groups.is_empty() {
            return self.new_terminal(window, cx);
        }
        let focused = self.terminals.focused_pane(window, cx);
        if self.terminals.visible && focused.is_some() {
            self.terminals.visible = false;
            self.focus_active_editor(window, cx);
        } else if let Some((group, pane)) = self.terminals.target(window, cx) {
            self.terminals.visible = true;
            self.focus_terminal(group, pane, window, cx);
        }
        cx.notify();
    }

    /// The title bar's button: shows or hides the panel without the focus rule of ⌃`.
    pub(super) fn toggle_terminal_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminals.visible {
            self.terminals.visible = false;
            self.focus_active_editor(window, cx);
            cx.notify();
        } else {
            self.toggle_terminal(window, cx);
        }
    }

    /// ⌃⇧`: a new terminal in its own group.
    pub(super) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self.spawn_terminal(window, cx) else {
            return;
        };
        self.terminals.groups.push(Group {
            panes: vec![pane],
            focused: 0,
        });
        self.terminals.visible = true;
        let group = self.terminals.groups.len() - 1;
        self.focus_terminal(group, 0, window, cx);
        cx.notify();
    }

    /// ⌘\: a new terminal to the right of the focused one.
    pub(super) fn split_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((group, pane)) = self.terminals.target(window, cx) else {
            return self.new_terminal(window, cx);
        };
        let Some(new) = self.spawn_terminal(window, cx) else {
            return;
        };
        self.terminals.groups[group].panes.insert(pane + 1, new);
        self.terminals.visible = true;
        self.focus_terminal(group, pane + 1, window, cx);
        cx.notify();
    }

    pub(super) fn kill_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((group, pane)) = self.terminals.target(window, cx) else {
            return;
        };
        let pane = self.terminals.groups[group].panes[pane].clone();
        self.remove_terminal(&pane, cx);
        self.refocus_after_close(window, cx);
    }

    /// Focus moves to the group's next terminal, or to the editor when none is left.
    fn refocus_after_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((group, pane)) = self.terminals.target(window, cx) {
            self.focus_terminal(group, pane, window, cx);
        } else {
            self.focus_active_editor(window, cx);
        }
    }

    /// Drops a terminal (its shell is hung up); the last one hides the panel.
    fn remove_terminal(&mut self, pane: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let id = pane.entity_id();
        self.terminals.subscriptions.retain(|(pane, _)| *pane != id);
        let terminals = &mut self.terminals;
        for g in 0..terminals.groups.len() {
            let group = &mut terminals.groups[g];
            let Some(index) = group.panes.iter().position(|p| p.entity_id() == id) else {
                continue;
            };
            group.panes.remove(index);
            group.focused = group.focused.min(group.panes.len().saturating_sub(1));
            if group.panes.is_empty() {
                terminals.groups.remove(g);
                if terminals.active > g || terminals.active >= terminals.groups.len() {
                    terminals.active = terminals.active.saturating_sub(1);
                }
            }
            break;
        }
        if terminals.groups.is_empty() {
            terminals.visible = false;
            terminals.active = 0;
        }
        cx.notify();
    }

    pub(super) fn select_terminal_group(
        &mut self,
        group: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.terminals.groups.get(group).map_or(0, |g| g.focused);
        self.focus_terminal(group, pane, window, cx);
        cx.notify();
    }

    pub(super) fn render_terminal_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let terminals = &self.terminals;
        let label = |group: &Group, cx: &App| {
            group
                .panes
                .iter()
                .map(|pane| pane.read(cx).title().to_string())
                .collect::<Vec<_>>()
                .join("、")
        };
        let chips = (terminals.groups.len() > 1).then(|| {
            h_flex()
                .gap_1()
                .children(terminals.groups.iter().enumerate().map(|(g, group)| {
                    let active = g == terminals.active;
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
                        .child(format!("{}: {}", g + 1, label(group, cx)))
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
                button("terminal-hide", IconName::Close, "隐藏面板（⌃`）").on_click(cx.listener(
                    |this, _, window, cx| {
                        this.terminals.visible = false;
                        this.focus_active_editor(window, cx);
                        cx.notify();
                    },
                )),
            );
        let group = terminals.active;
        let panes = terminals
            .groups
            .get(group)
            .map(|group| group.panes.clone())
            .unwrap_or_default();
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
                                    if let Some(group) = this.terminals.groups.get_mut(group) {
                                        group.focused = p;
                                    }
                                }),
                            )
                            .child(pane)
                    })),
            )
            .into_any_element()
    }
}
