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
    embed!("icons/arrow-up.svg"),
    embed!("icons/file.svg"),
    embed!("icons/check.svg"),
    embed!("icons/undo-2.svg"),
    embed!("icons/minus.svg"),
    embed!("icons/plus.svg"),
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
            assert!(bytes.starts_with(b"<svg"), "{name}");
        }
        let total: usize = EXTRA.iter().map(|(_, bytes)| bytes.len()).sum();
        assert!(total < 100_000, "embedded icons grew to {total} bytes");
    }
}
