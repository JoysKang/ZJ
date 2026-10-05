//! The windows open when ZJ last quit, each with its folder and frame, reopened by a launch
//! without arguments. `session.json` sits next to `settings.json`; `ZJ_SESSION` overrides.
//!
//! The record lives in memory and is written when a window opens, gets its folder, or closes
//! while others stay open. A last workspace window that closes gives way to an empty window,
//! which records itself; a last empty window that closes stays recorded (ZJ stays in the
//! Dock). Quitting writes the record with every window's current frame: GPUI runs the quit
//! callbacks before it drops the windows, without calling `on_window_closed` for them.

use gpui_kit::{App, Bounds, Global, Pixels, Window, WindowBounds, WindowId, point, px, size};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// How a window was shown, with its frame in screen points (the restore frame when maximized
/// or fullscreen).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub state: FrameState,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FrameState {
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedWindow {
    /// `None`: a window without a folder.
    pub root: Option<PathBuf>,
    pub frame: Option<Frame>,
    /// The file tabs, in order (untitled buffers are not files to reopen).
    pub tabs: Vec<PathBuf>,
    /// The tab that was active, if it was a file.
    pub active: Option<PathBuf>,
}

pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ZJ_SESSION") {
        return Some(PathBuf::from(path));
    }
    crate::settings::Settings::path().map(|settings| settings.with_file_name("session.json"))
}

/// A missing, unreadable or invalid file is an empty session: the launch opens one window.
pub fn load_from(path: &Path) -> Vec<SavedWindow> {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => from_json(&value),
            Err(error) => {
                eprintln!("event=session_invalid error={error}");
                Vec::new()
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            eprintln!("event=session_unreadable error={error}");
            Vec::new()
        }
    }
}

/// Writes through a temporary file and a rename, so a crash never leaves half a file.
pub fn save_to(path: &Path, windows: &[SavedWindow]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    let text = serde_json::to_vec_pretty(&to_json(windows)).map_err(io::Error::other)?;
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

fn from_json(value: &Value) -> Vec<SavedWindow> {
    let Some(windows) = value.get("windows").and_then(Value::as_array) else {
        return Vec::new();
    };
    windows
        .iter()
        .map(|window| SavedWindow {
            root: window
                .get("root")
                .and_then(Value::as_str)
                .map(PathBuf::from),
            frame: frame_from_json(window),
            tabs: window
                .get("tabs")
                .and_then(Value::as_array)
                .map(|tabs| {
                    tabs.iter()
                        .filter_map(Value::as_str)
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default(),
            active: window
                .get("active")
                .and_then(Value::as_str)
                .map(PathBuf::from),
        })
        .collect()
}

fn frame_from_json(window: &Value) -> Option<Frame> {
    let number = |key: &str| {
        window
            .get(key)
            .and_then(Value::as_f64)
            .map(|value| value as f32)
            .filter(|value| value.is_finite())
    };
    let state = match window.get("state").and_then(Value::as_str)? {
        "windowed" => FrameState::Windowed,
        "maximized" => FrameState::Maximized,
        "fullscreen" => FrameState::Fullscreen,
        _ => return None,
    };
    let frame = Frame {
        state,
        x: number("x")?,
        y: number("y")?,
        width: number("width")?,
        height: number("height")?,
    };
    (frame.width > 0. && frame.height > 0.).then_some(frame)
}

fn to_json(windows: &[SavedWindow]) -> Value {
    let windows: Vec<Value> = windows
        .iter()
        .map(|window| {
            let mut value = json!({
                // A path that is not UTF-8 cannot round-trip; it reopens as an empty window.
                "root": window.root.as_deref().and_then(Path::to_str),
            });
            if let Some(frame) = window.frame {
                value["state"] = json!(match frame.state {
                    FrameState::Windowed => "windowed",
                    FrameState::Maximized => "maximized",
                    FrameState::Fullscreen => "fullscreen",
                });
                value["x"] = json!(frame.x);
                value["y"] = json!(frame.y);
                value["width"] = json!(frame.width);
                value["height"] = json!(frame.height);
            }
            if !window.tabs.is_empty() {
                // Paths that are not UTF-8 cannot round-trip and are left out.
                let tabs: Vec<&str> = window.tabs.iter().filter_map(|t| t.to_str()).collect();
                value["tabs"] = json!(tabs);
            }
            if let Some(active) = window.active.as_deref().and_then(Path::to_str) {
                value["active"] = json!(active);
            }
            value
        })
        .collect();
    json!({ "windows": windows })
}

/// The windows a launch reopens: folders that are gone or no longer directories become empty
/// windows (their frames stay), at most `max` windows.
pub fn restorable(saved: Vec<SavedWindow>, max: usize) -> Vec<SavedWindow> {
    saved
        .into_iter()
        .take(max)
        .map(|window| SavedWindow {
            root: window
                .root
                .and_then(|root| fs::canonicalize(root).ok())
                .filter(|root| root.is_dir()),
            frame: window.frame,
            // Files that are gone are not reopened.
            tabs: window
                .tabs
                .into_iter()
                .filter(|tab| tab.is_file())
                .collect(),
            active: window.active.filter(|active| active.is_file()),
        })
        .collect()
}

impl Frame {
    pub fn of(bounds: WindowBounds) -> Self {
        let (state, frame) = match bounds {
            WindowBounds::Windowed(frame) => (FrameState::Windowed, frame),
            WindowBounds::Maximized(frame) => (FrameState::Maximized, frame),
            WindowBounds::Fullscreen(frame) => (FrameState::Fullscreen, frame),
        };
        Self {
            state,
            x: frame.origin.x.into(),
            y: frame.origin.y.into(),
            width: frame.size.width.into(),
            height: frame.size.height.into(),
        }
    }

    /// `None` when no display shows the frame's top edge any more (a disconnected monitor):
    /// the window then opens at the default place.
    pub fn window_bounds(self, displays: &[Bounds<Pixels>]) -> Option<WindowBounds> {
        let frame = Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        );
        let title = Bounds::new(
            frame.origin,
            size(frame.size.width, crate::theme::TITLE_HEIGHT),
        );
        displays
            .iter()
            .any(|display| display.intersects(&title))
            .then_some(match self.state {
                FrameState::Windowed => WindowBounds::Windowed(frame),
                FrameState::Maximized => WindowBounds::Maximized(frame),
                FrameState::Fullscreen => WindowBounds::Fullscreen(frame),
            })
    }
}

/// The in-memory record; present only in the real app (tests install it themselves).
struct Tracker {
    /// `None`: no place to keep the file; windows are still tracked for closing.
    path: Option<PathBuf>,
    windows: Vec<(WindowId, SavedWindow)>,
}

impl Global for Tracker {}

impl Tracker {
    fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let windows: Vec<SavedWindow> = self
            .windows
            .iter()
            .map(|(_, window)| window.clone())
            .collect();
        if let Err(error) = save_to(path, &windows) {
            eprintln!("event=session_save_failed error={error}");
        }
    }
}

/// Starts recording windows; called once at launch.
pub fn track(cx: &mut App) {
    track_at(path(), cx);
}

pub fn track_at(path: Option<PathBuf>, cx: &mut App) {
    cx.set_global(Tracker {
        path,
        windows: Vec::new(),
    });
    cx.on_app_quit(|cx| {
        if let Some(tracker) = cx.try_global::<Tracker>()
            && !tracker.windows.is_empty()
        {
            tracker.save();
        }
        async {}
    })
    .detach();
}

/// Forgets a closed window and returns what it showed. A last window without a folder stays
/// recorded: ZJ stays in the Dock with no windows, and the next launch reopens it.
pub fn closed(id: WindowId, cx: &mut App) -> Option<SavedWindow> {
    let last = cx.windows().is_empty();
    if !cx.has_global::<Tracker>() {
        return None;
    }
    let tracker = cx.global_mut::<Tracker>();
    let index = tracker
        .windows
        .iter()
        .position(|(window, _)| *window == id)?;
    let saved = tracker.windows[index].1.clone();
    if last && saved.root.is_none() {
        return Some(saved);
    }
    tracker.windows.remove(index);
    // A last workspace window is replaced by an empty window, which writes the record.
    if !last {
        tracker.save();
    }
    Some(saved)
}

/// Records a window's folder and frame. `persist` writes the file too (a window opened or got
/// its folder); frame changes alone only update memory until the next write.
pub fn remember(window: &Window, root: Option<&Path>, persist: bool, cx: &mut App) {
    if !cx.has_global::<Tracker>() {
        return;
    }
    let id = window.window_handle().window_id();
    let root = root.map(Path::to_path_buf);
    let frame = Some(Frame::of(window.window_bounds()));
    let open: Vec<WindowId> = cx
        .windows()
        .iter()
        .map(|window| window.window_id())
        .collect();
    let tracker = cx.global_mut::<Tracker>();
    // A kept last window that has since closed is no longer part of the session.
    tracker
        .windows
        .retain(|(window, _)| *window == id || open.contains(window));
    match tracker.windows.iter_mut().find(|(window, _)| *window == id) {
        // The tabs are kept: `remember_tabs` owns them.
        Some((_, entry)) => {
            entry.root = root;
            entry.frame = frame;
        }
        None => tracker.windows.push((
            id,
            SavedWindow {
                root,
                frame,
                ..Default::default()
            },
        )),
    }
    if persist {
        tracker.save();
    }
}

/// Records a window's file tabs and the active one. `persist` writes the file (a tab opened
/// or closed); switching tabs only updates memory until the next write.
pub fn remember_tabs(
    window: &Window,
    tabs: Vec<PathBuf>,
    active: Option<PathBuf>,
    persist: bool,
    cx: &mut App,
) {
    if !cx.has_global::<Tracker>() {
        return;
    }
    let id = window.window_handle().window_id();
    let tracker = cx.global_mut::<Tracker>();
    let Some((_, entry)) = tracker.windows.iter_mut().find(|(window, _)| *window == id) else {
        return;
    };
    if entry.tabs == tabs && entry.active == active {
        return;
    }
    entry.tabs = tabs;
    entry.active = active;
    if persist {
        tracker.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zj-session-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frame(state: FrameState) -> Frame {
        Frame {
            state,
            x: 10.,
            y: 20.,
            width: 1100.,
            height: 720.,
        }
    }

    #[test]
    fn windows_round_trip_with_folders_and_frames() {
        let dir = temp("round-trip");
        let path = dir.join("nested/session.json");
        let windows = vec![
            SavedWindow {
                root: Some(dir.join("a")),
                frame: Some(frame(FrameState::Windowed)),
                tabs: vec![dir.join("a/x.rs"), dir.join("a/y.rs")],
                active: Some(dir.join("a/y.rs")),
            },
            SavedWindow {
                root: None,
                frame: Some(frame(FrameState::Maximized)),
                ..Default::default()
            },
            SavedWindow {
                root: Some(dir.join("b")),
                frame: None,
                ..Default::default()
            },
        ];
        save_to(&path, &windows).unwrap();
        assert_eq!(load_from(&path), windows);
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tabs_of_files_that_are_gone_are_not_reopened() {
        let dir = temp("tabs");
        fs::write(dir.join("here.rs"), "").unwrap();
        let restored = restorable(
            vec![SavedWindow {
                root: Some(dir.clone()),
                tabs: vec![dir.join("here.rs"), dir.join("gone.rs")],
                active: Some(dir.join("gone.rs")),
                ..Default::default()
            }],
            5,
        );
        assert_eq!(restored[0].tabs, vec![dir.join("here.rs")]);
        assert_eq!(restored[0].active, None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_broken_files_are_empty_sessions() {
        let dir = temp("broken");
        let path = dir.join("session.json");
        assert!(load_from(&path).is_empty());
        fs::write(&path, "{ not json").unwrap();
        assert!(load_from(&path).is_empty());
        fs::write(&path, r#"{"windows": "no"}"#).unwrap();
        assert!(load_from(&path).is_empty());
        // A bad frame drops only the frame.
        fs::write(
            &path,
            r#"{"windows": [{"root": "/x", "state": "windowed", "x": 0, "y": 0, "width": -1, "height": 5},
                            {"root": null, "state": "tiled", "x": 0, "y": 0, "width": 9, "height": 9}]}"#,
        )
        .unwrap();
        assert_eq!(
            load_from(&path),
            [
                SavedWindow {
                    root: Some("/x".into()),
                    frame: None,
                    ..Default::default()
                },
                SavedWindow {
                    root: None,
                    frame: None,
                    ..Default::default()
                }
            ]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn gone_folders_reopen_as_empty_windows_and_the_count_is_capped() {
        let dir = temp("restorable");
        fs::create_dir_all(dir.join("kept")).unwrap();
        fs::write(dir.join("file"), "").unwrap();
        let saved = [dir.join("kept"), dir.join("gone"), dir.join("file")]
            .into_iter()
            .map(|root| SavedWindow {
                root: Some(root),
                frame: Some(frame(FrameState::Windowed)),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let restored = restorable(saved.clone(), 5);
        assert_eq!(
            restored.iter().map(|w| w.root.clone()).collect::<Vec<_>>(),
            [
                Some(fs::canonicalize(dir.join("kept")).unwrap()),
                None,
                None
            ]
        );
        assert!(restored.iter().all(|w| w.frame.is_some()));
        assert_eq!(restorable(saved, 2).len(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn frames_off_every_display_open_at_the_default_place() {
        let display = Bounds::new(point(px(0.), px(0.)), size(px(1512.), px(982.)));
        let windowed = frame(FrameState::Windowed);
        assert_eq!(
            windowed.window_bounds(&[display]),
            Some(WindowBounds::Windowed(Bounds::new(
                point(px(10.), px(20.)),
                size(px(1100.), px(720.))
            )))
        );
        let maximized = frame(FrameState::Maximized).window_bounds(&[display]);
        assert!(matches!(maximized, Some(WindowBounds::Maximized(_))));
        let elsewhere = Frame {
            x: 3000.,
            ..windowed
        };
        assert_eq!(elsewhere.window_bounds(&[display]), None);
        assert_eq!(windowed.window_bounds(&[]), None);
        assert_eq!(
            Frame::of(WindowBounds::Fullscreen(display)).state,
            FrameState::Fullscreen
        );
    }
}
