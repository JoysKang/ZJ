//! Preview replacement and tab moves operate on stable file/pane identities.
use super::editor_area::{DraggedTab, TabKey};
use super::*;

impl Workbench {
    pub(super) fn open_preview_file(
        &mut self,
        path: PathBuf,
        root: Option<PathBuf>,
        fixed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if fixed {
            self.tabs.preview = None;
        }
        let existing = self
            .documents
            .iter()
            .find(|doc| doc.path == path)
            .map(|doc| Pane::Document(doc.id))
            .or_else(|| {
                self.images
                    .iter()
                    .find(|image| image.path == path)
                    .map(|image| Pane::Image(image.id))
            });
        let preview = !fixed && existing.is_none_or(|pane| self.tabs.preview == Some(pane));
        self.open_file(path.clone(), root, window, cx);
        if preview {
            self.tabs.preview_request = Some((self.file_generation, path));
        }
    }

    pub(super) fn preview_replacement(&self, path: &std::path::Path) -> Option<Pane> {
        let (generation, requested) = self.tabs.preview_request.as_ref()?;
        if *generation != self.file_generation || requested != path {
            return None;
        }
        let pane = self.tabs.preview?;
        match pane {
            Pane::Document(id)
                if self
                    .document(id)
                    .is_some_and(|doc| !doc.dirty && !doc.saving) =>
            {
                Some(pane)
            }
            Pane::Image(_) => Some(pane),
            _ => None,
        }
    }

    pub(super) fn install_preview_tab(
        &mut self,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((generation, path)) = self.tabs.preview_request.clone() else {
            return;
        };
        if generation != self.file_generation {
            self.tabs.preview_request = None;
            return;
        }
        if self.pane_path(pane).as_ref() != Some(&path) {
            return;
        }
        self.tabs.preview_request = None;
        let previous = self.tabs.preview.replace(pane);
        if previous == Some(pane) {
            return;
        }
        match previous {
            Some(Pane::Document(id))
                if self
                    .document(id)
                    .is_some_and(|doc| !doc.dirty && !doc.saving) =>
            {
                self.remove_document(id, window, cx)
            }
            Some(Pane::Image(id)) => self.close_image(id, window, cx),
            _ => {}
        }
    }

    pub(super) fn promote_edited_preview(&mut self) {
        if let Some(Pane::Document(id)) = self.tabs.preview
            && self.document(id).is_none_or(|doc| doc.dirty || doc.saving)
        {
            self.tabs.preview = None;
        }
    }

    pub(super) fn reorder_tabs(
        &mut self,
        dragged: &DraggedTab,
        target: Option<TabKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if dragged.origin != cx.entity_id() || target.as_ref() == Some(&dragged.key) {
            return;
        }
        let mut order = dragged.order.as_ref().clone();
        if !order.contains(&dragged.key) {
            return;
        }
        order.retain(|key| *key != dragged.key);
        let index = target
            .as_ref()
            .and_then(|target| order.iter().position(|key| key == target))
            .unwrap_or(order.len());
        order.insert(index, dragged.key.clone());
        self.tab_order = order
            .iter()
            .filter_map(|key| match key {
                TabKey::File(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        self.tabs.order = order;
        self.tabs.reveal_selection();
        self.remember_tabs(true, window, cx);
        cx.notify();
    }
}
