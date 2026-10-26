//! File loading and editing for the Explorer's viewer.
//!
//! Split in two: [`load`] is pure filesystem work (no GPUI), so it runs on the
//! background executor and is unit-tested without a window; the `FileViewer`
//! entity further down only paints what `load` returns.
//!
//! Guards are deliberate and honest: a directory, a binary file, or a file
//! over [`MAX_FILE_BYTES`] is reported as such — never dumped lossily at the
//! user or read into memory unbounded. Code and plain-text files open in an
//! editable [`ComposerInput`] with syntax highlighting; edits autosave back to
//! disk (debounced, atomic). Markdown, images, binary, and oversized files stay
//! read-only.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, img, prelude::*, px, AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable,
    Font, FontFeatures, FontStyle, FontWeight, Hsla, Image, ImageFormat, ImageSource,
    ListAlignment, ListState, ObjectFit, Render, ScrollHandle, StyledText, Subscription, TextRun,
    Timer, Window,
};

use crate::app::{
    button_frame, empty_state, file_badge, file_glyph, icon, icon_button_frame, nerd_font_family,
    press, EmptyFill, TipExt,
};
use crate::composer::ComposerInput;
use crate::highlight::{self, Lang, Token};
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{ButtonSize, DynamicSpacing, IconSize, Radius, TextSize};
use crate::theme::{self, Theme, ThemeMode};

/// Files above this are not previewed. Two megabytes is far past any source
/// file a person reads and well under anything that would stall a frame.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// A line cap for pathological generated files that are still under the byte
/// cap (minified bundles). Long past anything readable.
pub const MAX_LINES: usize = 200_000;

/// How many leading bytes to sniff for a NUL when deciding "binary".
const BINARY_SNIFF_BYTES: usize = 8_000;

/// Idle beat after the last keystroke before the editor writes to disk.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(500);

/// How a loaded file should be painted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Code { lang: Lang },
    Markdown,
    Text,
    Image,
    Binary,
    TooLarge,
}

/// A file ready to paint. `lines`/`tokens` are empty for the modes that do not
/// render text.
#[derive(Clone, Debug)]
pub struct FileContent {
    pub path: PathBuf,
    /// Workspace-relative path for the header, when known.
    pub display: String,
    pub mode: Mode,
    /// The decoded file text, verbatim (used to seed the editor).
    pub text: String,
    pub lines: Vec<String>,
    /// Paint-only syntax spans over `lines`, computed off the UI thread.
    /// `None` when the mode is not [`Mode::Code`].
    pub tokens: Option<Vec<Vec<Token>>>,
    /// Raw bytes for [`Mode::Image`], already bounded by [`MAX_FILE_BYTES`].
    pub image_bytes: Option<Vec<u8>>,
    pub bytes: u64,
    /// The line cap trimmed the document.
    pub truncated: bool,
    /// The bytes were not valid UTF-8, so the decoded text cannot round-trip
    /// through the editor — the file stays read-only rather than corrupting it.
    pub lossy: bool,
    /// A read error worth showing in place of the content.
    pub error: Option<String>,
}

impl FileContent {
    /// Whether this content opens in the editable buffer rather than a
    /// read-only body. Truncated and lossy files stay read-only: writing their
    /// in-memory text back would destroy the bytes the viewer never held.
    pub fn editable(&self) -> bool {
        self.error.is_none()
            && !self.truncated
            && !self.lossy
            && matches!(self.mode, Mode::Code { .. } | Mode::Text | Mode::Markdown)
    }

    fn failed(path: PathBuf, display: String, error: impl Into<String>) -> Self {
        Self {
            path,
            display,
            mode: Mode::Text,
            text: String::new(),
            lines: Vec::new(),
            tokens: None,
            image_bytes: None,
            bytes: 0,
            truncated: false,
            lossy: false,
            error: Some(error.into()),
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// Load a file for preview. Never returns `Err`: failures become a
/// [`FileContent`] carrying [`FileContent::error`], so the viewer always has
/// something honest to render.
pub fn load(path: &Path, display: impl Into<String>) -> FileContent {
    let display = display.into();
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => return FileContent::failed(path.to_path_buf(), display, error.to_string()),
    };
    if metadata.is_dir() {
        return FileContent::failed(path.to_path_buf(), display, "not a file");
    }

    let bytes = metadata.len();
    if bytes > MAX_FILE_BYTES {
        return FileContent {
            path: path.to_path_buf(),
            display,
            mode: Mode::TooLarge,
            text: String::new(),
            lines: Vec::new(),
            tokens: None,
            image_bytes: None,
            bytes,
            truncated: false,
            lossy: false,
            error: None,
        };
    }

    let Ok(raw) = std::fs::read(path) else {
        return FileContent::failed(path.to_path_buf(), display, "could not read file");
    };

    let name = display.rsplit('/').next().unwrap_or(&display);
    if is_image(name) {
        return FileContent {
            path: path.to_path_buf(),
            display,
            mode: Mode::Image,
            text: String::new(),
            lines: Vec::new(),
            tokens: None,
            image_bytes: Some(raw),
            bytes,
            truncated: false,
            lossy: false,
            error: None,
        };
    }

    if is_binary(&raw) {
        return FileContent {
            path: path.to_path_buf(),
            display,
            mode: Mode::Binary,
            text: String::new(),
            lines: Vec::new(),
            tokens: None,
            image_bytes: None,
            bytes,
            truncated: false,
            lossy: false,
            error: None,
        };
    }

    let decoded = String::from_utf8_lossy(&raw);
    let lossy = matches!(decoded, std::borrow::Cow::Owned(_));
    let text = decoded.into_owned();
    let mut lines: Vec<String> = text.split('\n').map(|line| line.to_string()).collect();
    // `split` yields a trailing empty line for a file ending in a newline;
    // drop it so the gutter does not show a phantom last line.
    if lines.last().is_some_and(String::is_empty) && text.ends_with('\n') {
        lines.pop();
    }
    let truncated = lines.len() > MAX_LINES;
    if truncated {
        lines.truncate(MAX_LINES);
    }

    let (mode, tokens) = if is_markdown(name) {
        (Mode::Markdown, None)
    } else if let Some(lang) = crate::review::language_for_path(name) {
        (Mode::Code { lang }, Some(highlight::tokenize(lang, &text)))
    } else {
        (Mode::Text, None)
    };

    FileContent {
        path: path.to_path_buf(),
        display,
        mode,
        text,
        lines,
        tokens,
        image_bytes: None,
        bytes,
        truncated,
        lossy,
        error: None,
    }
}

fn extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

pub fn is_markdown(path: &str) -> bool {
    matches!(extension(path).as_deref(), Some("md" | "markdown" | "mdx"))
}

pub fn is_image(path: &str) -> bool {
    matches!(
        extension(path).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "svg")
    )
}

/// The GPUI decoder for an image path, if any. `.ico`/`.avif` are recognized
/// by consumers but not decodable by gpui, so they fall through to the binary
/// notice rather than failing a decode.
pub(crate) fn image_format(path: &Path) -> Option<ImageFormat> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase())?;
    match extension(&name).as_deref() {
        Some("png") => Some(ImageFormat::Png),
        Some("jpg" | "jpeg") => Some(ImageFormat::Jpeg),
        Some("gif") => Some(ImageFormat::Gif),
        Some("webp") => Some(ImageFormat::Webp),
        Some("bmp") => Some(ImageFormat::Bmp),
        Some("tiff") => Some(ImageFormat::Tiff),
        Some("svg") => Some(ImageFormat::Svg),
        _ => None,
    }
}

/// Rewrite a tab's workspace-relative display path after `old_display` was
/// renamed to `new_display`: a tab exactly at the old path takes the new name,
/// one under it keeps its suffix (`src` → `src2` maps `src/a.rs` → `src2/a.rs`).
fn renamed_display(display: &str, old_display: &str, new_display: &str) -> String {
    let suffix = display.strip_prefix(old_display).unwrap_or(display);
    format!("{new_display}{suffix}")
}

/// A NUL byte in the leading window is the classic binary tell and is right
/// for source trees; a stray high byte is not enough to call something binary.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0)
}

/// Write `text` to `path` atomically: a sibling temp file, then a rename.
/// Falls back to a direct write when the directory will not take a temp file,
/// so a real error is still surfaced rather than swallowed.
fn save_file(path: &Path, text: &str) -> std::io::Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.orbit-save"));
    match std::fs::write(&tmp, text) {
        Ok(()) => match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(_) => {
                let result = std::fs::write(path, text);
                let _ = std::fs::remove_file(&tmp);
                result
            }
        },
        Err(_) => std::fs::write(path, text),
    }
}

// ── the GPUI entity ────────────────────────────────────────────────────

/// Autosave status for one editable tab.
#[derive(Clone, Debug)]
enum SaveState {
    /// Matches disk.
    Saved,
    /// Edits made since the last write.
    Unsaved,
    /// A write is in flight.
    Saving,
    /// The last write failed; the message is shown in the toolbar.
    Failed(String),
}

struct Tab {
    /// Stable id: observers and save timers outlive a shift in the tab vec.
    id: u64,
    path: PathBuf,
    display: String,
    /// Decoded image for the active file, when it is an image.
    image: Option<Arc<Image>>,
    /// Loaded content: the toolbar's size/line count and the read-only bodies.
    content: Option<FileContent>,
    /// The editable buffer. Always present; only rendered for editable modes.
    editor: Entity<ComposerInput>,
    /// Text last written to disk (or loaded), for dirty comparison.
    saved: String,
    /// Editor revision last seen, so a caret move is not taken for an edit.
    seen_revision: u64,
    dirty: bool,
    /// The file changed on disk while this tab had unsaved edits.
    conflict: bool,
    /// Markdown tabs default to the rendered preview; the toolbar toggles
    /// between it and the editable source.
    markdown_preview: bool,
    save: SaveState,
    /// Guards a late load from a previous request.
    generation: u64,
    _sub: Subscription,
}

impl Tab {
    fn editable(&self) -> bool {
        self.content.as_ref().is_some_and(FileContent::editable)
    }
}

/// The full-page Files surface: a tab strip over editable code/text files and
/// read-only Markdown/image/binary/oversized views.
///
/// Loading and saving always happen on the background executor; the entity
/// only ever paints already-loaded [`FileContent`] and an already-seeded
/// [`ComposerInput`].
pub struct FileViewer {
    open: bool,
    tabs: Vec<Tab>,
    active: usize,
    loading: bool,
    list: ListState,
    /// Leading inset the tab strip gives its first tab. The surface spans the
    /// window when the sessions sidebar is collapsed, so this clears the macOS
    /// traffic lights and the sidebar/history controls overlaid in the
    /// titlebar (mirrors the Git/Usage page headers).
    chrome_leading: f32,
    /// Reserve the top-right caption-controls inset (Windows only, and only
    /// while this surface is the window's rightmost column).
    reserve_controls: bool,
    /// Horizontal scroll of the tab strip, so a long run of open files stays
    /// reachable instead of being clipped off the right edge.
    tab_scroll: ScrollHandle,
    /// Carries the `Files` key context so `cmd-w` closes the surface.
    focus: FocusHandle,
    /// Focus the active editor on the next paint (`show` has no window).
    focus_pending: bool,
    next_id: u64,
    /// Debounce epoch; a newer edit supersedes the pending autosave.
    save_epoch: u64,
    /// The app's close callback (the toolbar's X).
    on_close: Rc<dyn Fn(&mut App)>,
}

impl FileViewer {
    pub fn new(on_close: Rc<dyn Fn(&mut App)>, cx: &mut Context<Self>) -> Self {
        Self {
            open: false,
            tabs: Vec::new(),
            active: 0,
            loading: false,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            chrome_leading: 12.,
            reserve_controls: false,
            tab_scroll: ScrollHandle::new(),
            focus: cx.focus_handle().tab_stop(true),
            focus_pending: false,
            next_id: 0,
            save_epoch: 0,
            on_close,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn active_path(&self) -> Option<PathBuf> {
        self.tabs.get(self.active).map(|tab| tab.path.clone())
    }

    pub fn active_display(&self) -> Option<String> {
        self.tabs.get(self.active).map(|tab| tab.display.clone())
    }

    /// Whether any open tab at or under `prefix` has unsaved edits. Rename and
    /// delete check this first, so a file operation can never race the autosave
    /// or silently discard a buffer.
    pub fn has_dirty_under(&self, prefix: &Path) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.path.starts_with(prefix) && tab.dirty)
    }

    /// A rename landed on disk: move every open tab at or under `old` to
    /// `new`, rewriting both the absolute path and the workspace-relative
    /// display the toolbar shows. `*_display` are the `/`-separated relative
    /// forms. Clean tabs only — the caller refused a dirty one before acting.
    pub fn reconcile_rename(
        &mut self,
        old: &Path,
        new: &Path,
        old_display: &str,
        new_display: &str,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for tab in self.tabs.iter_mut() {
            let Ok(suffix) = tab.path.strip_prefix(old) else {
                continue;
            };
            tab.path = new.join(suffix);
            tab.display = renamed_display(&tab.display, old_display, new_display);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// A delete landed on disk: drop every tab at or under `prefix` without
    /// writing. The caller refuses a dirty tab first, so nothing is lost.
    pub fn close_under(&mut self, prefix: &Path, cx: &mut Context<Self>) {
        let before = self.tabs.len();
        self.tabs.retain(|tab| !tab.path.starts_with(prefix));
        if self.tabs.len() == before {
            return;
        }
        if self.tabs.is_empty() {
            self.open = false;
            self.list.reset(0);
            self.loading = false;
        } else {
            self.active = self.active.min(self.tabs.len() - 1);
            self.focus_pending = true;
            self.sync_list();
        }
        cx.notify();
    }

    /// Open (or focus) a file. A file already open in a tab keeps its buffer
    /// and unsaved edits; only a new tab is loaded from disk.
    pub fn show(&mut self, path: PathBuf, display: String, cx: &mut Context<Self>) {
        self.open = true;
        self.focus_pending = true;
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.active = index;
            cx.notify();
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let editor = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder("")
                .with_gutter(true)
                .with_wrap(false)
                .with_fill(true)
                .with_key_context("Editor Files")
        });
        let sub = cx.observe(&editor, move |this, editor, cx| {
            this.on_editor_changed(id, &editor, cx);
        });
        self.tabs.push(Tab {
            id,
            path,
            display,
            image: None,
            content: None,
            editor,
            saved: String::new(),
            seen_revision: 0,
            dirty: false,
            conflict: false,
            markdown_preview: true,
            save: SaveState::Saved,
            generation: 0,
            _sub: sub,
        });
        self.active = self.tabs.len() - 1;
        self.read(self.active, cx);
        cx.notify();
    }

    /// Leave the surface, flushing any unsaved edits first.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.flush_all(cx);
        self.open = false;
        cx.notify();
    }

    pub fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        let id = self.tabs[index].id;
        self.flush(id, cx);
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.open = false;
            self.list.reset(0);
            self.loading = false;
        } else {
            // Removing a tab before the active one shifts it down; closing the
            // active one falls to the next tab (or the previous, at the end).
            self.active = active_after_close(self.active, index, self.tabs.len());
            self.focus_pending = true;
            self.sync_list();
        }
        cx.notify();
    }

    /// The workspace watcher fired: re-read the active file. The load
    /// completion tells our own save (which echoes back through the watcher)
    /// apart from a real external change, so uncommitted edits are never
    /// clobbered and a self-write never flashes a conflict.
    pub fn reload(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.path == path) else {
            return;
        };
        if index != self.active {
            return;
        }
        self.read(index, cx);
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        self.focus_pending = true;
        self.sync_list();
        cx.notify();
    }

    /// Keep the read-only list's item count in step with the active tab.
    fn sync_list(&mut self) {
        let count = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.content.as_ref())
            .map_or(0, FileContent::line_count);
        if count != self.list.item_count() {
            self.list.reset(count);
        }
    }

    fn read(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        tab.generation = tab.generation.wrapping_add(1);
        let generation = tab.generation;
        let id = tab.id;
        let path = tab.path.clone();
        let display = tab.display.clone();
        // A reload keeps the old content on screen until the new bytes land
        // (no spinner flash on every autosave); only a first load blanks it.
        let first_load = tab.content.is_none();
        if first_load {
            tab.image = None;
            self.loading = true;
            self.list.reset(0);
        }
        cx.spawn(async move |this, cx| {
            let content = cx
                .background_executor()
                .spawn(async move { load(&path, display) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(index) = this
                    .tabs
                    .iter()
                    .position(|tab| tab.id == id && tab.generation == generation)
                else {
                    return;
                };
                let image = if content.mode == Mode::Image {
                    content.image_bytes.as_deref().and_then(|bytes| {
                        image_format(&content.path)
                            .map(|format| Arc::new(Image::from_bytes(format, bytes.to_vec())))
                    })
                } else {
                    None
                };
                this.loading = false;
                this.tabs[index].image = image;
                this.tabs[index].content = Some(content);
                this.sync_list();
                this.seed_editor(index, cx);
                // A first load can land while the editor was still absent (the
                // loading spinner had focus); claim focus for the new file.
                if first_load && index == this.active {
                    this.focus_pending = true;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Copy loaded text into a tab's editor and reconcile its save state.
    /// Read-only modes leave the buffer alone (it is never rendered).
    fn seed_editor(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((editable, lang, text)) = self.tabs.get(index).and_then(|tab| {
            let content = tab.content.as_ref()?;
            let lang = match content.mode {
                Mode::Code { lang } => Some(lang),
                _ => None,
            };
            Some((content.editable(), lang, content.text.clone()))
        }) else {
            return;
        };
        if !editable {
            return;
        }
        let editor = self.tabs[index].editor.clone();
        editor.update(cx, |editor, cx| editor.set_syntax(lang, cx));
        let current = editor.read(cx).text();
        let revision = editor.read(cx).revision();

        {
            let tab = &mut self.tabs[index];
            // Did the bytes on disk change since we last read or wrote them?
            let external = text != tab.saved;
            tab.saved = text.clone();
            if current == text {
                // Fresh empty file, or the watcher echoing our own save. Keep
                // the buffer — and its caret and scroll — untouched.
                tab.seen_revision = revision;
                tab.dirty = false;
                tab.conflict = false;
                tab.save = SaveState::Saved;
                return;
            }
            if tab.dirty {
                // Uncommitted edits win; a pending write of ours is not a
                // conflict.
                tab.conflict = external;
                return;
            }
            if !external {
                return;
            }
        }
        // Clean buffer, real external change: replace it from disk.
        editor.update(cx, |editor, cx| editor.set_text_at_start(text, cx));
        let revision = editor.read(cx).revision();
        let tab = &mut self.tabs[index];
        tab.seen_revision = revision;
        tab.dirty = false;
        tab.conflict = false;
        tab.save = SaveState::Saved;
    }

    /// An edit landed in a tab's editor: mark it dirty and (re)arm autosave.
    fn on_editor_changed(
        &mut self,
        id: u64,
        editor: &Entity<ComposerInput>,
        cx: &mut Context<Self>,
    ) {
        let revision = editor.read(cx).revision();
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if self.tabs[index].seen_revision == revision {
            return;
        }
        self.tabs[index].seen_revision = revision;
        self.tabs[index].dirty = true;
        self.tabs[index].save = SaveState::Unsaved;
        self.schedule_save(cx);
        cx.notify();
    }

    /// (Re)arm the autosave debounce. It flushes *all* dirty tabs when it
    /// fires — not just the tab that was edited — so moving to another tab
    /// within the window cannot cancel a pending save.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_epoch = self.save_epoch.wrapping_add(1);
        let epoch = self.save_epoch;
        cx.spawn(async move |this, cx| {
            Timer::after(AUTOSAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.save_epoch == epoch {
                    this.flush_all(cx);
                }
            });
        })
        .detach();
    }

    fn flush_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<u64> = self
            .tabs
            .iter()
            .filter(|tab| tab.dirty)
            .map(|tab| tab.id)
            .collect();
        for id in ids {
            self.flush(id, cx);
        }
    }

    /// Write one tab's editor to disk on the background executor. No-op when
    /// the tab is clean or gone.
    fn flush(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if !self.tabs[index].dirty {
            return;
        }
        let path = self.tabs[index].path.clone();
        let text = self.tabs[index].editor.read(cx).text();
        self.tabs[index].saved = text.clone();
        self.tabs[index].dirty = false;
        self.tabs[index].conflict = false;
        self.tabs[index].save = SaveState::Saving;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { save_file(&path, &text) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.save = match result {
                        Ok(()) => SaveState::Saved,
                        Err(error) => {
                            tab.dirty = true;
                            SaveState::Failed(error.to_string())
                        }
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── rendering ──────────────────────────────────────────────────────

    fn tab_strip(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        // The tabs live in their own horizontally scrollable row: a long run of
        // open files stays reachable (trackpad / shift-wheel) instead of
        // clipping off the right edge. The close button stays pinned outside it.
        let mut tabs = div()
            .id("viewer-tabs")
            .h_full()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll);
        for (index, tab) in self.tabs.iter().enumerate() {
            let active = index == self.active;
            let name = tab
                .display
                .rsplit('/')
                .next()
                .unwrap_or(&tab.display)
                .to_string();
            let nerd = nerd_font_family(cx);
            let dark = theme.mode == ThemeMode::Dark;
            let fallback = file_badge(&tab.display, theme);
            let glyph = file_glyph(
                &tab.display,
                dark,
                nerd.as_ref(),
                IconSize::XSmall.px(&theme),
                fallback,
            );
            tabs = tabs.child(
                press(
                    div()
                        .id(gpui::ElementId::Name(format!("viewer-tab-{index}").into()))
                        .h(ButtonSize::Medium.height(&theme))
                        .max_w(px(220.))
                        // Never shrink below a readable width: past the strip's
                        // edge the row scrolls instead of crushing every tab.
                        .flex_none()
                        .px(DynamicSpacing::Base08.px(&theme))
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .rounded(Radius::Large.px(&theme))
                        .cursor_pointer()
                        .text_size(TextSize::Small.px(&theme))
                        .when(active, |el| el.bg(theme.surface_active))
                        .when(!active, |el| {
                            el.text_color(theme.text_muted).hover(|el| {
                                el.bg(theme.surface_hover).text_color(theme.text_secondary)
                            })
                        }),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.activate(index, cx);
                    // Reveal the tab the user just activated if the strip has
                    // scrolled.
                    this.tab_scroll.scroll_to_item(index);
                }))
                .child(glyph)
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
                        .child(name),
                )
                .when(tab.dirty, |el| {
                    el.child(
                        div()
                            .size(DynamicSpacing::Base06.px(&theme))
                            .flex_none()
                            .rounded_full()
                            .bg(theme.accent),
                    )
                })
                .child(
                    icon_button_frame(
                        div().id(gpui::ElementId::Name(
                            format!("viewer-tab-close-{index}").into(),
                        )),
                        &theme,
                        ButtonSize::None,
                    )
                    .tip(tr!("explorer.close_tab"))
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.surface_hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.close_tab(index, cx);
                        // Click handlers bubble: without this the tab's
                        // own `on_click` below would also run with the
                        // now-stale `index` and index past the shrunken
                        // tab list (out-of-bounds panic).
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
                ),
            );
        }
        div()
            .h(px(40.))
            .flex_none()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            // The surface spans the window when the sidebar is collapsed, so
            // the leading inset keeps the first tab clear of the macOS traffic
            // lights and the overlaid titlebar controls; the trailing inset
            // clears the app's own caption buttons where it draws them.
            .pl(px(self.chrome_leading))
            .pr(px(if self.reserve_controls {
                crate::platform::WINDOW_CONTROLS_W
            } else {
                f32::from(DynamicSpacing::Base16.px(&theme))
            }))
            .border_b_1()
            .border_color(theme.border)
            .child(tabs.flex_1().min_w_0())
            .child(
                icon_button_frame(div().id("viewer-close"), &theme, ButtonSize::Default)
                    .tip(tr!("common.close"))
                    .cursor_pointer()
                    .hover(|el| el.bg(theme.surface_hover))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        // Close here — this listener already holds the viewer's
                        // lease, so the app callback must not re-enter `update`
                        // on this entity (that would double-lease and abort).
                        let on_close = this.on_close.clone();
                        this.hide(cx);
                        on_close(cx);
                    }))
                    .child(icon(
                        "icons/x.svg",
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_muted,
                    )),
            )
            .into_any_element()
    }

    fn toolbar(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(tab) = self.tabs.get(self.active) else {
            return div().h(px(34.)).flex_none().into_any_element();
        };
        let Some(content) = tab.content.as_ref() else {
            return div().h(px(34.)).flex_none().into_any_element();
        };
        let size = format_bytes(content.bytes);
        let lines = tr!("explorer.lines", count = content.line_count());
        div()
            .h(px(34.))
            .flex_none()
            .px(DynamicSpacing::Base16.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .border_b_1()
            .border_color(theme.border)
            .font_family(theme::code_font_family())
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(theme.text_secondary)
                    .child(content.display.clone()),
            )
            .child(div().flex_none().child(size))
            .when(
                content.mode != Mode::Binary && content.mode != Mode::TooLarge,
                |el| el.child(div().flex_none().child(lines)),
            )
            .when(content.truncated, |el| {
                el.child(
                    div()
                        .flex_none()
                        .text_color(theme.warning)
                        .child(tr!("explorer.truncated")),
                )
            })
            .when(content.mode == Mode::Markdown && content.editable(), |el| {
                el.child(
                    button_frame(div().id("viewer-md-toggle"), &theme, ButtonSize::Compact)
                        .raised(theme.surface_elevated, &theme)
                        .border_1()
                        .border_color(theme.border)
                        .font_family(theme::ui_font_family())
                        .cursor_pointer()
                        .text_color(theme.text_secondary)
                        .hover(|el| el.raised(theme.surface_hover, &theme))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.toggle_markdown_preview(cx)
                        }))
                        .child(if tab.markdown_preview {
                            tr!("explorer.edit")
                        } else {
                            tr!("explorer.preview")
                        }),
                )
            })
            .child(self.status_chip(theme, tab))
            .into_any_element()
    }

    /// Read-only lock for non-editable modes; autosave status for editable ones.
    fn status_chip(&self, theme: Theme, tab: &Tab) -> AnyElement {
        let (label, color) = if !tab.editable() {
            (tr!("explorer.read_only"), theme.text_muted)
        } else if tab.conflict {
            (tr!("explorer.changed_on_disk"), theme.warning)
        } else {
            match &tab.save {
                SaveState::Saved => (tr!("explorer.saved"), theme.text_muted),
                SaveState::Unsaved => (tr!("explorer.unsaved"), theme.text_secondary),
                SaveState::Saving => (tr!("explorer.saving"), theme.text_muted),
                SaveState::Failed(error) => (
                    format!("{}: {error}", tr!("explorer.save_failed")),
                    theme.warning,
                ),
            }
        };
        div()
            .flex_none()
            .h(px(20.))
            .px(DynamicSpacing::Base06.px(&theme))
            .rounded(Radius::Small.px(&theme))
            .bg(theme.surface_elevated)
            .border_1()
            .border_color(theme.border)
            .font_family(theme::ui_font_family())
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base04.px(&theme))
            .when(!tab.editable(), |el| {
                el.child(icon(
                    "icons/lock.svg",
                    IconSize::Indicator.px(&theme),
                    theme.text_muted,
                ))
            })
            .child(div().text_color(color).child(label))
            .into_any_element()
    }

    fn body(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.loading {
            return loading_state(&theme);
        }
        let Some(tab) = self.tabs.get(self.active) else {
            let detail = tr!("explorer.select_file_detail");
            return empty_state(
                theme,
                "icons/file.svg",
                &tr!("explorer.select_file"),
                Some(detail.as_str()),
                EmptyFill::Grow,
            );
        };
        let Some(content) = tab.content.as_ref() else {
            let detail = tr!("explorer.select_file_detail");
            return empty_state(
                theme,
                "icons/file.svg",
                &tr!("explorer.select_file"),
                Some(detail.as_str()),
                EmptyFill::Grow,
            );
        };
        if let Some(error) = &content.error {
            return empty_state(
                theme,
                "icons/stop.svg",
                &tr!("explorer.read_error"),
                Some(error.as_str()),
                EmptyFill::Grow,
            );
        }
        match content.mode {
            // Editable: the buffer is the body (empty files included, so the
            // user can type into a new file).
            Mode::Code { .. } | Mode::Text if content.editable() => editor_body(theme, tab),
            // Markdown gets an editor too, with a Preview toggle.
            Mode::Markdown if content.editable() && !tab.markdown_preview => {
                editor_body(theme, tab)
            }
            // Truncated / non-UTF-8 text: read-only, since saving the buffer
            // would not reproduce the file's real bytes.
            Mode::Code { .. } | Mode::Text => self.text_body(theme, cx),
            Mode::Markdown => {
                // Render the live buffer so edits show up in the preview.
                let text = if content.editable() {
                    tab.editor.read(cx).text()
                } else {
                    content.lines.join("\n")
                };
                div()
                    .id("viewer-markdown")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(760.))
                            .mx_auto()
                            .p(DynamicSpacing::Base20.px(&theme))
                            .child(crate::transcript_view::render_markdown_document(
                                &text, theme,
                            )),
                    )
                    .into_any_element()
            }
            Mode::Image => match tab.image.clone() {
                Some(image) => div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(DynamicSpacing::Base24.px(&theme))
                    .child(
                        img(ImageSource::Image(image))
                            .max_w_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .into_any_element(),
                None => empty_state(
                    theme,
                    "icons/file.svg",
                    &tr!("explorer.binary"),
                    None,
                    EmptyFill::Grow,
                ),
            },
            Mode::Binary => {
                let detail = tr!("explorer.binary_detail");
                empty_state(
                    theme,
                    "icons/file.svg",
                    &tr!("explorer.binary"),
                    Some(detail.as_str()),
                    EmptyFill::Grow,
                )
            }
            Mode::TooLarge => {
                let detail = tr!("explorer.too_large", size = format_bytes(content.bytes));
                empty_state(
                    theme,
                    "icons/file.svg",
                    &tr!("explorer.too_large_title"),
                    Some(detail.as_str()),
                    EmptyFill::Grow,
                )
            }
        }
    }

    fn text_body(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity().downgrade();
        let list = gpui::list(self.list.clone(), move |index, _window, cx| {
            entity
                .upgrade()
                .map(|entity| entity.update(cx, |this, cx| this.render_line(index, cx)))
                .unwrap_or_else(|| div().into_any_element())
        })
        .size_full();
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .font_family(theme::code_font_family())
            .text_size(theme.code_px(12.5))
            .line_height(theme.code_px(18.))
            .child(list)
            .into_any_element()
    }

    fn render_line(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(content) = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.content.as_ref())
        else {
            return div().into_any_element();
        };
        let Some(line) = content.lines.get(index) else {
            return div().into_any_element();
        };
        let theme = *theme::get(cx);
        let tokens = content
            .tokens
            .as_ref()
            .and_then(|tokens| tokens.get(index))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let gutter_width = content.lines.len().to_string().len().max(3) as f32 * 7.0 + 16.0;
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_start()
            .child(
                div()
                    .w(px(gutter_width))
                    .flex_none()
                    .pr(px(9.))
                    .flex()
                    .justify_end()
                    .border_r_1()
                    .border_color(theme.border)
                    .text_color(theme.text_muted)
                    .child((index + 1).to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl(px(10.))
                    .pr(DynamicSpacing::Base12.px(&theme))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(line_text(line, tokens, theme)),
            )
            .into_any_element()
    }

    /// `cmd-s`: write the active tab (and any other dirty tab) now.
    fn on_save_active(&mut self, _: &crate::SaveFile, _: &mut Window, cx: &mut Context<Self>) {
        self.flush_all(cx);
        cx.notify();
    }

    /// `cmd-w`: close the active file tab, flushing it first. Closing the last
    /// tab closes the whole surface.
    fn on_close_tab_action(
        &mut self,
        _: &crate::CloseFileTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.is_empty() {
            return;
        }
        let on_close = self.on_close.clone();
        self.close_tab(self.active, cx);
        if !self.open {
            on_close(cx);
        }
        cx.notify();
    }

    /// Toggle a Markdown tab between the rendered preview and its source.
    fn toggle_markdown_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.markdown_preview = !tab.markdown_preview;
        }
        // Re-resolve focus either way: the editor appears/disappears, so the
        // target is the editor in edit mode and the viewer root in preview.
        self.focus_pending = true;
        cx.notify();
    }
}

fn loading_state(theme: &Theme) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .gap(DynamicSpacing::Base08.px(theme))
        .child(crate::app::spinner(
            "viewer-loading",
            IconSize::Small.px(theme),
            theme.text_muted,
            *theme,
        ))
        .child(
            div()
                .text_size(TextSize::Small.px(theme))
                .text_color(theme.text_muted)
                .child(tr!("explorer.loading")),
        )
        .into_any_element()
}

fn mono_font() -> Font {
    Font {
        family: theme::code_font_family(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

fn run(len: usize, color: Hsla, font: &Font) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Build syntax-colored text for one line. Paint-only: the tokens were
/// computed off-thread when the file loaded.
fn line_text(text: &str, tokens: &[Token], theme: Theme) -> StyledText {
    let font = mono_font();
    let base = theme.text_secondary;
    if tokens.is_empty() {
        return StyledText::new(text.to_string()).with_runs(vec![run(text.len(), base, &font)]);
    }
    let mut runs: Vec<TextRun> = Vec::new();
    let mut offset = 0usize;
    for token in tokens {
        let start = token.range.start.min(text.len());
        let end = token.range.end.min(text.len());
        if start > offset {
            runs.push(run(start - offset, base, &font));
        }
        if end > start {
            runs.push(run(end - start, theme.token_color(token.class), &font));
        }
        offset = offset.max(end);
    }
    if offset < text.len() {
        runs.push(run(text.len() - offset, base, &font));
    }
    if runs.is_empty() {
        runs.push(run(text.len(), base, &font));
    }
    StyledText::new(text.to_string()).with_runs(runs)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The tab index that becomes active after removing `index` from a list that
/// had `active` selected and now has `len_after` tabs (`len_after >= 1`).
fn active_after_close(active: usize, index: usize, len_after: usize) -> usize {
    if index < active {
        active - 1
    } else {
        active.min(len_after - 1)
    }
}

/// Whether a tab currently paints its editor, as opposed to a read-only body
/// or the Markdown preview. Used to pick a focus target that exists in the
/// tree: focusing a hidden editor would drop focus and the `Files` context.
fn editor_shown(tab: &Tab) -> bool {
    let Some(content) = tab.content.as_ref() else {
        return false;
    };
    match content.mode {
        Mode::Code { .. } | Mode::Text => content.editable(),
        Mode::Markdown => content.editable() && !tab.markdown_preview,
        _ => false,
    }
}

/// The Explorer's editing surface. A column so the editor's `flex_1` grows
/// vertically; a row would leave its height auto and collapse the buffer to 0.
fn editor_body(theme: Theme, tab: &Tab) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .font_family(theme::code_font_family())
        .text_size(theme.code_px(12.5))
        .line_height(theme.code_px(18.))
        .child(tab.editor.clone())
        .into_any_element()
}

impl Render for FileViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        if self.focus_pending {
            let viewer = cx.entity();
            // Claim focus after this frame's effects have settled: the target
            // depends on the active tab actually rendering an editor, which
            // only holds once the frame is built.
            window.defer(cx, move |window, cx| {
                let target = viewer.update(cx, |viewer, cx| {
                    if !viewer.focus_pending {
                        return None;
                    }
                    viewer.focus_pending = false;
                    let target = viewer
                        .tabs
                        .get(viewer.active)
                        .filter(|tab| editor_shown(tab))
                        .map(|tab| tab.editor.read(cx).focus_handle(cx))
                        .unwrap_or_else(|| viewer.focus.clone());
                    cx.notify();
                    Some(target)
                });
                if let Some(target) = target {
                    window.focus(&target);
                }
            });
        }
        let theme = *theme::get(cx);
        div()
            .id("file-viewer")
            .key_context("Files")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_close_tab_action))
            .on_action(cx.listener(Self::on_save_active))
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.background)
            .child(self.tab_strip(theme, cx))
            .child(self.toolbar(theme, cx))
            .child(self.body(theme, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("orbit-explorer-viewer-tests");
        std::fs::create_dir_all(&dir).unwrap();
        // Per-test file name keeps parallel tests from sharing a path.
        dir.join(format!("{}-{name}", std::process::id()))
    }

    #[test]
    fn loads_code_with_tokens_and_drops_the_trailing_blank_line() {
        let path = temp("sample.rs");
        std::fs::write(&path, "fn main() {\n    println!(\"hi\");\n}\n").unwrap();
        let content = load(&path, "sample.rs");
        assert!(matches!(content.mode, Mode::Code { lang: Lang::Rust }));
        assert_eq!(content.lines.len(), 3);
        assert!(content.tokens.is_some());
        assert!(content.error.is_none());
        assert!(content.editable());
        // The exact text is preserved for the editor seed.
        assert!(content.text.ends_with("}\n"));
    }

    #[test]
    fn detects_markdown() {
        let path = temp("notes.md");
        std::fs::write(&path, "# Title\n\nbody\n").unwrap();
        let content = load(&path, "notes.md");
        assert_eq!(content.mode, Mode::Markdown);
        assert!(content.tokens.is_none());
        assert_eq!(content.lines[0], "# Title");
        // Markdown opens in the editor as source; the toolbar Preview toggle
        // still renders it.
        assert!(content.editable());
    }

    #[test]
    fn detects_binary_from_a_nul_byte() {
        let path = temp("blob.bin");
        std::fs::write(&path, [0u8, 1, 2, 3, 255]).unwrap();
        let content = load(&path, "blob.bin");
        assert_eq!(content.mode, Mode::Binary);
        assert!(content.lines.is_empty());
    }

    #[test]
    fn oversized_files_are_not_read() {
        let path = temp("big.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(file);
        let content = load(&path, "big.txt");
        assert_eq!(content.mode, Mode::TooLarge);
        assert!(content.lines.is_empty());
        assert_eq!(content.bytes, MAX_FILE_BYTES + 1);
    }

    #[test]
    fn invalid_utf8_is_decoded_lossily_not_rejected() {
        let path = temp("latin1.txt");
        std::fs::write(&path, [b'c', b'a', b'f', 0xE9, b'\n']).unwrap();
        let content = load(&path, "latin1.txt");
        assert_eq!(content.mode, Mode::Text);
        assert!(content.lines[0].starts_with("caf"));
        // Lossy decode stays read-only so a save cannot rewrite the bytes.
        assert!(!content.editable());
    }

    #[test]
    fn directories_report_an_error_instead_of_panicking() {
        let dir = std::env::temp_dir().join("orbit-explorer-viewer-tests/dir");
        std::fs::create_dir_all(&dir).unwrap();
        let content = load(&dir, "dir");
        assert!(content.error.is_some());
    }

    #[test]
    fn image_extensions_are_recognized() {
        assert!(is_image("a.png"));
        assert!(is_image("dir/B.JPEG"));
        assert!(!is_image("a.rs"));
        assert!(is_markdown("README.MD"));
    }

    /// A rename must rewrite the toolbar path for a tab at the old path and
    /// for every tab beneath a renamed directory.
    #[test]
    fn rename_rewrites_display_for_a_file_and_a_subtree() {
        assert_eq!(
            renamed_display("src/old.rs", "src/old.rs", "src/new.rs"),
            "src/new.rs"
        );
        assert_eq!(renamed_display("src/foo.rs", "src", "src2"), "src2/foo.rs");
        assert_eq!(renamed_display("main.rs", "main.rs", "app.rs"), "app.rs");
    }

    #[test]
    fn active_tab_after_close_is_a_neighbour() {
        // Closing the active tab falls to the next tab, or the previous at the
        // end of the strip.
        assert_eq!(active_after_close(0, 0, 1), 0);
        assert_eq!(active_after_close(1, 1, 2), 1);
        assert_eq!(active_after_close(2, 2, 2), 1);
        // Closing a tab before the active one shifts it down.
        assert_eq!(active_after_close(2, 0, 2), 1);
        assert_eq!(active_after_close(1, 0, 2), 0);
        // Closing a tab after the active one leaves it put.
        assert_eq!(active_after_close(0, 1, 1), 0);
    }

    #[test]
    fn truncated_and_lossy_content_is_not_editable() {
        let mut content = FileContent {
            path: PathBuf::from("x"),
            display: "x".into(),
            mode: Mode::Text,
            text: "x".into(),
            lines: vec!["x".into()],
            tokens: None,
            image_bytes: None,
            bytes: 1,
            truncated: true,
            lossy: false,
            error: None,
        };
        assert!(!content.editable(), "a truncated buffer must not save");
        content.truncated = false;
        content.lossy = true;
        assert!(!content.editable(), "a lossy buffer must not save");
        content.lossy = false;
        assert!(content.editable());
    }

    #[test]
    fn save_file_round_trips_and_is_atomic() {
        let path = temp("save-roundtrip.txt");
        std::fs::write(&path, "before\n").unwrap();
        save_file(&path, "after\nsecond\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\nsecond\n");
        // The temp sibling is renamed away, not left behind.
        assert!(!path
            .with_file_name(format!(
                ".{}.orbit-save",
                path.file_name().unwrap().to_str().unwrap()
            ))
            .exists());
    }

    #[test]
    fn save_file_surfaces_a_missing_directory_error() {
        let path = std::env::temp_dir()
            .join("orbit-explorer-viewer-tests/does-not-exist")
            .join("file.txt");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert!(save_file(&path, "x").is_err());
    }

    /// A long run of open files must scroll horizontally instead of clipping
    /// off the right edge (the reported overflow). The strip keeps the last
    /// tabs reachable by giving the row a positive horizontal scroll range.
    #[gpui::test]
    fn tab_strip_scrolls_when_tabs_overflow(cx: &mut gpui::TestAppContext) {
        use gpui::{point, size};

        let cx = cx.add_empty_window();
        cx.update(|_, app| crate::theme::init(app));
        let viewer = cx.new(|cx| FileViewer::new(Rc::new(|_| {}), cx));
        viewer.update(cx, |viewer, cx| {
            for index in 0..12 {
                viewer.show(
                    PathBuf::from(format!("/tmp/orbit-viewer-tab-{index}.rs")),
                    format!("file-{index}.rs"),
                    cx,
                );
            }
        });
        let _ = cx.draw(point(px(0.), px(0.)), size(px(420.), px(600.)), |_, _| {
            viewer.clone()
        });
        let max = cx.update(|_, app| viewer.read(app).tab_scroll.max_offset().width);
        assert!(
            max > px(0.),
            "the strip must scroll when 12 tabs overflow a 420px window, max offset {max:?}"
        );
    }
}
