//! ZJ patch (see `vendor/README.md`): the policy behind the low-memory Metal renderer, kept free
//! of Metal / AppKit types so it can be unit-tested on any platform (the ZJ app crate includes
//! this file in its tests).
//!
//! What the low-memory mode changes, per window:
//! - two drawables instead of three (`LOW_MEMORY_DRAWABLE_COUNT`);
//! - the path intermediate texture (window-sized BGRA8, plus a memoryless MSAA texture on Apple
//!   GPUs) is allocated on the first frame that draws a path, not on every resize, and dropped
//!   after `PATH_TEXTURE_IDLE` without paths;
//! - while the window can't be seen, the drawable pool and the path textures are given back
//!   (and, when minimized or the app is hidden, the last presented frame too);
//! - one sprite atlas is shared by all windows instead of one per window.
//!
//! `ZJ_GPU_LOWMEM=0` turns all of it off (upstream behaviour) for A/B measurements.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// Environment variable that disables the low-memory mode when set to `0` / `false` / `off`.
pub const ENV: &str = "ZJ_GPU_LOWMEM";
/// Upstream: triple buffering.
pub const UPSTREAM_DRAWABLE_COUNT: u64 = 3;
/// Double buffering. With `displaySyncEnabled` still on, a frame that takes longer than one
/// refresh makes the next `nextDrawable` wait for the display instead of queueing a third
/// frame, which keeps latency the same or lower and only lowers throughput for frames that
/// already miss vsync; an editor frame takes a few milliseconds.
pub const LOW_MEMORY_DRAWABLE_COUNT: u64 = 2;
/// How long the path textures stay allocated after the last frame that drew a path.
pub const PATH_TEXTURE_IDLE: Duration = Duration::from_secs(10);

/// Whether a value of `ZJ_GPU_LOWMEM` leaves the low-memory mode on (the default).
pub fn enabled_from(value: Option<&str>) -> bool {
    !matches!(
        value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

/// Read once per process.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| enabled_from(std::env::var(ENV).ok().as_deref()))
}

/// The drawable count for the layer.
pub fn drawable_count(low_memory: bool) -> u64 {
    if low_memory {
        LOW_MEMORY_DRAWABLE_COUNT
    } else {
        UPSTREAM_DRAWABLE_COUNT
    }
}

/// Bytes of one BGRA8 surface of `width` × `height` device pixels (a drawable, or the path
/// intermediate texture).
pub fn surface_bytes(width: i32, height: i32) -> u64 {
    width.max(0) as u64 * height.max(0) as u64 * 4
}

/// Whether the path textures have to be (re)allocated for a frame of `wanted` device pixels.
pub fn needs_path_texture(current: Option<(u64, u64)>, wanted: (i32, i32)) -> bool {
    if wanted.0 <= 0 || wanted.1 <= 0 {
        return false;
    }
    current != Some((wanted.0 as u64, wanted.1 as u64))
}

/// Tracks when the path textures were last used.
#[derive(Debug, Default)]
pub struct PathTextureClock {
    last_used: Option<Instant>,
}

impl PathTextureClock {
    pub fn used(&mut self, now: Instant) {
        self.last_used = Some(now);
    }

    /// After a frame without paths: drop the textures once they have been idle long enough.
    pub fn should_release(&self, now: Instant, allocated: bool) -> bool {
        allocated
            && self
                .last_used
                .is_none_or(|last| now.saturating_duration_since(last) >= PATH_TEXTURE_IDLE)
    }
}

/// How much of a window's surfaces can go while it is not visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hidden {
    /// Covered by other windows or on another Space: keep the presented frame so the window
    /// shows its old content (not a blank layer) for the moment before it redraws.
    Occluded,
    /// Minimized or the app is hidden: nothing is on screen, so the presented frame goes too.
    Gone,
}

impl Hidden {
    pub fn new(minimized: bool, app_hidden: bool) -> Self {
        if minimized || app_hidden {
            Hidden::Gone
        } else {
            Hidden::Occluded
        }
    }

    pub fn clears_contents(self) -> bool {
        self == Hidden::Gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_toggle() {
        assert!(enabled_from(None));
        assert!(enabled_from(Some("1")));
        assert!(enabled_from(Some("")));
        for off in ["0", "false", "OFF", " no "] {
            assert!(!enabled_from(Some(off)), "{off}");
        }
        assert_eq!(drawable_count(true), 2);
        assert_eq!(drawable_count(false), 3);
    }

    #[test]
    fn surface_math() {
        // A full-screen 16" MacBook Pro window: 1728 × 1117 points at 2×.
        assert_eq!(surface_bytes(3456, 2234), 30_882_816);
        assert_eq!(surface_bytes(-1, 10), 0);
    }

    #[test]
    fn path_textures_follow_the_frame_size() {
        assert!(needs_path_texture(None, (800, 600)));
        assert!(!needs_path_texture(Some((800, 600)), (800, 600)));
        assert!(needs_path_texture(Some((800, 600)), (801, 600)));
        // Zero-sized textures abort in Metal.
        assert!(!needs_path_texture(None, (0, 600)));
    }

    #[test]
    fn path_textures_are_released_after_idle() {
        let start = Instant::now();
        let mut clock = PathTextureClock::default();
        assert!(!clock.should_release(start, false));
        assert!(clock.should_release(start, true));
        clock.used(start);
        assert!(!clock.should_release(start + Duration::from_secs(9), true));
        assert!(clock.should_release(start + PATH_TEXTURE_IDLE, true));
    }

    #[test]
    fn hidden_windows() {
        assert_eq!(Hidden::new(false, false), Hidden::Occluded);
        assert!(!Hidden::new(false, false).clears_contents());
        assert!(Hidden::new(true, false).clears_contents());
        assert!(Hidden::new(false, true).clears_contents());
    }
}
