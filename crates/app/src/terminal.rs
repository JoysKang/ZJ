//! The integrated terminal's backend: a shell on a pty, emulated by `alacritty_terminal`
//! (docs/adr/0006). This side owns the process and the grid; `workbench/terminal_view.rs`
//! draws the grid and turns keys and mouse input into bytes for the shell.

use alacritty_terminal::{
    event::{Event, EventListener, Notify, OnResize, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg, Notifier},
    grid::Dimensions,
    sync::FairMutex,
    term::{Config, Term, TermMode, color::COUNT},
    tty,
    vte::ansi::{Color, CursorShape as AnsiCursorShape, CursorStyle, NamedColor, Rgb},
};
use gpui_kit::{Hsla, Keystroke, Rgba};
use std::{
    borrow::Cow,
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub use alacritty_terminal::{
    grid::Scroll,
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{cell::Flags, color::Colors},
    tty::Shell,
    vte::ansi::CursorShape,
};

/// Scrollback kept per terminal, VS Code's default (`terminal.integrated.scrollback`).
const SCROLLBACK: usize = 1000;

/// In-flight emulator events per terminal. The bound keeps a stalled UI from piling up
/// events without limit; a full channel never makes the pty reader wait (see [`Listener`]).
const EVENTS: usize = 4096;

/// Forwards the emulator's events to the view's task; the channel is the view's only wakeup,
/// so an idle terminal costs no timer.
///
/// The emulator sends events from the pty reader while it holds the terminal lock, and the
/// view takes that lock to paint and to answer color queries, so sending must never wait for
/// the view: on a full channel the two would deadlock. So nothing blocks:
///
/// - Title changes and the shell's exit are state the view reads after each wakeup
///   ([`Terminal::take_title`], [`Terminal::exited`]): the latest title wins, and exit is a
///   flag that cannot be lost.
/// - Replies for the shell (`PtyWrite`) go through the view, in order with the replies it
///   computes; when the channel is full they go straight to the pty writer instead.
/// - Repaint hints are coalescible, and the remaining requests (clipboard, color and size
///   queries) are dropped on a full channel: a program flooding them loses some answers
///   rather than hanging the window.
#[derive(Clone)]
pub struct Listener(Arc<Shared>);

struct Shared {
    events: async_channel::Sender<Event>,
    /// The pty writer, set once the event loop exists (before it reads anything).
    pty: OnceLock<EventLoopSender>,
    /// A title change the view has not seen yet; `Some(None)` resets the title.
    title: Mutex<Option<Option<String>>>,
    exited: AtomicBool,
}

impl Listener {
    fn new() -> (Self, async_channel::Receiver<Event>) {
        let (events, receiver) = async_channel::bounded(EVENTS);
        let shared = Shared {
            events,
            pty: OnceLock::new(),
            title: Mutex::new(None),
            exited: AtomicBool::new(false),
        };
        (Listener(Arc::new(shared)), receiver)
    }

    fn wake(&self) {
        let _ = self.0.events.try_send(Event::Wakeup);
    }

    fn set_title(&self, title: Option<String>) {
        *self.0.title.lock().unwrap_or_else(|e| e.into_inner()) = Some(title);
        self.wake();
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => self.set_title(Some(title)),
            Event::ResetTitle => self.set_title(None),
            Event::Exit | Event::ChildExit(_) => {
                self.0.exited.store(true, Ordering::Release);
                self.wake();
            }
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Bell => {
                self.wake()
            }
            Event::PtyWrite(text) => {
                if let Err(error) = self.0.events.try_send(Event::PtyWrite(text))
                    && let Event::PtyWrite(text) = error.into_inner()
                    && let Some(pty) = self.0.pty.get()
                {
                    let _ = pty.send(Msg::Input(text.into_bytes().into()));
                }
            }
            request => {
                let _ = self.0.events.try_send(request);
            }
        }
    }
}

/// The grid size in cells, plus the cell size in pixels that some programs ask for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub columns: usize,
    pub lines: usize,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for Size {
    fn default() -> Self {
        Size {
            columns: 80,
            lines: 24,
            cell_width: 8.,
            cell_height: 16.,
        }
    }
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }

    fn screen_lines(&self) -> usize {
        self.lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

impl Size {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines.min(u16::MAX as usize) as u16,
            num_cols: self.columns.min(u16::MAX as usize) as u16,
            cell_width: self.cell_width as u16,
            cell_height: self.cell_height as u16,
        }
    }
}

pub struct Terminal {
    pub term: Arc<FairMutex<Term<Listener>>>,
    /// Wakeups and requests that need the UI; after each, the view reads
    /// [`Terminal::take_title`] and [`Terminal::exited`].
    pub events: async_channel::Receiver<Event>,
    shared: Arc<Shared>,
    notifier: Notifier,
    size: Size,
}

impl Terminal {
    /// Starts `shell` (the user's login shell when `None`) in `cwd`. The pty reader runs on
    /// its own thread until the shell exits or the terminal is dropped.
    pub fn spawn(cwd: Option<PathBuf>, shell: Option<Shell>) -> io::Result<Self> {
        let size = Size::default();
        let (listener, events) = Listener::new();
        let config = Config {
            scrolling_history: SCROLLBACK,
            // A thin bar like the editor's; programs can still ask for a block (vim's normal
            // mode does).
            default_cursor_style: CursorStyle {
                shape: AnsiCursorShape::Beam,
                blinking: false,
            },
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        // Passed to the shell only: setting them on ZJ's own environment (`tty::setup_env`)
        // would leak into the git processes it runs.
        let env = HashMap::from([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("TERM_PROGRAM".to_string(), "ZJ".to_string()),
        ]);
        let options = tty::Options {
            shell,
            working_directory: cwd,
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&options, size.window_size(), 0)?;
        let shared = listener.0.clone();
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let _ = shared.pty.set(event_loop.channel());
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Terminal {
            term,
            events,
            shared,
            notifier,
            size,
        })
    }

    /// The title the shell set since the last call; `Some(None)` when it reset the title.
    pub fn take_title(&self) -> Option<Option<String>> {
        self.shared
            .title
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// The shell has exited.
    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.notifier.on_resize(size.window_size());
        self.term.lock().resize(size);
    }

    /// Pasted text: newlines become Enter, and bracketed-paste mode wraps it so the shell
    /// does not run it line by line.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let bytes = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.write(bytes.into_bytes());
    }
}

impl Drop for Terminal {
    /// Stops the reader thread; dropping the pty there hangs up the shell (SIGHUP) and reaps it.
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

/// Bytes a key sends to the shell (xterm sequences, as VS Code's xterm.js sends them).
/// Printable text without Control goes through the input handler instead, so IMEs compose;
/// ⌘ keys are the app's.
pub fn key_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = &keystroke.modifiers;
    if m.platform || m.function {
        return None;
    }
    // xterm's modifier parameter: 1 + Shift + 2·Alt + 4·Ctrl.
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let cursor = |code: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{code}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{code}").into_bytes()
        } else {
            format!("\x1b[{code}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[{code};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };
    let alt = |bytes: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if m.alt {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };
    let bytes = match keystroke.key.as_str() {
        "enter" => alt(b"\r"),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => alt(b"\t"),
        "escape" => alt(b"\x1b"),
        "backspace" if m.control => alt(b"\x08"),
        "backspace" => alt(b"\x7f"),
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => cursor('P'),
        "f2" => cursor('Q'),
        "f3" => cursor('R'),
        "f4" => cursor('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        key if m.control => alt(&[control_byte(key)?]),
        _ => return None,
    };
    Some(bytes)
}

/// ⌃ + key as a C0 control byte (⌃C → 0x03, ⌃[ → ESC, ⌃Space → NUL).
fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return match key {
            "space" => Some(0),
            _ => None,
        };
    };
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// The tab label for a title the shell set: the path only, without the `user@host:` prefix
/// that oh-my-zsh, bash and others put in front of it.
pub fn tab_title(title: &str) -> &str {
    let title = title.trim();
    match title.split_once(':') {
        Some((who, path))
            if who.contains('@')
                && !who.contains(char::is_whitespace)
                && !path.trim().is_empty() =>
        {
            path.trim()
        }
        _ => title,
    }
}

/// The 16 ANSI colors, then the default foreground, background and cursor.
#[derive(Clone, Copy)]
pub struct Palette {
    pub ansi: [Hsla; 16],
    pub foreground: Hsla,
    pub background: Hsla,
    pub cursor: Hsla,
}

impl Palette {
    /// A cell color: the program's OSC overrides first, then the theme, then xterm's 256-color
    /// cube and gray ramp.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Hsla {
        match color {
            Color::Spec(rgb) => from_rgb(rgb),
            Color::Indexed(index) => self.indexed(index as usize, overrides),
            Color::Named(name) => self.named(name, overrides),
        }
    }

    fn indexed(&self, index: usize, overrides: &Colors) -> Hsla {
        if let Some(rgb) = overrides[index] {
            return from_rgb(rgb);
        }
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let index = index - 16;
                let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
                from_rgb(Rgb {
                    r: level(index / 36),
                    g: level(index / 6 % 6),
                    b: level(index % 6),
                })
            }
            _ => {
                let gray = ((index.min(255) - 232) * 10 + 8) as u8;
                from_rgb(Rgb {
                    r: gray,
                    g: gray,
                    b: gray,
                })
            }
        }
    }

    fn named(&self, name: NamedColor, overrides: &Colors) -> Hsla {
        if let Some(rgb) = overrides[name] {
            return from_rgb(rgb);
        }
        match name {
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimForeground => self.foreground.opacity(0.66),
            NamedColor::DimBlack
            | NamedColor::DimRed
            | NamedColor::DimGreen
            | NamedColor::DimYellow
            | NamedColor::DimBlue
            | NamedColor::DimMagenta
            | NamedColor::DimCyan
            | NamedColor::DimWhite => {
                let base = name as usize - NamedColor::DimBlack as usize;
                self.ansi[base].opacity(0.66)
            }
            other => self.ansi[(other as usize).min(15)],
        }
    }

    /// `OSC 4/10/11` color queries answer in the theme's colors.
    pub fn rgb(&self, index: usize, overrides: &Colors) -> Rgb {
        if let Some(rgb) = overrides[index.min(COUNT - 1)] {
            return rgb;
        }
        let color = match index {
            0..=255 => self.indexed(index, overrides),
            i if i == NamedColor::Foreground as usize => self.foreground,
            i if i == NamedColor::Background as usize => self.background,
            _ => self.cursor,
        };
        let Rgba { r, g, b, .. } = color.to_rgb();
        let byte = |v: f32| (v * 255.).round() as u8;
        Rgb {
            r: byte(r),
            g: byte(g),
            b: byte(b),
        }
    }
}

fn from_rgb(rgb: Rgb) -> Hsla {
    gpui_kit::rgb(u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(key: &str, mode: TermMode) -> Option<String> {
        key_bytes(&Keystroke::parse(key).unwrap(), mode).map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn events_never_block_the_reader_and_keep_title_and_exit() {
        let (listener, events) = Listener::new();
        let (done, finished) = std::sync::mpsc::channel();
        let sender = listener.clone();
        // Nobody reads `events`, as when the UI thread waits for the terminal lock.
        std::thread::spawn(move || {
            for i in 0..EVENTS * 2 {
                sender.send_event(Event::Title(format!("title {i}")));
                sender.send_event(Event::Wakeup);
                sender.send_event(Event::PtyWrite("\x1b[0n".into()));
                sender.send_event(Event::TextAreaSizeRequest(Arc::new(|_| String::new())));
            }
            sender.send_event(Event::ChildExit(std::process::ExitStatus::default()));
            let _ = done.send(());
        });
        finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("sending an event blocked");
        assert_eq!(events.len(), EVENTS);
        let shared = &listener.0;
        assert!(shared.exited.load(Ordering::Acquire));
        let title = shared.title.lock().unwrap().take();
        assert_eq!(title, Some(Some(format!("title {}", EVENTS * 2 - 1))));
        listener.send_event(Event::ResetTitle);
        assert_eq!(shared.title.lock().unwrap().take(), Some(None));
    }

    #[test]
    fn keys_map_to_xterm_sequences() {
        let normal = TermMode::empty();
        assert_eq!(bytes("enter", normal).as_deref(), Some("\r"));
        assert_eq!(bytes("ctrl-c", normal).as_deref(), Some("\x03"));
        assert_eq!(bytes("ctrl-[", normal).as_deref(), Some("\x1b"));
        assert_eq!(bytes("ctrl-space", normal).as_deref(), Some("\0"));
        assert_eq!(bytes("alt-backspace", normal).as_deref(), Some("\x1b\x7f"));
        assert_eq!(bytes("shift-tab", normal).as_deref(), Some("\x1b[Z"));
        assert_eq!(bytes("up", normal).as_deref(), Some("\x1b[A"));
        assert_eq!(bytes("up", TermMode::APP_CURSOR).as_deref(), Some("\x1bOA"));
        assert_eq!(bytes("ctrl-left", normal).as_deref(), Some("\x1b[1;5D"));
        assert_eq!(bytes("alt-left", normal).as_deref(), Some("\x1b[1;3D"));
        assert_eq!(bytes("delete", normal).as_deref(), Some("\x1b[3~"));
        assert_eq!(bytes("shift-pageup", normal).as_deref(), Some("\x1b[5;2~"));
        assert_eq!(bytes("f5", normal).as_deref(), Some("\x1b[15~"));
        // Text goes through the input handler; ⌘ keys stay with the app.
        assert_eq!(bytes("a", normal), None);
        assert_eq!(bytes("shift-a", normal), None);
        assert_eq!(bytes("cmd-c", normal), None);
    }

    #[test]
    fn tab_titles_drop_user_and_host() {
        assert_eq!(tab_title("joys@MacBook-Pro: ~/ZJ"), "~/ZJ");
        assert_eq!(tab_title("joys@mbp:~/work/a b"), "~/work/a b");
        assert_eq!(tab_title("vim main.rs"), "vim main.rs");
        assert_eq!(tab_title("~/ZJ"), "~/ZJ");
        assert_eq!(tab_title("ssh: user@host"), "ssh: user@host");
        assert_eq!(tab_title("joys@mbp:"), "joys@mbp:");
    }

    #[test]
    fn colors_follow_the_theme_then_xterm() {
        let white: Hsla = gpui_kit::rgb(0xffffff).into();
        let palette = Palette {
            ansi: [white; 16],
            foreground: white,
            background: white,
            cursor: white,
        };
        let colors = Colors::default();
        assert_eq!(palette.rgb(16, &colors), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(palette.rgb(196, &colors), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(palette.rgb(232, &colors), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(
            palette.rgb(1, &colors),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
    }
}
