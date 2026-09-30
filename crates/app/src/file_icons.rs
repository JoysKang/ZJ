//! File-type icons (vscode-icons subset). Returns asset paths served by `assets::AppAssets`.

use crate::theme;
use gpui_kit::{IntoElement, Styled, img};

pub const FOLDER: &str = "file-icons/default_folder.svg";
pub const FOLDER_OPEN: &str = "file-icons/default_folder_opened.svg";
pub const FILE: &str = "file-icons/default_file.svg";

/// Exact file names first, then extensions (case-insensitive).
pub fn for_file(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    let by_name = match lower.as_str() {
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => Some("git"),
        "dockerfile" | ".dockerignore" => Some("docker"),
        ".env" | ".env.local" | ".env.example" => Some("dotenv"),
        "license" | "licence" | "copying" => Some("text"),
        "cargo.lock" => Some("toml"),
        _ => None,
    };
    let kind = by_name.or_else(|| {
        let extension = lower.rsplit_once('.')?.1;
        Some(match extension {
            "rs" => "rust",
            "md" | "markdown" | "mdx" => "markdown",
            "toml" => "toml",
            "json" | "jsonc" | "json5" => "json",
            "js" | "mjs" | "cjs" | "jsx" => "js",
            "ts" | "mts" | "cts" | "tsx" => "typescript",
            "py" | "pyi" | "pyw" => "python",
            "sh" | "bash" | "zsh" | "fish" => "shell",
            "html" | "htm" => "html",
            "css" | "scss" | "sass" | "less" => "css",
            "yaml" | "yml" => "yaml",
            "key" | "pem" | "crt" | "cer" | "p12" | "pub" => "key",
            "txt" | "text" => "text",
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" => "image",
            "svg" => "svg",
            "sql" => "sql",
            "go" => "go",
            "c" | "h" => "c",
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
            "java" => "java",
            "xml" | "plist" => "xml",
            "log" => "log",
            "diff" | "patch" => "diff",
            "ini" | "cfg" | "conf" => "ini",
            "zip" | "tar" | "gz" | "tgz" | "xz" | "zst" | "7z" => "zip",
            "bin" | "exe" | "dll" | "so" | "dylib" | "o" | "a" => "binary",
            "vue" => "vue",
            "swift" => "swift",
            "kt" | "kts" => "kotlin",
            "lock" => "text",
            _ => return None,
        })
    });
    match kind {
        Some(kind) => ASSETS
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or(FILE, |(_, path)| path),
        None => FILE,
    }
}

const ASSETS: &[(&str, &str)] = &[
    ("rust", "file-icons/file_type_rust.svg"),
    ("markdown", "file-icons/file_type_markdown.svg"),
    ("toml", "file-icons/file_type_toml.svg"),
    ("json", "file-icons/file_type_json.svg"),
    ("js", "file-icons/file_type_js.svg"),
    ("typescript", "file-icons/file_type_typescript.svg"),
    ("python", "file-icons/file_type_python.svg"),
    ("shell", "file-icons/file_type_shell.svg"),
    ("html", "file-icons/file_type_html.svg"),
    ("css", "file-icons/file_type_css.svg"),
    ("yaml", "file-icons/file_type_yaml.svg"),
    ("key", "file-icons/file_type_key.svg"),
    ("git", "file-icons/file_type_git.svg"),
    ("text", "file-icons/file_type_text.svg"),
    ("image", "file-icons/file_type_image.svg"),
    ("svg", "file-icons/file_type_svg.svg"),
    ("sql", "file-icons/file_type_sql.svg"),
    ("go", "file-icons/file_type_go.svg"),
    ("c", "file-icons/file_type_c.svg"),
    ("cpp", "file-icons/file_type_cpp.svg"),
    ("java", "file-icons/file_type_java.svg"),
    ("docker", "file-icons/file_type_docker.svg"),
    ("xml", "file-icons/file_type_xml.svg"),
    ("log", "file-icons/file_type_log.svg"),
    ("diff", "file-icons/file_type_diff.svg"),
    ("ini", "file-icons/file_type_ini.svg"),
    ("dotenv", "file-icons/file_type_dotenv.svg"),
    ("zip", "file-icons/file_type_zip.svg"),
    ("binary", "file-icons/file_type_binary.svg"),
    ("vue", "file-icons/file_type_vue.svg"),
    ("swift", "file-icons/file_type_swift.svg"),
    ("kotlin", "file-icons/file_type_kotlin.svg"),
];

/// A colored 16 px file-type icon.
pub fn icon(path: &'static str) -> impl IntoElement {
    img(path).size(theme::FILE_ICON_SIZE).flex_shrink_0()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_names_and_extensions() {
        assert_eq!(for_file("main.rs"), "file-icons/file_type_rust.svg");
        assert_eq!(for_file("README.MD"), "file-icons/file_type_markdown.svg");
        assert_eq!(for_file(".gitignore"), "file-icons/file_type_git.svg");
        assert_eq!(for_file("Dockerfile"), "file-icons/file_type_docker.svg");
        assert_eq!(for_file("fireblocks.key"), "file-icons/file_type_key.svg");
        assert_eq!(for_file("no_extension"), FILE);
        assert_eq!(for_file("archive.unknown"), FILE);
    }

    #[test]
    fn every_icon_is_embedded() {
        let embedded = |path: &str| crate::assets::EXTRA.iter().any(|(name, _)| *name == path);
        for path in [FOLDER, FOLDER_OPEN, FILE]
            .into_iter()
            .chain(ASSETS.iter().map(|(_, path)| *path))
        {
            assert!(embedded(path), "{path} is not embedded");
        }
        for (name, _) in crate::assets::EXTRA {
            if name.starts_with("file-icons/") {
                assert!(
                    [FOLDER, FOLDER_OPEN, FILE].contains(name)
                        || ASSETS.iter().any(|(_, path)| path == name),
                    "{name} is embedded but unused"
                );
            }
        }
    }
}
