//! Explicit legacy decoding opens a bounded read-only buffer without changing disk bytes.
use super::*;
use std::collections::BTreeSet;

impl Workbench {
    pub(super) fn open_encoded(
        &mut self,
        path: PathBuf,
        encoding: &'static encoding_rs::Encoding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_file_open();
        let generation = self.file_generation;
        let root = self.root_for(&path);
        let job = cx.background_spawn(async move {
            files::text_file_with_encoding(root.as_deref(), &path, Some(encoding))
        });
        self.file_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.file_generation != generation {
                    return;
                }
                match result {
                    Ok(loaded) => {
                        if let Some(id) = this
                            .documents
                            .iter()
                            .find(|doc| doc.path == loaded.path)
                            .map(|doc| doc.id)
                        {
                            let doc = this.document_mut(id).unwrap();
                            if doc.dirty
                                || doc.saving
                                || explorer_ops::file_is_moving(&doc.path, cx)
                            {
                                this.message =
                                    "文件有未保存修改或正在保存／移动，请完成后再按编码打开".into();
                            } else {
                                doc.editor.update(cx, |state, cx| {
                                    state.set_value(loaded.text, window, cx);
                                    state.set_readonly(true, cx);
                                });
                                doc.encoding_generation += 1;
                                doc.encoding = loaded.encoding;
                                doc.readonly = true;
                                doc.disk = Some(loaded.disk);
                                doc.bytes = loaded.bytes;
                                doc.crlf = loaded.crlf;
                                doc.bom = loaded.bom;
                                doc.markdown = None;
                                this.show_pane(Pane::Document(id), window, cx);
                            }
                        } else {
                            let indent = crate::indent::language_default(&loaded.path);
                            this.install_loaded(loaded, indent, false, window, cx);
                        }
                    }
                    Err(error) => {
                        this.message = format!("无法按 {} 打开：{error}", encoding.name())
                    }
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn refresh_encoded(
        &mut self,
        paths: Option<&BTreeSet<PathBuf>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let jobs: Vec<_> = self
            .documents
            .iter_mut()
            .filter(|doc| paths.is_none_or(|paths| paths.contains(&doc.path)))
            .filter_map(|doc| {
                let encoding = encoding_rs::Encoding::for_label(doc.encoding?.as_bytes())?;
                doc.encoding_generation += 1;
                Some((
                    doc.id,
                    doc.path.clone(),
                    doc.disk,
                    encoding,
                    doc.encoding_generation,
                ))
            })
            .collect();
        let jobs: Vec<_> = jobs
            .into_iter()
            .map(|(id, path, known, encoding, generation)| {
                (id, self.root_for(&path), path, known, encoding, generation)
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        let job = cx.background_spawn(async move {
            jobs.into_iter()
                .map(|(id, root, path, known, encoding, generation)| {
                    let loaded =
                        files::text_file_with_encoding(root.as_deref(), &path, Some(encoding));
                    (id, path, known, encoding, generation, loaded)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let loaded = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                for (id, path, known, encoding, generation, result) in loaded {
                    if !this.document(id).is_some_and(|doc| {
                        doc.path == path
                            && doc.disk == known
                            && doc.encoding == Some(encoding.name())
                            && doc.encoding_generation == generation
                    }) {
                        continue;
                    }
                    match result {
                        Ok(loaded) => {
                            if let Some(doc) = this.document_mut(id) {
                                doc.editor.update(cx, |state, cx| {
                                    let cursor = state.cursor();
                                    let scroll = state.scroll_offset();
                                    state.set_value(loaded.text, window, cx);
                                    let text = state.text().to_string();
                                    let mut cursor = cursor.min(text.len());
                                    while !text.is_char_boundary(cursor) {
                                        cursor -= 1;
                                    }
                                    state.set_selected_range(cursor..cursor, cx);
                                    state.set_scroll_offset(scroll, cx);
                                });
                                doc.disk = Some(loaded.disk);
                                doc.bytes = loaded.bytes;
                            }
                        }
                        Err(error) => {
                            this.message = format!("编码只读视图刷新失败，保留旧内容：{error}")
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
