//! One terminal: draws the emulator's grid, sends keys, IME text, paste and wheel input to the
//! shell, and selects with the mouse. The process and the grid live in `crate::terminal`.

use crate::terminal::{
    self, Column, CursorShape, Flags, Line, Point as GridPoint, Scroll, Selection, SelectionType,
    Shell, Side, Size as GridSize, Terminal,
};
use crate::theme;
use alacritty_terminal::{event::Event, term::TermMode};
use gpui_kit::{component::Theme, *};
use std::{ops::Range, path::PathBuf};

pub(super) enum TerminalEvent {
    /// The shell exited; the panel closes the terminal, as VS Code does.
    Exited,
    TitleChanged,
}

/// Where the last frame put the grid, for mapping the mouse and placing the IME window.
#[derive(Clone, Copy)]
struct Frame {
    origin: Point<Pixels>,
    cell: Size<Pixels>,
    size: GridSize,
    display_offset: usize,
    cursor: GridPoint,
}

pub(super) struct TerminalView {
    terminal: Terminal,
    focus: FocusHandle,
    title: String,
    /// IME composition shown at the cursor until committed.
    marked: Option<String>,
    frame: Option<Frame>,
    selecting: bool,
    /// Wheel movement smaller than a line, kept for the next event.
    scroll_rest: Pixels,
    exited: bool,
    _events: Task<()>,
    _keys: Subscription,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    /// Starts the shell; the view is built only once it runs.
    pub(super) fn spawn(
        cwd: Option<PathBuf>,
        shell: Option<Shell>,
        cx: &mut App,
    ) -> std::io::Result<Entity<Self>> {
        let terminal = Terminal::spawn(cwd, shell)?;
        Ok(cx.new(|cx| TerminalView::new(terminal, cx)))
    }

    fn new(terminal: Terminal, cx: &mut Context<Self>) -> Self {
        let title = shell_name();
        let events = terminal.events.clone();
        let _events = cx.spawn(async move |this, cx| {
            while let Ok(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if this
                    .update(cx, |this, cx| this.handle_events(batch, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let focus = cx.focus_handle();
        let weak = cx.weak_entity();
        let _keys = cx.intercept_keystrokes({
            let focus = focus.clone();
            move |event, window, cx| {
                if !focus.is_focused(window) {
                    return;
                }
                let handled = weak
                    .update(cx, |this, cx| this.key(&event.keystroke, cx))
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            }
        });
        TerminalView {
            terminal,
            focus,
            title,
            marked: None,
            frame: None,
            selecting: false,
            scroll_rest: px(0.),
            exited: false,
            _events,
            _keys,
        }
    }

    pub(super) fn title(&self) -> &str {
        &self.title
    }

    #[cfg(test)]
    pub(super) fn screen_text(&self) -> String {
        let term = self.terminal.term.lock();
        let grid = term.grid();
        let mut text = String::new();
        for line in 0..self.terminal.size().lines {
            for column in 0..self.terminal.size().columns {
                text.push(grid[GridPoint::new(Line(line as i32), Column(column))].c);
            }
            text.push('\n');
        }
        text
    }

    fn handle_events(&mut self, events: Vec<Event>, cx: &mut Context<Self>) {
        for event in events {
            match event {
                Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
                Event::Title(title) => {
                    self.title = title;
                    cx.emit(TerminalEvent::TitleChanged);
                }
                Event::ResetTitle => {
                    self.title = shell_name();
                    cx.emit(TerminalEvent::TitleChanged);
                }
                Event::PtyWrite(text) => self.terminal.write(text.into_bytes()),
                Event::ClipboardStore(_, text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text))
                }
                Event::ClipboardLoad(_, format) => {
                    let text = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .unwrap_or_default();
                    self.terminal.write(format(&text).into_bytes());
                }
                Event::ColorRequest(index, format) => {
                    let palette = theme::colors(cx).terminal;
                    let rgb = palette.rgb(index, self.terminal.term.lock().colors());
                    self.terminal.write(format(rgb).into_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    let size = self.terminal.size();
                    let window = alacritty_terminal::event::WindowSize {
                        num_lines: size.lines as u16,
                        num_cols: size.columns as u16,
                        cell_width: size.cell_width as u16,
                        cell_height: size.cell_height as u16,
                    };
                    self.terminal.write(format(window).into_bytes());
                }
                Event::Bell => {}
                Event::Exit | Event::ChildExit(_) => {
                    if !self.exited {
                        self.exited = true;
                        cx.emit(TerminalEvent::Exited);
                    }
                }
            }
        }
        cx.notify();
    }

    /// Keys the shell gets before the app's bindings (Tab, ⌃ keys, arrows…). ⌃` and ⌃⇧` stay
    /// the panel's; ⌘ keys are the app's except copy, paste and VS Code's line-editing ones.
    fn key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = &keystroke.modifiers;
        if m.control && matches!(keystroke.key.as_str(), "`" | "~") {
            return false;
        }
        if m.platform {
            if m.control || m.alt || m.shift {
                return false;
            }
            match keystroke.key.as_str() {
                "c" => {
                    let text = self.terminal.term.lock().selection_to_string();
                    match text {
                        Some(text) if !text.is_empty() => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text))
                        }
                        _ => return false,
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.scroll_to_bottom();
                        self.terminal.paste(&text);
                        cx.notify();
                    }
                }
                "left" => self.input(b"\x01".to_vec(), cx),
                "right" => self.input(b"\x05".to_vec(), cx),
                "backspace" => self.input(b"\x15".to_vec(), cx),
                _ => return false,
            }
            return true;
        }
        let mode = *self.terminal.term.lock().mode();
        match terminal::key_bytes(keystroke, mode) {
            Some(bytes) => {
                self.input(bytes, cx);
                true
            }
            None => false,
        }
    }

    fn scroll_to_bottom(&mut self) {
        let mut term = self.terminal.term.lock();
        term.selection = None;
        if term.grid().display_offset() != 0 {
            term.scroll_display(Scroll::Bottom);
        }
    }

    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.scroll_to_bottom();
        self.terminal.write(bytes);
        cx.notify();
    }

    fn grid_point(&self, position: Point<Pixels>) -> Option<(GridPoint, Side)> {
        let frame = self.frame?;
        let x = (position.x - frame.origin.x) / frame.cell.width;
        let y = (position.y - frame.origin.y) / frame.cell.height;
        let column = (x.max(0.) as usize).min(frame.size.columns - 1);
        let row = (y.max(0.) as usize).min(frame.size.lines - 1);
        let side = if x.fract() > 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        let line = Line(row as i32 - frame.display_offset as i32);
        Some((GridPoint::new(line, Column(column)), side))
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some((point, side)) = self.grid_point(event.position) else {
            return;
        };
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.terminal.term.lock().selection = Some(Selection::new(ty, point, side));
        self.selecting = true;
        cx.notify();
    }

    fn mouse_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((point, side)) = self.grid_point(position) else {
            return;
        };
        if let Some(selection) = self.terminal.term.lock().selection.as_mut() {
            selection.update(point, side);
        }
        cx.notify();
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut term = self.terminal.term.lock();
        if term.selection.as_ref().is_some_and(Selection::is_empty) {
            term.selection = None;
        }
        cx.notify();
    }

    /// Scrolls the history; full-screen programs (less, vim) get arrow keys instead, as in
    /// xterm's alternate scroll mode.
    fn scroll(&mut self, delta: ScrollDelta, cx: &mut Context<Self>) {
        let Some(frame) = self.frame else {
            return;
        };
        self.scroll_rest += delta.pixel_delta(frame.cell.height).y;
        let lines = (self.scroll_rest / frame.cell.height) as i32;
        if lines == 0 {
            return;
        }
        self.scroll_rest -= frame.cell.height * lines as f32;
        let mode = *self.terminal.term.lock().mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.terminal
                .write(key.repeat(lines.unsigned_abs() as usize));
        } else {
            self.terminal
                .term
                .lock()
                .scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }
}

fn shell_name() -> String {
    std::env::var("SHELL")
        .ok()
        .and_then(|shell| shell.rsplit('/').next().map(str::to_string))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "zsh".to_string())
}

/// What one frame paints, computed in prepaint while the grid is locked.
struct Painted {
    hitbox: Hitbox,
    background: Hsla,
    line_height: Pixels,
    rects: Vec<(Bounds<Pixels>, Hsla)>,
    texts: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<(Bounds<Pixels>, Hsla, bool)>,
    /// The character under a block cursor (redrawn in the background color) or the IME text.
    over_cursor: Option<(Point<Pixels>, ShapedLine)>,
}

#[derive(Clone, PartialEq)]
struct RunStyle {
    color: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

struct Batch {
    row: usize,
    column: usize,
    end: usize,
    text: String,
    style: RunStyle,
    wide: bool,
}

fn shape(
    text: String,
    style: &RunStyle,
    font: &Font,
    font_size: Pixels,
    cell_width: Option<Pixels>,
    window: &mut Window,
) -> ShapedLine {
    let mut font = font.clone();
    if style.bold {
        font = font.bold();
    }
    if style.italic {
        font = font.italic();
    }
    let line = |on: bool| {
        on.then_some(UnderlineStyle {
            thickness: theme::STRIKE,
            color: Some(style.color),
            wavy: false,
        })
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: style.color,
        background_color: None,
        underline: line(style.underline),
        strikethrough: style.strike.then_some(StrikethroughStyle {
            thickness: theme::STRIKE,
            color: Some(style.color),
        }),
    };
    window
        .text_system()
        .shape_line(text.into(), font_size, &[run], cell_width)
}

fn prepaint(
    view: &Entity<TerminalView>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Painted {
    let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
    let colors = theme::colors(cx);
    let palette = colors.terminal;
    let font_size = Theme::global(cx).mono_font_size;
    let font = font(Theme::global(cx).mono_font_family.clone());
    let line_height = theme::terminal_line_height(font_size);
    let font_id = window.text_system().resolve_font(&font);
    let cell_width = window
        .text_system()
        .advance(font_id, font_size, 'm')
        .map(|size| size.width)
        .ok()
        .filter(|width| *width > px(0.))
        .unwrap_or_else(|| theme::diff_metrics(font_size).column);
    let origin = bounds.origin + point(theme::TERMINAL_PAD_X, theme::TERMINAL_PAD_Y);
    let inner = bounds.size - size(theme::TERMINAL_PAD_X * 2., theme::TERMINAL_PAD_Y);
    let grid = GridSize {
        columns: ((inner.width / cell_width) as usize).max(2),
        lines: ((inner.height / line_height) as usize).max(1),
        cell_width: f32::from(cell_width),
        cell_height: f32::from(line_height),
    };
    let (term, focused, marked) = view.update(cx, |view, _| {
        view.terminal.resize(grid);
        (
            view.terminal.term.clone(),
            view.focus.is_focused(window),
            view.marked.clone(),
        )
    });
    let term = term.lock();
    let content = term.renderable_content();
    let offset = content.display_offset;
    let cursor = content.cursor;
    let selection = content.selection;
    let overrides = *content.colors;
    let cell_origin = |row: usize, column: usize| {
        origin + point(cell_width * column as f32, line_height * row as f32)
    };
    let mut rects: Vec<(Bounds<Pixels>, Hsla)> = Vec::new();
    let mut texts = Vec::new();
    let mut batch: Option<Batch> = None;
    let mut flush = |batch: &mut Option<Batch>, window: &mut Window| {
        if let Some(batch) = batch.take() {
            let width = (!batch.wide).then_some(cell_width);
            let line = shape(batch.text, &batch.style, &font, font_size, width, window);
            texts.push((cell_origin(batch.row, batch.column), line));
        }
    };
    for indexed in content.display_iter {
        let row = (indexed.point.line.0 + offset as i32) as usize;
        let column = indexed.point.column.0;
        let cell = indexed.cell;
        let flags = cell.flags;
        let mut fg = palette.resolve(cell.fg, &overrides);
        let mut bg = palette.resolve(cell.bg, &overrides);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.intersects(Flags::DIM) {
            fg = fg.opacity(0.66);
        }
        if selection.is_some_and(|s| s.contains_cell(&indexed, cursor.point, cursor.shape)) {
            bg = colors.selection;
        }
        if bg != palette.background {
            let cell_bounds = Bounds::new(cell_origin(row, column), size(cell_width, line_height));
            match rects.last_mut() {
                Some((last, color))
                    if *color == bg
                        && last.origin.y == cell_bounds.origin.y
                        && (last.right() - cell_bounds.left()).abs() < px(0.5) =>
                {
                    last.size.width += cell_width;
                }
                _ => rects.push((cell_bounds, bg)),
            }
        }
        let style = RunStyle {
            color: fg,
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            strike: flags.contains(Flags::STRIKEOUT),
        };
        let blank = cell.c == ' ' && !style.underline && !style.strike;
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            || flags.contains(Flags::HIDDEN)
            || blank
        {
            flush(&mut batch, window);
            continue;
        }
        let wide = flags.contains(Flags::WIDE_CHAR);
        let joins = batch.as_ref().is_some_and(|b| {
            !b.wide && !wide && b.row == row && b.end == column && b.style == style
        });
        if !joins {
            flush(&mut batch, window);
        }
        let current = batch.get_or_insert_with(|| Batch {
            row,
            column,
            end: column,
            text: String::new(),
            style,
            wide,
        });
        current.text.push(cell.c);
        if let Some(extra) = cell.zerowidth() {
            current.text.extend(extra);
        }
        current.end = column + 1;
    }
    flush(&mut batch, window);

    let cursor_row = cursor.point.line.0 + offset as i32;
    let mut cursor_paint = None;
    let mut over_cursor = None;
    if (0..grid.lines as i32).contains(&cursor_row) {
        let row = cursor_row as usize;
        let column = cursor.point.column.0;
        let at = cell_origin(row, column);
        let under = &term.grid()[cursor.point];
        let width = if under.flags.contains(Flags::WIDE_CHAR) {
            cell_width * 2.
        } else {
            cell_width
        };
        if let Some(text) = marked.filter(|text| !text.is_empty()) {
            let style = RunStyle {
                color: palette.foreground,
                bold: false,
                italic: false,
                underline: true,
                strike: false,
            };
            let line = shape(text, &style, &font, font_size, None, window);
            rects.push((
                Bounds::new(at, size(line.width, line_height)),
                palette.background,
            ));
            over_cursor = Some((at, line));
        } else {
            let color = palette.cursor;
            match (cursor.shape, focused) {
                (CursorShape::Hidden, _) => {}
                (CursorShape::Block, true) => {
                    cursor_paint = Some((Bounds::new(at, size(width, line_height)), color, false));
                    if under.c != ' ' {
                        let style = RunStyle {
                            color: palette.background,
                            bold: under.flags.contains(Flags::BOLD),
                            italic: under.flags.contains(Flags::ITALIC),
                            underline: false,
                            strike: false,
                        };
                        let line =
                            shape(under.c.to_string(), &style, &font, font_size, None, window);
                        over_cursor = Some((at, line));
                    }
                }
                (CursorShape::Beam, true) => {
                    let bar = size(theme::TERMINAL_CURSOR_BAR, line_height);
                    cursor_paint = Some((Bounds::new(at, bar), color, false));
                }
                (CursorShape::Underline, true) => {
                    let bar = size(width, theme::TERMINAL_CURSOR_BAR);
                    let top = at + point(px(0.), line_height - theme::TERMINAL_CURSOR_BAR);
                    cursor_paint = Some((Bounds::new(top, bar), color, false));
                }
                _ => cursor_paint = Some((Bounds::new(at, size(width, line_height)), color, true)),
            }
        }
    }
    let frame = Frame {
        origin,
        cell: size(cell_width, line_height),
        size: grid,
        display_offset: offset,
        cursor: GridPoint::new(Line(cursor_row), cursor.point.column),
    };
    drop(term);
    view.update(cx, |view, _| view.frame = Some(frame));
    Painted {
        hitbox,
        background: palette.background,
        line_height,
        rects,
        texts,
        cursor: cursor_paint,
        over_cursor,
    }
}

fn paint(
    view: Entity<TerminalView>,
    bounds: Bounds<Pixels>,
    painted: Painted,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(fill(bounds, painted.background));
    for (rect, color) in &painted.rects {
        window.paint_quad(fill(*rect, *color));
    }
    for (origin, line) in &painted.texts {
        let _ = line.paint(
            *origin,
            painted.line_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    if let Some((rect, color, hollow)) = painted.cursor {
        if hollow {
            window.paint_quad(outline(rect, color, BorderStyle::Solid));
        } else {
            window.paint_quad(fill(rect, color));
        }
    }
    if let Some((origin, line)) = &painted.over_cursor {
        let _ = line.paint(
            *origin,
            painted.line_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }
    let hitbox = painted.hitbox;
    window.set_cursor_style(CursorStyle::IBeam, &hitbox);
    let focus = view.read(cx).focus.clone();
    window.handle_input(&focus, ElementInputHandler::new(bounds, view.clone()), cx);
    window.on_mouse_event({
        let view = view.clone();
        let hitbox = hitbox.clone();
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase.bubble() && event.button == MouseButton::Left && hitbox.is_hovered(window) {
                window.focus(&focus, cx);
                view.update(cx, |view, cx| view.mouse_down(event, cx));
            }
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseMoveEvent, phase, _, cx| {
            if phase.bubble()
                && event.pressed_button == Some(MouseButton::Left)
                && view.read(cx).selecting
            {
                view.update(cx, |view, cx| view.mouse_drag(event.position, cx));
            }
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseUpEvent, phase, _, cx| {
            if phase.bubble() && event.button == MouseButton::Left && view.read(cx).selecting {
                view.update(cx, |view, cx| view.mouse_up(cx));
            }
        }
    });
    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
        if phase.bubble() && hitbox.is_hovered(window) {
            view.update(cx, |view, cx| view.scroll(event.delta, cx));
            cx.stop_propagation();
        }
    });
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let paint_view = view.clone();
        div()
            .size_full()
            .key_context("Terminal")
            .track_focus(&self.focus)
            .child(
                canvas(
                    move |bounds, window, cx| prepaint(&view, bounds, window, cx),
                    move |bounds, painted, window, cx| {
                        paint(paint_view, bounds, painted, window, cx)
                    },
                )
                .size_full(),
            )
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let marked: Vec<u16> = self.marked.as_deref()?.encode_utf16().collect();
        let range = range.start.min(marked.len())..range.end.min(marked.len());
        *adjusted_range = Some(range.clone());
        String::from_utf16(&marked[range]).ok()
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self
            .marked
            .as_deref()
            .map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_deref()
            .map(|marked| 0..marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        if !text.is_empty() {
            self.input(text.as_bytes().to_vec(), cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!text.is_empty()).then(|| text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let frame = self.frame?;
        let at = frame.origin
            + point(
                frame.cell.width * frame.cursor.column.0 as f32,
                frame.cell.height * frame.cursor.line.0.max(0) as f32,
            );
        Some(Bounds::new(at, frame.cell))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
