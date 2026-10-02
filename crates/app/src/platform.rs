//! The two macOS integrations the workbench needs beyond GPUI: the system "Reduce motion"
//! setting, and replacing the Dock icon for the optional cursor blink. Elsewhere they are
//! no-ops (`ZJ_REDUCE_MOTION=1` simulates the setting for tests and screenshots).

/// Whether animation should be avoided (macOS: 辅助功能 › 显示 › 减少动态效果).
pub fn reduce_motion() -> bool {
    if std::env::var_os("ZJ_REDUCE_MOTION").is_some_and(|v| v == "1") {
        return true;
    }
    imp::reduce_motion()
}

/// Shows `png` as the Dock icon, or the bundle's own icon again for `None`. Main thread only.
pub fn set_dock_icon(png: Option<&'static [u8]>) {
    imp::set_dock_icon(png)
}

/// The optional Dock icon blink (`dock_icon_blink`): every 530 ms the Dock shows the icon
/// without its cursor, then the bundle icon again. One timer for the whole app, only while the
/// setting is on; macOS only.
#[derive(Default)]
pub struct DockBlink {
    task: Option<gpui_kit::Task<()>>,
}

impl gpui_kit::Global for DockBlink {}

#[cfg(target_os = "macos")]
const CURSOR_OFF: &[u8] = include_bytes!("../assets/app-icon/dock-cursor-off.png");
#[cfg(not(target_os = "macos"))]
const CURSOR_OFF: &[u8] = &[];

impl DockBlink {
    /// Follows the setting; call at startup and whenever settings change.
    pub fn apply(cx: &mut gpui_kit::App) {
        let wanted = cfg!(target_os = "macos")
            && cx.global::<crate::settings::Settings>().dock_icon_blink
            && !reduce_motion();
        let running = cx.default_global::<DockBlink>().task.is_some();
        if wanted == running {
            return;
        }
        if !wanted {
            cx.global_mut::<DockBlink>().task = None;
            set_dock_icon(None);
            return;
        }
        let task = cx.spawn(async move |cx| {
            let mut off = false;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(530))
                    .await;
                off = !off;
                // Reduce motion turned on while blinking: stop with the full icon.
                if reduce_motion() {
                    set_dock_icon(None);
                    cx.update(|cx| cx.global_mut::<DockBlink>().task = None);
                    return;
                }
                set_dock_icon(off.then_some(CURSOR_OFF));
            }
        });
        cx.global_mut::<DockBlink>().task = Some(task);
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    pub fn reduce_motion() -> bool {
        let Some(workspace) = AnyClass::get(c"NSWorkspace") else {
            return false;
        };
        // SAFETY: `sharedWorkspace` and `accessibilityDisplayShouldReduceMotion` are plain
        // AppKit getters (macOS 10.12+) with no arguments; the workspace is checked for nil.
        unsafe {
            let shared: *mut AnyObject = msg_send![workspace, sharedWorkspace];
            if shared.is_null() {
                return false;
            }
            let reduce: Bool = msg_send![shared, accessibilityDisplayShouldReduceMotion];
            reduce.as_bool()
        }
    }

    pub fn set_dock_icon(png: Option<&'static [u8]>) {
        let (Some(app_class), Some(image_class), Some(data_class)) = (
            AnyClass::get(c"NSApplication"),
            AnyClass::get(c"NSImage"),
            AnyClass::get(c"NSData"),
        ) else {
            return;
        };
        // SAFETY: AppKit calls made on the main thread (the caller is a GPUI foreground task).
        // `png` is 'static, so the bytes outlive the NSData that wraps them without copying
        // ownership; the NSImage is created with alloc/init and released after AppKit has
        // retained it via setApplicationIconImage. nil restores the bundle icon.
        unsafe {
            let app: *mut AnyObject = msg_send![app_class, sharedApplication];
            if app.is_null() {
                return;
            }
            let Some(png) = png else {
                let none: *mut AnyObject = std::ptr::null_mut();
                let _: () = msg_send![app, setApplicationIconImage: none];
                return;
            };
            let data: *mut AnyObject = msg_send![data_class, dataWithBytes: png.as_ptr().cast::<std::ffi::c_void>(), length: png.len()];
            if data.is_null() {
                return;
            }
            let image: *mut AnyObject = msg_send![image_class, alloc];
            let image: *mut AnyObject = msg_send![image, initWithData: data];
            if image.is_null() {
                return;
            }
            let _: () = msg_send![app, setApplicationIconImage: image];
            let _: () = msg_send![image, release];
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn reduce_motion() -> bool {
        false
    }

    pub fn set_dock_icon(_: Option<&'static [u8]>) {}
}
