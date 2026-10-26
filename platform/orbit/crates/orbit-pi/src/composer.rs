//! Multi-line composer input: text wraps, the editor grows to
//! `MAX_LINES` and then scrolls internally, `Enter` submits, `Shift+Enter`
//! inserts a newline, and ↑/↓ move the caret between visual rows.
//!
//! Built on gpui's `shape_text`/`WrappedLine` (the 0.2.2 text system caches
//! shaped layouts, so re-shaping on keystrokes is cheap). IME composition is
//! tracked as a marked range: preedit updates replace the span in place and
//! commits/cancellations clear it (see `EntityInputHandler` below).

use std::ops::Range;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

use gpui::{
    div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardEntry, ClipboardItem,
    ContentMask, Context, CursorStyle, Element, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, Image, InspectorElementId, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, ScrollWheelEvent,
    SharedString, Style, Task, TextAlign, TextRun, Timer, UTF16Selection, Window, WrappedLine,
};

use crate::{
    highlight::{self, Lang},
    mentions::{detect_trigger, tokenize_mentions, MentionKind, SharedAutocomplete, Trigger},
    theme, Backspace, Copy, Cut, Delete, Down, End, Home, Left, LineLeft, LineRight, Newline,
    Paste, Redo, Right, SelectAll, SelectLeft, SelectLineLeft, SelectLineRight, SelectRight,
    SelectWordLeft, SelectWordRight, Undo, Up, WordLeft, WordRight,
};

/// Visual rows the editor grows to before it scrolls internally.
const MAX_LINES: usize = 8;

/// Idle beat after an edit or caret move before the caret starts blinking —
/// the caret holds solid while typing, the way a native field behaves.
const BLINK_RESUME_DELAY: Duration = Duration::from_millis(300);
/// Caret on/off half-period (a ~1s cycle).
const BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// Undo history bounds: at most this many snapshots, and this many bytes of
/// buffer across them. A whole-file editor produces large snapshots, so the
/// byte budget matters more than the count.
const UNDO_LIMIT: usize = 128;
const UNDO_BYTES: usize = 16 * 1024 * 1024;
/// Edits within this window coalesce into one undo step (a typing run).
const UNDO_COALESCE: Duration = Duration::from_millis(400);

/// One undo point: the buffer plus the caret/selection to restore.
#[derive(Clone)]
struct Snapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

pub struct ComposerInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    /// True while the field still uses the shared default placeholder, which
    /// is resolved at paint time so a language change is picked up live.
    placeholder_is_default: bool,
    /// Translation key for the placeholder, resolved at paint time (like the
    /// default placeholder) so switching the interface language updates the
    /// field without rebuilding it. Preferred over [`Self::with_placeholder`]
    /// for user-facing copy; `with_placeholder` stays for literal text such
    /// as `sk-…` or a URL example.
    placeholder_key: Option<SharedString>,
    /// Named `%{…}` values substituted into [`Self::placeholder_key`] at paint
    /// time. Empty for keys without interpolation.
    placeholder_vars: Vec<(SharedString, SharedString)>,
    /// Element id used in `render`. Defaults to `composer-input`; form
    /// fields override it so several inputs can coexist as siblings.
    element_id: SharedString,
    /// Key context flags for this input (space separated). Defaults to
    /// `Composer`; the model picker's filter input adds a `Picker` flag so
    /// the picker's enter/escape/arrow bindings can take precedence at the
    /// same dispatch depth.
    key_context: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    is_selecting: bool,
    /// Active IME composition span (byte offsets into `content`), or `None`
    /// outside a composition. Platform composition updates carry ranges
    /// relative to the composition text and often pass no replacement range,
    /// so the previous preedit span must be remembered to replace it in
    /// place instead of appending a new copy each keystroke.
    marked_range: Option<Range<usize>>,
    /// Layout snapshot from the last prepaint, for hit-testing and IME
    /// bounds outside the paint pass.
    last_lines: Vec<WrappedLine>,
    /// Byte offset of each logical line's first byte (`WrappedLine::len`
    /// excludes the newline that `shape_text` split off, hence +1 steps).
    last_line_starts: Vec<usize>,
    last_line_height: Pixels,
    last_bounds: Option<Bounds<Pixels>>,
    last_wrap_width: Option<Pixels>,
    /// Visual rows across all logical lines at the last wrap width.
    last_total_rows: usize,
    /// Vertical scroll in content pixels (0 until the editor exceeds
    /// `max_lines` rows).
    scroll_offset: Pixels,
    /// Caret byte offset at the last prepaint. The caret is only pulled back
    /// into view when it actually moves; a mouse-wheel scroll leaves it put,
    /// so the offset the wheel set is not snapped back on the next frame.
    last_caret: usize,
    max_lines: usize,
    /// Shared `/`-command and `@`-mention menu state. When the menu is
    /// open, ↑/↓ move the highlight instead of the caret (Enter/Escape are
    /// intercepted by the app, which owns those actions).
    autocomplete: Option<SharedAutocomplete>,
    /// Images pasted (or attached) since the app last drained them — the
    /// app turns these into message attachments.
    pub pasted_images: Vec<Image>,
    /// Caret blink: whether the caret paints this frame, the epoch that
    /// invalidates superseded timers, and the pending timer task (dropped
    /// when a newer wake replaces it, which cancels it).
    caret_visible: bool,
    blink_active: bool,
    blink_epoch: usize,
    _blink_task: Option<Task<()>>,
    /// Content + selection at the last render, so an edit or caret move can
    /// hold the caret solid for a beat before it resumes blinking.
    blink_content: String,
    blink_selection: Range<usize>,
    /// When set, the editor paints syntax colors for `lang` (the Explorer's
    /// code files). `None` keeps the plain/mention ink.
    syntax: Option<Lang>,
    /// Paint a line-number gutter (the Explorer's editor).
    gutter: bool,
    /// Wrap long lines. The Explorer disables it so one logical line is one
    /// visual row and the gutter stays aligned.
    wrap: bool,
    /// Fill the parent's height and scroll internally, instead of auto-growing
    /// to `max_lines`.
    fill: bool,
    /// Bumped on every text mutation. Observers use it to tell an edit from a
    /// caret move or blink without cloning the buffer on each notify.
    revision: u64,
    /// Gutter width from the last prepaint, for mouse hit-testing.
    last_gutter: Pixels,
    /// Undo/redo history. Bounded by count and total bytes so a whole-file
    /// buffer cannot balloon memory.
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    undo_bytes: usize,
    /// Timestamp of the last recorded edit, for coalescing a typing run into
    /// one undo step. `None` after a caret move or undo/redo.
    last_edit: Option<Instant>,
}

impl ComposerInput {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx_focus_handle(_cx),
            content: String::new(),
            placeholder: "".into(),
            placeholder_is_default: true,
            placeholder_key: None,
            placeholder_vars: Vec::new(),
            element_id: "composer-input".into(),
            key_context: "Composer".into(),
            selected_range: 0..0,
            selection_reversed: false,
            is_selecting: false,
            marked_range: None,
            last_lines: Vec::new(),
            last_line_starts: Vec::new(),
            last_line_height: px(18.),
            last_bounds: None,
            last_wrap_width: None,
            last_total_rows: 1,
            scroll_offset: px(0.),
            last_caret: 0,
            max_lines: MAX_LINES,
            autocomplete: None,
            pasted_images: Vec::new(),
            caret_visible: true,
            blink_active: false,
            blink_epoch: 0,
            _blink_task: None,
            blink_content: String::new(),
            blink_selection: 0..0,
            syntax: None,
            gutter: false,
            wrap: true,
            fill: false,
            revision: 0,
            last_gutter: px(0.),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_bytes: 0,
            last_edit: None,
        }
    }

    /// Override the placeholder with literal text (not translated). Prefer
    /// [`Self::with_placeholder_key`] for any copy a translator should see.
    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self.placeholder_key = None;
        self.placeholder_vars.clear();
        self.placeholder_is_default = false;
        self
    }

    /// Set the placeholder from a translation key, resolved at paint time so
    /// a language change updates it live. Pair with
    /// [`Self::with_placeholder_var`] when the key interpolates `%{…}`.
    pub fn with_placeholder_key(mut self, key: impl Into<SharedString>) -> Self {
        self.placeholder_key = Some(key.into());
        self.placeholder = SharedString::default();
        self.placeholder_vars.clear();
        self.placeholder_is_default = false;
        self
    }

    /// Supply a named interpolation value for the placeholder key
    /// (`%{name}`), resolved at paint time.
    pub fn with_placeholder_var(
        mut self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        self.placeholder_vars.push((name.into(), value.into()));
        self
    }

    /// The placeholder text to paint in the current locale. Default, keyed,
    /// and literal placeholders all converge here so the paint path and tests
    /// share one implementation.
    pub(crate) fn resolved_placeholder(&self) -> SharedString {
        resolve_placeholder(
            self.placeholder_is_default,
            self.placeholder_key.as_ref().map(|key| key.as_ref()),
            &self.placeholder,
            &self.placeholder_vars,
            None,
        )
        .into()
    }

    /// Replace the key context flags for this input (space separated).
    pub fn with_key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = context.into();
        self
    }

    /// Seed the content (used by the provider editor's form fields).
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self
    }

    /// Override the element id so sibling inputs don't collide.
    pub fn with_element_id(mut self, id: impl Into<SharedString>) -> Self {
        self.element_id = id.into();
        self
    }

    /// Cap how many visual rows the editor grows to before scrolling.
    /// Form fields pass `1` so they stay a single line.
    pub fn with_max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self
    }

    /// Set the syntax language after construction — the file's language is
    /// only known once the background read finishes.
    pub fn set_syntax(&mut self, lang: Option<Lang>, cx: &mut Context<Self>) {
        if self.syntax == lang {
            return;
        }
        self.syntax = lang;
        cx.notify();
    }

    /// Paint a line-number gutter down the left edge.
    pub fn with_gutter(mut self, gutter: bool) -> Self {
        self.gutter = gutter;
        self
    }

    /// Disable wrapping so one logical line is one visual row.
    pub fn with_wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Fill the parent's height and scroll internally, instead of growing to
    /// `max_lines`.
    pub fn with_fill(mut self, fill: bool) -> Self {
        self.fill = fill;
        self
    }

    /// Share the `/`+`@` autocomplete state (see `mentions::AutocompleteState`).
    pub fn with_autocomplete(mut self, state: SharedAutocomplete) -> Self {
        self.autocomplete = Some(state);
        self
    }

    pub fn text(&self) -> String {
        self.content.clone()
    }

    /// Whether an IME composition is in flight. Submit-like handlers must bail
    /// while this is set: Enter may be confirming an IME candidate, not
    /// asking to submit the draft.
    pub fn is_composing(&self) -> bool {
        self.marked_range.is_some()
    }

    /// Monotonic text-revision counter; changes only on actual edits, not on
    /// caret moves or blinks.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The active `/`-command or `@`-file trigger at the caret, if any.
    pub fn active_trigger(&self) -> Option<Trigger> {
        detect_trigger(&self.content, self.cursor_offset())
    }

    /// Whether any pasted images are waiting to be drained by the app.
    pub fn has_pasted_images(&self) -> bool {
        !self.pasted_images.is_empty()
    }

    /// Replace `range` with `text` (used by autocomplete commits).
    pub fn replace_range(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.record_undo();
        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len()).max(start);
        self.content = self.content[0..start].to_owned() + text + &self.content[end..];
        self.selected_range = start + text.len()..start + text.len();
        self.marked_range = None;
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.selected_range = 0..0;
        self.marked_range = None;
        self.scroll_offset = px(0.);
        self.revision = self.revision.wrapping_add(1);
        self.reset_history();
        cx.notify();
    }

    /// Replace the whole content and place the caret at the end. Used to seed
    /// form fields from app state (session rename, restored queue text).
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_offset = px(0.);
        self.revision = self.revision.wrapping_add(1);
        self.reset_history();
        cx.notify();
    }

    /// Replace the whole content and place the caret at the start. The Explorer
    /// opens a file at the top, not scrolled to its last line.
    pub fn set_text_at_start(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.scroll_offset = px(0.);
        self.revision = self.revision.wrapping_add(1);
        self.reset_history();
        cx.notify();
    }

    /// Insert `text` at the caret (files dropped on the composer reference
    /// non-image attachments by path here).
    pub fn insert_at_caret(&mut self, text: &str, cx: &mut Context<Self>) {
        let at = self.cursor_offset();
        self.replace_range(at..at, text, cx);
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    // ── caret blink ────────────────────────────────────────────────────

    /// Drive the caret blink from focus: start it when the input gains
    /// focus, stop it when focus leaves, and hold the caret solid for a beat
    /// after any edit or caret move before it resumes. Called once per render.
    fn sync_caret_blink(&mut self, window: &Window, cx: &mut Context<Self>) {
        let edited =
            self.content != self.blink_content || self.selected_range != self.blink_selection;
        if edited {
            self.blink_content = self.content.clone();
            self.blink_selection = self.selected_range.clone();
        }

        if !self.focus_handle.is_focused(window) {
            if self.blink_active {
                self.blink_active = false;
                self.blink_epoch += 1; // invalidate the pending timer
                self.caret_visible = true;
            }
            return;
        }

        if !self.blink_active {
            self.blink_active = true;
            self.wake_caret(cx);
        } else if edited {
            self.wake_caret(cx);
        }
    }

    /// Show the caret now and schedule the first blink after the idle delay.
    fn wake_caret(&mut self, cx: &mut Context<Self>) {
        self.caret_visible = true;
        self.blink_epoch += 1;
        let epoch = self.blink_epoch;
        self.schedule_blink(BLINK_RESUME_DELAY, epoch, cx);
    }

    /// Toggle the caret and schedule the next toggle, unless a newer wake has
    /// superseded this timer or focus has left.
    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if epoch != self.blink_epoch || !self.blink_active {
            return;
        }
        self.caret_visible = !self.caret_visible;
        cx.notify();
        self.schedule_blink(BLINK_INTERVAL, epoch, cx);
    }

    fn schedule_blink(&mut self, after: Duration, epoch: usize, cx: &mut Context<Self>) {
        self._blink_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(after).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |input, cx| input.blink(epoch, cx)).ok();
            }
        }));
    }

    // ── movement ───────────────────────────────────────────────────────

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        // While the autocomplete menu is open the arrows navigate it, not
        // the caret.
        if self.autocomplete_navigate(-1, cx) {
            return;
        }
        self.move_vertically(-1., cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.autocomplete_navigate(1, cx) {
            return;
        }
        self.move_vertically(1., cx);
    }

    /// Move the autocomplete highlight when the menu is open. Returns true
    /// when the keystroke was consumed by the menu.
    fn autocomplete_navigate(&mut self, delta: i32, cx: &mut Context<Self>) -> bool {
        let Some(state) = &self.autocomplete else {
            return false;
        };
        let mut state = state.borrow_mut();
        if !state.open || state.count == 0 {
            return false;
        }
        state.move_highlight(delta);
        drop(state);
        cx.notify();
        true
    }

    /// Move the caret one visual row up/down, preserving the horizontal
    /// position when possible.
    fn move_vertically(&mut self, rows: f32, cx: &mut Context<Self>) {
        if self.last_lines.is_empty() {
            return;
        }
        let line_height = self.last_line_height;
        let caret = self.position_for_offset(self.cursor_offset());
        let target_y = caret.y + line_height * rows;
        if target_y < px(0.) {
            self.move_to(0, cx);
            return;
        }
        self.move_to(self.index_at_content_position(point(caret.x, target_y)), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx)
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.selected_range.end), cx)
    }

    /// Option+Left / Option+Right: move one word at a time (macOS text-field
    /// convention).
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word_boundary(self.cursor_offset()), cx)
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word_boundary(self.selected_range.end), cx)
    }

    /// Cmd+Left / Cmd+Right: jump to the start/end of the wrapped row the
    /// caret is on (a logical line's edge when it doesn't wrap).
    fn line_left(&mut self, _: &LineLeft, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.line_start(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn line_right(&mut self, _: &LineRight, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.line_end(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_line_left(&mut self, _: &SelectLineLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_start(self.cursor_offset()), cx)
    }

    fn select_line_right(&mut self, _: &SelectLineRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_end(self.selected_range.end), cx)
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    // ── undo / redo ────────────────────────────────────────────────────

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    /// Record the pre-edit buffer as an undo point. A run of edits within
    /// [`UNDO_COALESCE`] shares one point, so `cmd-z` rewinds a burst of typing
    /// rather than one keystroke at a time.
    fn record_undo(&mut self) {
        let now = Instant::now();
        let coalesce = self
            .last_edit
            .is_some_and(|at| now.duration_since(at) < UNDO_COALESCE);
        if !coalesce {
            self.push_undo(self.snapshot());
        }
        self.redo_stack.clear();
        self.last_edit = Some(now);
    }

    fn push_undo(&mut self, snapshot: Snapshot) {
        self.undo_bytes = self.undo_bytes.saturating_add(snapshot.content.len());
        self.undo_stack.push(snapshot);
        while self.undo_stack.len() > UNDO_LIMIT || self.undo_bytes > UNDO_BYTES {
            let dropped = self.undo_stack.remove(0);
            self.undo_bytes = self.undo_bytes.saturating_sub(dropped.content.len());
            if self.undo_stack.is_empty() {
                break;
            }
        }
    }

    /// A programmatic whole-buffer replacement (seeding a file, clearing a
    /// field) is a new document, not an edit: it starts a fresh history.
    fn reset_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.undo_bytes = 0;
        self.last_edit = None;
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.scroll_offset = px(0.);
        self.revision = self.revision.wrapping_add(1);
        self.last_edit = None;
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.undo_bytes = self.undo_bytes.saturating_sub(snapshot.content.len());
        let current = self.snapshot();
        self.redo_stack.push(current);
        self.apply_snapshot(snapshot, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.redo_stack.pop() else {
            return;
        };
        self.push_undo(self.snapshot());
        self.apply_snapshot(snapshot, cx);
    }

    // ── editing ────────────────────────────────────────────────────────

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Focus on press, like a native field: the arrows and clipboard keys
        // are live before the button is released, and the input does not
        // depend on an ancestor's mouse-up handler to claim focus.
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        match event.click_count {
            // Double-click selects the run under the pointer; triple-click
            // selects the whole logical line.
            2 => self.select_word_at(offset, cx),
            n if n >= 3 => self.select_line_at(offset, cx),
            // Shift extends the existing selection, a plain click drops it.
            _ if event.modifiers.shift => self.select_to(offset, cx),
            _ => self.move_to(offset, cx),
        }
    }

    /// Select the character-class run (word, punctuation, or whitespace)
    /// containing `offset`.
    fn select_word_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = word_range_at(&self.content, offset);
        self.selection_reversed = false;
        cx.notify();
    }

    /// Select the logical line containing `offset` (its trailing newline
    /// included, so deleting the selection merges the lines like a native
    /// field).
    fn select_line_at(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = line_range_at(&self.content, offset);
        self.selection_reversed = false;
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.last_total_rows.max(1);
        let visible = rows.min(self.max_lines);
        let max_scroll = ((rows - visible) as f32 * self.last_line_height).max(px(0.));
        let dy = event.delta.pixel_delta(self.last_line_height).y;
        self.scroll_offset = (self.scroll_offset - dy).clamp(px(0.), max_scroll);
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        // Pasted images become message attachments ("Show pasted
        // images as attachments") — the app drains `pasted_images` and
        // renders chips above the composer.
        let images: Vec<Image> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image.clone()),
                ClipboardEntry::String(_) => None,
            })
            .collect();
        if !images.is_empty() {
            self.pasted_images.extend(images);
            cx.notify();
            return;
        }
        if let Some(text) = item.text() {
            self.replace_text_in_range(None, &text, window, cx)
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    // ── geometry ───────────────────────────────────────────────────────

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        // A caret move ends the current undo run.
        self.last_edit = None;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// Window position → position in content coordinates (scroll applied).
    fn content_position(&self, position: gpui::Point<Pixels>) -> gpui::Point<Pixels> {
        let Some(bounds) = self.last_bounds else {
            return point(px(0.), px(0.));
        };
        point(
            position.x - bounds.origin.x - self.last_gutter,
            position.y - bounds.origin.y + self.scroll_offset,
        )
    }

    /// The byte index under a point in *content* coordinates.
    fn index_at_content_position(&self, pos: gpui::Point<Pixels>) -> usize {
        if self.content.is_empty() || self.last_lines.is_empty() {
            return 0;
        }
        let line_height = self.last_line_height;
        let mut y_acc = px(0.);
        for (i, line) in self.last_lines.iter().enumerate() {
            let height = line.size(line_height).height;
            let is_last = i == self.last_lines.len() - 1;
            // `<` so a point exactly on a line boundary (e.g. the target of a
            // vertical caret move) belongs to the line below, not the one above.
            if pos.y < y_acc + height || is_last {
                let local_x = pos.x.clamp(px(0.), line.width());
                let local_y = pos.y.clamp(y_acc, y_acc + height - px(0.5)) - y_acc;
                // `closest_index_for_position` indexes within this logical
                // line; add the line's byte start to get a content offset.
                let local = line
                    .closest_index_for_position(point(local_x, local_y), line_height)
                    .unwrap_or_else(|ix| ix);
                return self.last_line_starts.get(i).copied().unwrap_or(0) + local;
            }
            y_acc += height;
        }
        self.content.len()
    }

    fn index_for_mouse_position(&self, position: gpui::Point<Pixels>) -> usize {
        self.index_at_content_position(self.content_position(position))
    }

    /// The (x, y) of a byte offset in content coordinates — y is the top of
    /// the visual row the offset sits on, counting all preceding lines.
    fn position_for_offset(&self, offset: usize) -> gpui::Point<Pixels> {
        let line_height = self.last_line_height;
        let line_lens: Vec<usize> = self.last_lines.iter().map(|line| line.len()).collect();
        if let Some((i, local)) = line_at_offset(&self.last_line_starts, &line_lens, offset) {
            let line = &self.last_lines[i];
            let line_y: Pixels = self.last_lines[..i]
                .iter()
                .fold(px(0.), |acc, line| acc + line.size(line_height).height);
            let pos = line
                .position_for_index(local, line_height)
                .unwrap_or_else(|| {
                    point(
                        line.width(),
                        line.wrap_boundaries().len() as f32 * line_height,
                    )
                });
            return point(pos.x, line_y + pos.y);
        }
        point(px(0.), px(0.))
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        utf16_offset_to_utf8(&self.content, offset)
    }

    /// The document span a platform text replacement targets: the explicit
    /// range when the platform supplies one, else the active composition
    /// span, else the current selection. Windows IME passes `None` for both
    /// incremental updates and the final commit, so the marked range is what
    /// keeps those edits from landing on the wrong span.
    fn replacement_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        if let Some(range) = range_utf16 {
            return clamp_range(&self.content, self.range_from_utf16(&range));
        }
        if let Some(marked) = self.marked_range.clone() {
            return marked;
        }
        clamp_range(&self.content, self.selected_range.clone())
    }

    /// Splice `new_text` over `range` and return the inserted byte span.
    /// The range is clamped to character boundaries first, so a stale or
    /// malformed platform range can never slice mid-character.
    fn splice(&mut self, range: Range<usize>, new_text: &str) -> Range<usize> {
        let range = clamp_range(&self.content, range);
        self.content.replace_range(range.clone(), new_text);
        range.start..range.start + new_text.len()
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// The start of the word before `offset`, skipping any separators between.
    /// Words are alphanumeric + `_`, the same classes double-click selects.
    fn previous_word_boundary(&self, offset: usize) -> usize {
        let text = &self.content;
        let mut i = offset.min(text.len());
        while i > 0 {
            let prev = prev_char_boundary(text, i);
            if text[prev..i].chars().next().is_some_and(is_word_char) {
                break;
            }
            i = prev;
        }
        while i > 0 {
            let prev = prev_char_boundary(text, i);
            if text[prev..i].chars().next().is_some_and(is_word_char) {
                i = prev;
            } else {
                break;
            }
        }
        i
    }

    /// The end of the word after `offset`, skipping any separators between.
    fn next_word_boundary(&self, offset: usize) -> usize {
        let text = &self.content;
        let mut i = offset.min(text.len());
        while i < text.len() {
            let next = next_char_boundary(text, i);
            if text[i..next].chars().next().is_some_and(is_word_char) {
                break;
            }
            i = next;
        }
        while i < text.len() {
            let next = next_char_boundary(text, i);
            if text[i..next].chars().next().is_some_and(is_word_char) {
                i = next;
            } else {
                break;
            }
        }
        i
    }

    /// Start of the visual row containing `offset` (a wrapped row's left edge,
    /// or the logical line start when it doesn't wrap).
    fn line_start(&self, offset: usize) -> usize {
        if !self.last_lines.is_empty() {
            let row = self.position_for_offset(offset).y;
            return self.index_at_content_position(point(px(0.), row));
        }
        let offset = offset.min(self.content.len());
        self.content[..offset]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0)
    }

    /// End of the visual row containing `offset`.
    fn line_end(&self, offset: usize) -> usize {
        if !self.last_lines.is_empty() {
            let row = self.position_for_offset(offset).y;
            return self.index_at_content_position(point(px(1_000_000.), row));
        }
        let offset = offset.min(self.content.len());
        self.content[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(self.content.len())
    }
}

/// Whether a character is part of a word for Option+arrow / double-click
/// purposes (alphanumeric or underscore).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn cx_focus_handle(cx: &mut Context<ComposerInput>) -> FocusHandle {
    cx.focus_handle()
}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for ComposerInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        // The trait's ranges are UTF-16 (see `text_for_range` and
        // `replace_text_in_range`), and Linux platforms feed this value back
        // into those methods, so convert the tracked byte span.
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A plain replacement ends any active composition. On Windows the
        // commit (`GCS_RESULTSTR`) and cancel (`lparam == 0`) paths both route
        // here with `None`, so the marked span — not the selection — is what
        // gets replaced; otherwise the preedit text would be left behind and
        // the final text duplicated.
        let range = self.replacement_range(range_utf16);
        self.record_undo();
        let inserted = self.splice(range, new_text);
        self.selected_range = inserted.end..inserted.end;
        self.marked_range = None;
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.replacement_range(range_utf16);
        self.record_undo();
        let inserted = self.splice(range, new_text);
        if new_text.is_empty() {
            // An empty composition clears the preedit text rather than
            // leaving an empty marked span behind.
            self.selected_range = inserted.start..inserted.start;
            self.marked_range = None;
        } else {
            // The IME's selection is relative to the composition text, so map
            // it against `new_text` and offset by the splice start. Converting
            // it against the whole document and offsetting by `new_text.len()`
            // (the old stub) produced ranges past the end of the buffer — for
            // example, composing "n" with selection 1..1 gave a 2..2 selection
            // on a one-byte document — which the next update then sliced and
            // panicked on.
            self.selected_range = new_selected_range_utf16
                .map(|selection| {
                    let start = inserted.start + utf16_offset_to_utf8(new_text, selection.start);
                    let end = inserted.start + utf16_offset_to_utf8(new_text, selection.end);
                    start..end.max(start)
                })
                .unwrap_or_else(|| inserted.end..inserted.end);
            self.selection_reversed = false;
            self.marked_range = Some(inserted);
        }
        self.revision = self.revision.wrapping_add(1);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.position_for_offset(range.start);
        let end = self.position_for_offset(range.end);
        let origin = point(
            bounds.origin.x + start.x,
            bounds.origin.y + start.y - self.scroll_offset,
        );
        Some(Bounds::from_corners(
            origin,
            point(
                origin.x + (end.x - start.x).abs(),
                origin.y + self.last_line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.index_for_mouse_position(point))
    }
}

/// Locate the logical line that contains byte `offset`, returning the line
/// index and the local byte index within it. `line_starts[i]` is the byte
/// offset of line `i`; `line_lens[i]` is its length excluding the trailing
/// newline that `shape_text` split off.
///
/// A caret can sit on a newline byte, which belongs to no line's text; we
/// attribute it to the end of the preceding line. `offset` can also be stale
/// relative to freshly shaped lines, so the local index saturates instead of
/// underflowing (which would panic in debug builds).
fn line_at_offset(
    line_starts: &[usize],
    line_lens: &[usize],
    offset: usize,
) -> Option<(usize, usize)> {
    let last = line_lens.len().checked_sub(1)?;
    line_starts
        .iter()
        .zip(line_lens)
        .enumerate()
        .find_map(|(i, (&start, &len))| {
            (offset <= start + len || i == last).then(|| (i, offset.saturating_sub(start).min(len)))
        })
}

/// The byte before `offset`, or 0 at the start. `offset` must be a char
/// boundary.
fn prev_char_boundary(text: &str, offset: usize) -> usize {
    text[..offset]
        .char_indices()
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Clamp `offset` to `text.len()` and walk back to the nearest character
/// boundary, so it is always safe to slice `text` there.
fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Clamp a byte range to `text` and snap its ends to character boundaries.
/// Platform IME ranges are UTF-16 conversions that can go stale between a
/// preedit update and the next key event; this keeps them from slicing
/// mid-character (or out of bounds) and panicking.
fn clamp_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = floor_char_boundary(text, range.start);
    let end = floor_char_boundary(text, range.end).max(start);
    start..end
}

/// Convert a UTF-16 offset within `text` to a UTF-8 byte offset. Offsets past
/// the end (or landing inside a surrogate pair) clamp to the nearest scalar
/// boundary, so the result always slices `text` safely.
fn utf16_offset_to_utf8(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;
    for ch in text.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += ch.len_utf16();
        utf8_offset += ch.len_utf8();
    }
    utf8_offset
}

/// The byte after the char at `offset`, or the end. `offset` must be a char
/// boundary.
fn next_char_boundary(text: &str, offset: usize) -> usize {
    text[offset..]
        .chars()
        .next()
        .map(|c| offset + c.len_utf8())
        .unwrap_or(text.len())
}

/// The double-click selection range: the run of the same character class
/// (word / punctuation / whitespace) around `offset`. Returning the class run
/// rather than only a word means double-clicking punctuation selects it too,
/// matching native text views.
fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    if text.is_empty() {
        return 0..0;
    }
    let class = |c: char| -> u8 {
        if c.is_alphanumeric() || c == '_' {
            0
        } else if c.is_whitespace() {
            2
        } else {
            1
        }
    };
    let offset = offset.min(text.len());
    let probe = if offset < text.len() {
        offset
    } else {
        prev_char_boundary(text, offset)
    };
    let target = match text[probe..].chars().next() {
        Some(c) => class(c),
        None => return probe..probe,
    };
    let mut start = probe;
    while start > 0 {
        let prev = prev_char_boundary(text, start);
        if text[prev..start].chars().next().map(class) != Some(target) {
            break;
        }
        start = prev;
    }
    let mut end = probe;
    while end < text.len() {
        let next = next_char_boundary(text, end);
        if text[end..next].chars().next().map(class) != Some(target) {
            break;
        }
        end = next;
    }
    start..end
}

/// The triple-click selection range: the logical line around `offset`,
/// including its trailing newline so deleting the selection joins lines.
fn line_range_at(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[offset..]
        .find('\n')
        .map(|i| offset + i + 1)
        .unwrap_or(text.len());
    start..end
}

/// Overlay scrollbar thumb height. `Ord::clamp` panics when min > max, so
/// the 24px minimum is lowered when the track itself is shorter — a
/// one-line form field is typically ~18px and used to abort if a wrapped
/// session name overflowed it.
fn scrollbar_thumb_height(track: Pixels, visible_rows: usize, total_rows: usize) -> Pixels {
    if track <= px(0.) || total_rows == 0 {
        return px(0.);
    }
    let ratio = (visible_rows as f32 / total_rows as f32).clamp(0.0, 1.0);
    let min_thumb = px(24.).min(track);
    (track * ratio).clamp(min_thumb, track)
}

/// Horizontal scroll that keeps the caret inside a `visible_width`-wide field
/// of non-wrapping text, like a native single-line input. Capped so it never
/// scrolls past the line's end, and zero once the caret is back in view.
fn horizontal_scroll(caret_x: Pixels, line_width: Pixels, visible_width: Pixels) -> Pixels {
    let max_scroll = (line_width - visible_width).max(px(0.));
    if caret_x > visible_width {
        (caret_x - visible_width).min(max_scroll)
    } else {
        px(0.)
    }
}

/// Resolve a placeholder for `locale` (or the active locale when `locale` is
/// `None`). Shared by the paint path and tests so both agree on precedence:
/// default key → explicit key (+ interpolated vars) → literal.
fn resolve_placeholder(
    placeholder_is_default: bool,
    placeholder_key: Option<&str>,
    placeholder: &str,
    vars: &[(SharedString, SharedString)],
    locale: Option<&str>,
) -> String {
    if placeholder_is_default {
        return match locale {
            Some(locale) => crate::i18n::translate_in(locale, "composer.placeholder"),
            None => tr!("composer.placeholder"),
        };
    }
    let Some(key) = placeholder_key else {
        return placeholder.to_owned();
    };
    let mut text = match locale {
        Some(locale) => crate::i18n::translate_in(locale, key),
        None => crate::i18n::translate(key),
    };
    if !vars.is_empty() {
        let names: Vec<&str> = vars.iter().map(|(name, _)| name.as_ref()).collect();
        let values: Vec<String> = vars.iter().map(|(_, value)| value.to_string()).collect();
        text = rust_i18n::replace_patterns(&text, &names, &values);
    }
    text
}

/// Split the composer text into paint runs: the base ink plus the
/// `/command` and `@file` token colors. Gaps keep `base`'s color. Runs span
/// the whole text (newlines included) so `shape_text` never runs dry.
fn mention_runs(text: &str, base: &TextRun, theme: &theme::Theme) -> Vec<TextRun> {
    let spans = tokenize_mentions(text);
    if spans.is_empty() {
        return vec![TextRun {
            len: text.len(),
            ..base.clone()
        }];
    }
    let mut runs = Vec::with_capacity(spans.len() * 2 + 1);
    let mut cursor = 0usize;
    for span in spans {
        if span.range.start > cursor {
            runs.push(TextRun {
                len: span.range.start - cursor,
                ..base.clone()
            });
        }
        let color = match span.kind {
            MentionKind::Command => theme.mention_command(),
            MentionKind::File => theme.mention_file(),
        };
        runs.push(TextRun {
            len: span.range.end - span.range.start,
            color,
            ..base.clone()
        });
        cursor = span.range.end;
    }
    if cursor < text.len() {
        runs.push(TextRun {
            len: text.len() - cursor,
            ..base.clone()
        });
    }
    runs
}

/// Build syntax-colored runs over the whole content for `shape_text`.
///
/// The lexer returns one token list per `\n`-split line; runs must cover the
/// text byte-for-byte, newlines included, so the shaped layout is identical to
/// the plain path and caret/selection geometry stays valid.
fn syntax_runs(text: &str, lang: Lang, base: &TextRun, theme: &theme::Theme) -> Vec<TextRun> {
    let tokens = highlight::tokenize_cached(lang, text);
    let mut runs: Vec<TextRun> = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let spans = tokens.get(i).map(Vec::as_slice).unwrap_or(&[]);
        let mut offset = 0usize;
        for token in spans {
            let start = token.range.start.min(line.len());
            let end = token.range.end.min(line.len());
            if start > offset {
                runs.push(TextRun {
                    len: start - offset,
                    color: base.color,
                    ..base.clone()
                });
            }
            if end > start {
                runs.push(TextRun {
                    len: end - start,
                    color: theme.token_color(token.class),
                    ..base.clone()
                });
            }
            offset = offset.max(end);
        }
        if offset < line.len() {
            runs.push(TextRun {
                len: line.len() - offset,
                color: base.color,
                ..base.clone()
            });
        }
        if i + 1 < lines.len() {
            runs.push(TextRun {
                len: 1,
                color: base.color,
                ..base.clone()
            });
        }
    }
    if runs.is_empty() {
        runs.push(TextRun {
            len: text.len(),
            ..base.clone()
        });
    }
    runs
}

/// The painted text element for the input.
struct TextElement {
    input: Entity<ComposerInput>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

struct PrepaintState {
    lines: Vec<WrappedLine>,
    /// y offset of each logical line in content coordinates.
    line_y: Vec<Pixels>,
    scroll_offset: Pixels,
    /// Horizontal content offset: keeps the caret on screen in a
    /// non-wrapping (single-line) field, like a native text input.
    scroll_x: Pixels,
    /// Selection wash quads, painted under the text.
    selection: Vec<PaintQuad>,
    /// Caret quad, painted over the text (focused only).
    caret: Option<PaintQuad>,
    /// Scroll thumb, painted at the right edge while the text overflows.
    scrollbar: Option<PaintQuad>,
    /// Line-number rows for the visible slice (empty without a gutter).
    numbers: Vec<WrappedLine>,
    /// Index of the first painted line number.
    number_first: usize,
    /// Gutter width, so the text paints to its right.
    gutter: Pixels,
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_height = window.line_height();
        let input = self.input.read(cx);
        // Auto-grow: one line up to `max_lines`, then the height pins and
        // the content scrolls internally.
        let visible_rows = input.last_total_rows.max(1).min(input.max_lines);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = if input.fill {
            // A full-height editor fills its pane; prepaint derives the visible
            // row count from the real bounds.
            relative(1.).into()
        } else {
            (visible_rows as f32 * line_height).into()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let line_height = window.line_height();
        let (
            content,
            selected_range,
            cursor,
            max_lines,
            caret_visible,
            last_caret,
            fill_height,
            gutter,
            wrap,
            syntax,
        ) = {
            let input = self.input.read(cx);
            (
                input.content.clone(),
                input.selected_range.clone(),
                input.cursor_offset(),
                input.max_lines,
                input.caret_visible,
                input.last_caret,
                input.fill,
                input.gutter,
                input.wrap,
                input.syntax,
            )
        };
        let style = window.text_style();
        let theme = theme::get(cx);

        // Base ink run; token runs override only the color, so the shaped
        // layout (and therefore caret/selection geometry) is unchanged.
        let base = TextRun {
            len: 0,
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let (display_text, runs) = if content.is_empty() {
            let placeholder = self.input.read(cx).resolved_placeholder();
            let mut run = base.clone();
            run.len = placeholder.len();
            run.color = theme.text_muted;
            (placeholder, vec![run])
        } else if let Some(lang) = syntax {
            // Code files: paint the whole buffer with the same lexer the
            // read-only viewer uses.
            (
                SharedString::from(content.clone()),
                syntax_runs(&content, lang, &base, theme),
            )
        } else {
            // Token colors belong to mention-capable inputs (the main
            // composer); plain form fields keep one ink.
            let runs = if self.input.read(cx).autocomplete.is_some() {
                mention_runs(&content, &base, theme)
            } else {
                vec![TextRun {
                    len: content.len(),
                    ..base.clone()
                }]
            };
            (SharedString::from(content.clone()), runs)
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let wrap_width = (wrap && bounds.size.width > px(0.)).then_some(bounds.size.width);
        let gutter_width = if gutter {
            px(content.split('\n').count().to_string().len().max(3) as f32 * 7.0 + 16.0)
        } else {
            px(0.)
        };
        let lines: Vec<WrappedLine> = window
            .text_system()
            .shape_text(display_text, font_size, &runs, wrap_width, None)
            .map(|shaped| shaped.to_vec())
            .unwrap_or_default();

        // Byte offset and y of each logical line (`WrappedLine::len`
        // excludes the newline that `shape_text` split off).
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut line_lens = Vec::with_capacity(lines.len());
        let mut line_y = Vec::with_capacity(lines.len());
        let mut byte_acc = 0usize;
        let mut y_acc = px(0.);
        for line in &lines {
            line_starts.push(byte_acc);
            line_lens.push(line.len());
            line_y.push(y_acc);
            byte_acc += line.len() + 1;
            y_acc += line.size(line_height).height;
        }
        let total_rows: usize = lines
            .iter()
            .map(|line| line.wrap_boundaries().len() + 1)
            .sum::<usize>()
            .max(1);
        // A full-height editor pins the visible row count to the pane, not to
        // `max_lines` (the auto-growing composer's cap).
        let visible_rows = if fill_height {
            ((bounds.size.height / line_height).floor() as usize).max(1)
        } else {
            total_rows.min(max_lines)
        };
        let content_height = total_rows as f32 * line_height;
        let visible_height = visible_rows as f32 * line_height;

        // Clamp the scroll so the caret stays on screen after edits. Only do
        // this when the caret actually moved: a scroll-wheel event leaves the
        // caret where it was, and snapping to it every frame would undo the
        // scroll instead of letting the reader browse the text.
        let mut scroll_offset = self.input.read(cx).scroll_offset;
        let max_scroll = (content_height - visible_height).max(px(0.));
        scroll_offset = scroll_offset.min(max_scroll).max(px(0.));
        if cursor != last_caret && !lines.is_empty() {
            if let Some((i, local)) = line_at_offset(&line_starts, &line_lens, cursor) {
                if let Some(pos) = lines[i].position_for_index(local, line_height) {
                    let caret_y = line_y[i] + pos.y;
                    scroll_offset = scroll_offset.max(caret_y + line_height - visible_height);
                    scroll_offset = scroll_offset.min(caret_y).min(max_scroll).max(px(0.));
                }
            }
        }

        let caret_fallback = |line: &WrappedLine| -> gpui::Point<Pixels> {
            point(
                line.width(),
                line.wrap_boundaries().len() as f32 * line_height,
            )
        };

        // Caret anchor in content coordinates. Computed before `map` so the
        // horizontal scroll below can keep it on screen.
        let mut caret_pos: Option<gpui::Point<Pixels>> = None;
        if !lines.is_empty() {
            if content.is_empty() {
                caret_pos = Some(point(px(0.), px(0.)));
            } else if let Some((i, local)) = line_at_offset(&line_starts, &line_lens, cursor) {
                let line = &lines[i];
                let raw = line
                    .position_for_index(local, line_height)
                    .unwrap_or_else(|| caret_fallback(line));
                caret_pos = Some(point(raw.x, line_y[i] + raw.y));
            }
        }

        // Non-wrapping text can overflow its width, so slide it left until
        // the caret is inside the field — a native single-line input. Wrapping
        // text never overflows, so its scroll stays at 0.
        let visible_width = (bounds.size.width - gutter_width).max(px(0.));
        let mut scroll_x = px(0.);
        if !wrap {
            if let Some(caret) = caret_pos {
                let line_width = line_at_offset(&line_starts, &line_lens, cursor)
                    .and_then(|(i, _)| lines.get(i))
                    .map(|line| line.width())
                    .unwrap_or(px(0.));
                scroll_x = horizontal_scroll(caret.x, line_width, visible_width);
            }
        }

        // Window-coordinate mapping of a content point.
        let map = |x: Pixels, y: Pixels| -> gpui::Point<Pixels> {
            point(
                bounds.origin.x + gutter_width + x - scroll_x,
                bounds.origin.y + y - scroll_offset,
            )
        };

        // Selection wash per logical line: one rect per visual row.
        let mut selection: Vec<PaintQuad> = Vec::new();
        let sel = selected_range.start..selected_range.end;
        if !sel.is_empty() {
            for (i, line) in lines.iter().enumerate() {
                let line_start = line_starts[i];
                let line_end = line_start + line.len();
                let overlap_start = sel.start.max(line_start);
                let overlap_end = sel.end.min(line_end);
                if overlap_start >= overlap_end {
                    continue;
                }
                let start_pos = line
                    .position_for_index(overlap_start - line_start, line_height)
                    .unwrap_or_else(|| point(px(0.), px(0.)));
                let end_pos = line
                    .position_for_index(overlap_end - line_start, line_height)
                    .unwrap_or_else(|| caret_fallback(line));
                let row_start = (start_pos.y / line_height).floor() as i32;
                let row_end = (end_pos.y / line_height).floor() as i32;
                for row in row_start..=row_end {
                    let (x0, x1) = if row == row_start && row == row_end {
                        (start_pos.x, end_pos.x)
                    } else if row == row_start {
                        (start_pos.x, line.width())
                    } else if row == row_end {
                        (px(0.), end_pos.x)
                    } else {
                        (px(0.), line.width())
                    };
                    let y = line_y[i] + row as f32 * line_height;
                    selection.push(fill(
                        Bounds::from_corners(
                            map(x0, y),
                            point(map(x1, y).x, map(x1, y).y + line_height),
                        ),
                        theme.accent.opacity(0.25),
                    ));
                }
            }
        }

        // Caret: 2px accent bar spanning the visual row.
        let caret = if caret_visible {
            caret_pos.map(|caret| {
                let origin = map(caret.x, caret.y);
                fill(
                    Bounds::new(
                        point(origin.x, origin.y + px(2.)),
                        size(px(2.), line_height - px(4.)),
                    ),
                    theme.accent_text,
                )
            })
        } else {
            None
        };

        // Overlay scrollbar: a thin thumb at the right edge, sized to the
        // visible share of the text and positioned by the scroll offset. Only
        // painted while the content overflows `max_lines` *and* the track is
        // tall enough — a one-line form field is shorter than the 24px
        // minimum thumb, and `Ord::clamp(24, track)` panics when min > max.
        let track = bounds.size.height;
        let scrollbar = (max_scroll > px(0.) && track >= px(24.)).then(|| {
            let thumb_height = scrollbar_thumb_height(track, visible_rows, total_rows);
            let travel = track - thumb_height;
            let thumb_top = (scroll_offset / max_scroll) * travel;
            let width = px(4.);
            let inset = px(2.);
            fill(
                Bounds::new(
                    point(
                        bounds.origin.x + bounds.size.width - width - inset,
                        bounds.origin.y + thumb_top,
                    ),
                    size(width, thumb_height),
                ),
                theme.text_muted.opacity(0.45),
            )
        });

        // Gutter numbers: shape only the visible slice. Wrapping is off for
        // the Explorer editor, so a logical line is exactly one row and the
        // numbers align with `line_y` without extra measurement.
        let number_first = if gutter {
            (scroll_offset / line_height).floor().max(0.) as usize
        } else {
            0
        };
        let number_lines: Vec<WrappedLine> = if gutter && !lines.is_empty() {
            let last = (number_first + visible_rows + 1).min(lines.len());
            if number_first < last {
                let text = (number_first..last)
                    .map(|i| (i + 1).to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut run = base.clone();
                run.len = text.len();
                run.color = theme.text_muted.opacity(0.7);
                window
                    .text_system()
                    .shape_text(text.into(), font_size, &[run], None, None)
                    .map(|shaped| shaped.to_vec())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let width_changed = self.input.read(cx).last_wrap_width != wrap_width;
        let rows_changed = self.input.read(cx).last_total_rows != total_rows;
        let snapshot = (lines.clone(), line_starts.clone(), scroll_offset);
        self.input.update(cx, |input, _| {
            input.last_lines = snapshot.0;
            input.last_line_starts = snapshot.1;
            input.scroll_offset = snapshot.2;
            input.last_caret = cursor;
            input.last_line_height = line_height;
            input.last_bounds = Some(bounds);
            input.last_wrap_width = wrap_width;
            input.last_total_rows = total_rows;
            input.last_gutter = gutter_width;
        });
        // A resize changed the wrap width *and* the row count — the height
        // used at request_layout was one frame stale; re-layout.
        if width_changed && rows_changed {
            window.refresh();
        }

        PrepaintState {
            lines,
            line_y,
            scroll_offset,
            scroll_x,
            selection,
            caret,
            scrollbar,
            numbers: number_lines,
            number_first,
            gutter: gutter_width,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        let focused = focus_handle.is_focused(window);
        let scroll = prepaint.scroll_offset;
        let scroll_x = prepaint.scroll_x;
        let gutter = prepaint.gutter;
        let line_height = window.line_height();
        let selection = std::mem::take(&mut prepaint.selection);
        let caret = prepaint.caret.take();
        let scrollbar = prepaint.scrollbar.take();
        // Text lives right of the gutter and clips there, so a horizontal
        // scroll never slides the text under the line numbers.
        let text_bounds = Bounds {
            origin: point(bounds.origin.x + gutter, bounds.origin.y),
            size: size((bounds.size.width - gutter).max(px(0.)), bounds.size.height),
        };

        // Clip everything to the element: wrapping keeps text inside, the
        // mask handles the scrolled state past `max_lines`.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // Line numbers, right-aligned in the gutter (never x-scrolled).
            for (k, number) in prepaint.numbers.iter().enumerate() {
                let i = prepaint.number_first + k;
                if let Some(y) = prepaint.line_y.get(i).copied() {
                    let x = bounds.origin.x + gutter - px(9.) - number.width();
                    if let Err(error) = number.paint(
                        point(x, bounds.origin.y + y - scroll),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    ) {
                        eprintln!("Orbit composer: could not paint a line number: {error}");
                    }
                }
            }
            window.with_content_mask(
                Some(ContentMask {
                    bounds: text_bounds,
                }),
                |window| {
                    // Selection wash under the text.
                    for quad in selection {
                        window.paint_quad(quad);
                    }
                    for (i, line) in prepaint.lines.iter().enumerate() {
                        if let Err(error) = line.paint(
                            point(
                                bounds.origin.x + gutter - scroll_x,
                                bounds.origin.y + prepaint.line_y[i] - scroll,
                            ),
                            line_height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        ) {
                            eprintln!("Orbit composer: could not paint a text line: {error}");
                        }
                    }
                    // Caret over the text.
                    if focused {
                        if let Some(caret) = caret {
                            window.paint_quad(caret);
                        }
                    }
                },
            );
            // Scroll thumb last, so it sits above the text and caret.
            if let Some(scrollbar) = scrollbar {
                window.paint_quad(scrollbar);
            }
        });
    }
}

impl Render for ComposerInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep the caret blink in step with focus and this frame's edit.
        self.sync_caret_blink(window, cx);
        // Transparent: the floating composer box in app.rs provides the
        // background/border; this is just the editable (auto-growing) area.
        let debug_selector = self.element_id.to_string();
        div()
            .id(self.element_id.clone())
            .debug_selector(move || debug_selector)
            .flex_1()
            .min_w_0()
            .key_context(self.key_context.as_ref())
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::line_left))
            .on_action(cx.listener(Self::line_right))
            .on_action(cx.listener(Self::select_line_left))
            .on_action(cx.listener(Self::select_line_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(TextElement { input: cx.entity() })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        horizontal_scroll, line_at_offset, line_range_at, resolve_placeholder,
        scrollbar_thumb_height, word_range_at,
    };
    use gpui::{px, SharedString};

    /// A placeholder built from a translation key must follow the locale it is
    /// painted in, so switching the interface language updates search fields
    /// that were constructed long before (the provider/model/settings filters).
    #[test]
    fn placeholder_keys_follow_the_locale() {
        let en = resolve_placeholder(false, Some("app.search_providers"), "", &[], Some("en"));
        let zh = resolve_placeholder(false, Some("app.search_providers"), "", &[], Some("zh-CN"));
        assert_eq!(en, "Search providers…");
        assert_ne!(en, zh, "keyed placeholder still renders English in zh-CN");
        assert!(!zh.is_empty());

        // A literal placeholder (`sk-…`, a URL example) is never translated.
        assert_eq!(
            resolve_placeholder(false, None, "sk-…", &[], Some("zh-CN")),
            "sk-…"
        );

        // The shared default placeholder follows the locale too.
        assert_ne!(
            resolve_placeholder(true, None, "", &[], Some("en")),
            resolve_placeholder(true, None, "", &[], Some("zh-CN")),
        );
    }

    /// `%{…}` values in a keyed placeholder are substituted at paint time, so
    /// the branch picker's "Search <workspace> branches" stays translated.
    #[test]
    fn keyed_placeholder_vars_are_interpolated() {
        let vars = [(SharedString::from("workspace"), SharedString::from("orbit"))];
        let text = resolve_placeholder(
            false,
            Some("branch_picker.search_branches"),
            "",
            &vars,
            Some("en"),
        );
        assert!(text.contains("orbit"), "{text}");
        assert!(!text.contains("%{workspace}"), "{text}");
    }

    /// The paint path resolves against the *active* locale (`None`), not a
    /// string baked when the field was built. Tests never change the global
    /// locale, so the active one is English here.
    #[test]
    fn resolved_placeholder_uses_the_active_locale() {
        let live = resolve_placeholder(false, Some("app.search_providers"), "", &[], None);
        let en = resolve_placeholder(false, Some("app.search_providers"), "", &[], Some("en"));
        assert_eq!(live, en);
    }

    /// A caret on the newline byte (`"abc\n"` at offset 3) must resolve to
    /// the end of the preceding line, not underflow into the next line's
    /// start (the panic this guard was added for).
    #[test]
    fn caret_on_newline_belongs_to_preceding_line() {
        let starts = [0, 4];
        let lens = [3, 0];
        assert_eq!(line_at_offset(&starts, &lens, 3), Some((0, 3)));
    }

    #[test]
    fn caret_positions_map_to_their_line() {
        let starts = [0, 4, 8];
        let lens = [3, 3, 2];
        assert_eq!(line_at_offset(&starts, &lens, 0), Some((0, 0)));
        assert_eq!(line_at_offset(&starts, &lens, 3), Some((0, 3)));
        assert_eq!(line_at_offset(&starts, &lens, 4), Some((1, 0)));
        assert_eq!(line_at_offset(&starts, &lens, 7), Some((1, 3)));
        assert_eq!(line_at_offset(&starts, &lens, 8), Some((2, 0)));
        assert_eq!(line_at_offset(&starts, &lens, 10), Some((2, 2)));
    }

    /// A stale offset past the shaped lines clamps to the last line instead
    /// of panicking.
    #[test]
    fn stale_offset_clamps_to_last_line() {
        let starts = [0, 4];
        let lens = [3, 0];
        assert_eq!(line_at_offset(&starts, &lens, 99), Some((1, 0)));
    }

    #[test]
    fn no_lines_yields_none() {
        assert_eq!(line_at_offset(&[], &[], 0), None);
    }

    #[test]
    fn double_click_selects_the_run_under_the_caret() {
        let text = "hello world";
        // Anywhere in a word selects the whole word.
        assert_eq!(word_range_at(text, 1), 0..5);
        assert_eq!(word_range_at(text, 4), 0..5);
        // On the space, the whitespace run.
        assert_eq!(word_range_at(text, 5), 5..6);
        // In the second word.
        assert_eq!(word_range_at(text, 8), 6..11);
        // At the very end, the last word.
        assert_eq!(word_range_at(text, text.len()), 6..11);
        // Punctuation is its own run, like a native text view.
        assert_eq!(word_range_at("foo.bar", 3), 3..4);
        assert_eq!(word_range_at("foo.bar", 1), 0..3);
        // Empty input is a no-op, not a panic.
        assert_eq!(word_range_at("", 0), 0..0);
    }

    #[test]
    fn triple_click_selects_the_whole_line() {
        let text = "one\ntwo\nthree";
        // The middle line, including its trailing newline.
        assert_eq!(line_range_at(text, 4), 4..8);
        // The first line ends at the newline.
        assert_eq!(line_range_at(text, 0), 0..4);
        // The last line has no trailing newline.
        assert_eq!(line_range_at(text, 9), 8..13);
        // An offset at the end still selects the last line.
        assert_eq!(line_range_at(text, text.len()), 8..13);
    }

    /// A one-line form field is shorter than the 24px thumb minimum.
    /// `Ord::clamp(24, track)` panics when min > max — the crash opening
    /// the session-details rename field with a wrapped title.
    #[test]
    fn scrollbar_thumb_fits_a_short_track() {
        assert_eq!(scrollbar_thumb_height(px(18.), 1, 8), px(18.));
        assert_eq!(scrollbar_thumb_height(px(0.), 1, 8), px(0.));
        assert_eq!(scrollbar_thumb_height(px(100.), 1, 8), px(24.));
        assert_eq!(scrollbar_thumb_height(px(100.), 8, 8), px(100.));
    }

    /// A long single-line value scrolls so the caret stays inside the field,
    /// capped at the line's own width and zero once the caret is back in view.
    #[test]
    fn horizontal_scroll_follows_the_caret() {
        // Caret already inside the field: no scroll.
        assert_eq!(horizontal_scroll(px(40.), px(80.), px(100.)), px(0.));
        // Caret past the right edge: scroll by exactly the overflow.
        assert_eq!(horizontal_scroll(px(150.), px(200.), px(100.)), px(50.));
        // Capped so the line's end lands on the field's right edge.
        assert_eq!(horizontal_scroll(px(150.), px(120.), px(100.)), px(20.));
        // Nothing to scroll.
        assert_eq!(horizontal_scroll(px(0.), px(0.), px(100.)), px(0.));
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;
    use gpui::{
        point, size, FocusHandle, KeyBinding, Modifiers, MouseButton, TestAppContext,
        VisualTestContext,
    };

    struct Harness {
        input: Entity<ComposerInput>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // Bottom-anchored like the app: a flex-1 spacer above the input, so
            // the input's top moves up when its height grows.
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1())
                .child(div().p(px(12.)).child(self.input.clone()))
        }
    }

    fn draw(cx: &mut VisualTestContext, harness: &Entity<Harness>) {
        let _ = cx.draw(point(px(0.), px(0.)), size(px(500.), px(400.)), |_, _| {
            harness.clone()
        });
    }

    /// Clicking a visual row must land on that row's start even after the
    /// editor grows to fit pasted text and its top edge moves up.
    #[gpui::test]
    fn click_mapping_survives_a_paste_growth(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.new(ComposerInput::new)
        });
        let harness = cx.update(|_, cx| {
            cx.new(|_| Harness {
                input: input.clone(),
            })
        });

        draw(cx, &harness);

        // A pasted block: several short lines plus one long line that wraps.
        let text = format!("alpha\nbravo\n{}\ndelta", "x".repeat(400));
        cx.update(|_, cx| {
            input.update(cx, |input, cx| input.set_text(text.clone(), cx));
        });

        draw(cx, &harness);

        let mapped = cx.update(|_, cx| input.read(cx).last_bounds.expect("bounds"));
        let painted = cx.debug_bounds("composer-input").expect("painted bounds");
        assert_eq!(mapped, painted, "click mapping uses stale bounds");

        let (lines, line_starts, line_height, bounds, scroll) = cx.update(|_, cx| {
            let input = input.read(cx);
            (
                input.last_lines.clone(),
                input.last_line_starts.clone(),
                input.last_line_height,
                input.last_bounds.expect("bounds"),
                input.scroll_offset,
            )
        });
        assert!(
            line_starts.len() >= 4,
            "expected the wrapped lines to shape"
        );

        // Walk each logical line's visual rows and click at its first column:
        // every click must resolve to that line's byte start.
        let mut y = px(0.);
        for (ix, line) in lines.iter().enumerate() {
            let start = line_starts[ix];
            let pos = point(
                bounds.origin.x + px(1.),
                bounds.origin.y + y + line_height / 2. - scroll,
            );
            let idx =
                cx.update(|_, cx| input.update(cx, |input, _| input.index_for_mouse_position(pos)));
            assert_eq!(
                idx, start,
                "click at the start of line {ix} maps to {start}"
            );
            y += line.size(line_height).height;
        }

        // A click on the wrapped line's second visual row resolves inside that
        // line, not back at the start of the buffer.
        let wrapped_start = line_starts[2];
        let wrapped_end = line_starts[3] - 1;
        let wrapped_line_y = lines[0].size(line_height).height + lines[1].size(line_height).height;
        let mid = cx.update(|_, cx| {
            input.update(cx, |input, _| {
                input.index_for_mouse_position(point(
                    bounds.origin.x + px(200.),
                    bounds.origin.y + wrapped_line_y + line_height * 1.5 - scroll,
                ))
            })
        });
        assert!(
            mid > wrapped_start && mid < wrapped_end,
            "click inside the wrapped line stayed inside it: {mid} not in {wrapped_start}..{wrapped_end}"
        );
    }

    /// Up/Down move the caret one visual row across logical lines, the way a
    /// native multi-line field behaves.
    #[gpui::test]
    fn vertical_caret_movement_spans_lines(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.new(ComposerInput::new)
        });
        let harness = cx.update(|_, cx| {
            cx.new(|_| Harness {
                input: input.clone(),
            })
        });
        draw(cx, &harness);
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.set_text("alpha\nbravo\ncharlie\ndelta", cx)
            })
        });
        draw(cx, &harness);

        // Start at the beginning of "bravo" (byte 6).
        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(6, cx)));
        let caret =
            |cx: &mut VisualTestContext| cx.update(|_, cx| input.read(cx).selected_range.start);

        cx.update(|_, cx| input.update(cx, |input, cx| input.move_vertically(1., cx)));
        assert_eq!(caret(cx), 12, "down from line 1 lands on line 2");
        cx.update(|_, cx| input.update(cx, |input, cx| input.move_vertically(1., cx)));
        assert_eq!(caret(cx), 20, "down again lands on line 3");
        cx.update(|_, cx| input.update(cx, |input, cx| input.move_vertically(-1., cx)));
        assert_eq!(caret(cx), 12, "up returns to line 2");
    }

    /// The arrow keys bound in the `Composer` context must move the caret
    /// while the input is focused.
    #[gpui::test]
    fn arrow_keys_move_the_caret(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.bind_keys([
                KeyBinding::new("left", Left, Some("Composer")),
                KeyBinding::new("right", Right, Some("Composer")),
                KeyBinding::new("up", Up, Some("Composer")),
                KeyBinding::new("down", Down, Some("Composer")),
            ]);
            cx.new(ComposerInput::new)
        });
        let harness = cx.update(|_, cx| {
            cx.new(|_| Harness {
                input: input.clone(),
            })
        });
        draw(cx, &harness);
        cx.update(|_, cx| input.update(cx, |input, cx| input.set_text("abc\ndef", cx)));
        cx.update(|window, cx| input.update(cx, |input, _| input.focus(window)));
        draw(cx, &harness);

        // Caret starts at the end (byte 7); two lefts land on byte 5.
        cx.simulate_keystrokes("left left");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).selected_range.start),
            5,
            "left arrow moves the caret"
        );
        cx.simulate_keystrokes("right");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).selected_range.start),
            6,
            "right arrow moves the caret"
        );

        // Byte 1 is on the first line; Down drops to the same column on the
        // second line (byte 5), and Up returns.
        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(1, cx)));
        cx.simulate_keystrokes("down");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).selected_range.start),
            5,
            "down arrow moves to the line below"
        );
        cx.simulate_keystrokes("up");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).selected_range.start),
            1,
            "up arrow moves to the line above"
        );
    }

    /// The app focuses the composer from the box's mouse-up handler; a click on
    /// the input must leave the arrows working, exactly like the real flow.
    #[gpui::test]
    fn clicking_the_input_keeps_arrow_keys_working(cx: &mut TestAppContext) {
        struct ClickHarness {
            input: Entity<ComposerInput>,
            root_focus: FocusHandle,
        }

        impl Render for ClickHarness {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .track_focus(&self.root_focus)
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("test-composer-box")
                            .debug_selector(|| "test-composer-box".to_string())
                            .p(px(12.))
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.input.read(cx).focus(window);
                                }),
                            )
                            .child(self.input.clone()),
                    )
            }
        }

        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.bind_keys([
                KeyBinding::new("left", Left, Some("Composer")),
                KeyBinding::new("right", Right, Some("Composer")),
                KeyBinding::new("up", Up, Some("Composer")),
                KeyBinding::new("down", Down, Some("Composer")),
            ]);
            cx.new(ComposerInput::new)
        });
        let harness = cx.update(|_, cx| {
            cx.new(|cx| ClickHarness {
                input: input.clone(),
                root_focus: cx.focus_handle(),
            })
        });
        let _ = cx.draw(point(px(0.), px(0.)), size(px(500.), px(400.)), |_, _| {
            harness.clone()
        });
        cx.update(|_, cx| input.update(cx, |input, cx| input.set_text("abc\ndef", cx)));
        let _ = cx.draw(point(px(0.), px(0.)), size(px(500.), px(400.)), |_, _| {
            harness.clone()
        });

        let bounds = cx.debug_bounds("composer-input").expect("input bounds");
        cx.simulate_click(bounds.center(), Modifiers::none());
        let _ = cx.draw(point(px(0.), px(0.)), size(px(500.), px(400.)), |_, _| {
            harness.clone()
        });

        let focused = cx.update(|window, cx| input.read(cx).focus_handle(cx).is_focused(window));
        assert!(focused, "the input is focused after clicking it");

        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(1, cx)));
        cx.dispatch_action(Left);
        assert_eq!(
            cx.update(|_, cx| input.read(cx).selected_range.start),
            0,
            "Left action dispatches to the focused input"
        );

        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(7, cx)));
        cx.simulate_keystrokes("left left");
        assert_eq!(
            cx.update(|_, cx| input.read(cx).selected_range.start),
            5,
            "arrows still work after clicking the input"
        );
    }

    /// Option+Left/Right move by word; Cmd+Left/Right jump to the line edges.
    #[gpui::test]
    fn word_and_line_navigation(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.bind_keys([
                KeyBinding::new("alt-left", WordLeft, Some("Composer")),
                KeyBinding::new("alt-right", WordRight, Some("Composer")),
                KeyBinding::new("secondary-left", LineLeft, Some("Composer")),
                KeyBinding::new("secondary-right", LineRight, Some("Composer")),
            ]);
            cx.new(ComposerInput::new)
        });
        let harness = cx.update(|_, cx| {
            cx.new(|_| Harness {
                input: input.clone(),
            })
        });
        draw(cx, &harness);
        cx.update(|_, cx| input.update(cx, |input, cx| input.set_text("hello world\nfoo bar", cx)));
        cx.update(|window, cx| input.update(cx, |input, _| input.focus(window)));
        draw(cx, &harness);
        let caret =
            |cx: &mut VisualTestContext| cx.update(|_, cx| input.read(cx).selected_range.start);

        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(3, cx)));
        cx.simulate_keystrokes("alt-left");
        assert_eq!(caret(cx), 0, "alt-left moves to the word start");

        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(0, cx)));
        cx.simulate_keystrokes("alt-right");
        assert_eq!(caret(cx), 5, "alt-right moves past the word");

        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(15, cx)));
        cx.simulate_keystrokes("secondary-left");
        assert_eq!(
            caret(cx),
            12,
            "the primary modifier + left moves to the line start"
        );

        cx.update(|_, cx| input.update(cx, |input, cx| input.move_to(0, cx)));
        cx.simulate_keystrokes("secondary-right");
        assert_eq!(
            caret(cx),
            11,
            "the primary modifier + right moves to the line end"
        );
    }

    /// A fixed-height harness: `with_fill(true)` needs a definite parent
    /// height to derive its visible row count from.
    struct EditorHarness {
        input: Entity<ComposerInput>,
    }

    impl Render for EditorHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // A column, like the Explorer's body: `flex_1` must grow vertically
            // and the editor must fill the parent's (definite) height.
            div()
                .flex()
                .flex_col()
                .w(px(500.))
                .h(px(300.))
                .child(self.input.clone())
        }
    }

    fn draw_editor(cx: &mut VisualTestContext, harness: &Entity<EditorHarness>) {
        let _ = cx.draw(point(px(0.), px(0.)), size(px(500.), px(300.)), |_, _| {
            harness.clone()
        });
    }

    /// The Explorer editor path — gutter, no wrap, fill, syntax runs — paints
    /// without panicking, and hit-testing subtracts the gutter width.
    #[gpui::test]
    fn gutter_syntax_editor_paints_and_hit_tests(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            let input = cx.new(|cx| {
                ComposerInput::new(cx)
                    .with_gutter(true)
                    .with_wrap(false)
                    .with_fill(true)
            });
            input.update(cx, |input, cx| {
                input.set_syntax(Some(Lang::Rust), cx);
                input.set_text_at_start("fn main() {\n    let x = 1;\n}\n", cx);
            });
            input
        });
        let harness = cx.update(|_, cx| {
            cx.new(|_| EditorHarness {
                input: input.clone(),
            })
        });
        draw_editor(cx, &harness);

        let (gutter, rows) = cx.update(|_, cx| {
            let input = input.read(cx);
            (input.last_gutter, input.last_total_rows)
        });
        assert!(gutter > px(0.), "the gutter took width");
        assert!(rows >= 3, "the three logical lines are rows");

        // A click just right of the gutter maps to column 0 of the first line.
        let bounds = cx.update(|_, cx| input.read(cx).last_bounds.expect("bounds"));
        // Regression guard: the editor must actually occupy the pane. A row
        // parent left it zero-height and the file rendered blank.
        assert!(
            bounds.size.height >= px(100.),
            "editor collapsed to {}px",
            bounds.size.height
        );
        let idx = cx.update(|_, cx| {
            input.update(cx, |input, _| {
                input.index_for_mouse_position(point(
                    bounds.origin.x + gutter + px(1.),
                    bounds.origin.y + px(2.),
                ))
            })
        });
        assert_eq!(idx, 0, "click to the right of the gutter is line start");
    }

    /// Undo rewinds an edit and redo replays it; a programmatic `set_text`
    /// starts a clean history.
    #[gpui::test]
    fn undo_and_redo_round_trip(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.new(ComposerInput::new)
        });
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_text("abc", cx);
                // Two separate commits (the caret move breaks coalescing).
                input.replace_range(3..3, "d", cx);
                input.move_to(0, cx);
                input.replace_range(0..0, "X", cx);
            });
            assert_eq!(input.read(cx).text(), "Xabcd");

            input.update(cx, |input, cx| input.undo(&Undo, window, cx));
            assert_eq!(input.read(cx).text(), "abcd");
            input.update(cx, |input, cx| input.undo(&Undo, window, cx));
            assert_eq!(input.read(cx).text(), "abc");
            // Nothing left to undo is a no-op, not a panic.
            input.update(cx, |input, cx| input.undo(&Undo, window, cx));
            assert_eq!(input.read(cx).text(), "abc");

            input.update(cx, |input, cx| input.redo(&Redo, window, cx));
            assert_eq!(input.read(cx).text(), "abcd");
            input.update(cx, |input, cx| input.redo(&Redo, window, cx));
            assert_eq!(input.read(cx).text(), "Xabcd");
        });
    }

    fn ime_input(cx: &mut VisualTestContext) -> Entity<ComposerInput> {
        cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.new(ComposerInput::new)
        })
    }

    /// Issue #51 regression: incremental IME composition ("n" → "ni" → "你")
    /// replaces the previous preedit in place. The old stub mapped the
    /// composition-relative selection against the whole document, producing
    /// out-of-range selections that the next update sliced and panicked on.
    #[gpui::test]
    fn ime_composition_replaces_the_previous_preedit_in_place(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = ime_input(cx);
        cx.update(|window, cx| {
            // Windows sends `GCS_COMPSTR` updates with no replacement range and
            // a selection relative to the composition text.
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "n", Some(1..1), window, cx);
                input.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
                input.replace_and_mark_text_in_range(None, "你", Some(1..1), window, cx);
            });
            let (text, marked, selected, composing) = input.update(cx, |input, cx| {
                (
                    input.text(),
                    input.marked_text_range(window, cx),
                    input.selected_range.clone(),
                    input.is_composing(),
                )
            });
            assert_eq!(text, "你", "each update replaced the previous preedit");
            assert_eq!(marked, Some(0..1), "marked range is UTF-16");
            assert_eq!(selected, "你".len().."你".len());
            assert!(composing);

            // The commit path (`GCS_RESULTSTR`) replaces the marked span, not
            // the selection, and ends the composition.
            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "你好", window, cx)
            });
            let (text, marked, composing) = input.update(cx, |input, cx| {
                (
                    input.text(),
                    input.marked_text_range(window, cx),
                    input.is_composing(),
                )
            });
            assert_eq!(text, "你好", "commit replaced the preedit, not appended");
            assert_eq!(marked, None);
            assert!(!composing, "submit works again once composition ends");
        });
    }

    /// Composing inside existing text splices over the marked span, and the
    /// IME's selection — relative to the preedit — maps into the document.
    #[gpui::test]
    fn ime_composition_splices_inside_existing_text(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = ime_input(cx);
        cx.update(|_, cx| {
            input.update(cx, |input, cx| {
                input.set_text("hello world", cx);
                input.move_to(6, cx); // before "world"
            });
        });
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "n", None, window, cx);
                input.replace_and_mark_text_in_range(None, "ni", Some(1..1), window, cx);
            });
            let (text, marked, selected, composing) = input.update(cx, |input, cx| {
                (
                    input.text(),
                    input.marked_text_range(window, cx),
                    input.selected_range.clone(),
                    input.is_composing(),
                )
            });
            assert_eq!(text, "hello niworld");
            assert_eq!(marked, Some(6..8));
            assert_eq!(selected, 7..7, "selection is relative to the preedit");
            assert!(composing);

            // Cancel: the platform replaces the marked span with an empty
            // string, removing the preedit without touching the rest.
            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "", window, cx)
            });
            let (text, marked, composing) = input.update(cx, |input, cx| {
                (
                    input.text(),
                    input.marked_text_range(window, cx),
                    input.is_composing(),
                )
            });
            assert_eq!(text, "hello world", "cancellation removed the preedit");
            assert_eq!(marked, None);
            assert!(!composing);
        });
    }

    /// An empty platform range and an empty composition both clamp instead of
    /// slicing, and `unmark_text` ends the composition without dropping text.
    #[gpui::test]
    fn ime_stale_ranges_clamp_and_unmark_clears(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = ime_input(cx);
        cx.update(|window, cx| {
            // A selection that overruns the short preedit clamps to its end
            // instead of setting an invalid document selection.
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "n", Some(99..120), window, cx)
            });
            let (text, marked, selected) = input.update(cx, |input, cx| {
                (
                    input.text(),
                    input.marked_text_range(window, cx),
                    input.selected_range.clone(),
                )
            });
            assert_eq!(text, "n");
            assert_eq!(marked, Some(0..1));
            assert_eq!(selected, 1..1);

            // An out-of-bounds replacement range clamps to the buffer rather
            // than panicking: 0..900 covers the preedit and commits "ok".
            input.update(cx, |input, cx| {
                input.replace_text_in_range(Some(0..900), "ok", window, cx)
            });
            assert_eq!(input.read(cx).text(), "ok");
            assert!(!input.read(cx).is_composing());

            // A range entirely past the end clamps to the end (append).
            input.update(cx, |input, cx| {
                input.replace_text_in_range(Some(400..900), "!", window, cx)
            });
            assert_eq!(input.read(cx).text(), "ok!");

            // `unmark_text` ends composition but keeps the text, like
            // NSTextView; an empty composition clears the preedit outright.
            input.update(cx, |input, cx| {
                input.clear(cx);
                input.replace_and_mark_text_in_range(None, "n", None, window, cx);
                input.replace_and_mark_text_in_range(None, "", None, window, cx);
            });
            assert_eq!(input.read(cx).text(), "", "empty composition cleared");
            assert!(!input.read(cx).is_composing());

            input.update(cx, |input, cx| {
                input.clear(cx);
                input.replace_and_mark_text_in_range(None, "ni", None, window, cx);
                input.unmark_text(window, cx);
            });
            assert_eq!(input.read(cx).text(), "ni", "unmark keeps the text");
            assert!(!input.read(cx).is_composing());
        });
    }

    /// Non-BMP characters are two UTF-16 units; a selection inside a surrogate
    /// pair clamps to the scalar's end and commit spans all four bytes.
    #[gpui::test]
    fn ime_non_bmp_composition_tracks_utf16_offsets(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let input = ime_input(cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "😀", Some(1..1), window, cx);
            });
            let (marked, selected) = input.update(cx, |input, cx| {
                (
                    input.marked_text_range(window, cx),
                    input.selected_range.clone(),
                )
            });
            assert_eq!(marked, Some(0..2));
            assert_eq!(selected, 4..4, "a surrogate-half selection clamps");

            input.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "😀!", Some(3..3), window, cx);
            });
            let (text, marked, selected) = input.update(cx, |input, cx| {
                (
                    input.text(),
                    input.marked_text_range(window, cx),
                    input.selected_range.clone(),
                )
            });
            assert_eq!(text, "😀!");
            assert_eq!(marked, Some(0..3));
            assert_eq!(selected, 5..5, "3 UTF-16 units = 5 bytes");

            input.update(cx, |input, cx| {
                input.replace_text_in_range(None, "😀!", window, cx)
            });
            assert_eq!(input.read(cx).text(), "😀!");
            assert!(!input.read(cx).is_composing());
        });
    }
}
