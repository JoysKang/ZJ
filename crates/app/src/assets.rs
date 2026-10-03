//! Embedded assets: Kit's default icons plus the few extra SVGs this app ships.
//!
//! - `icons/`: Lucide icons Kit does not embed by default (ISC, see `assets/icons`).
//! - `file-icons/`: a small subset of vscode-icons (MIT, see `assets/file-icons`), drawn in
//!   color with `img()`; GPUI rasterizes each once and caches it.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

macro_rules! embed {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!("../assets/", $path)).as_slice(),
        )
    };
}

pub const EXTRA: &[(&str, &[u8])] = &[
    embed!("icons/chevrons-down-up.svg"),
    embed!("icons/files.svg"),
    embed!("icons/git-branch.svg"),
    embed!("icons/columns-2.svg"),
    embed!("icons/rows-2.svg"),
    embed!("icons/square-function.svg"),
    embed!("icons/box.svg"),
    embed!("icons/braces.svg"),
    embed!("icons/package.svg"),
    embed!("icons/type.svg"),
    embed!("icons/variable.svg"),
    embed!("icons/hash.svg"),
    embed!("icons/file-plus.svg"),
    embed!("icons/folder-plus.svg"),
    embed!("icons/whole-word.svg"),
    embed!("icons/regex.svg"),
    embed!("icons/book-marked.svg"),
    embed!("icons/git-commit-horizontal.svg"),
    embed!("icons/git-graph.svg"),
    embed!("icons/tag.svg"),
    embed!("icons/cloud.svg"),
    embed!("icons/replace-all.svg"),
    embed!("icons/case-upper.svg"),
    embed!("icons/text-align-start.svg"),
    // Agent panel (glyphs, session list, cards, composer).
    embed!("icons/fish.svg"),
    embed!("icons/sparkle.svg"),
    embed!("icons/pin.svg"),
    embed!("icons/pin-off.svg"),
    embed!("icons/rotate-ccw-clock.svg"),
    embed!("icons/archive.svg"),
    embed!("icons/archive-restore.svg"),
    embed!("icons/pencil.svg"),
    embed!("icons/square-pen.svg"),
    embed!("icons/at-sign.svg"),
    embed!("icons/paperclip.svg"),
    embed!("icons/shield-check.svg"),
    embed!("icons/shield-alert.svg"),
    embed!("icons/list-todo.svg"),
    embed!("icons/file-pen.svg"),
    embed!("icons/git-compare.svg"),
    embed!("icons/circle.svg"),
    embed!("icons/square-dashed-text.svg"),
    embed!("icons/square.svg"),
    embed!("icons/brain.svg"),
    embed!("icons/database.svg"),
    embed!("icons/shield.svg"),
    embed!("icons/trash.svg"),
    // Terminal panel: split.
    embed!("icons/square-split-horizontal.svg"),
    embed!("icons/code.svg"),
    // Rendered at 3× from logo/zj-sprig*.svg: the dry-brush filter needs a browser-grade
    // rasterizer at this size.
    embed!("logo/zj-sprig.png"),
    embed!("logo/zj-sprig-dark.png"),
    embed!("file-icons/default_folder.svg"),
    embed!("file-icons/default_folder_opened.svg"),
    embed!("file-icons/default_file.svg"),
    embed!("file-icons/file_type_rust.svg"),
    embed!("file-icons/file_type_markdown.svg"),
    embed!("file-icons/file_type_toml.svg"),
    embed!("file-icons/file_type_json.svg"),
    embed!("file-icons/file_type_js.svg"),
    embed!("file-icons/file_type_typescript.svg"),
    embed!("file-icons/file_type_python.svg"),
    embed!("file-icons/file_type_shell.svg"),
    embed!("file-icons/file_type_html.svg"),
    embed!("file-icons/file_type_css.svg"),
    embed!("file-icons/file_type_yaml.svg"),
    embed!("file-icons/file_type_key.svg"),
    embed!("file-icons/file_type_git.svg"),
    embed!("file-icons/file_type_text.svg"),
    embed!("file-icons/file_type_image.svg"),
    embed!("file-icons/file_type_sql.svg"),
    embed!("file-icons/file_type_go.svg"),
    embed!("file-icons/file_type_c.svg"),
    embed!("file-icons/file_type_cpp.svg"),
    embed!("file-icons/file_type_java.svg"),
    embed!("file-icons/file_type_docker.svg"),
    embed!("file-icons/file_type_xml.svg"),
    embed!("file-icons/file_type_log.svg"),
    embed!("file-icons/file_type_diff.svg"),
    embed!("file-icons/file_type_ini.svg"),
    embed!("file-icons/file_type_dotenv.svg"),
    embed!("file-icons/file_type_svg.svg"),
    embed!("file-icons/file_type_zip.svg"),
    embed!("file-icons/file_type_binary.svg"),
    embed!("file-icons/file_type_vue.svg"),
    embed!("file-icons/file_type_swift.svg"),
    embed!("file-icons/file_type_kotlin.svg"),
];

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match EXTRA.iter().find(|(name, _)| *name == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut names = gpui_kit::assets::Assets.list(path)?;
        names.extend(
            EXTRA
                .iter()
                .filter(|(name, _)| name.starts_with(path))
                .map(|(name, _)| SharedString::from(*name)),
        );
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icons_are_svg() {
        for (name, bytes) in EXTRA {
            if name.ends_with(".png") {
                assert!(bytes.starts_with(b"\x89PNG"), "{name}");
            } else {
                assert!(bytes.starts_with(b"<svg"), "{name}");
            }
        }
        let total: usize = EXTRA.iter().map(|(_, bytes)| bytes.len()).sum();
        // ~70 KB of icons plus the ~50 KB welcome logo.
        assert!(total < 160_000, "embedded assets grew to {total} bytes");
    }

    /// Icons the workbench uses that Kit already embeds; they must not be duplicated in EXTRA.
    #[test]
    fn kit_defaults_cover_the_common_icons() {
        for name in [
            "icons/arrow-up.svg",
            "icons/arrow-down.svg",
            "icons/file.svg",
            "icons/check.svg",
            "icons/undo-2.svg",
            "icons/minus.svg",
            "icons/plus.svg",
            "icons/ellipsis.svg",
            "icons/refresh-cw.svg",
        ] {
            assert!(
                gpui_kit::assets::Assets.load(name).unwrap().is_some(),
                "{name}"
            );
            assert!(!EXTRA.iter().any(|(extra, _)| *extra == name), "{name}");
        }
    }
}
