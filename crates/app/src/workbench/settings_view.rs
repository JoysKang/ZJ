//! The Settings tab (⌘,): the settings file's values as switches, a font stepper, the auto-save
//! choices and the hidden rules, without editing JSON.
//!
//! Every change goes through `settings::update`, so it takes effect in all windows and is
//! written from the background. The hidden rules wait for 应用, because a half-typed pattern
//! would otherwise reload the tree on each keystroke. The JSON file stays one click away for
//! keys this tab does not show (`extra_repos`, unknown keys).

use super::*;
use crate::settings::{EDITOR_FONT_DEFAULT, EDITOR_FONT_MAX, EDITOR_FONT_MIN};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Textarea, TextareaState},
        switch::Switch,
        v_flex,
    },
};

/// The hidden-rules textarea while the tab is open. It holds only what the user typed; the
/// applied rules live in the settings.
pub(super) struct SettingsPane {
    pub(super) hidden: Entity<TextareaState>,
    /// The rules the textarea last showed; text that still equals them has no unapplied edits.
    hidden_synced: String,
}

impl Workbench {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_pane.is_none() {
            let text = cx
                .global::<crate::settings::Settings>()
                .hidden_files
                .patterns()
                .join("\n");
            let hidden = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 8)
                    .placeholder("例如 .DS_Store, *.log, !重要.log")
                    .default_value(text.clone())
            });
            self.settings_pane = Some(SettingsPane {
                hidden,
                hidden_synced: text,
            });
        }
        self.select_pane(Pane::Settings, window, cx);
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_pane = None;
        if self.active == Pane::Settings {
            self.show_remaining_tab(window, cx);
        }
        cx.notify();
    }

    /// 应用 under the hidden rules: an invalid pattern keeps the rules in use.
    fn apply_hidden_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = &self.settings_pane else {
            return;
        };
        let patterns: Vec<String> = pane
            .hidden
            .read(cx)
            .value()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        match crate::hidden_files::HiddenFiles::new(patterns) {
            Ok(rules) => {
                self.change_settings(window, cx, |settings| settings.hidden_files = rules);
                if let Some(pane) = &mut self.settings_pane {
                    // The textarea keeps the user's typed form (extra spaces, a trailing
                    // newline); the next external change must still be recognised as one.
                    let text = pane.hidden.read(cx).value().to_string();
                    pane.hidden_synced = text;
                }
                self.message = "隐藏规则已生效".into();
            }
            Err(error) => self.message = format!("{error}；仍使用原来的规则"),
        }
        cx.notify();
    }

    /// Keeps the textarea showing the saved rules while it has no unapplied edits, so a newer
    /// edit (the JSON file, another window) is not undone by 应用. Text the user changed is
    /// left alone.
    pub(super) fn sync_hidden_rules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((hidden, synced)) = self
            .settings_pane
            .as_ref()
            .map(|pane| (pane.hidden.clone(), pane.hidden_synced.clone()))
        else {
            return;
        };
        if hidden.read(cx).value().as_ref() != synced {
            return;
        }
        let text = cx
            .global::<crate::settings::Settings>()
            .hidden_files
            .patterns()
            .join("\n");
        if text == synced {
            return;
        }
        hidden.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        if let Some(pane) = &mut self.settings_pane {
            pane.hidden_synced = text;
        }
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let settings = cx.global::<crate::settings::Settings>().clone();
        let font = settings.editor_font_size;
        let Some(pane) = &self.settings_pane else {
            return div().into_any_element();
        };

        let section = |title: &'static str| {
            div()
                .pt_3()
                .pb_1()
                .text_size(theme::TEXT_SECTION)
                .text_color(colors.muted)
                .child(title)
        };
        let row = |label: &'static str, hint: &'static str, control: AnyElement| {
            h_flex()
                .py_2()
                .gap_4()
                .items_center()
                .border_b_1()
                .border_color(colors.border)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .text_size(theme::TEXT_BODY)
                                .text_color(colors.foreground)
                                .child(label),
                        )
                        .child(
                            div()
                                .text_size(theme::TEXT_CAPTION)
                                .text_color(colors.muted)
                                .child(hint),
                        ),
                )
                .child(control)
                .into_any_element()
        };
        let switch =
            |id: &'static str,
             label: &'static str,
             checked: bool,
             toggle: fn(&mut Workbench, bool, &mut Window, &mut Context<Workbench>)| {
                Switch::new(id)
                    .small()
                    .checked(checked)
                    .accessibility_label(label)
                    .on_click(cx.listener(move |this, on: &bool, window, cx| {
                        toggle(this, *on, window, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            };
        let auto_save_option = |id: &'static str,
                                label: &'static str,
                                mode: crate::save::AutoSave| {
            Button::new(id)
                .small()
                .ghost()
                .label(label)
                .selected(settings.auto_save == mode)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_auto_save(mode, window, cx)),
                )
        };

        let header = h_flex()
            .items_center()
            .justify_between()
            .pb_2()
            .child(
                div()
                    .text_size(theme::TEXT_BODY)
                    .text_color(colors.foreground)
                    .child("设置"),
            )
            .child(
                Button::new("settings-json")
                    .small()
                    .ghost()
                    .label("打开 settings.json")
                    .accessibility_label("打开 settings.json")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_settings_file(window, cx)),
                    ),
            );

        let editor = v_flex().child(section("编辑器")).child(row(
            "字号",
            "编辑器和 Diff 的字号，⌘= / ⌘- 也可调整",
            h_flex()
                .gap_1()
                .min_w_0()
                .flex_wrap()
                .items_center()
                .child(
                    Button::new("settings-font-smaller")
                        .small()
                        .ghost()
                        .icon(IconName::Minus)
                        .accessibility_label("减小编辑器字号")
                        .disabled(font <= EDITOR_FONT_MIN)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.zoom(Some(-1.), window, cx)),
                        ),
                )
                .child(
                    div()
                        .min_w(theme::KEYCAP * 2.)
                        .text_size(theme::TEXT_BODY)
                        .text_color(colors.foreground)
                        .text_center()
                        .child(format!("{font} px")),
                )
                .child(
                    Button::new("settings-font-larger")
                        .small()
                        .ghost()
                        .icon(IconName::Plus)
                        .accessibility_label("增大编辑器字号")
                        .disabled(font >= EDITOR_FONT_MAX)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.zoom(Some(1.), window, cx)),
                        ),
                )
                .child(
                    Button::new("settings-font-reset")
                        .small()
                        .ghost()
                        .label("默认")
                        .accessibility_label("恢复默认编辑器字号")
                        .disabled(font == EDITOR_FONT_DEFAULT)
                        .on_click(cx.listener(|this, _, window, cx| this.zoom(None, window, cx))),
                )
                .into_any_element(),
        ));

        let files = v_flex()
            .child(section("文件"))
            .child(row(
                "显示隐藏文件",
                "显示名称匹配下面规则的条目；不显示 .gitignore 的文件",
                switch(
                    "settings-show-hidden",
                    "显示隐藏文件",
                    settings.show_hidden,
                    |this, on, window, cx| {
                        this.change_settings(window, cx, |settings| settings.show_hidden = on);
                        this.message = if on {
                            "已显示隐藏文件".into()
                        } else {
                            "已隐藏设置中匹配的文件和文件夹".into()
                        };
                    },
                ),
            ))
            .child(
                v_flex()
                    .py_2()
                    .gap_1()
                    .child(
                        div()
                            .text_size(theme::TEXT_BODY)
                            .text_color(colors.foreground)
                            .child("隐藏规则"),
                    )
                    .child(
                        div()
                            .text_size(theme::TEXT_CAPTION)
                            .text_color(colors.muted)
                            .child("每行一个名称模式，! 开头的规则保留；更改后点应用，关闭标签会丢弃未应用的修改。"),
                    )
                    .child(
                        div()
                            .w_full()
                            .rounded(theme::RADIUS)
                            .border_1()
                            .border_color(colors.border)
                            .child(
                                Textarea::new(&pane.hidden)
                                    .text_size(theme::TEXT_BODY)
                                    .line_height(theme::FIELD_LINE),
                            ),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("settings-hidden-apply")
                                .small()
                                .label("应用")
                                .accessibility_label("应用隐藏规则")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_hidden_rules(window, cx)
                                })),
                        ),
                    ),
            );

        let diff = v_flex().child(section("Diff")).child(row(
            "内联显示",
            "更改显示在同一个文件里；关闭时左右并排",
            switch(
                "settings-diff-inline",
                "内联显示 Diff",
                settings.diff_inline,
                |this, on, window, cx| this.set_diff_layout(on, window, cx),
            ),
        ));

        let git = v_flex().child(section("Git")).child(row(
            "隐藏无变更的仓库",
            "源代码管理只列出有变更的仓库",
            switch(
                "settings-hide-clean",
                "隐藏无变更的仓库",
                settings.hide_clean_repos,
                |this, on, window, cx| {
                    this.change_settings(window, cx, |settings| settings.hide_clean_repos = on)
                },
            ),
        ));

        let search = v_flex().child(section("搜索")).child(row(
            "跳过忽略的文件",
            "不搜 .gitignore 忽略的文件和 .venv、node_modules 等目录；排除列表在搜索视图里改",
            switch(
                "settings-search-excludes",
                "搜索跳过忽略的文件",
                settings.search_use_excludes,
                |this, on, window, cx| {
                    this.change_settings(window, cx, |settings| settings.search_use_excludes = on)
                },
            ),
        ));

        let auto_save = v_flex().child(section("自动保存")).child(row(
            "保存编辑",
            "未命名缓冲区和在磁盘上被删除的文件不自动保存",
            h_flex()
                .gap_1()
                .min_w_0()
                .flex_wrap()
                .child(auto_save_option(
                    "settings-auto-save-off",
                    "关闭",
                    crate::save::AutoSave::Off,
                ))
                .child(auto_save_option(
                    "settings-auto-save-delay",
                    "编辑后 1 秒",
                    crate::save::AutoSave::AfterDelay,
                ))
                .child(auto_save_option(
                    "settings-auto-save-focus",
                    "失去焦点时",
                    crate::save::AutoSave::OnFocusChange,
                ))
                .into_any_element(),
        ));

        let input = v_flex().child(section("输入法")).child(row(
            "终端使用英文输入法",
            "进入终端和窗口回到前台时切到英文输入源（macOS）",
            switch(
                "settings-input-source",
                "终端使用英文输入法",
                settings.auto_input_source,
                |this, on, window, cx| {
                    this.change_settings(window, cx, |settings| settings.auto_input_source = on)
                },
            ),
        ));

        let appearance = v_flex().child(section("外观")).child(row(
            "Dock 图标闪烁",
            "运行期间光标每 530 毫秒闪一次；开启“减少动态效果”时不会闪（macOS）",
            switch(
                "settings-dock-blink",
                "Dock 图标闪烁",
                settings.dock_icon_blink,
                |this, on, window, cx| {
                    this.change_settings(window, cx, |settings| settings.dock_icon_blink = on)
                },
            ),
        ));

        v_flex()
            .id("settings-scroll")
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .w_full()
                    .flex()
                    .items_center()
                    .px(theme::TREE_BASE)
                    .py_3()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(theme::SETTINGS_WIDTH)
                            .flex_shrink_0()
                            .child(header)
                            .child(editor)
                            .child(auto_save)
                            .child(files)
                            .child(diff)
                            .child(git)
                            .child(search)
                            .child(input)
                            .child(appearance),
                    ),
            )
            .into_any_element()
    }
}
