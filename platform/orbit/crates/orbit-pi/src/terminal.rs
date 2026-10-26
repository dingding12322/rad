//! Integrated terminal — a real shell in a bottom panel.
//!
//! The emulator is [`alacritty_terminal`] (the same Apache-2.0 crate Zed
//! builds its terminal on), not a bundled terminal app: `tty` opens the
//! PTY, `Term` owns the VT grid and the ANSI parser, and `EventLoop` drives
//! PTY I/O on its own thread. Orbit owns the shell selection, the GPUI
//! rendering, and the key/mouse translation.
//!
//! Rendering is a [`canvas`](gpui::canvas) rather than a text element: the
//! grid is shaped one row at a time with an explicit cell width, so a glyph
//! can never reflow a row, and per-cell colors travel as `TextRun`s.
//!
//! Direction contract (impeccable):
//! THESIS: the agent's shell is one keystroke away and never leaves the
//! window — output, scrollback, and selection land in the same surface as the
//! transcript instead of a separate app.
//! OWN-WORLD: the terminal borrows Orbit's code surface (`code_bg`, the
//! monospace face, hairline borders) and the theme's semantic colors for the
//! ANSI palette, so it reads as part of the workbench on every theme.
//! STORY: the operator runs a build, watches it stream, selects the failing
//! line, and copies it into the composer — without switching apps.
//! FIRST VIEWPORT: a resizable bottom panel, header first (title, cwd,
//! restart/close), then the grid, focus already in the shell.
//! FORM: a bottom workbench panel — a right-hand terminal rotated to the
//! bottom edge — with a session/view split.

use std::borrow::Cow;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TermPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty::{self, Shell};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb};
use anyhow::{Context as _, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use gpui::{
    canvas, div, fill, point, prelude::*, px, size, AnyElement, App, Background, Bounds,
    ClipboardItem, Context, CursorStyle, Entity, FocusHandle, Focusable, Font, FontFallbacks,
    FontFeatures, FontStyle, FontWeight, Hsla, IntoElement, Keystroke, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, Rgba, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, StrikethroughStyle, Styled, Task, TextRun, UnderlineStyle,
    Window,
};

use crate::app::{
    button_frame, icon, icon_button_frame, nerd_font_family, press, refresh_glyph, TipExt,
    BUTTON_GROUP,
};
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{ButtonSize, DynamicSpacing, IconSize, Radius, TextSize};
use crate::theme::{self, Theme};

/// Emulator grid bounds, clamped so a collapsing panel never asks the PTY for
/// a zero-sized window.
const MIN_COLUMNS: usize = 2;
const MIN_ROWS: usize = 1;
/// History kept above the visible grid.
const SCROLLBACK_LINES: usize = 10_000;
/// Fallback advance until the terminal font has been measured.
const CELL_WIDTH_FALLBACK: f32 = 7.8;
/// Line box as a multiple of the font size; keeps glyphs centered per cell.
const LINE_HEIGHT_RATIO: f32 = 1.4;
/// PTY drain cadence — one frame at 60 Hz.
const POLL_MS: u64 = 16;
/// Cursor blink half-period, in poll ticks (500 ms at 16 ms).
const BLINK_TICKS: u32 = 31;
/// Panel height defaults and drag clamps.
const PANEL_DEFAULT_H: f32 = 260.;
const PANEL_MIN_H: f32 = 96.;
/// Header height — the tab strip.
const HEADER_H: f32 = 32.;
/// The resize handle above the header, where a panel's drag affordance
/// belongs: the tab strip below it stays free for tabs.
const SPLITTER_H: f32 = 6.;
/// One tab chip's widest size: narrow enough that a handful fit without
/// scrolling, wide enough for a readable truncated title.
const TAB_MAX_W: f32 = 190.;

/// How long the restart button spins after a click, so restarting a shell
/// that comes back quickly still reads as acknowledged.
const RESTART_FEEDBACK: Duration = Duration::from_millis(650);

// ── palette ────────────────────────────────────────────────────────────────

/// The 16 ANSI slots plus the three terminal defaults, resolved from the
/// active theme. Phase 1 derives every slot from an existing semantic role, so
/// the terminal recolors with the workbench and needs no second palette file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Palette {
    ansi: [Hsla; 16],
    foreground: Hsla,
    background: Hsla,
    cursor: Hsla,
}

impl Palette {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        let foreground = theme.code_text;
        let background = theme.code_bg;
        Self {
            ansi: [
                theme.text_muted,
                theme.error,
                theme.success,
                theme.syn_number,
                theme.syn_function,
                theme.syn_string,
                theme.syn_type,
                theme.text_primary,
                theme.text_muted,
                theme.error,
                theme.success,
                theme.syn_literal,
                theme.syn_function,
                theme.syn_string,
                theme.syn_type,
                theme.text_primary,
            ],
            foreground,
            background,
            cursor: theme.accent_text,
        }
    }

    /// The RGB an OSC 4/10/11/12 query must answer with. `index` is a
    /// `NamedColor` discriminant, so 0–15 are the ANSI slots and 256/257/258
    /// are foreground/background/cursor.
    fn rgb_for_index(&self, index: usize) -> Rgb {
        let hsla = match index {
            0..=15 => self.ansi[index],
            256 => self.foreground,
            257 => self.background,
            258 => self.cursor,
            _ => self.indexed(index),
        };
        hsla_to_rgb(hsla)
    }

    /// xterm's 256-color table: 16 system slots, a 6×6×6 cube, then greys.
    pub(crate) fn indexed(&self, index: usize) -> Hsla {
        match index {
            0..=15 => self.ansi[index],
            16..=231 => {
                let i = index - 16;
                let level = |v: usize| -> u8 {
                    if v == 0 {
                        0
                    } else {
                        (55 + v * 40) as u8
                    }
                };
                rgb_to_hsla(Rgb {
                    r: level((i / 36) % 6),
                    g: level((i / 6) % 6),
                    b: level(i % 6),
                })
            }
            232..=255 => {
                let v = (8 + (index - 232) * 10) as u8;
                rgb_to_hsla(Rgb { r: v, g: v, b: v })
            }
            _ => self.foreground,
        }
    }
}

pub(crate) fn rgb_to_hsla(rgb: Rgb) -> Hsla {
    Hsla::from(Rgba {
        r: rgb.r as f32 / 255.,
        g: rgb.g as f32 / 255.,
        b: rgb.b as f32 / 255.,
        a: 1.,
    })
}

fn hsla_to_rgb(color: Hsla) -> Rgb {
    let rgba = Rgba::from(color);
    Rgb {
        r: (rgba.r.clamp(0., 1.) * 255.).round() as u8,
        g: (rgba.g.clamp(0., 1.) * 255.).round() as u8,
        b: (rgba.b.clamp(0., 1.) * 255.).round() as u8,
    }
}

/// A cell's foreground/background and decoration, in the shape the run
/// splitter compares.
#[derive(Debug, Clone, PartialEq)]
struct CellStyle {
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

/// Resolve a parsed ANSI color against the theme palette and the terminal's
/// own OSC-overridden defaults.
fn resolve_color(color: AnsiColor, palette: &Palette, fg_default: Hsla) -> Hsla {
    match color {
        AnsiColor::Spec(rgb) => rgb_to_hsla(rgb),
        AnsiColor::Indexed(index) => palette.indexed(index as usize),
        AnsiColor::Named(named) => match named {
            NamedColor::Foreground => fg_default,
            NamedColor::Background => palette.background,
            NamedColor::Cursor => palette.cursor,
            NamedColor::DimBlack => palette.ansi[0],
            NamedColor::DimRed => palette.ansi[1],
            NamedColor::DimGreen => palette.ansi[2],
            NamedColor::DimYellow => palette.ansi[3],
            NamedColor::DimBlue => palette.ansi[4],
            NamedColor::DimMagenta => palette.ansi[5],
            NamedColor::DimCyan => palette.ansi[6],
            NamedColor::DimWhite => palette.ansi[7],
            other => {
                let index = other as usize;
                if index < 16 {
                    palette.ansi[index]
                } else {
                    fg_default
                }
            }
        },
    }
}

// ── PTY events ─────────────────────────────────────────────────────────────

/// Terminal-initiated actions that must run on the UI thread.
enum UiEvent {
    Title(String),
    ResetTitle,
    ClipboardStore(String),
    ClipboardLoad(Arc<dyn Fn(&str) -> String + Send + Sync>),
    Exited,
}

/// The bridge from alacritty's PTY thread back into GPUI. It never touches
/// the UI directly: everything becomes a channel message or a dirty flag.
#[derive(Clone)]
struct EventProxy {
    dirty: Arc<AtomicBool>,
    sender: Arc<OnceLock<EventLoopSender>>,
    ui: Sender<UiEvent>,
    window_size: Arc<Mutex<WindowSize>>,
    palette: Arc<Mutex<Palette>>,
}

impl EventProxy {
    fn write_pty(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if let Some(sender) = self.sender.get() {
            let _ = sender.send(Msg::Input(bytes.into()));
        }
    }

    /// A poisoned lock only means another thread panicked mid-write; the
    /// window size and palette are plain values, so recovering is safe.
    fn lock_window_size(&self) -> std::sync::MutexGuard<'_, WindowSize> {
        self.window_size.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_palette(&self) -> std::sync::MutexGuard<'_, Palette> {
        self.palette.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {
                self.dirty.store(true, Ordering::Release);
            }
            Event::Title(title) => {
                let _ = self.ui.send(UiEvent::Title(title));
            }
            Event::ResetTitle => {
                let _ = self.ui.send(UiEvent::ResetTitle);
            }
            Event::ClipboardStore(_, text) => {
                let _ = self.ui.send(UiEvent::ClipboardStore(text));
            }
            Event::ClipboardLoad(_, formatter) => {
                let _ = self.ui.send(UiEvent::ClipboardLoad(formatter));
            }
            Event::PtyWrite(text) => self.write_pty(text.into_bytes()),
            Event::ColorRequest(index, formatter) => {
                let rgb = self.lock_palette().rgb_for_index(index);
                self.write_pty(formatter(rgb).into_bytes());
            }
            Event::TextAreaSizeRequest(formatter) => {
                let size = *self.lock_window_size();
                self.write_pty(formatter(size).into_bytes());
            }
            Event::Exit | Event::ChildExit(_) => {
                let _ = self.ui.send(UiEvent::Exited);
                self.dirty.store(true, Ordering::Release);
            }
            Event::Bell => {}
        }
    }
}

// ── session ────────────────────────────────────────────────────────────────

/// Grid shape handed to alacritty's `Dimensions`.
struct GridDims {
    columns: usize,
    rows: usize,
}

impl Dimensions for GridDims {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// One shell: the PTY, the emulator grid, and the channel that wakes the UI.
///
/// The PTY is owned by alacritty's `EventLoop`, which runs on its own thread;
/// dropping the session asks it to shut down, and that thread then drops the
/// `Pty`, which signals the child and reaps it.
struct TerminalSession {
    term: Arc<FairMutex<Term<EventProxy>>>,
    sender: EventLoopSender,
    dirty: Arc<AtomicBool>,
    ui: Receiver<UiEvent>,
    /// Shared with the render closure, which resizes the PTY to the panel.
    window_size: Arc<Mutex<WindowSize>>,
    grid_size: Arc<Mutex<(usize, usize)>>,
    /// Shared with the render closure so OSC color replies match the theme.
    palette: Arc<Mutex<Palette>>,
}

impl TerminalSession {
    fn with_shell(
        working_directory: &std::path::Path,
        columns: usize,
        rows: usize,
        palette: Palette,
        shell: &std::path::Path,
        args: Vec<String>,
    ) -> Result<Self> {
        let columns = columns.max(MIN_COLUMNS);
        let rows = rows.max(MIN_ROWS);
        let window_size = WindowSize {
            num_lines: rows.min(u16::MAX as usize) as u16,
            num_cols: columns.min(u16::MAX as usize) as u16,
            cell_width: CELL_WIDTH_FALLBACK.round() as u16,
            cell_height: (13. * LINE_HEIGHT_RATIO).round() as u16,
        };
        let window_size = Arc::new(Mutex::new(window_size));
        let grid_size = Arc::new(Mutex::new((columns, rows)));
        let palette = Arc::new(Mutex::new(palette));
        let dirty = Arc::new(AtomicBool::new(true));
        let sender_slot = Arc::new(OnceLock::new());
        let (ui_tx, ui) = unbounded();

        let proxy = EventProxy {
            dirty: dirty.clone(),
            sender: sender_slot.clone(),
            ui: ui_tx,
            window_size: window_size.clone(),
            palette: palette.clone(),
        };

        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Default::default()
        };
        let dimensions = GridDims { columns, rows };
        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &dimensions,
            proxy.clone(),
        )));

        let mut options = tty::Options {
            shell: Some(Shell::new(shell.to_string_lossy().into_owned(), args)),
            working_directory: Some(working_directory.to_path_buf()),
            drain_on_exit: false,
            ..Default::default()
        };
        options.env.insert("TERM".into(), "xterm-256color".into());
        options.env.insert("COLORTERM".into(), "truecolor".into());

        let pty = tty::new(
            &options,
            *window_size.lock().unwrap_or_else(|e| e.into_inner()),
            0,
        )
        .with_context(|| {
            tr!(
                "terminal.spawn_shell_in",
                dir = working_directory.display().to_string()
            )
        })?;
        let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)
            .context("create terminal event loop")?;
        let sender = event_loop.channel();
        // The proxy needs the sender to answer OSC/DA queries; only the very
        // first query can race this, and it is answered again on the next one.
        let _ = sender_slot.set(sender.clone());
        event_loop.spawn();

        Ok(Self {
            term,
            sender,
            dirty,
            ui,
            window_size,
            grid_size,
            palette,
        })
    }

    fn write(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.sender.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Stops the event-loop thread, which then drops the `Pty` — SIGHUP
        // plus a wait for the child.
        let _ = self.sender.send(Msg::Shutdown);
    }
}

/// The login shell a tab runs, as its tab chip shows it: a profile mark at the
/// left of the label.
///
/// Orbit spawns one shell — the user's login shell — so every tab carries the
/// same mark today. It is still the honest label for the tab (and the seam a
/// per-tab shell picker would slot into), and it tells a `fish` tab from a
/// `zsh` one at a glance once more than one shell can be launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellProfile {
    Zsh,
    Bash,
    Fish,
    /// `sh`, `dash`, `ksh`, `cmd`, `pwsh` — anything without a mark in
    /// Simple Icons, which withdraws trademarked logos (PowerShell, Windows).
    Other,
}

impl ShellProfile {
    /// Classify the shell by its executable name. `-l` and a `/usr/local/bin`
    /// prefix are irrelevant here, so only the file stem is compared.
    fn of(shell: &std::path::Path) -> Self {
        match shell.file_stem().and_then(|name| name.to_str()) {
            Some(name) if name.eq_ignore_ascii_case("zsh") => Self::Zsh,
            Some(name) if name.eq_ignore_ascii_case("bash") => Self::Bash,
            Some(name) if name.eq_ignore_ascii_case("fish") => Self::Fish,
            _ => Self::Other,
        }
    }

    /// The asset that draws this profile, in the same `currentColor` style as
    /// the rest of the icon set.
    fn icon(self) -> &'static str {
        match self {
            Self::Zsh => "icons/zsh.svg",
            Self::Bash => "icons/bash.svg",
            Self::Fish => "icons/fish.svg",
            Self::Other => "icons/terminal.svg",
        }
    }

    /// The shell's name, for the tab's tooltip. Lowercase and short: it sits
    /// beside a path, not on a page of its own.
    fn name(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Fish => "fish",
            Self::Other => "shell",
        }
    }
}

/// The user's login shell, opened the way their own terminal opens it.
///
/// `$SHELL` is set for a normal macOS login session; the absolute fallbacks
/// cover a stripped environment. A POSIX shell gets `-l` so the login files
/// that build `PATH` are read.
fn default_shell() -> (PathBuf, Vec<String>) {
    #[cfg(windows)]
    {
        let shell = std::env::var_os("COMSPEC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cmd.exe"));
        (shell, Vec::new())
    }
    #[cfg(not(windows))]
    {
        let mut candidates: Vec<PathBuf> = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .map(PathBuf::from)
            .into_iter()
            .collect();
        candidates.extend([
            PathBuf::from("/bin/zsh"),
            PathBuf::from("/bin/bash"),
            PathBuf::from("/bin/sh"),
        ]);
        let shell = candidates
            .into_iter()
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        (shell, vec!["-l".to_owned()])
    }
}

// ── grid snapshot ──────────────────────────────────────────────────────────

/// How the cursor draws. A solid block is painted by swapping the cell's own
/// colors; everything else needs a rect drawn on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorPaint {
    /// The run's colors are already swapped — nothing to draw over it.
    Solid,
    /// The unfocused block cursor: an outline, so a terminal that does not own
    /// focus still shows where the shell would type.
    Hollow,
    Beam,
    Underline,
}

#[derive(Debug, Clone, Copy)]
struct CursorCell {
    paint: CursorPaint,
    row: usize,
    column: usize,
}

/// Viewport position inside the scrollback, as fractions of the buffer.
#[derive(Debug, Clone, Copy)]
struct ScrollIndicator {
    /// Thumb top, already in `0..=1 - visible`.
    top: f32,
    visible: f32,
}

/// A row built while holding the terminal lock, shaped after it is released so
/// the PTY parser is never blocked behind text shaping.
struct PreparedRow {
    text: String,
    runs: Vec<TextRun>,
}

struct PreparedGrid {
    rows: Vec<PreparedRow>,
    cursor: Option<CursorCell>,
    cursor_color: Hsla,
    /// `None` until there is history to scroll through.
    scroll: Option<ScrollIndicator>,
}

/// Read the visible grid into styled rows. The caller holds the term lock for
/// exactly this call.
///
/// `focused` and `blinking` drive the cursor: a focused grid blinks a solid
/// cursor, an unfocused one draws a hollow block (the convention every
/// terminal shares, and the cue that keystrokes are going elsewhere).
fn prepare_grid(
    term: &Term<EventProxy>,
    palette: &Palette,
    theme: &Theme,
    font: &Font,
    focused: bool,
    blinking: bool,
) -> PreparedGrid {
    let content = term.renderable_content();
    let columns = term.grid().columns();
    let rows_count = term.grid().screen_lines();
    let display_offset = content.display_offset as i32;
    let selection = content.selection;
    let cursor = content.cursor;
    // One decision drives both the run colors and the drawn rect: a focused
    // grid blinks a solid cursor, an unfocused one keeps a hollow outline, and
    // the blink's off phase draws nothing at all.
    let cursor_row = cursor.point.line.0 + display_offset;
    let cursor_paint = if cursor_row >= 0 && (cursor_row as usize) < rows_count {
        match (cursor.shape, focused, blinking) {
            (CursorShape::Hidden, _, _) => None,
            (CursorShape::Block | CursorShape::HollowBlock, true, true) => Some(CursorPaint::Solid),
            (CursorShape::Block | CursorShape::HollowBlock, false, _) => Some(CursorPaint::Hollow),
            (CursorShape::Beam, true, true) => Some(CursorPaint::Beam),
            (CursorShape::Underline, true, true) => Some(CursorPaint::Underline),
            _ => None,
        }
    } else {
        None
    };
    let solid_cursor = cursor_paint == Some(CursorPaint::Solid);

    let default_fg = content.colors[NamedColor::Foreground]
        .map(rgb_to_hsla)
        .unwrap_or(palette.foreground);
    let default_bg = content.colors[NamedColor::Background]
        .map(rgb_to_hsla)
        .unwrap_or(palette.background);

    let all: Vec<_> = content.display_iter.collect();
    let mut rows = Vec::with_capacity(rows_count);

    for chunk in all.chunks(columns.max(1)) {
        let mut text = String::with_capacity(columns);
        let mut runs: Vec<TextRun> = Vec::with_capacity(columns);
        let mut run_style: Option<CellStyle> = None;
        let mut run_len = 0usize;

        for indexed in chunk {
            let cell = indexed.cell;
            let cell_point = indexed.point;
            let flags = cell.flags;

            // The spacer belongs to the wide glyph before it, which already
            // occupies both columns.
            if flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut fg = resolve_color(cell.fg, palette, default_fg);
            let mut bg = resolve_color(cell.bg, palette, default_bg);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::DIM) {
                fg = fg.opacity(0.7);
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }

            let mut background = (bg != default_bg).then_some(bg);
            if selection
                .as_ref()
                .is_some_and(|range| cell_selected(range, cell_point))
            {
                background = Some(theme.selection);
            }

            if solid_cursor && cell_point == cursor.point {
                background = Some(palette.cursor);
                fg = default_bg;
            }

            let style = CellStyle {
                fg,
                bg: background,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikethrough: flags.contains(Flags::STRIKEOUT),
            };

            if run_style.as_ref() != Some(&style) {
                if let Some(previous) = run_style.take() {
                    runs.push(text_run(run_len, &previous, font));
                    run_len = 0;
                }
                run_style = Some(style);
            }

            let character = if cell.c == '\0' { ' ' } else { cell.c };
            run_len += character.len_utf8();
            text.push(character);
            if let Some(zerowidth) = cell.zerowidth() {
                for mark in zerowidth {
                    run_len += mark.len_utf8();
                    text.push(*mark);
                }
            }
        }

        if let Some(previous) = run_style.take() {
            runs.push(text_run(run_len, &previous, font));
        }
        // A row of nothing but wide-char spacers would shape an empty line;
        // give it one space so paint always has a glyph box.
        if text.is_empty() {
            text.push(' ');
            runs.push(text_run(
                1,
                &CellStyle {
                    fg: default_fg,
                    bg: None,
                    bold: false,
                    italic: false,
                    underline: false,
                    strikethrough: false,
                },
                font,
            ));
        }

        rows.push(PreparedRow { text, runs });
    }

    let cursor_cell = cursor_paint.map(|paint| CursorCell {
        paint,
        row: cursor_row as usize,
        column: cursor.point.column.0,
    });

    // The scrollbar thumb: how much of the buffer is on screen and where.
    let history = term.grid().history_size();
    let scroll = (history > 0).then(|| {
        let total = (history + rows_count) as f32;
        let visible = (rows_count as f32 / total).clamp(0.05, 1.);
        let top = (((history - content.display_offset) as f32) / total).clamp(0., 1. - visible);
        ScrollIndicator { top, visible }
    });

    PreparedGrid {
        rows,
        cursor: cursor_cell,
        cursor_color: palette.cursor,
        scroll,
    }
}

fn text_run(len: usize, style: &CellStyle, font: &Font) -> TextRun {
    TextRun {
        len,
        font: Font {
            weight: if style.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            ..font.clone()
        },
        color: style.fg,
        background_color: style.bg,
        underline: style.underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: None,
            wavy: false,
        }),
        strikethrough: style.strikethrough.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: None,
        }),
    }
}

/// Whether a cell falls inside an alacritty selection range. Points are
/// absolute grid coordinates on both sides, so they compare directly.
fn cell_selected(range: &SelectionRange, cell: TermPoint) -> bool {
    if range.is_block {
        cell.line >= range.start.line
            && cell.line <= range.end.line
            && cell.column >= range.start.column
            && cell.column <= range.end.column
    } else {
        cell >= range.start && cell <= range.end
    }
}

// ── key encoding ───────────────────────────────────────────────────────────

/// Control characters for the C0-mapped keys xterm defines.
fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match first {
        'a'..='z' => (first as u8) - b'a' + 1,
        'A'..='Z' => (first as u8) - b'A' + 1,
        ' ' | '@' => 0,
        '[' => 27,
        '\\' => 28,
        ']' => 29,
        '^' => 30,
        '_' => 31,
        '?' => 127,
        _ => return None,
    })
}

fn cursor_bytes(final_byte: u8, application: bool) -> Vec<u8> {
    if application {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

/// Translate a GPUI keystroke into the bytes a terminal expects, or `None`
/// when the key belongs to the app (every `cmd`/`super` combination).
pub(crate) fn encode_key(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let modifiers = &keystroke.modifiers;
    // `cmd`/`super` is app territory; the terminal never sees it.
    if modifiers.platform {
        return None;
    }
    let application = mode.contains(TermMode::APP_CURSOR);
    let key = keystroke.key.as_str();

    let special: Option<Vec<u8>> = match key {
        "enter" => Some(vec![b'\r']),
        "backspace" => Some(vec![0x7f]),
        "tab" if modifiers.shift => Some(b"\x1b[Z".to_vec()),
        "tab" => Some(vec![b'\t']),
        "escape" => Some(vec![0x1b]),
        "up" => Some(cursor_bytes(b'A', application)),
        "down" => Some(cursor_bytes(b'B', application)),
        "right" => Some(cursor_bytes(b'C', application)),
        "left" => Some(cursor_bytes(b'D', application)),
        "home" => Some(if application { b"\x1bOH" } else { b"\x1b[H" }.to_vec()),
        "end" => Some(if application { b"\x1bOF" } else { b"\x1b[F" }.to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "insert" => Some(b"\x1b[2~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "f1" => Some(b"\x1bOP".to_vec()),
        "f2" => Some(b"\x1bOQ".to_vec()),
        "f3" => Some(b"\x1bOR".to_vec()),
        "f4" => Some(b"\x1bOS".to_vec()),
        "f5" => Some(b"\x1b[15~".to_vec()),
        "f6" => Some(b"\x1b[17~".to_vec()),
        "f7" => Some(b"\x1b[18~".to_vec()),
        "f8" => Some(b"\x1b[19~".to_vec()),
        "f9" => Some(b"\x1b[20~".to_vec()),
        "f10" => Some(b"\x1b[21~".to_vec()),
        "f11" => Some(b"\x1b[23~".to_vec()),
        "f12" => Some(b"\x1b[24~".to_vec()),
        _ => None,
    };
    if let Some(mut bytes) = special {
        if modifiers.alt {
            bytes.insert(0, 0x1b);
        }
        return Some(bytes);
    }

    if modifiers.control {
        let byte = control_byte(key)?;
        let mut bytes = Vec::with_capacity(2);
        if modifiers.alt {
            bytes.push(0x1b);
        }
        bytes.push(byte);
        return Some(bytes);
    }

    // Text: `key_char` is what the layout would have typed (macOS option-s is
    // "ß"); with alt held the terminal wants meta, so send the base key.
    let text = if modifiers.alt {
        key
    } else {
        keystroke.key_char.as_deref().unwrap_or(key)
    };
    if text.is_empty() {
        return None;
    }
    // A multi-character name with no typed character is a named key we do not
    // map (e.g. a modifier key arriving as a key-down), never text.
    if keystroke.key_char.is_none() && text.chars().count() > 1 {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() + 1);
    if modifiers.alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

// ── view ───────────────────────────────────────────────────────────────────

/// The terminal surface: one shell plus everything needed to draw and drive
/// it. Owned by [`TerminalPanel`], which keeps it alive while the panel is
/// closed so the shell's state survives a toggle.
pub struct TerminalView {
    session: Option<TerminalSession>,
    error: Option<String>,
    focus_handle: FocusHandle,
    title: Option<String>,
    /// The directory this shell was started in. The tab strip labels the tab
    /// from it, a tooltip shows it, and a restart lands in the same place
    /// rather than wherever the process happens to have `cd`-ed to.
    directory: PathBuf,
    /// Which shell backs this tab: the tab chip's profile mark, and the
    /// tooltip's first word.
    profile: ShellProfile,
    /// Whether the panel is on screen; a hidden terminal still drains the PTY
    /// but does not ask for frames.
    active: bool,
    /// Whether the grid owns keyboard focus, mirrored from render so the blink
    /// timer knows whether the cursor is worth animating.
    focused: bool,
    exited: bool,
    font_size: Pixels,
    cell_width: Pixels,
    cell_height: Pixels,
    cursor_on: bool,
    blink_ticks: u32,
    selecting: bool,
    scroll_accumulator: f32,
    grid_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _task: Task<()>,
}

impl TerminalView {
    pub fn new(working_directory: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let directory = working_directory.clone();
        // Captured on the UI thread: the emulator's first OSC color reply and
        // its initial background must match the theme already on screen.
        let initial_palette = Palette::from_theme(theme::get(cx));
        // Resolved once, here: the tab's mark and the shell the PTY actually
        // runs come from the same answer, so they cannot disagree.
        let (shell, args) = default_shell();
        let profile = ShellProfile::of(&shell);

        // Build the session off the UI thread, then poll it: alacritty's event
        // loop owns PTY I/O, so polling is a dirty-flag check plus a channel
        // drain. The loop ends when this entity is dropped (`update` errors).
        let task = cx.spawn(async move |this, cx| {
            let started = cx
                .background_executor()
                .spawn(async move {
                    TerminalSession::with_shell(&directory, 80, 24, initial_palette, &shell, args)
                })
                .await;
            let updated = this.update(cx, |this, cx| {
                match started {
                    Ok(session) => {
                        this.session = Some(session);
                        this.error = None;
                    }
                    Err(error) => this.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
            if updated.is_err() {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(POLL_MS))
                    .await;
                if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                    break;
                }
            }
        });

        let mut view = Self {
            session: None,
            error: None,
            focus_handle,
            title: None,
            directory: working_directory,
            profile,
            active: false,
            focused: false,
            exited: false,
            font_size: px(13.),
            cell_width: px(CELL_WIDTH_FALLBACK),
            cell_height: px(13. * LINE_HEIGHT_RATIO),
            cursor_on: true,
            blink_ticks: 0,
            selecting: false,
            scroll_accumulator: 0.0,
            grid_bounds: Rc::new(Cell::new(None)),
            _task: task,
        };
        view.measure_font(cx);
        view
    }

    /// Resolve the face's advance once, from the app's text system, so the
    /// grid math matches the cell box gpui actually shapes with.
    fn measure_font(&mut self, cx: &App) {
        let font_size = theme::get(cx).term_px(13.0);
        let font = terminal_font(cx);
        let font_id = cx.text_system().resolve_font(&font);
        if let Ok(advance) = cx.text_system().advance(font_id, font_size, 'M') {
            if advance.width > px(0.) {
                self.font_size = font_size;
                self.cell_width = advance.width;
                self.cell_height = px(f32::from(font_size) * LINE_HEIGHT_RATIO);
            }
        }
    }

    pub fn handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn directory(&self) -> &std::path::Path {
        &self.directory
    }

    /// The shell profile mark the tab chip draws before the label.
    pub fn profile_icon(&self) -> &'static str {
        self.profile.icon()
    }

    /// `zsh — ~/Personal/orbit`: what the tab runs, where it started.
    pub fn tooltip(&self) -> String {
        format!(
            "{} — {}",
            self.profile.name(),
            abbreviate_home(&self.directory)
        )
    }

    /// The tab's label: the shell's OSC title when it sets one (the running
    /// command, the way every terminal names its tabs), else the folder the
    /// shell runs in. Never empty, so a tab always reads as something.
    pub fn label(&self) -> SharedString {
        if let Some(title) = self.title.as_deref().filter(|title| !title.is_empty()) {
            return SharedString::from(title.to_owned());
        }
        self.folder_label()
    }

    /// The last path component of [`Self::directory`], which is what a tab
    /// shows before its shell sets a title.
    fn folder_label(&self) -> SharedString {
        match self.directory.file_name().and_then(|name| name.to_str()) {
            Some(name) if !name.is_empty() => SharedString::from(name.to_owned()),
            _ => SharedString::from("Terminal"),
        }
    }

    pub fn is_exited(&self) -> bool {
        self.exited
    }

    /// Tell the view whether it is on screen. A hidden terminal keeps draining
    /// the PTY but stops requesting frames. Deliberately silent: callers run
    /// this from the app's own render, which is already drawing the panel.
    pub fn set_active(&mut self, active: bool, _cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        self.cursor_on = true;
    }

    /// Drain PTY/UI events and drive the cursor blink. Returns whether the
    /// caller should repaint.
    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;

        if let Some(session) = &self.session {
            while let Ok(event) = session.ui.try_recv() {
                changed = true;
                match event {
                    UiEvent::Title(title) => self.title = Some(title),
                    UiEvent::ResetTitle => self.title = None,
                    UiEvent::ClipboardStore(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    UiEvent::ClipboardLoad(formatter) => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            session.write(formatter(&text).into_bytes());
                        }
                    }
                    UiEvent::Exited => self.exited = true,
                }
            }
            if session.dirty.swap(false, Ordering::AcqRel) {
                changed = true;
            }
        }

        if self.active && self.focused && self.session.is_some() {
            self.blink_ticks += 1;
            if self.blink_ticks >= BLINK_TICKS {
                self.blink_ticks = 0;
                self.cursor_on = !self.cursor_on;
                changed = true;
            }
        }

        if changed && self.active {
            cx.notify();
        }
        changed
    }

    /// Grid coordinate under a window-space point, clamped to the viewport so
    /// a drag past the edge keeps extending the selection.
    fn grid_point(&self, position: Point<Pixels>) -> Option<TermPoint> {
        let bounds = self.grid_bounds.get()?;
        let session = self.session.as_ref()?;
        let x = f32::from(position.x - bounds.origin.x);
        let y = f32::from(position.y - bounds.origin.y);
        let column = (x / f32::from(self.cell_width)).floor().max(0.0) as usize;
        let row = (y / f32::from(self.cell_height)).floor().max(0.0) as usize;

        let term = session.term.lock();
        let columns = term.grid().columns();
        let screen_lines = term.grid().screen_lines();
        let display_offset = term.grid().display_offset() as i32;
        let column = column.min(columns.saturating_sub(1));
        let row = row.min(screen_lines.saturating_sub(1));
        // Visible row 0 is the top of the display, which sits `display_offset`
        // lines above the live grid bottom.
        Some(TermPoint::new(
            Line(row as i32 - display_offset),
            Column(column),
        ))
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        if let Some(text) = session.selection_text().filter(|text| !text.is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(session) = &self.session else {
            return;
        };
        let bytes = if session.mode().contains(TermMode::BRACKETED_PASTE) {
            let mut bytes = Vec::with_capacity(text.len() + 12);
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            bytes
        } else {
            text.into_bytes()
        };
        session.write(bytes);
        self.scroll_to_bottom();
    }

    /// Typing returns the view to the live bottom, the way every terminal
    /// does — otherwise a shell that keeps printing looks frozen while the
    /// user is scrolled back.
    fn scroll_to_bottom(&self) {
        let Some(session) = &self.session else {
            return;
        };
        let mut term = session.term.lock();
        if term.grid().display_offset() > 0 {
            term.scroll_display(Scroll::Bottom);
            session.dirty.store(true, Ordering::Release);
        }
    }

    /// Escape is claimed by the global `AbortRun` binding, so the `Terminal`
    /// key context re-binds it here and this forwards the byte the shell
    /// expects. Without it, vim/less/`read` could never be escaped.
    fn on_terminal_escape(
        &mut self,
        _: &crate::TerminalEscape,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if let Some(session) = &self.session {
            session.write(vec![0x1b]);
        }
        self.scroll_to_bottom();
    }

    fn on_key_down(&mut self, event: &gpui::KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        // macOS copies with cmd, other platforms follow the terminal
        // convention of ctrl+shift.
        if modifiers.platform || (modifiers.control && modifiers.shift) {
            match event.keystroke.key.as_str() {
                "c" => self.copy_selection(cx),
                "v" => self.paste(cx),
                _ => {}
            }
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        if let Some(bytes) = encode_key(&event.keystroke, session.mode()) {
            session.write(bytes);
        }
        self.scroll_to_bottom();
        self.cursor_on = true;
        self.blink_ticks = 0;
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        if event.button != MouseButton::Left {
            return;
        }
        let Some(cell) = self.grid_point(event.position) else {
            return;
        };
        if let Some(session) = &self.session {
            session.term.lock().selection =
                Some(Selection::new(SelectionType::Simple, cell, Side::Left));
        }
        self.selecting = true;
        self.cursor_on = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(cell) = self.grid_point(event.position) else {
            return;
        };
        if let Some(session) = &self.session {
            let mut term = session.term.lock();
            if let Some(selection) = term.selection.as_mut() {
                selection.update(cell, Side::Left);
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(delta) => delta.y,
            ScrollDelta::Pixels(delta) => {
                f32::from(delta.y) / f32::from(self.cell_height.max(px(1.)))
            }
        };
        self.scroll_accumulator += lines;
        let whole = self.scroll_accumulator.trunc();
        if whole == 0.0 {
            return;
        }
        self.scroll_accumulator -= whole;

        if let Some(session) = &self.session {
            session
                .term
                .lock()
                .scroll_display(Scroll::Delta(whole as i32));
            session.dirty.store(true, Ordering::Release);
        }
        self.cursor_on = true;
        cx.notify();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(all(test, unix))]
impl TerminalView {
    /// A view around an already-open session and no auto-start task, so a test
    /// drives an exact shell (say `/bin/cat`) instead of the login shell. Its
    /// only caller runs on Unix, so Windows test builds do not carry it.
    fn with_session(session: TerminalSession, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            session: Some(session),
            error: None,
            focus_handle: cx.focus_handle().tab_stop(true),
            title: None,
            directory: std::env::temp_dir(),
            profile: ShellProfile::Other,
            active: true,
            focused: false,
            exited: false,
            font_size: px(13.),
            cell_width: px(CELL_WIDTH_FALLBACK),
            cell_height: px(13. * LINE_HEIGHT_RATIO),
            cursor_on: true,
            blink_ticks: 0,
            selecting: false,
            scroll_accumulator: 0.,
            grid_bounds: Rc::new(Cell::new(None)),
            _task: Task::ready(()),
        };
        view.measure_font(cx);
        view
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        let font = terminal_font(cx);

        let background = Palette::from_theme(&theme).background;
        let focused = self.focus_handle.is_focused(window);
        // Mirrored for `poll`: an unfocused grid shows a static hollow cursor,
        // so there is no reason to spend a frame every 500 ms on it.
        self.focused = focused;
        let body = match (&self.session, self.error.clone()) {
            (_, Some(error)) => {
                terminal_message(&theme, &tr!("terminal.unavailable"), Some(&error))
            }
            (Some(_), None) => self.grid_element(&theme, &font, focused, window, cx),
            (None, None) => terminal_message(&theme, &tr!("terminal.starting_shell"), None),
        };

        div()
            .id("terminal-view")
            .debug_selector(|| "terminal-view".to_string())
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(background)
            .cursor(CursorStyle::IBeam)
            // Breathing room so glyphs never touch the panel's borders; the
            // canvas (and therefore mouse mapping) is inset by exactly this.
            .px(px(8.))
            .py(px(6.))
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::on_terminal_escape))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(body)
    }
}

impl TerminalView {
    /// The grid canvas. Prepaint resizes the PTY to the panel's cell box,
    /// snapshots the grid, and shapes it; paint blits the shaped rows.
    fn grid_element(
        &self,
        theme: &Theme,
        font: &Font,
        focused: bool,
        _window: &Window,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(session) = &self.session else {
            return div().into_any_element();
        };

        let term = session.term.clone();
        let palette = session.palette.clone();
        let grid_size = session.grid_size.clone();
        let window_size = session.window_size.clone();
        let sender = session.sender.clone();
        let dirty = session.dirty.clone();
        let grid_bounds = self.grid_bounds.clone();
        let font = font.clone();
        let theme = *theme;
        let font_size = self.font_size;
        let cell_width = self.cell_width;
        let cell_height = self.cell_height;
        let blinking = self.cursor_on;
        let scroll_thumb = theme.text_muted.opacity(0.45);

        canvas(
            move |bounds, window, _| {
                grid_bounds.set(Some(bounds));
                let current = Palette::from_theme(&theme);
                *palette.lock().unwrap_or_else(|e| e.into_inner()) = current.clone();

                let columns = ((f32::from(bounds.size.width) / f32::from(cell_width))
                    .floor()
                    .max(0.0) as usize)
                    .max(MIN_COLUMNS);
                let rows = ((f32::from(bounds.size.height) / f32::from(cell_height))
                    .floor()
                    .max(0.0) as usize)
                    .max(MIN_ROWS);

                let resized = {
                    let mut grid = grid_size.lock().unwrap_or_else(|e| e.into_inner());
                    if *grid == (columns, rows) {
                        false
                    } else {
                        *grid = (columns, rows);
                        true
                    }
                };
                if resized {
                    let size = WindowSize {
                        num_lines: rows.min(u16::MAX as usize) as u16,
                        num_cols: columns.min(u16::MAX as usize) as u16,
                        cell_width: f32::from(cell_width).round().max(1.) as u16,
                        cell_height: f32::from(cell_height).round().max(1.) as u16,
                    };
                    term.lock().resize(GridDims { columns, rows });
                    *window_size.lock().unwrap_or_else(|e| e.into_inner()) = size;
                    let _ = sender.send(Msg::Resize(size));
                    dirty.store(true, Ordering::Release);
                }

                // Snapshot under the lock, shape after releasing it.
                let prepared = {
                    let term = term.lock();
                    prepare_grid(&term, &current, &theme, &font, focused, blinking)
                };
                // `force_width` is a per-glyph pitch, not the row width: gpui
                // snaps each glyph to `glyph_index * force_width` so the grid
                // stays column-aligned. The full row is `cell_width * columns`,
                // but passing that would fling every glyph after the first a
                // whole row off the edge.
                let force_width = cell_width;
                let lines = prepared
                    .rows
                    .into_iter()
                    .map(|row| {
                        window.text_system().shape_line(
                            SharedString::from(row.text),
                            font_size,
                            &row.runs,
                            Some(force_width),
                        )
                    })
                    .collect::<Vec<_>>();

                let cursor = prepared.cursor.map(|cell| {
                    let origin = point(
                        bounds.origin.x + cell_width * cell.column as f32,
                        bounds.origin.y + cell_height * cell.row as f32,
                    );
                    (
                        cell.paint,
                        Bounds::new(origin, size(cell_width, cell_height)),
                    )
                });

                GridPaintState {
                    lines,
                    origin: bounds.origin,
                    cell_height,
                    cursor,
                    cursor_color: prepared.cursor_color,
                    scroll: prepared.scroll,
                    scroll_thumb,
                }
            },
            move |bounds, state, window, cx| {
                for (row, line) in state.lines.iter().enumerate() {
                    let origin = point(
                        state.origin.x,
                        state.origin.y + state.cell_height * row as f32,
                    );
                    let _ = line.paint_background(origin, state.cell_height, window, cx);
                    let _ = line.paint(origin, state.cell_height, window, cx);
                }
                if let Some((paint, rect)) = state.cursor {
                    let thickness = px(2.);
                    let color = Background::from(state.cursor_color);
                    let quad = match paint {
                        // The run's colors were already swapped for this cell.
                        CursorPaint::Solid => None,
                        CursorPaint::Hollow => Some(
                            fill(rect, Background::from(Hsla::transparent_black()))
                                .border_widths(px(1.))
                                .border_color(state.cursor_color),
                        ),
                        CursorPaint::Beam => Some(fill(
                            Bounds::new(rect.origin, size(thickness, rect.size.height)),
                            color,
                        )),
                        CursorPaint::Underline => Some(fill(
                            Bounds::new(
                                point(rect.origin.x, rect.origin.y + rect.size.height - thickness),
                                size(rect.size.width, thickness),
                            ),
                            color,
                        )),
                    };
                    if let Some(quad) = quad {
                        window.paint_quad(quad);
                    }
                }
                // Scrollback thumb, hugging the right edge of the grid. The
                // track is the full height; the thumb is the visible slice.
                if let Some(scroll) = state.scroll {
                    let x = bounds.origin.x + bounds.size.width - px(4.);
                    let top = bounds.origin.y + bounds.size.height * scroll.top;
                    let height = (bounds.size.height * scroll.visible).max(px(12.));
                    window.paint_quad(fill(
                        Bounds::new(point(x, top), size(px(3.), height)),
                        Background::from(state.scroll_thumb),
                    ));
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct GridPaintState {
    lines: Vec<gpui::ShapedLine>,
    origin: Point<Pixels>,
    cell_height: Pixels,
    cursor: Option<(CursorPaint, Bounds<Pixels>)>,
    cursor_color: Hsla,
    scroll: Option<ScrollIndicator>,
    scroll_thumb: Hsla,
}

fn terminal_font(cx: &App) -> Font {
    Font {
        family: theme::code_font_family(),
        // Ligatures are wrong in a grid: `calt` would collapse `->`/`!=`/`=>`
        // into one glyph with a different advance, shifting every column after
        // it out of alignment.
        features: FontFeatures::disable_ligatures(),
        // Powerline/Nerd glyphs fall through to an installed patched face.
        fallbacks: nerd_font_family(cx)
            .map(|family| FontFallbacks::from_fonts(vec![family.to_string()])),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

fn terminal_message(theme: &Theme, title: &str, detail: Option<&str>) -> AnyElement {
    let mut column = div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(DynamicSpacing::Base04.px(theme))
        .child(
            div()
                .text_size(TextSize::Small.px(theme))
                .text_color(theme.text_secondary)
                .child(SharedString::from(title.to_owned())),
        );
    if let Some(detail) = detail {
        column = column.child(
            div()
                .max_w(px(520.))
                .text_size(TextSize::Small.px(theme))
                .text_color(theme.error)
                .child(SharedString::from(detail.to_owned())),
        );
    }
    column.into_any_element()
}

// ── panel ──────────────────────────────────────────────────────────────────

/// Drag marker for the panel's height (gpui typed drag state).
pub struct TerminalResize;

/// An invisible drag ghost — resizing leaves no floating preview.
struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The bottom terminal panel: header, resizable body, and the terminal it
/// owns. The shell outlives a close so a toggle does not restart the session;
/// a workspace change does restart it, in the new directory.
/// The bottom terminal panel: a tab strip, a resizable body, and one shell
/// per tab.
///
/// Tabs are the pooled unit, the way sessions are in the chat column: a
/// background tab keeps its PTY (and whatever it is printing) alive, and only
/// the active one is rendered, focused, or asks for frames. The shells outlive
/// closing the panel, so ⌘J is a show/hide rather than a restart.
pub struct TerminalPanel {
    open: bool,
    height: Pixels,
    /// The directory a *new* tab starts in — the workspace the panel was last
    /// handed. Existing tabs keep their own: switching sessions must not kill
    /// a build that is still running in a shell.
    workspace: Option<PathBuf>,
    /// One shell per tab, in strip order.
    tabs: Vec<Entity<TerminalView>>,
    /// Index into `tabs` that is rendered and focused. Always in range while
    /// `tabs` is non-empty.
    active: usize,
    /// Keeps the active tab visible when the strip overflows.
    tab_scroll: ScrollHandle,
    /// Restart feedback: the header button spins until this instant, so the
    /// click is acknowledged while the fresh shell starts.
    restart_spin_until: Option<Instant>,
}

impl TerminalPanel {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            open: false,
            height: px(crate::layout::terminal_height()
                .unwrap_or(PANEL_DEFAULT_H)
                .max(PANEL_MIN_H)),
            workspace: None,
            tabs: Vec::new(),
            active: 0,
            tab_scroll: ScrollHandle::new(),
            restart_spin_until: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// How many shells the panel is holding — the palette gates the tab
    /// commands on it.
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn set_height(&mut self, height: Pixels, cx: &mut Context<Self>) {
        let height = height.clamp(px(PANEL_MIN_H), px(f32::MAX / 2.));
        if height != self.height {
            self.height = height;
            crate::layout::set_terminal_height(f32::from(height));
            cx.notify();
        }
    }

    /// Point *new* tabs at the selected workspace. Existing tabs are left
    /// alone: they are live shells doing work the user started, and a session
    /// switch is not a reason to SIGHUP them. The tab strip keeps showing each
    /// one's own directory, so nothing here is implied that is not true.
    pub fn set_workspace(&mut self, workspace: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        cx.notify();
    }

    /// The active tab's shell, if any.
    fn active_terminal(&self) -> Option<&Entity<TerminalView>> {
        self.tabs.get(self.active)
    }

    /// The directory a fresh tab starts in: the current workspace, else the
    /// process's own directory, else the filesystem root — never a path that
    /// cannot be spawned.
    fn spawn_directory(&self) -> PathBuf {
        self.workspace
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open {
            match self.active_terminal() {
                // The shell that was here before is still here: show it and put
                // the keyboard back in it.
                Some(terminal) => {
                    self.sync_active(true, cx);
                    window.focus(&terminal.read(cx).handle());
                }
                None => self.new_tab(window, cx),
            }
        } else {
            self.sync_active(false, cx);
        }
        cx.notify();
    }

    /// Open a shell in a new tab, in the current workspace, and make it the
    /// active one. The panel opens if it was closed.
    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self.spawn_directory();
        let terminal = cx.new(|cx| TerminalView::new(directory, cx));
        self.tabs.push(terminal.clone());
        self.active = self.tabs.len() - 1;
        self.open = true;
        // The tab that was visible stops asking for frames; its shell keeps
        // running and keeps draining into its own grid.
        self.sync_active(true, cx);
        window.focus(&terminal.read(cx).handle());
        self.tab_scroll.scroll_to_item(self.active);
        cx.notify();
    }

    /// Close one tab. Its shell gets SIGHUP as the entity drops. Closing the
    /// panel's last tab closes the panel — an empty terminal panel is a blank
    /// strip that only wastes the transcript's height.
    pub fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = 0;
            self.open = false;
            cx.notify();
            return;
        }
        // Closing a tab left of the active one shifts it; closing the active
        // one leaves the index on its right-hand neighbour (or on the new last
        // tab when it was the rightmost).
        if index < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self.sync_active(true, cx);
        if let Some(terminal) = self.active_terminal() {
            window.focus(&terminal.read(cx).handle());
        }
        self.tab_scroll.scroll_to_item(self.active);
        cx.notify();
    }

    /// Close the tab that is on screen (⌘W while the panel has focus).
    pub fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active, window, cx);
    }

    /// Show one tab and put the keyboard in it.
    pub fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        self.active = index;
        self.sync_active(true, cx);
        if let Some(terminal) = self.active_terminal() {
            window.focus(&terminal.read(cx).handle());
        }
        self.tab_scroll.scroll_to_item(self.active);
        cx.notify();
    }

    /// Step to the next (`1`) or previous (`-1`) tab, wrapping at the ends —
    /// Ctrl+Tab muscle memory from every other tabbed surface.
    pub fn cycle_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() < 2 {
            return;
        }
        let len = self.tabs.len() as isize;
        let next = (self.active as isize + delta).rem_euclid(len) as usize;
        self.select_tab(next, window, cx);
    }

    /// Close the panel if it is open. Idempotent, and unlike [`Self::toggle`]
    /// it does not need a window — used when a fresh task takes the chat
    /// column and the shell must get out of the way.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.sync_active(false, cx);
        cx.notify();
    }

    /// Kill the active tab's shell and start a fresh one in the same
    /// directory. The tab keeps its place in the strip; the other tabs are
    /// untouched.
    fn restart(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.get(self.active) else {
            return;
        };
        let directory = index.read(cx).directory().to_path_buf();
        let terminal = cx.new(|cx| TerminalView::new(directory, cx));
        self.tabs[self.active] = terminal;
        self.sync_active(true, cx);
        // A short spin on the header button: the fresh shell usually starts in
        // well under a frame, leaving the click otherwise invisible.
        self.restart_spin_until = Some(Instant::now() + RESTART_FEEDBACK);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESTART_FEEDBACK).await;
            let _ = this.update(cx, |panel, cx| {
                // A second click extends the floor; only the last timer clears.
                if panel
                    .restart_spin_until
                    .is_some_and(|until| Instant::now() >= until)
                {
                    panel.restart_spin_until = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Mirror the app's "is this panel actually on screen" state into the
    /// views. The panel can be open while another page owns the main column,
    /// and a terminal nobody can see should not ask for frames. Hidden tabs
    /// are inactive for the same reason, whichever one is selected.
    pub fn sync_active(&self, active: bool, cx: &mut Context<Self>) {
        for (index, terminal) in self.tabs.iter().enumerate() {
            let visible = active && index == self.active;
            terminal.update(cx, |view, cx| view.set_active(visible, cx));
        }
    }

    /// Clicking anywhere on the panel — its header included — puts the
    /// keyboard in the shell, so the panel never looks inert.
    fn on_panel_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.active_terminal() {
            window.focus(&terminal.read(cx).handle());
        }
    }

    /// The panel's top edge: a thin grab strip that resizes the panel. It sits
    /// above the header so the tab strip below it never fights a drag for the
    /// same pixels.
    fn splitter(&self, theme: Theme) -> AnyElement {
        div()
            .id("terminal-panel-splitter")
            .flex_none()
            .w_full()
            .h(px(SPLITTER_H))
            .cursor(CursorStyle::ResizeUpDown)
            .on_drag(TerminalResize, |_, _, _, cx| cx.new(|_| DragGhost))
            // Hover is the only affordance a borderless splitter can carry on
            // a code surface; it reads as the edge thickening, not a new rule.
            .hover(|style| style.bg(theme.border))
            .into_any_element()
    }

    /// One chip in the tab strip: label, and a close button that appears with
    /// the active tab and on hover so a quiet strip stays quiet.
    fn tab_chip(&self, index: usize, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let terminal = &self.tabs[index];
        let active = index == self.active;
        let (label, icon_path, tooltip) = {
            let view = terminal.read(cx);
            (view.label(), view.profile_icon(), view.tooltip())
        };
        press(
            div()
                .id(gpui::ElementId::Name(
                    format!("terminal-tab-{index}").into(),
                ))
                .h(ButtonSize::Default.height(&theme))
                .max_w(px(TAB_MAX_W))
                .flex_none()
                .px(DynamicSpacing::Base06.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base04.px(&theme))
                .rounded(Radius::Medium.px(&theme))
                .cursor_pointer()
                .text_size(TextSize::Small.px(&theme))
                .when(active, |el| el.bg(theme.surface_active))
                .when(!active, |el| {
                    el.text_color(theme.text_muted)
                        .hover(|el| el.bg(theme.surface_hover).text_color(theme.text_secondary))
                }),
        )
        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
            this.select_tab(index, window, cx);
        }))
        // The profile mark: which shell this tab runs, ahead of what it is
        // doing. Dimmed with the label so an inactive tab stays one quiet
        // shape rather than two tones.
        .child(icon(
            icon_path,
            IconSize::XSmall.px(&theme),
            if active {
                theme.text_primary
            } else {
                theme.text_muted
            },
        ))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active {
                    theme.text_primary
                } else {
                    theme.text_muted
                })
                .child(label),
        )
        .child(
            icon_button_frame(
                div().id(gpui::ElementId::Name(
                    format!("terminal-tab-close-{index}").into(),
                )),
                &theme,
                ButtonSize::None,
            )
            .group(BUTTON_GROUP)
            .tip(tr!("terminal.close_tab"))
            .cursor_pointer()
            .hover(|el| el.bg(theme.surface_hover))
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                this.close_tab(index, window, cx);
                // Click handlers bubble: without this the chip's own handler
                // would select the stale index inside the shrunken strip.
                cx.stop_propagation();
            }))
            .child(icon(
                "icons/x.svg",
                ButtonSize::None.icon_size().px(&theme),
                if active {
                    theme.text_primary
                } else {
                    theme.text_muted
                },
            )),
        )
        .tip(tooltip)
        .into_any_element()
    }

    fn header(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let exited = self
            .active_terminal()
            .is_some_and(|terminal| terminal.read(cx).is_exited());

        let mut strip = div()
            .id("terminal-tabs")
            .h_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base04.px(&theme))
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll);
        for index in 0..self.tabs.len() {
            strip = strip.child(self.tab_chip(index, theme, cx));
        }

        let mut header = div()
            .id("terminal-panel-header")
            .flex_none()
            .h(px(HEADER_H))
            .w_full()
            .px_2()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .border_b_1()
            .border_color(theme.border)
            // No leading panel glyph: every tab already carries its profile
            // mark, and a second terminal icon would only eat tab width.
            .child(strip.flex_1())
            // New tab sits with the tabs, where a browser and every editor put
            // it, and never scrolls away with them.
            .child(
                icon_button_frame(div().id("terminal-new-tab"), &theme, ButtonSize::Default)
                    .group(BUTTON_GROUP)
                    .tip(tr!("terminal.new_tab"))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_hover))
                    .active(|style| style.bg(theme.surface_active))
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                        this.new_tab(window, cx);
                    }))
                    .child(icon(
                        "icons/plus.svg",
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_secondary,
                    )),
            );
        if exited {
            header = header.child(
                div()
                    .flex_none()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(Radius::Small.px(&theme))
                    .bg(theme.error.opacity(0.15))
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.error)
                    .child(tr!("terminal.exited")),
            );
        }
        header
            // Restart reads as the primary action once the shell has exited,
            // and only ever touches the active tab.
            .child(
                icon_button_frame(div().id("terminal-restart"), &theme, ButtonSize::Default)
                    .group(BUTTON_GROUP)
                    .tip(tr!("terminal.restart"))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_hover))
                    .active(|style| style.bg(theme.surface_active))
                    .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.restart(cx)))
                    .child(refresh_glyph(
                        "terminal-restart-spin",
                        ButtonSize::Default.icon_size().px(&theme),
                        self.restart_spin_until.is_some(),
                        if exited {
                            theme.accent_text
                        } else {
                            theme.text_secondary
                        },
                        theme,
                    )),
            )
            .child(
                icon_button_frame(div().id("terminal-close"), &theme, ButtonSize::Default)
                    .group(BUTTON_GROUP)
                    .tip(tr!("common.close"))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_hover))
                    .on_click(
                        cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                            this.toggle(window, cx)
                        }),
                    )
                    .child(icon(
                        "icons/x.svg",
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_secondary,
                    )),
            )
            .into_any_element()
    }

    /// A quiet card over the frozen grid once the shell is gone, so "why is
    /// nothing happening" has an answer and a one-click fix.
    fn exited_overlay(&self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let terminal = self.active_terminal()?;
        if !terminal.read(cx).is_exited() {
            return None;
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .px(DynamicSpacing::Base16.px(&theme))
                        .py(DynamicSpacing::Base12.px(&theme))
                        .rounded(Radius::XLarge.px(&theme))
                        .bg(theme.surface_elevated)
                        .border_1()
                        .border_color(theme.border)
                        .shadow(theme.composer_shadow())
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_secondary)
                                .child(tr!("terminal.shell_exited")),
                        )
                        .child(
                            button_frame(
                                div().id("terminal-exited-restart"),
                                &theme,
                                ButtonSize::Medium,
                            )
                            .raised(theme.action, &theme)
                            .cursor_pointer()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.action_fg)
                            .hover(|style| style.opacity(0.9))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.restart(cx)),
                            )
                            .child(icon(
                                "icons/refresh.svg",
                                ButtonSize::Medium.icon_size().px(&theme),
                                theme.text_primary,
                            ))
                            .child(tr!("terminal.restart")),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let theme = *theme::get(cx);
        let active = self.active_terminal().cloned();
        div()
            .id("terminal-panel")
            .relative()
            .flex_none()
            .w_full()
            .h(self.height)
            .bg(theme.code_bg)
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .min_h_0()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_panel_mouse_down))
            .child(self.splitter(theme))
            .child(self.header(theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .relative()
                    .overflow_hidden()
                    // Only the active tab is mounted: a background shell keeps
                    // its PTY and its grid, but an unmounted view cannot cost a
                    // layout or a paint.
                    .children(active)
                    .children(self.exited_overlay(theme, cx)),
            )
            .into_any_element()
    }
}

fn home_dir() -> Option<PathBuf> {
    crate::platform::home_dir_opt()
}

/// `~/Personal/orbit` rather than the full path, so the header reads at a
/// glance instead of wrapping into noise.
fn abbreviate_home(path: &std::path::Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_owned();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Processor;

    fn stroke(key: &str) -> Keystroke {
        Keystroke {
            modifiers: gpui::Modifiers::default(),
            key: key.to_owned(),
            key_char: None,
        }
    }

    #[test]
    fn control_bytes_follow_the_c0_table() {
        assert_eq!(control_byte("c"), Some(3));
        assert_eq!(control_byte("a"), Some(1));
        assert_eq!(control_byte("["), Some(27));
        assert_eq!(control_byte("?"), Some(127));
        assert_eq!(control_byte(" "), Some(0));
        assert_eq!(control_byte("enter"), None);
        assert_eq!(control_byte("ab"), None);
    }

    #[test]
    fn encode_key_sends_expected_sequences() {
        let mode = TermMode::empty();
        // A bare letter types itself, even without a `key_char`.
        assert_eq!(encode_key(&stroke("a"), mode), Some(vec![b'a']));
        // A named key we do not map is never typed as text.
        assert_eq!(encode_key(&stroke("capslock"), mode), None);
        assert_eq!(encode_key(&stroke("enter"), mode), Some(vec![b'\r']));
        assert_eq!(encode_key(&stroke("up"), mode), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            encode_key(&stroke("up"), mode | TermMode::APP_CURSOR),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(encode_key(&stroke("backspace"), mode), Some(vec![0x7f]));

        // ctrl maps to the C0 control byte; alt prefixes meta.
        let mut control = stroke("c");
        control.modifiers.control = true;
        assert_eq!(encode_key(&control, mode), Some(vec![3]));
        let mut meta = stroke("s");
        meta.modifiers.alt = true;
        meta.key_char = Some("\u{df}".to_owned());
        assert_eq!(encode_key(&meta, mode), Some(b"\x1bs".to_vec()));

        // cmd/super never reaches the PTY.
        let mut command = stroke("c");
        command.modifiers.platform = true;
        assert_eq!(encode_key(&command, mode), None);
    }

    fn test_palette() -> Palette {
        let solid = |hex: u32| Hsla::from(gpui::rgba(hex));
        Palette {
            ansi: [solid(0x1d1f21); 16],
            foreground: solid(0xc5c8c6),
            background: solid(0x1d1f21),
            cursor: solid(0xffffff),
        }
    }

    fn test_font() -> Font {
        Font {
            family: SharedString::from("mono"),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    /// A term with a live (but never-fed) event proxy, so ANSI bytes can be
    /// driven through the real parser without a PTY or a GPUI app.
    fn test_term(columns: usize, rows: usize) -> Term<EventProxy> {
        let (ui, _receiver) = unbounded();
        let proxy = EventProxy {
            dirty: Arc::new(AtomicBool::new(false)),
            sender: Arc::new(OnceLock::new()),
            ui,
            window_size: Arc::new(Mutex::new(WindowSize {
                num_lines: rows as u16,
                num_cols: columns as u16,
                cell_width: 8,
                cell_height: 16,
            })),
            palette: Arc::new(Mutex::new(test_palette())),
        };
        Term::new(Config::default(), &GridDims { columns, rows }, proxy)
    }

    /// The grid builder is where ANSI colors, text attributes, and selection
    /// become GPUI runs — the one piece `encode_key`-style tests cannot reach.
    #[test]
    fn prepare_grid_resolves_colors_flags_and_selection() {
        let palette = test_palette();
        let theme = Theme::for_id(theme::ThemeId::Orbit);
        let font = test_font();
        let mut term = test_term(20, 3);
        let mut processor: Processor = Processor::new();
        processor.advance(
            &mut term,
            b"\x1b[31mab\x1b[0m\x1b[1mcd\x1b[0m\x1b[7mef\x1b[0m",
        );

        let prepared = prepare_grid(&term, &palette, &theme, &font, true, true);
        let row = &prepared.rows[0];
        assert!(row.text.starts_with("abcdef"), "text was {:?}", row.text);
        assert_eq!(row.text.chars().count(), 20, "row pads to the grid width");

        // "ab" — ANSI red, no background.
        assert_eq!(row.runs[0].len, 2);
        assert_eq!(row.runs[0].color, palette.ansi[1]);
        assert_eq!(row.runs[0].background_color, None);
        // "cd" — bold carries through to the run's font.
        assert_eq!(row.runs[1].len, 2);
        assert_eq!(row.runs[1].font.weight, FontWeight::BOLD);
        // "ef" — reverse swaps the default fg into the background.
        assert_eq!(row.runs[2].len, 2);
        assert_eq!(row.runs[2].color, palette.background);
        assert_eq!(row.runs[2].background_color, Some(palette.foreground));
    }

    #[test]
    fn prepare_grid_paints_the_selection() {
        let palette = test_palette();
        let theme = Theme::for_id(theme::ThemeId::Orbit);
        let font = test_font();
        let mut term = test_term(10, 2);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"hello");

        // Columns 1..=2 on the first line. The end anchor's `Side` decides
        // whether the cell under the cursor is included: `Left` stops before
        // it, `Right` includes it (what a drag past the cell does).
        let start = TermPoint::new(Line(0), Column(1));
        term.selection = Some(Selection::new(SelectionType::Simple, start, Side::Left));
        if let Some(selection) = term.selection.as_mut() {
            selection.update(TermPoint::new(Line(0), Column(2)), Side::Right);
        }

        let prepared = prepare_grid(&term, &palette, &theme, &font, true, true);
        let row = &prepared.rows[0];
        let selected = row
            .runs
            .iter()
            .filter(|run| run.background_color == Some(theme.selection))
            .map(|run| run.len)
            .sum::<usize>();
        assert_eq!(
            selected, 2,
            "exactly the selected cells take the selection wash"
        );
    }

    #[test]
    fn selection_side_left_stops_before_the_cursor_cell() {
        let palette = test_palette();
        let theme = Theme::for_id(theme::ThemeId::Orbit);
        let font = test_font();
        let mut term = test_term(10, 2);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"hello");

        let start = TermPoint::new(Line(0), Column(1));
        term.selection = Some(Selection::new(SelectionType::Simple, start, Side::Left));
        if let Some(selection) = term.selection.as_mut() {
            selection.update(TermPoint::new(Line(0), Column(2)), Side::Left);
        }

        let prepared = prepare_grid(&term, &palette, &theme, &font, true, true);
        let selected = prepared.rows[0]
            .runs
            .iter()
            .filter(|run| run.background_color == Some(theme.selection))
            .map(|run| run.len)
            .sum::<usize>();
        assert_eq!(selected, 1);
    }

    /// A right prompt (zsh's RPROMPT) parks the cursor at the right margin,
    /// writes, then restores it with DECSC/DECRC so the shell's own input
    /// continues where it left off. If the restore were mishandled, every
    /// keystroke after the prompt would land at the far-right column — which
    /// reads to a user as "I can't type".
    #[test]
    fn prepare_grid_survives_a_right_prompt_round_trip() {
        let palette = test_palette();
        let theme = Theme::for_id(theme::ThemeId::Orbit);
        let font = test_font();
        let mut term = test_term(20, 3);
        let mut processor: Processor = Processor::new();
        // left / save / jump to column 16 / "right" / restore / type "X"
        processor.advance(&mut term, b"left\x1b7\x1b[16Gright\x1b8X");

        let prepared = prepare_grid(&term, &palette, &theme, &font, true, true);
        let row = &prepared.rows[0];
        assert!(
            row.text.starts_with("leftX"),
            "input resumes where the prompt left off: {:?}",
            row.text
        );
        assert!(
            row.text.ends_with("right"),
            "right prompt sits on the right margin: {:?}",
            row.text
        );
    }

    /// The cursor must land on the cell the shell is about to write to, and
    /// change shape (solid → hollow) with focus — the two things that make a
    /// grid feel alive or broken.
    #[test]
    fn prepare_grid_places_the_cursor_and_tracks_focus() {
        let palette = test_palette();
        let theme = Theme::for_id(theme::ThemeId::Orbit);
        let font = test_font();
        let mut term = test_term(10, 2);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"ab");

        let focused = prepare_grid(&term, &palette, &theme, &font, true, true);
        let cursor = focused.cursor.expect("cursor present");
        assert_eq!((cursor.row, cursor.column), (0, 2), "two cells in");
        assert_eq!(cursor.paint, CursorPaint::Solid);

        let blurred = prepare_grid(&term, &palette, &theme, &font, false, true);
        assert_eq!(
            blurred.cursor.expect("hollow cursor still drawn").paint,
            CursorPaint::Hollow
        );

        // The blink's off phase hides a focused cursor entirely.
        let blinked_off = prepare_grid(&term, &palette, &theme, &font, true, false);
        assert!(
            blinked_off.cursor.is_none(),
            "blink off must not draw the cursor"
        );
        assert!(
            !blinked_off.rows[0]
                .runs
                .iter()
                .any(|run| run.background_color == Some(palette.cursor)),
            "blink off must not paint the block either"
        );
    }

    #[test]
    fn prepare_grid_reports_scrollback_position() {
        let palette = test_palette();
        let theme = Theme::for_id(theme::ThemeId::Orbit);
        let font = test_font();
        let mut term = test_term(10, 2);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n");

        let prepared = prepare_grid(&term, &palette, &theme, &font, true, true);
        let scroll = prepared.scroll.expect("scrolled-off lines are history");
        assert!(scroll.visible > 0. && scroll.visible < 1.);
        assert!(scroll.top >= 0.);
        assert!(scroll.top <= 1. - scroll.visible + f32::EPSILON);
    }

    #[test]
    fn abbreviates_the_home_directory() {
        let Some(home) = home_dir() else {
            return;
        };
        assert_eq!(abbreviate_home(&home), "~");
        assert_eq!(abbreviate_home(&home.join("a/b")), "~/a/b");
        assert_eq!(abbreviate_home(std::path::Path::new("/tmp")), "/tmp");
    }

    /// Rich prompts (zsh's RPROMPT, powerlevel10k) place themselves with a
    /// cursor-position report — `ESC [ 6 n` — and wait for the reply. A
    /// terminal that swallows DSR leaves the prompt stranded at the right
    /// edge, which reads as a broken/read-only shell.
    #[cfg(unix)]
    #[test]
    fn session_answers_a_cursor_position_report() {
        let session = TerminalSession::with_shell(
            &std::env::temp_dir(),
            40,
            8,
            test_palette(),
            std::path::Path::new("/bin/bash"),
            Vec::new(),
        )
        .expect("spawn shell");

        // Ask for DSR, capture the reply up to its trailing `R`, then print it
        // with the ESC swapped out so the terminal renders it literally.
        session.write(
            b"printf '\\033[6n'; IFS= read -r -d R reply; printf 'CPR:%s\\n' \"$(printf %s \"$reply\" | tr '\\033' '@')\"\r"
                .to_vec(),
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut screen = String::new();
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            let term = session.term.lock();
            screen = term
                .renderable_content()
                .display_iter
                .map(|cell| cell.c)
                .collect();
            if screen.contains("CPR:@[") {
                return;
            }
        }
        panic!("terminal never answered ESC[6n; screen was:\n{screen}");
    }

    /// End-to-end: a real shell runs on a real PTY and its output reaches the
    /// emulator grid. The probe string is split in the echoed command, so a
    /// match proves `printf` actually executed rather than being echoed.
    #[cfg(unix)]
    #[test]
    fn session_runs_a_shell_command() {
        let session = TerminalSession::with_shell(
            &std::env::temp_dir(),
            80,
            24,
            test_palette(),
            std::path::Path::new("/bin/sh"),
            Vec::new(),
        )
        .expect("spawn shell");

        session.write(b"printf 'ORBIT%s\\n' -OK\r".to_vec());

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut screen = String::new();
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            let term = session.term.lock();
            screen = term
                .renderable_content()
                .display_iter
                .map(|cell| cell.c)
                .collect();
            if screen.contains("ORBIT-OK") {
                return;
            }
        }
        panic!("shell output never reached the grid; screen was:\n{screen}");
    }

    /// End-to-end through GPUI: a real grid is shaped and painted into a real
    /// window. This is the only test that runs the canvas prepaint (PTY
    /// resize + `shape_line`) and paint (`paint_background`, `paint`,
    /// `paint_quad`) paths, so it catches a panic inside them.
    #[cfg(unix)]
    #[gpui::test]
    fn typing_reaches_the_shell(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        // `cat` echoes stdin verbatim, so anything that reaches the PTY comes
        // back on the grid — a keystroke that never arrives leaves it empty.
        let session = TerminalSession::with_shell(
            &std::env::temp_dir(),
            40,
            8,
            test_palette(),
            std::path::Path::new("/bin/cat"),
            Vec::new(),
        )
        .expect("spawn cat");
        let term = session.term.clone();
        let view = cx.new(|cx| TerminalView::with_session(session, cx));

        let element = view.clone();
        cx.draw(
            point(px(0.), px(0.)),
            size(px(400.), px(160.)),
            move |_, _| element.clone(),
        );
        cx.update(|window, cx| view.read(cx).handle().focus(window));
        // A frame must land between focusing and typing: the focus handle is
        // registered while the grid prepaints, and key dispatch resolves it
        // against the rendered frame.
        let element = view.clone();
        cx.draw(
            point(px(0.), px(0.)),
            size(px(400.), px(160.)),
            move |_, _| element.clone(),
        );
        cx.simulate_keystrokes("o r b i t");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut screen = String::new();
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            let term = term.lock();
            screen = term
                .renderable_content()
                .display_iter
                .map(|cell| cell.c)
                .collect();
            if screen.contains("orbit") {
                return;
            }
        }
        panic!("typed input never reached the shell; screen was:\n{screen}");
    }

    /// The production path: `TerminalPanel::toggle` (what ⌘J calls) creates the
    /// view, focuses it, and the grid receives keystrokes through the panel's
    /// nesting rather than as a bare root element.
    #[cfg(unix)]
    #[gpui::test]
    fn opening_the_panel_and_typing_reaches_the_shell(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));
        // The app's real keymap: a global binding that matched typed letters
        // would swallow them here exactly as it does in the running app.
        cx.update(|_, cx| crate::bind_keys(cx));

        let panel = cx.new(TerminalPanel::new);
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_workspace(Some(std::env::temp_dir()), cx)
            })
        });
        let p = panel.clone();
        cx.draw(
            point(px(0.), px(0.)),
            size(px(400.), px(200.)),
            move |_, _| p.clone(),
        );

        // Open it the way ⌘J does: create the view and focus it.
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.toggle(window, cx)));
        let p = panel.clone();
        cx.draw(
            point(px(0.), px(0.)),
            size(px(400.), px(200.)),
            move |_, _| p.clone(),
        );

        // The autostart shell is the login shell; swap in `cat` so the test is
        // deterministic, then type.
        let terminal = cx
            .update(|_, cx| panel.read(cx).active_terminal().cloned())
            .expect("terminal created");
        let cat = TerminalSession::with_shell(
            &std::env::temp_dir(),
            40,
            8,
            test_palette(),
            std::path::Path::new("/bin/cat"),
            Vec::new(),
        )
        .expect("spawn cat");
        let term = cat.term.clone();
        cx.update(|_, cx| terminal.update(cx, |view, _| view.session = Some(cat)));

        let p = panel.clone();
        cx.draw(
            point(px(0.), px(0.)),
            size(px(400.), px(200.)),
            move |_, _| p.clone(),
        );
        cx.simulate_keystrokes("o r b i t");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut screen = String::new();
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            let term = term.lock();
            screen = term
                .renderable_content()
                .display_iter
                .map(|cell| cell.c)
                .collect();
            if screen.contains("orbit") {
                return;
            }
        }
        panic!("typing through the panel never reached the shell; screen was:\n{screen}");
    }

    #[cfg(unix)]
    #[gpui::test]
    fn terminal_view_draws_a_real_grid(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        // `TerminalView::render` reads the theme global, which only `init`
        // installs in the app. A test sets it directly to avoid disk prefs.
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        let directory = std::env::temp_dir();
        let _ = cx.draw(
            point(px(0.), px(0.)),
            size(px(640.), px(240.)),
            move |_, cx| {
                let view = cx.new(|cx| TerminalView::new(directory, cx));
                // Inject a ready session so prepaint shapes a real grid rather
                // than the "Starting shell…" placeholder; the shell gets a
                // moment to answer `printf` first.
                let session = TerminalSession::with_shell(
                    &std::env::temp_dir(),
                    40,
                    8,
                    test_palette(),
                    std::path::Path::new("/bin/sh"),
                    Vec::new(),
                )
                .expect("spawn shell");
                session.write(b"printf 'GRID%s\\n' -OK\r".to_vec());
                std::thread::sleep(Duration::from_millis(200));
                view.update(cx, |view, _| view.session = Some(session));
                view
            },
        );

        let bounds = cx
            .debug_bounds("terminal-view")
            .expect("grid view laid out");
        assert_eq!(bounds.size.width, px(640.));
        assert_eq!(bounds.size.height, px(240.));
    }

    // ── tabs ───────────────────────────────────────────────────────────

    /// Open a panel with `count` tabs, in `directory`, and return it. Only the
    /// last tab is active, as after ⌘T.
    fn panel_with_tabs(
        cx: &mut gpui::VisualTestContext,
        count: usize,
        directory: PathBuf,
    ) -> Entity<TerminalPanel> {
        let panel = cx.new(TerminalPanel::new);
        cx.update(|_, cx| panel.update(cx, |panel, cx| panel.set_workspace(Some(directory), cx)));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                for _ in 0..count {
                    panel.new_tab(window, cx);
                }
            });
        });
        panel
    }

    /// New tabs stack up, and the shells behind them are still held: a tab
    /// that is only hidden must never be dropped (that would SIGHUP a build
    /// the user left running).
    #[gpui::test]
    fn new_tabs_keep_the_earlier_shells_alive(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        let panel = panel_with_tabs(cx, 2, std::env::temp_dir());
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.tab_count(), 2);
            assert_eq!(panel.active, 1, "the fresh tab is the visible one");
            assert_ne!(
                panel.tabs[0].entity_id(),
                panel.tabs[1].entity_id(),
                "each tab owns its own shell"
            );
            // The first tab is unmounted and stops asking for frames, but it is
            // still in the strip — and therefore still alive.
            assert!(!panel.tabs[0].read(cx).active);
            assert!(panel.tabs[1].read(cx).active);
        });
    }

    /// Hiding the whole panel deactivates every tab, not just the selected one:
    /// a background shell must not keep requesting frames behind a full-page
    /// surface.
    #[gpui::test]
    fn hiding_the_panel_deactivates_every_tab(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        let panel = panel_with_tabs(cx, 2, std::env::temp_dir());
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.toggle(window, cx)));
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert!(!panel.is_open());
            assert!(panel.tabs.iter().all(|tab| !tab.read(cx).active));
        });
        // Reopening shows the tab that was selected, not a brand-new shell.
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.toggle(window, cx)));
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.tab_count(), 2);
            assert_eq!(panel.active, 1);
            assert!(panel.tabs[1].read(cx).active);
            assert!(!panel.tabs[0].read(cx).active);
        });
    }

    /// Closing the active tab hands the strip to its right-hand neighbour,
    /// and closing the rightmost one falls back to the left.
    #[gpui::test]
    fn closing_a_tab_selects_its_neighbour(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        let panel = panel_with_tabs(cx, 3, std::env::temp_dir());
        let third = cx.update(|_, cx| panel.read(cx).tabs[2].entity_id());
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.select_tab(1, window, cx)));
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.close_tab(1, window, cx)));
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.tab_count(), 2);
            assert_eq!(panel.active, 1, "the tab that moved up takes the focus");
            assert_eq!(panel.tabs[1].entity_id(), third);
        });

        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.close_active_tab(window, cx)));
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.tab_count(), 1);
            assert_eq!(panel.active, 0, "the last tab's neighbour is on the left");
            assert!(panel.is_open(), "one tab left: the panel stays up");
        });
    }

    /// The last tab taking the panel with it, rather than leaving an empty
    /// strip that only costs the transcript height.
    #[gpui::test]
    fn closing_the_last_tab_closes_the_panel(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        let panel = panel_with_tabs(cx, 1, std::env::temp_dir());
        assert!(cx.update(|_, cx| panel.read(cx).is_open()));
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.close_active_tab(window, cx)));
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.tab_count(), 0);
            assert!(!panel.is_open());
        });
    }

    /// ⌘⇧[ / ⌘⇧] wrap at both ends and are a no-op on a single tab.
    #[gpui::test]
    fn cycling_tabs_wraps_at_the_ends(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        let panel = panel_with_tabs(cx, 3, std::env::temp_dir());
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.select_tab(0, window, cx)));
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.cycle_tab(-1, window, cx)));
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).active),
            2,
            "wraps to the end"
        );
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.cycle_tab(1, window, cx)));
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).active),
            0,
            "wraps to the front"
        );

        let single = panel_with_tabs(cx, 0, std::env::temp_dir());
        cx.update(|window, cx| {
            single.update(cx, |panel, cx| {
                panel.new_tab(window, cx);
                panel.cycle_tab(1, window, cx);
                assert_eq!(panel.active, 0, "one tab has nowhere to cycle to");
            });
        });
    }

    /// A workspace switch points *new* tabs at the new directory and leaves the
    /// live shells where they are — a session switch is no reason to kill a
    /// running build.
    #[gpui::test]
    fn switching_workspaces_spares_running_shells(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));

        let first = std::env::temp_dir();
        let second = PathBuf::from("/tmp");
        let panel = panel_with_tabs(cx, 1, first.clone());
        let shell = cx.update(|_, cx| panel.read(cx).tabs[0].entity_id());

        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_workspace(Some(second.clone()), cx)
            })
        });
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.tab_count(), 1, "the shell is not replaced");
            assert_eq!(panel.tabs[0].entity_id(), shell);
            assert_eq!(panel.tabs[0].read(cx).directory(), first.as_path());
        });

        // The next tab follows the workspace the panel was last handed.
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.new_tab(window, cx)));
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.tab_count(), 2);
            assert_eq!(panel.tabs[1].read(cx).directory(), second.as_path());
        });
    }

    /// Each login shell gets its own profile mark; anything Simple Icons has
    /// no logo for (it withdraws the trademarked PowerShell and Windows marks)
    /// keeps the generic terminal glyph rather than a wrong or borrowed one.
    #[test]
    fn shell_profiles_map_to_their_own_marks() {
        use gpui::AssetSource as _;
        let of = |path: &str| ShellProfile::of(std::path::Path::new(path));

        assert_eq!(of("/bin/zsh"), ShellProfile::Zsh);
        assert_eq!(of("/usr/local/bin/bash"), ShellProfile::Bash);
        assert_eq!(of("/opt/homebrew/bin/fish"), ShellProfile::Fish);
        assert_eq!(of("/bin/sh"), ShellProfile::Other);
        assert_eq!(of("/bin/dash"), ShellProfile::Other);
        assert_eq!(of(r"C:\Windows\System32\cmd.exe"), ShellProfile::Other);
        assert_eq!(of(""), ShellProfile::Other);

        // A mark that is not embedded renders as nothing at all — silently.
        for profile in [
            ShellProfile::Zsh,
            ShellProfile::Bash,
            ShellProfile::Fish,
            ShellProfile::Other,
        ] {
            let loaded = crate::assets::Assets
                .load(profile.icon())
                .expect("asset load")
                .is_some();
            assert!(loaded, "{} is not embedded", profile.icon());
        }
        assert_eq!(ShellProfile::Zsh.name(), "zsh");
        assert_eq!(ShellProfile::Other.name(), "shell");
    }

    /// A tab's label is its shell title when there is one, and the folder it
    /// was spawned in otherwise — never an empty chip.
    #[gpui::test]
    fn tab_labels_fall_back_to_the_folder(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(theme::ThemeId::Orbit)));
        let (view, label) = cx.update(|_, cx| {
            let view = cx.new(|cx| TerminalView::new(PathBuf::from("/tmp/orbit-tab-label"), cx));
            let label = view.read(cx).label().to_string();
            (view, label)
        });
        assert_eq!(label, "orbit-tab-label");
        // The mark and the tooltip come from the same login-shell answer the
        // PTY gets, so this stays true on a bash desktop and a fish one.
        let (shell, _) = default_shell();
        assert_eq!(
            cx.update(|_, cx| view.read(cx).profile_icon()),
            ShellProfile::of(&shell).icon(),
            "the tab shows the mark of the shell it actually spawned"
        );
        assert_eq!(
            cx.update(|_, cx| view.read(cx).tooltip()),
            format!("{} — /tmp/orbit-tab-label", ShellProfile::of(&shell).name()),
            "the tooltip names the profile and where the tab started"
        );
        cx.update(|_, cx| view.update(cx, |view, _| view.title = Some("cargo test".to_owned())));
        assert_eq!(
            cx.update(|_, cx| view.read(cx).label().to_string()),
            "cargo test",
            "a shell that sets a title names its own tab"
        );
    }

    #[test]
    fn indexed_colors_cover_the_cube_and_greys() {
        let palette = test_palette();
        // Slot 16 is the cube's black; slot 231 its white; 244 is mid grey.
        assert_eq!(palette.indexed(16), rgb_to_hsla(Rgb { r: 0, g: 0, b: 0 }));
        assert_eq!(
            palette.indexed(231),
            rgb_to_hsla(Rgb {
                r: 255,
                g: 255,
                b: 255
            })
        );
        let grey = palette.indexed(244);
        let Rgb { r, g, b } = hsla_to_rgb(grey);
        assert_eq!((r, g), (b, b));
    }
}
