//! The macOS integrations the workbench needs beyond GPUI: the system "Reduce motion"
//! setting, replacing the Dock icon for the optional cursor blink, and the keyboard input
//! source (`input_switch`). Elsewhere they are no-ops (`ZJ_REDUCE_MOTION=1` simulates the
//! setting for tests and screenshots).

use crate::input_switch::Source;

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

/// The keyboard input source in use. Main thread only. Tests never touch the real one.
pub fn input_source() -> Option<Source> {
    if cfg!(test) {
        return None;
    }
    imp::input_source()
}

/// Selects the ASCII source the system would use (the user's English layout), or the
/// enabled source `id`; false when it isn't there or the system refused. Main thread only.
pub fn select_input_source(id: Option<&str>) -> bool {
    if cfg!(test) {
        return false;
    }
    imp::select_input_source(id)
}

/// The first enabled non-ASCII keyboard input source (an input method). Main thread only.
pub fn non_ascii_input_source() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    imp::non_ascii_input_source()
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
    pub use super::tis::{
        current as input_source, non_ascii as non_ascii_input_source, select as select_input_source,
    };
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

#[cfg(target_os = "macos")]
mod tis {
    //! Text Input Sources (Carbon HIToolbox): the system's keyboard input sources.
    use super::Source;
    use std::ffi::{c_char, c_void};

    type Ref = *const c_void;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCopyCurrentKeyboardInputSource() -> Ref;
        fn TISCopyCurrentASCIICapableKeyboardInputSource() -> Ref;
        fn TISSelectInputSource(source: Ref) -> i32;
        fn TISGetInputSourceProperty(source: Ref, key: Ref) -> Ref;
        fn TISCreateInputSourceList(properties: Ref, include_all_installed: u8) -> Ref;
        static kTISPropertyInputSourceID: Ref;
        static kTISPropertyInputSourceIsASCIICapable: Ref;
        static kTISPropertyInputSourceIsSelectCapable: Ref;
        static kTISPropertyInputSourceCategory: Ref;
        static kTISCategoryKeyboardInputSource: Ref;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: Ref);
        fn CFEqual(a: Ref, b: Ref) -> u8;
        fn CFStringGetCString(string: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFArrayGetCount(array: Ref) -> isize;
        fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
        fn CFBooleanGetValue(boolean: Ref) -> u8;
    }

    const UTF8: u32 = 0x0800_0100;

    // SAFETY (whole module): TIS and CF calls on input source objects the system returns.
    // `Copy` / `Create` results are owned and released here; `Get` results are borrowed from
    // their source and only read while it is alive. Every pointer is checked for null.

    fn string(value: Ref) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let mut buffer = [0 as c_char; 256];
        // SAFETY: see the module note; the buffer's size is passed with it.
        let ok = unsafe { CFStringGetCString(value, buffer.as_mut_ptr(), 256, UTF8) };
        if ok == 0 {
            return None;
        }
        // SAFETY: CFStringGetCString wrote a NUL-terminated string into the buffer.
        let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) };
        Some(text.to_string_lossy().into_owned())
    }

    fn flag(source: Ref, key: Ref) -> bool {
        // SAFETY: see the module note.
        unsafe {
            let value = TISGetInputSourceProperty(source, key);
            !value.is_null() && CFBooleanGetValue(value) != 0
        }
    }

    fn describe(source: Ref) -> Option<Source> {
        // SAFETY: see the module note.
        let id = string(unsafe { TISGetInputSourceProperty(source, kTISPropertyInputSourceID) })?;
        // SAFETY: as above.
        let ascii = flag(source, unsafe { kTISPropertyInputSourceIsASCIICapable });
        Some(Source { id, ascii })
    }

    pub fn current() -> Option<Source> {
        // SAFETY: see the module note; the copied source is released after use.
        unsafe {
            let source = TISCopyCurrentKeyboardInputSource();
            if source.is_null() {
                return None;
            }
            let described = describe(source);
            CFRelease(source);
            described
        }
    }

    /// The enabled keyboard sources the user can select, described, for `pick`.
    fn with_enabled<T>(pick: impl Fn(Ref, &Source) -> Option<T>) -> Option<T> {
        // SAFETY: see the module note; the list is released after use, its items are
        // borrowed from it.
        unsafe {
            let list = TISCreateInputSourceList(std::ptr::null(), 0);
            if list.is_null() {
                return None;
            }
            let mut found = None;
            for i in 0..CFArrayGetCount(list) {
                let source = CFArrayGetValueAtIndex(list, i);
                let category = TISGetInputSourceProperty(source, kTISPropertyInputSourceCategory);
                let keyboard =
                    !category.is_null() && CFEqual(category, kTISCategoryKeyboardInputSource) != 0;
                if !keyboard || !flag(source, kTISPropertyInputSourceIsSelectCapable) {
                    continue;
                }
                if let Some(result) = describe(source).and_then(|d| pick(source, &d)) {
                    found = Some(result);
                    break;
                }
            }
            CFRelease(list);
            found
        }
    }

    pub fn select(id: Option<&str>) -> bool {
        let selected = match id {
            // SAFETY: see the module note; the copied source is released after use.
            None => unsafe {
                let source = TISCopyCurrentASCIICapableKeyboardInputSource();
                if source.is_null() {
                    return false;
                }
                let status = TISSelectInputSource(source);
                CFRelease(source);
                status == 0
            },
            Some(id) => with_enabled(|source, d| {
                // SAFETY: see the module note.
                (d.id == id).then(|| unsafe { TISSelectInputSource(source) } == 0)
            })
            .unwrap_or(false),
        };
        if selected {
            reactivate_input();
        }
        selected
    }

    /// Some input methods change in the menu bar but only take keys once the text input
    /// context is activated again.
    fn reactivate_input() {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        let Some(class) = AnyClass::get(c"NSTextInputContext") else {
            return;
        };
        // SAFETY: AppKit calls on the main thread; the current context is checked for nil.
        unsafe {
            let context: *mut AnyObject = msg_send![class, currentInputContext];
            if !context.is_null() {
                let _: () = msg_send![context, deactivate];
                let _: () = msg_send![context, activate];
            }
        }
    }

    pub fn non_ascii() -> Option<String> {
        with_enabled(|_, d| (!d.ascii).then(|| d.id.clone()))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn reduce_motion() -> bool {
        false
    }

    pub fn set_dock_icon(_: Option<&'static [u8]>) {}

    pub fn input_source() -> Option<super::Source> {
        None
    }

    pub fn select_input_source(_: Option<&str>) -> bool {
        false
    }

    pub fn non_ascii_input_source() -> Option<String> {
        None
    }
}
