//! User settings, persisted as a small JSON file shared by all windows.
//!
//! macOS: `~/Library/Application Support/ZJ/settings.json`; elsewhere
//! `$XDG_CONFIG_HOME/zj/settings.json` (or `~/.config/zj/settings.json`). `ZJ_SETTINGS`
//! overrides the path (tests, screenshots). A missing or unreadable file means defaults; a
//! write failure is reported in the window that made the change.

use gpui_kit::AppContext as _;
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const EDITOR_FONT_DEFAULT: f32 = 14.;
pub const EDITOR_FONT_MIN: f32 = 10.;
pub const EDITOR_FONT_MAX: f32 = 24.;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Code editor and diff editor font size in pixels (⌘= / ⌘- / ⌘0).
    pub editor_font_size: f32,
    /// Show dot files and folders that are hidden by default (⌘⇧.).
    pub show_hidden: bool,
    /// The diff editor opens inline (top / bottom) instead of side by side.
    pub diff_inline: bool,
    /// Source Control lists only repositories with changes.
    pub hide_clean_repos: bool,
    /// Full-text search applies `.gitignore` and the default excludes.
    pub search_use_excludes: bool,
    /// macOS: the Dock icon's cursor blinks while the app runs (off by default; never with
    /// 减少动态效果).
    pub dock_icon_blink: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor_font_size: EDITOR_FONT_DEFAULT,
            show_hidden: false,
            diff_inline: true,
            hide_clean_repos: false,
            search_use_excludes: true,
            dock_icon_blink: false,
        }
    }
}

impl gpui_kit::Global for Settings {}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("ZJ_SETTINGS") {
            return Some(PathBuf::from(path));
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if cfg!(target_os = "macos") {
            return home.map(|home| home.join("Library/Application Support/ZJ/settings.json"));
        }
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| home.map(|home| home.join(".config")))
            .map(|dir| dir.join("zj/settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) => Self::from_json(&value),
                Err(error) => {
                    eprintln!("event=settings_invalid error={error}");
                    Self::default()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("event=settings_unreadable error={error}");
                Self::default()
            }
        }
    }

    /// Unknown keys are ignored and missing or mistyped ones keep their defaults, so files
    /// written by other versions still load.
    pub fn from_json(value: &Value) -> Self {
        let defaults = Self::default();
        let flag =
            |key: &str, default: bool| value.get(key).and_then(Value::as_bool).unwrap_or(default);
        Self {
            editor_font_size: value
                .get("editor_font_size")
                .and_then(Value::as_f64)
                .map(|size| (size as f32).clamp(EDITOR_FONT_MIN, EDITOR_FONT_MAX))
                .unwrap_or(defaults.editor_font_size),
            show_hidden: flag("show_hidden", defaults.show_hidden),
            diff_inline: flag("diff_inline", defaults.diff_inline),
            hide_clean_repos: flag("hide_clean_repos", defaults.hide_clean_repos),
            search_use_excludes: flag("search_use_excludes", defaults.search_use_excludes),
            dock_icon_blink: flag("dock_icon_blink", defaults.dock_icon_blink),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "editor_font_size": self.editor_font_size,
            "show_hidden": self.show_hidden,
            "diff_inline": self.diff_inline,
            "hide_clean_repos": self.hide_clean_repos,
            "search_use_excludes": self.search_use_excludes,
            "dock_icon_blink": self.dock_icon_blink,
        })
    }

    /// Writes through a temporary file and a rename, so a crash never leaves half a file.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        let text = serde_json::to_vec_pretty(&self.to_json()).map_err(io::Error::other)?;
        fs::write(&temporary, text)?;
        fs::rename(&temporary, path)
    }
}

/// Changes the settings for every window and saves them in the background. Returns the
/// receiver of the write result so the caller can show a failure.
pub fn update(
    cx: &mut gpui_kit::App,
    change: impl FnOnce(&mut Settings),
) -> gpui_kit::Task<io::Result<()>> {
    let mut settings = cx.global::<Settings>().clone();
    change(&mut settings);
    crate::theme::apply_editor_font(settings.editor_font_size, cx);
    cx.set_global(settings.clone());
    cx.refresh_windows();
    cx.background_spawn(async move {
        match Settings::path() {
            Some(path) => settings.save_to(&path),
            None => Err(io::Error::other("找不到设置目录（HOME 未设置）")),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_tolerate_bad_files() {
        let dir = std::env::temp_dir().join(format!("zj-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested/settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());
        let changed = Settings {
            editor_font_size: 16.,
            show_hidden: true,
            diff_inline: false,
            hide_clean_repos: true,
            search_use_excludes: false,
            dock_icon_blink: true,
        };
        changed.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), changed);
        assert!(!path.with_extension("json.tmp").exists());
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        fs::write(
            &path,
            r#"{"editor_font_size": 99, "show_hidden": "yes", "x": 1}"#,
        )
        .unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.editor_font_size, EDITOR_FONT_MAX);
        assert!(!loaded.show_hidden);
        fs::remove_dir_all(dir).unwrap();
    }
}
