//! File comparisons and repository reading commands share the same menu and command actions.
use super::*;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

gpui_kit::actions!(
    reading,
    [
        SelectForComparison,
        CompareWithSelected,
        CompareWithClipboard,
        FileHistory,
        CompareRevisions,
        ImportPatch,
        ExportPatch
    ]
);

impl Workbench {
    pub(super) fn reading_file_menu(
        &self,
        path: PathBuf,
        menu: PopupMenu,
        view: Entity<Self>,
    ) -> PopupMenu {
        let item =
            |label: &'static str, run: fn(&mut Self, PathBuf, &mut Window, &mut Context<Self>)| {
                let view = view.clone();
                let path = path.clone();
                PopupMenuItem::new(label).on_click(move |_, window, cx| {
                    view.update(cx, |this, cx| run(this, path.clone(), window, cx));
                })
            };
        menu.separator()
            .item(item("选择以进行比较", |this, path, _, cx| {
                this.select_for_comparison(path, cx)
            }))
            .item(item("与已选文件比较", Self::compare_with_selected))
            .item(item("与剪贴板比较", Self::compare_with_clipboard))
            .item(item("文件历史…", Self::open_file_history))
            .separator()
            .item(item(
                "以 GBK 编码只读打开",
                |this, path, window, cx| this.open_encoded(path, encoding_rs::GBK, window, cx),
            ))
            .item(item(
                "以 UTF-16LE 编码只读打开",
                |this, path, window, cx| this.open_encoded(path, encoding_rs::UTF_16LE, window, cx),
            ))
            .item(item(
                "以 UTF-16BE 编码只读打开",
                |this, path, window, cx| this.open_encoded(path, encoding_rs::UTF_16BE, window, cx),
            ))
    }

    pub(super) fn run_reading_action(
        &mut self,
        action: &dyn Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.as_any().is::<ImportPatch>() {
            self.open_patch_file(window, cx);
            return;
        }
        if action.as_any().is::<ExportPatch>() {
            self.export_diff_patch(window, cx);
            return;
        }
        if action.as_any().is::<CompareRevisions>() {
            if let Some(g) = self.active_group() {
                self.open_revision_comparison(g, window, cx);
            } else {
                self.message = "请先打开 Git 仓库".into();
                cx.notify();
            }
            return;
        }
        let Some(path) = self.pane_path(self.active) else {
            self.message = "请先打开要比较或查看历史的文件".into();
            cx.notify();
            return;
        };
        if action.as_any().is::<SelectForComparison>() {
            self.select_for_comparison(path, cx);
        } else if action.as_any().is::<CompareWithSelected>() {
            self.compare_with_selected(path, window, cx);
        } else if action.as_any().is::<CompareWithClipboard>() {
            self.compare_with_clipboard(path, window, cx);
        } else if action.as_any().is::<FileHistory>() {
            self.open_file_history(path, window, cx);
        }
    }
}
