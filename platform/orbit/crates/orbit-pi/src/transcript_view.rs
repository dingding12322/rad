//! Transcript paint — transcript layout, honest Orbit data.
//!
//! Rows are plain GPUI flex trees (no component library): user turns are
//! End-aligned neutral bubbles, assistant turns are Start-aligned prose. A
//! ghost copy/timestamp footer reveals on row hover. Settled: **Worked for**
//! fold → thinking/tool cards → answer → files → copy footer. Live: thinking
//! and activity cards → answer → files → **Working for** — work stays in
//! sequence with the text as it arrives from the agent.
//!
//! Timestamps render on the footer when pi provides one (snapshot
//! `timestamp` fields, or a wall-clock stamp taken at `message_end`).
//! Not painted (pi does not provide them): git Review, conversation fork,
//! per-tool checkpoint diffs — an edit/write tool instead shows an inline
//! diff built from its own `oldText`/`newText` arguments.

use std::{
    cell::{Cell, RefCell},
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    anchored, canvas, deferred, div, img, linear_color_stop, linear_gradient, list, point,
    prelude::*, px, Animation, AnimationExt, AnyElement, App, Bounds, ClipboardItem, CursorStyle,
    DispatchPhase, Element, ElementId, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId,
    Hitbox, HitboxBehavior, Hsla, Image, ImageSource, InspectorElementId, InteractiveText,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    ScrollHandle, ScrollWheelEvent, SharedString, StrikethroughStyle, StyledText, TextAlign,
    TextLayout, TextRun, TransformationMatrix, UnderlineStyle, Window,
};

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use serde_json::Value;

use orbit_rpc::MessageUsage;

use crate::app::{
    button_frame, file_badge, file_glyph, icon_button_frame, nerd_font_family, picker_entry,
    picker_surface, TipExt, BUTTON_GROUP,
};
use crate::context_meter::{format_tokens, hit_percent_label};
use crate::highlight::{self, Token};
use crate::message_scroller::{self, MessageScrollerState};
use crate::shimmer::ShimmerText;
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{
    context_menu, picker, popover, BufferLineHeight, ButtonSize, DynamicSpacing, IconSize, Radius,
    StyledExt, TextSize, BORDER_WIDTH,
};
use crate::theme::{self, Theme, ThemeMode};
use crate::transcript::{ChatMessage, Step, SummaryRow, ToolCall, ToolFacts};

/// Opens the changed-files Review in the side pane (see `sidepane.rs`) —
/// handed down from the app so the transcript's Review buttons can point
/// at it without knowing the pane exists.
pub(crate) type ReviewOpener = Rc<dyn Fn(&mut Window, &mut App)>;

/// Opens one attachment image in the app's full-window lightbox. Handed down
/// from the app so an image tile can open a surface it doesn't own.
pub(crate) type ImageOpener = Rc<dyn Fn(Arc<Image>, &mut Window, &mut App)>;

/// The transcript content column's max width.
/// Normal message content keeps this centered measure. Assistant tables
/// break out to the transcript pane's width; their scroll viewport must not
/// inherit this cap.
const CONTENT_MAX_WIDTH: f32 = 960.0;
/// User-bubble max width.
const USER_BUBBLE_MAX_WIDTH: f32 = 540.0;
/// Message-footer action button size; the footer row, its timestamp, and
/// its usage metric share the button's height.
const FOOTER_BUTTON: ButtonSize = ButtonSize::Medium;
const COPY_FEEDBACK: Duration = Duration::from_secs(2);
const NAVIGATION_RAIL_LEFT: f32 = 16.0;
const NAVIGATION_RAIL_WIDTH: f32 = 44.0;
const NAVIGATION_RAIL_TICK_WIDTH: f32 = 32.0;
const NAVIGATION_RAIL_TICK_HEIGHT: f32 = 2.0;
const NAVIGATION_RAIL_TURN_HEIGHT: f32 = 12.0;
const NAVIGATION_RAIL_INACTIVE_OPACITY: f32 = 0.45;
/// Tick width by emphasis distance from the hovered turn.
const NAVIGATION_RAIL_EMPHASIS_SCALE: [f32; 4] = [1.0, 0.68, 0.44, 0.25];
/// The rail caps at 80% of the viewport and hides below an 872px transcript
/// container.
const NAVIGATION_RAIL_MAX_HEIGHT: f32 = 0.8;
const NAVIGATION_RAIL_MIN_MAIN_WIDTH: f32 = 1040.0;
const CHANGED_FILES_PREVIEW_LIMIT: usize = 3;
const NAVIGATION_RAIL_CONTENT_GAP: f32 = 12.0;
const NAVIGATION_RAIL_PREVIEW_WIDTH: f32 = 320.0;
const NAVIGATION_RAIL_PREVIEW_MAX_HEIGHT: f32 = 126.0;
/// Detail JSON/outputs are truncated to keep one virtualized row bounded.
const DETAIL_TEXT_CAP: usize = 4000;
/// Tool *output* gets a much larger budget than arguments — logs are the
/// point of the Output section. The collapsed preview keeps the row small;
/// expanding paints up to [`OUTPUT_EXPANDED_PAINT_LINES`] lines.
const OUTPUT_TEXT_CAP: usize = 100_000;
/// A detail section taller than this collapses to a preview…
const OUTPUT_COLLAPSE_LINES: usize = 16;
/// …showing this many lines collapsed.
const OUTPUT_PREVIEW_LINES: usize = 12;
/// Reasoning is bounded by height, not characters: a "Thought" card taller
/// than this scrolls, so a long chain of thought stays readable without
/// burying the answer below it.
const THINKING_MAX_HEIGHT: f32 = 260.0;
/// Reasoning budget once the card scrolls — far larger than
/// [`DETAIL_TEXT_CAP`], since the max height keeps the virtualized row
/// bounded regardless of how much text is retained.
const THINKING_TEXT_CAP: usize = 40_000;
/// Even expanded, one virtualized row paints at most this many output lines
/// (the copy button always carries the full captured text).
const OUTPUT_EXPANDED_PAINT_LINES: usize = 400;
/// A settled code block taller than this collapses to a preview…
const CODE_COLLAPSE_LINES: usize = 28;
/// …showing this many lines collapsed. Live (streaming) rows never collapse
/// — a growing block's edge must stay visible.
const CODE_PREVIEW_LINES: usize = 24;
/// Code-block copy feedback shares the detail-section feedback map; section
/// `2` never collides with Arguments (`0`) / Output (`1`).
const CODE_COPY_SECTION: u8 = 2;
const CODE_COPY_BUTTON: ButtonSize = ButtonSize::Medium;
/// Inline edit diff — an edit/write tool's `oldText`/`newText` shown as a
/// compact diff. Row height and text size track the Review pane's diff.
const EDIT_DIFF_TEXT_SIZE: f32 = 12.5;
const EDIT_DIFF_ROW_HEIGHT: f32 = 19.0;
/// Copy feedback for an inline edit diff; section `3` never collides with
/// Arguments (`0`) / Output (`1`) / code block (`2`).
const EDIT_DIFF_COPY_SECTION: u8 = 3;
/// A diff taller than this collapses to a preview so one virtualized row
/// stays bounded; expanding paints at most [`EDIT_DIFF_PAINT_LINES`].
const EDIT_DIFF_COLLAPSE_LINES: usize = 40;
const EDIT_DIFF_PREVIEW_LINES: usize = 24;
const EDIT_DIFF_PAINT_LINES: usize = 400;

type ExpandedActivities = Rc<RefCell<HashMap<(usize, usize), bool>>>;
type ExpandedTools = Rc<RefCell<HashSet<(usize, usize)>>>;
type CopiedSections = Rc<RefCell<HashMap<(usize, usize, u8), Instant>>>;
/// Tool detail sections expanded past their collapsed preview, keyed
/// `(message_ix, flat_tool_ix, section)`.
type ExpandedSections = Rc<RefCell<HashSet<(usize, usize, u8)>>>;
/// Code blocks expanded past their collapsed preview, keyed
/// `(message_ix, prose_salt, block_ix)`.
type ExpandedBlocks = Rc<RefCell<HashSet<(usize, u64, usize)>>>;
/// Persistent scroll handles for expandable "Thought" cards, keyed
/// `(message_ix, step_ix)`. The handle lets the card read its own
/// offset/limit so a wheel that no longer scrolls the card can be chained
/// to the transcript (rather than swallowed by `occlude`).
pub(crate) type ThinkingScrolls = Rc<RefCell<HashMap<(usize, usize), ScrollHandle>>>;
/// "Thought" cards collapsed by the reader, keyed `(message_ix, step_ix)`.
/// Missing means expanded — the default once the activity group is open.
pub(crate) type CollapsedThoughts = Rc<RefCell<HashSet<(usize, usize)>>>;
/// Live "Thought" cards the reader scrolled away from the newest line,
/// keyed `(message_ix, step_ix)`. Missing means following: while a card
/// streams, its view stays pinned to the bottom so each new line of
/// reasoning is visible. Present means detached — the reader scrolled up
/// and the card stops chasing the stream until they return to the bottom.
pub(crate) type ThinkingDetached = Rc<RefCell<HashSet<(usize, usize)>>>;

pub(crate) struct TranscriptView {
    pub messages: Rc<RefCell<Vec<ChatMessage>>>,
    /// Cross-block text selection + the right-click copy menu.
    pub text_selection: TextSelectionState,
    pub scroller: MessageScrollerState,
    pub streaming: Rc<Cell<Option<usize>>>,
    pub stream_started: Rc<Cell<Option<Instant>>>,
    /// A prompt is out and nothing has streamed yet: the tail slot shows the
    /// live row so the send is acknowledged before the first token.
    pub awaiting_reply: Rc<Cell<bool>>,
    /// When the current turn's provider request began (`turn_start`). The
    /// live rate divides by this clock, the same basis as pi's settled
    /// `durationMs`, so a bursty stream is not read as the generation.
    pub turn_started: Rc<Cell<Option<Instant>>>,
    pub expanded_turns: Rc<RefCell<HashSet<usize>>>,
    pub expanded_files: Rc<RefCell<HashSet<usize>>>,
    pub expanded_activities: ExpandedActivities,
    /// Per-tool detail-card open state, keyed `(message_ix, tool_ix)`.
    pub expanded_tools: ExpandedTools,
    pub copied: Rc<RefCell<HashMap<usize, Instant>>>,
    /// Per detail-section copy feedback, keyed `(message_ix, tool_ix, section)`.
    pub copied_sections: CopiedSections,
    /// Tool output sections expanded past their collapsed preview.
    pub expanded_sections: ExpandedSections,
    /// Code blocks expanded past their collapsed preview.
    pub expanded_blocks: ExpandedBlocks,
    /// Persistent scroll handles for the "Thought" cards.
    pub thinking_scrolls: ThinkingScrolls,
    /// "Thought" cards collapsed by the reader.
    pub collapsed_thoughts: CollapsedThoughts,
    /// Live "Thought" cards the reader scrolled away from the newest line.
    pub thinking_detached: ThinkingDetached,
    /// Rail tick currently hovered (drives the turn preview card).
    pub hovered_turn: Rc<Cell<Option<usize>>>,
    /// Assistant row whose footer usage metric is hovered (drives the
    /// per-message token/cost breakdown card).
    pub hovered_usage: Rc<Cell<Option<usize>>>,
    /// One-time rail hint: dismissal state (per install) + when it was
    /// first shown (drives its self-dismiss timeout).
    pub rail_hint_dismissed: Rc<Cell<bool>>,
    pub rail_hint_shown_at: Rc<Cell<Option<Instant>>>,
    /// Workspace of the open session — roots the Review git diff.
    pub workspace: Option<PathBuf>,
    /// Viewport height (caps the rail at 80%).
    pub viewport_height: Pixels,
    /// Main-area width (gates the rail at 872px).
    pub main_width: Pixels,
    /// Rail scroll position + last auto-scrolled turn.
    pub rail_scroll: ScrollHandle,
    pub rail_autoscroll: Rc<Cell<Option<usize>>>,
    /// End-of-task changed-files summary — `Some` renders one extra row
    /// after the last message (the list's tail slot).
    pub summary_files: Option<Vec<(String, u64, u64)>>,
    /// When the settled run finished (last message's timestamp) — shown in
    /// the summary card's footer next to the copy affordance.
    pub summary_finished_at: Option<i64>,
    /// The whole run's reported usage — carried onto the summary footer so
    /// the copy/time/usage details are shown once, on the last element,
    /// instead of duplicated above and below the changed-files card.
    pub summary_usage: Option<MessageUsage>,
    /// Opens the changed-files Review in the side pane (the cards'
    /// "Review" buttons); `None` hides the buttons.
    pub review_changes: Option<ReviewOpener>,
    /// Opens an attachment image in the full-window lightbox; `None` leaves
    /// the tiles non-interactive.
    pub image_opener: Option<ImageOpener>,
    /// Message indices matching the open find query; `None` when find is
    /// closed. Rows get a quiet wash.
    pub search_hits: Option<Rc<RefCell<HashSet<usize>>>>,
    /// The selected find hit (stronger wash + a ring); `None` when find is
    /// closed.
    pub search_active: Option<Rc<Cell<Option<usize>>>>,
    /// User-turn row indices, cached in `Transcript` and shared here so the
    /// rail's O(message-count) pass does not run every frame.
    pub user_turns: Rc<Vec<usize>>,
}

// ── text selection ─────────────────────────────────────────────────────────

/// One rendered text block from the last painted frame, in visual order.
/// Registered by [`SelectableText`] so the transcript panel's mouse handlers
/// can hit-test a window position to a (block, byte offset) without owning
/// the layout themselves.
struct TextBlock {
    key: ElementId,
    /// The block's plain text — what a selection contributes to the clipboard.
    text: SharedString,
    /// Message this block rendered from — right-click "Copy Message" target.
    message_ix: usize,
    /// Link ranges inside `text` (markdown links), resolved on a plain click.
    links: Vec<(Range<usize>, String)>,
    layout: TextLayout,
    bounds: Bounds<Pixels>,
}

/// A block captured when a selection began; the selection's `blocks` list is
/// the visual order at that moment, so copying does not depend on later
/// frames or virtualization.
struct SelectedBlock {
    key: ElementId,
    text: SharedString,
}

/// One endpoint of a selection: index into [`Selection::blocks`] plus a byte
/// offset into that block's text.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SelectPoint {
    block: usize,
    offset: usize,
}

/// An active cross-block selection. Intermediate blocks are fully selected;
/// only the anchor and focus blocks carry partial ranges.
struct Selection {
    blocks: Vec<SelectedBlock>,
    anchor: SelectPoint,
    focus: SelectPoint,
}

impl Selection {
    fn normalized(&self) -> (SelectPoint, SelectPoint) {
        if (self.focus.block, self.focus.offset) < (self.anchor.block, self.anchor.offset) {
            (self.focus, self.anchor)
        } else {
            (self.anchor, self.focus)
        }
    }
}

/// The right-click menu over transcript text: `Copy Selection` reads the live
/// selection, `Copy Message` carries the message text captured on open.
#[derive(Clone)]
struct TextMenu {
    position: gpui::Point<Pixels>,
    message_text: String,
}

/// Shared text-selection state — one per transcript, cloned into every text
/// element and the panel's mouse handlers so all of them see the same drag.
pub(crate) struct TextSelection {
    /// Blocks registered by the current frame's paint, in visual order.
    blocks: Vec<TextBlock>,
    /// Key -> index into [`TextSelection::blocks`] (dedupe + lookup).
    index: HashMap<ElementId, usize>,
    /// Bumped once per transcript render; the first block painted after a
    /// bump drops the previous frame's registrations.
    frame: u64,
    painted_frame: u64,
    selection: Option<Selection>,
    dragging: bool,
    /// The drag moved away from the anchor — mouse-up is not a link click.
    moved: bool,
    /// The press that started the drag, for link resolution on release.
    down: Option<(ElementId, usize)>,
    menu: Option<TextMenu>,
}

pub(crate) type TextSelectionState = Rc<RefCell<TextSelection>>;

impl TextSelection {
    pub(crate) fn new() -> Self {
        Self {
            blocks: Vec::new(),
            index: HashMap::new(),
            frame: 0,
            painted_frame: 0,
            selection: None,
            dragging: false,
            moved: false,
            down: None,
            menu: None,
        }
    }

    /// Start a new render frame: blocks registered by the next paint replace
    /// the previous frame's registrations.
    fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    fn register(&mut self, block: TextBlock) {
        if self.painted_frame != self.frame {
            self.painted_frame = self.frame;
            self.blocks.clear();
            self.index.clear();
        }
        match self.index.get(&block.key).copied() {
            Some(existing) => self.blocks[existing] = block,
            None => {
                self.index.insert(block.key.clone(), self.blocks.len());
                self.blocks.push(block);
            }
        }
    }

    /// Index and byte offset of the text under `position`: the containing
    /// block, or the nearest one when the pointer sits in a gutter.
    fn hit(&self, position: gpui::Point<Pixels>) -> Option<(usize, usize)> {
        let index = self
            .blocks
            .iter()
            .position(|block| block.bounds.contains(&position))
            .or_else(|| self.nearest_block(position))?;
        let block = &self.blocks[index];
        let local = point(
            position.x.clamp(block.bounds.left(), block.bounds.right()),
            position.y.clamp(block.bounds.top(), block.bounds.bottom()),
        );
        let offset = match block.layout.index_for_position(local) {
            Ok(offset) | Err(offset) => offset,
        };
        Some((index, offset.min(block.text.len())))
    }

    fn nearest_block(&self, position: gpui::Point<Pixels>) -> Option<usize> {
        self.blocks
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = gutter_distance(a, position);
                let db = gutter_distance(b, position);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
    }

    fn begin_selection(&mut self, position: gpui::Point<Pixels>) {
        self.menu = None;
        self.moved = false;
        self.down = None;
        let Some((index, offset)) = self.hit(position) else {
            self.selection = None;
            self.dragging = false;
            return;
        };
        let key = self.blocks[index].key.clone();
        let blocks = self
            .blocks
            .iter()
            .map(|block| SelectedBlock {
                key: block.key.clone(),
                text: block.text.clone(),
            })
            .collect();
        let point = SelectPoint {
            block: index,
            offset,
        };
        self.down = Some((key, offset));
        self.selection = Some(Selection {
            blocks,
            anchor: point,
            focus: point,
        });
        self.dragging = true;
    }

    fn extend_selection(&mut self, position: gpui::Point<Pixels>) {
        if !self.dragging {
            return;
        }
        let Some((index, offset)) = self.hit(position) else {
            return;
        };
        let key = self.blocks[index].key.clone();
        let Some(selection) = self.selection.as_mut() else {
            return;
        };
        let Some(target) = selection.blocks.iter().position(|block| block.key == key) else {
            return;
        };
        let next = SelectPoint {
            block: target,
            offset,
        };
        if next != selection.anchor {
            self.moved = true;
        }
        selection.focus = next;
    }

    /// End a drag; a press that never moved resolves to the link under it.
    fn finish_selection(&mut self) -> Option<String> {
        self.dragging = false;
        if self.moved {
            return None;
        }
        let (key, offset) = self.down.take()?;
        let block = self.blocks.iter().find(|block| block.key == key)?;
        block
            .links
            .iter()
            .find(|(range, _)| range.contains(&offset))
            .map(|(_, url)| url.clone())
    }

    fn clear_selection(&mut self) -> bool {
        let had = self.selection.take().is_some();
        self.dragging = false;
        self.down = None;
        self.moved = false;
        had
    }

    /// Drop the selection and any open menu (session switch, outside click).
    pub(crate) fn clear(&mut self) {
        self.clear_selection();
        self.menu = None;
    }

    /// Message index of the block under `position` (context menu). A press in
    /// the row padding between blocks still counts when it lands close to one.
    fn message_at(&self, position: gpui::Point<Pixels>) -> Option<usize> {
        if let Some(index) = self
            .blocks
            .iter()
            .position(|block| block.bounds.contains(&position))
        {
            return Some(self.blocks[index].message_ix);
        }
        let (index, distance) = self
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (index, gutter_distance(block, position)))
            .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))?;
        (distance < 24.).then(|| self.blocks[index].message_ix)
    }

    /// Byte range of this block that is selected, clamped to its current
    /// text and snapped to char boundaries (the text can shrink mid-stream).
    fn range_for(&self, key: &ElementId, text: &str) -> Option<Range<usize>> {
        let selection = self.selection.as_ref()?;
        let (start, end) = selection.normalized();
        let index = selection
            .blocks
            .iter()
            .position(|block| &block.key == key)?;
        if index < start.block || index > end.block {
            return None;
        }
        let from = if index == start.block {
            byte_boundary(text, start.offset)
        } else {
            0
        };
        let to = if index == end.block {
            byte_boundary(text, end.offset)
        } else {
            text.len()
        };
        (from < to).then_some(from..to)
    }

    /// The selected text, one line per selected block (the clipboard form).
    pub(crate) fn selected_text(&self) -> Option<String> {
        let selection = self.selection.as_ref()?;
        let (start, end) = selection.normalized();
        if start == end {
            return None;
        }
        let mut parts: Vec<&str> = Vec::new();
        for (index, block) in selection.blocks.iter().enumerate() {
            if index < start.block || index > end.block {
                continue;
            }
            let from = if index == start.block {
                byte_boundary(&block.text, start.offset)
            } else {
                0
            };
            let to = if index == end.block {
                byte_boundary(&block.text, end.offset)
            } else {
                block.text.len()
            };
            if from < to {
                parts.push(&block.text[from..to]);
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }
}

fn gutter_distance(block: &TextBlock, position: gpui::Point<Pixels>) -> f32 {
    let dx: f32 = (position.x - position.x.clamp(block.bounds.left(), block.bounds.right()))
        .abs()
        .into();
    let dy: f32 = (position.y - position.y.clamp(block.bounds.top(), block.bounds.bottom()))
        .abs()
        .into();
    dx + dy
}

/// Snap a byte offset back to a char boundary (layout offsets are glyph
/// boundaries already; this guards stale offsets from a streaming edit).
fn byte_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Split `runs` so the selected byte range carries the selection wash as a
/// run background — gpui paints run backgrounds itself, so the highlight
/// needs no separate layout math and never reflows text.
fn highlight_runs(runs: Vec<TextRun>, range: Option<Range<usize>>, color: Hsla) -> Vec<TextRun> {
    let Some(range) = range else {
        return runs;
    };
    if range.is_empty() {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut offset = 0usize;
    for run in runs {
        let start = offset;
        let end = offset + run.len;
        offset = end;
        let hi_from = range.start.max(start);
        let hi_to = range.end.min(end);
        if hi_from >= hi_to {
            out.push(run);
            continue;
        }
        if hi_from > start {
            out.push(TextRun {
                len: hi_from - start,
                ..run.clone()
            });
        }
        out.push(TextRun {
            len: hi_to - hi_from,
            background_color: Some(color),
            ..run.clone()
        });
        if hi_to < end {
            out.push(TextRun {
                len: end - hi_to,
                ..run
            });
        }
    }
    out
}

fn selection_color(theme: Theme) -> Hsla {
    theme.accent.opacity(if theme.mode == ThemeMode::Dark {
        0.30
    } else {
        0.20
    })
}

thread_local! {
    /// The active selectable-text scope, set while a transcript row's element
    /// tree is built. `None` outside the transcript (the skills viewer), where
    /// prose keeps the plain clickable [`InteractiveText`].
    static TEXT_SCOPE: RefCell<Option<TextScope>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct TextScope {
    state: TextSelectionState,
    message_ix: usize,
}

struct TextScopeGuard(Option<TextScope>);

impl TextScopeGuard {
    fn enter(scope: TextScope) -> Self {
        Self(TEXT_SCOPE.with(|current| current.replace(Some(scope))))
    }
}

impl Drop for TextScopeGuard {
    fn drop(&mut self) {
        TEXT_SCOPE.with(|current| current.replace(self.0.take()));
    }
}

fn text_scope() -> Option<TextScope> {
    TEXT_SCOPE.with(|current| current.borrow().clone())
}

/// A [`StyledText`] that registers itself with the transcript's selection
/// state so the panel can hit-test it. Styling comes from the caller's wrapper
/// div, exactly as with a bare [`StyledText`].
struct SelectableText {
    key: ElementId,
    text: StyledText,
    /// Plain text this block contributes to the clipboard.
    plain: SharedString,
    links: Vec<(Range<usize>, String)>,
    /// Byte ranges whose font glyph is blanked and replaced by a hugeicon
    /// painted over the reserved space — the external-link arrow after a
    /// bare URL in prose.
    marks: Vec<Range<usize>>,
    mark_color: Hsla,
    mark_size: Pixels,
    scope: Option<TextScope>,
}

impl SelectableText {
    fn new(
        key: ElementId,
        text: StyledText,
        plain: SharedString,
        links: Vec<(Range<usize>, String)>,
        scope: Option<TextScope>,
    ) -> Self {
        Self {
            key,
            text,
            plain,
            links,
            marks: Vec::new(),
            mark_color: Hsla::transparent_black(),
            mark_size: px(0.),
            scope,
        }
    }

    /// Paint the given byte ranges as hugeicon arrows in `color`; their font
    /// glyphs must already be invisible (see [`hide_autolink_marks`]).
    fn with_marks(mut self, marks: Vec<Range<usize>>, color: Hsla, size: Pixels) -> Self {
        self.marks = marks;
        self.mark_color = color;
        self.mark_size = size;
        self
    }
}

/// Draw the hugeicon external-link arrow over the glyph space an autolink
/// reserves. `range` covers just the arrow character: the runs keep its
/// advance (so the mark stays part of the clickable link and the copied
/// selection), while the icon centers on the box where the font glyph sat.
fn paint_link_mark(
    layout: &TextLayout,
    range: &Range<usize>,
    color: Hsla,
    size: Pixels,
    window: &mut Window,
    cx: &App,
) {
    let Some(start) = layout.position_for_index(range.start) else {
        return;
    };
    // A mark that wrapped alone still sits on its own line; the end position
    // must share it for the box to be the arrow's, not a whole row's.
    let center_x = match layout.position_for_index(range.end) {
        Some(end) if end.y == start.y => (start.x + end.x) * 0.5,
        _ => start.x + size * 0.5,
    };
    let top = start.y + (layout.line_height() - size) * 0.5;
    let bounds = Bounds::new(point(center_x - size * 0.5, top), gpui::size(size, size));
    let _ = window.paint_svg(
        bounds,
        "icons/arrow-up-right.svg".into(),
        TransformationMatrix::unit(),
        color,
        cx,
    );
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = (Hitbox, TextLayout);

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.text.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> (Hitbox, TextLayout) {
        self.text
            .prepaint(None, inspector_id, bounds, &mut (), window, cx);
        let layout = self.text.layout().clone();
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        (hitbox, layout)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        (hitbox, layout): &mut (Hitbox, TextLayout),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);
        for mark in &self.marks {
            paint_link_mark(layout, mark, self.mark_color, self.mark_size, window, cx);
        }
        if let Some(scope) = self.scope.as_ref() {
            // The cursor rides this block's own hitbox: an I-beam over text,
            // a hand on links. A block's request wins only while the pointer
            // is actually on the text, so in-row buttons keep their pointer.
            if hitbox.is_hovered(window) {
                let position = window.mouse_position();
                let offset = match layout.index_for_position(position) {
                    Ok(offset) | Err(offset) => offset,
                };
                let over_link = self.links.iter().any(|(range, _)| range.contains(&offset));
                let style = if over_link {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::IBeam
                };
                window.set_cursor_style(style, hitbox);
            }
            scope.state.borrow_mut().register(TextBlock {
                key: self.key.clone(),
                text: self.plain.clone(),
                message_ix: scope.message_ix,
                links: self.links.clone(),
                layout: layout.clone(),
                bounds,
            });
        }
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Register the transcript panel's mouse handlers: drag-select (cross-block),
/// right-click menu, link clicks, and click-outside clearing. The canvas
/// listener receives every window mouse event, so a drag that leaves the
/// panel keeps updating (clamped to the nearest text block).
fn register_text_selection(
    state: TextSelectionState,
    messages: Rc<RefCell<Vec<ChatMessage>>>,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    window: &mut Window,
) {
    window.on_mouse_event({
        let state = state.clone();
        move |event: &MouseDownEvent, phase, window, _| {
            if phase != DispatchPhase::Capture || !hitbox.is_hovered(window) {
                return;
            }
            let menu_open = state.borrow().menu.is_some();
            match event.button {
                MouseButton::Left => {
                    // The open menu owns the next press; its outside-click
                    // handler dismisses it without disturbing the selection.
                    if menu_open {
                        return;
                    }
                    state.borrow_mut().begin_selection(event.position);
                    window.refresh();
                }
                MouseButton::Right => {
                    let Some(message_ix) = state.borrow().message_at(event.position) else {
                        return;
                    };
                    let message_text = messages
                        .borrow()
                        .get(message_ix)
                        .map(ChatMessage::text)
                        .unwrap_or_default();
                    state.borrow_mut().menu = Some(TextMenu {
                        position: event.position,
                        message_text,
                    });
                    window.refresh();
                }
                _ => {}
            }
        }
    });
    window.on_mouse_event({
        let state = state.clone();
        move |event: &MouseMoveEvent, phase, window, _| {
            if phase != DispatchPhase::Capture || !state.borrow().dragging {
                return;
            }
            state.borrow_mut().extend_selection(event.position);
            window.refresh();
        }
    });
    window.on_mouse_event({
        let state = state.clone();
        move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            let url = state.borrow_mut().finish_selection();
            if let Some(url) = url {
                cx.open_url(&url);
            }
            window.refresh();
        }
    });
    window.on_mouse_event({
        let state = state.clone();
        move |event: &MouseDownEvent, phase, window, _| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            if bounds.contains(&event.position) {
                return;
            }
            if state.borrow_mut().clear_selection() {
                window.refresh();
            }
        }
    });
}

/// The right-click menu: `Copy Selection` (disabled without one) and
/// `Copy Message` (the clicked message's full text). Wears the branch
/// selector's picker shell, like every other dropdown in the app.
fn text_selection_menu(menu: &TextMenu, state: TextSelectionState, theme: Theme) -> AnyElement {
    let selected = state.borrow().selected_text();
    let copy_selection = selected.clone();
    let close = state.clone();
    let copy_selection_item = selection_menu_item(
        "transcript-copy-selection",
        "icons/copy.svg",
        tr!("transcript.copy_selection"),
        selected.is_some(),
        theme,
        move |window, cx| {
            if let Some(text) = copy_selection.clone() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            close.borrow_mut().menu = None;
            cx.stop_propagation();
            window.refresh();
        },
    );
    let copy_message = menu.message_text.clone();
    let close = state.clone();
    let copy_message_item = selection_menu_item(
        "transcript-copy-message",
        "icons/chat.svg",
        tr!("transcript.copy_message"),
        true,
        theme,
        move |window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(copy_message.clone()));
            close.borrow_mut().menu = None;
            cx.stop_propagation();
            window.refresh();
        },
    );
    let dismiss = state.clone();
    deferred(
        anchored()
            .position(menu.position)
            .snap_to_window_with_margin(popover::WINDOW_MARGIN)
            .child(
                picker_surface(div().id("transcript-text-menu"), &theme)
                    .w(px(220.))
                    .py(picker::list_padding_y(&theme))
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base01.px(&theme))
                    .overflow_hidden()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down_out(move |_, window, _| {
                        dismiss.borrow_mut().menu = None;
                        window.refresh();
                    })
                    .child(copy_selection_item)
                    .child(copy_message_item),
            ),
    )
    .into_any_element()
}

fn selection_menu_item(
    id: &'static str,
    icon_path: &'static str,
    label: String,
    enabled: bool,
    theme: Theme,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    picker_entry(div().id(id), &theme)
        .h(picker::entry_height(&theme))
        .flex_none()
        .text_color(if enabled {
            theme.text_secondary
        } else {
            theme.text_muted
        })
        .when(enabled, |item| {
            item.cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover).text_color(theme.text_primary))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    on_click(window, cx);
                })
        })
        .child(crate::app::icon(
            icon_path,
            context_menu::ICON.px(&theme),
            theme.text_muted,
        ))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .into_any_element()
}

struct RowPaint {
    messages: Rc<RefCell<Vec<ChatMessage>>>,
    /// Selection state threaded to every text block this row renders.
    text_selection: TextSelectionState,
    ix: usize,
    row_count: usize,
    theme: Theme,
    /// The Nerd Fonts family for devicons file glyphs, resolved once per
    /// frame; `None` when no Nerd Font is available (extension badges fall
    /// back).
    nerd: Option<SharedString>,
    live: bool,
    live_elapsed: Option<Duration>,
    /// Live output rate for the streaming row (`None` until meaningful).
    live_tps: Option<f64>,
    fold_open: bool,
    copied: bool,
    /// True for the last message when the tail summary owns the footer, so
    /// the per-message footer is not drawn a second time above it.
    suppress_footer: bool,
    expanded_turns: Rc<RefCell<HashSet<usize>>>,
    expanded_activities: ExpandedActivities,
    copied_at: Rc<RefCell<HashMap<usize, Instant>>>,
    scroller: MessageScrollerState,
    expanded_tools: ExpandedTools,
    copied_sections: CopiedSections,
    expanded_sections: ExpandedSections,
    expanded_blocks: ExpandedBlocks,
    thinking_scrolls: ThinkingScrolls,
    collapsed_thoughts: CollapsedThoughts,
    thinking_detached: ThinkingDetached,
    hovered_usage: Rc<Cell<Option<usize>>>,
    image_opener: Option<ImageOpener>,
    search_hit: bool,
    search_active: bool,
    /// Tables alone can extend beyond the centered message column.
    table_breakout: TableBreakout,
}

/// The last message's own footer is suppressed when the tail summary owns
/// the run footer (copy + time + usage), so those details render exactly once
/// — on the last element, after the changed-files card.
fn suppress_message_footer(has_summary: bool, ix: usize, row_count: usize) -> bool {
    has_summary && ix + 1 == row_count
}

pub(crate) fn render_transcript(view: TranscriptView, cx: &gpui::App) -> impl IntoElement + use<> {
    let theme = *theme::get(cx);
    let messages = view.messages.clone();
    let text_selection = view.text_selection.clone();
    text_selection.borrow_mut().begin_frame();
    // Clones the list closure cannot move: the panel's mouse handlers and
    // right-click menu need them after the list (and its rows) have captured
    // the originals.
    let events_selection = text_selection.clone();
    let events_messages = messages.clone();
    let menu_selection = text_selection.clone();
    let streaming = view.streaming.clone();
    let stream_started = view.stream_started.clone();
    let awaiting_reply = view.awaiting_reply.clone();
    let turn_started = view.turn_started.clone();
    let expanded_turns = view.expanded_turns.clone();
    let expanded_files = view.expanded_files.clone();
    let expanded_activities = view.expanded_activities.clone();
    let expanded_tools = view.expanded_tools.clone();
    let copied = view.copied.clone();
    let copied_sections = view.copied_sections.clone();
    let expanded_sections = view.expanded_sections.clone();
    let expanded_blocks = view.expanded_blocks.clone();
    let thinking_scrolls = view.thinking_scrolls.clone();
    let collapsed_thoughts = view.collapsed_thoughts.clone();
    let thinking_detached = view.thinking_detached.clone();
    let hovered_turn = view.hovered_turn.clone();
    let hovered_usage = view.hovered_usage.clone();
    let rail_hint_dismissed = view.rail_hint_dismissed.clone();
    let rail_hint_shown_at = view.rail_hint_shown_at.clone();
    let workspace = view.workspace.clone();
    let rail_scroll = view.rail_scroll.clone();
    let rail_autoscroll = view.rail_autoscroll.clone();
    let scroller = view.scroller.clone();
    let summary_files = view.summary_files.clone();
    let summary_finished_at = view.summary_finished_at;
    let summary_usage = view.summary_usage.clone();
    let review_changes = view.review_changes.clone();
    let image_opener = view.image_opener.clone();
    let search_hits = view.search_hits.clone();
    let search_active = view.search_active.clone();

    // `user_turns` is cached in `Transcript` (rebuilt only when the message
    // list changes); only `active_turn` depends on the live scroll position
    // and is recomputed per frame.
    let user_turns = view.user_turns.clone();
    let active_turn = {
        let messages = messages.borrow();
        // While the reader sits at the live edge the indicator tracks the
        // streaming/latest turn; once they scroll away it follows the
        // viewport instead (fixes the tick staying pinned to the newest
        // turn while reading earlier ones).
        let viewport_hint = if scroller.is_following_tail() {
            None
        } else {
            Some(scroller.first_visible_index())
        };
        active_user_index(&messages, streaming.get(), viewport_hint)
    };
    let show_rail = user_turns.len() >= 2
        && view.main_width >= px(NAVIGATION_RAIL_MIN_MAIN_WIDTH)
        && scroller.is_scrollable();
    // Reserve the rail's gutter only from spare space outside the normal
    // text strip, so expanding a table never makes it narrower than prose.
    let table_inset = if show_rail {
        ((view.main_width - px(40. + CONTENT_MAX_WIDTH)) / 2.).clamp(
            px(0.),
            px(NAVIGATION_RAIL_LEFT + NAVIGATION_RAIL_WIDTH + NAVIGATION_RAIL_CONTENT_GAP - 20.),
        )
    } else {
        px(0.)
    };
    let available_width = (view.main_width - px(40.)).max(px(0.));
    let column_width = available_width.min(px(CONTENT_MAX_WIDTH));
    let table_breakout = TableBreakout {
        max_width: (available_width - table_inset * 2.).max(column_width),
        column_width,
    };
    // The one-time rail hint shows while the rail is up, until the reader
    // uses it (tick click or turn jump) or its TTL lapses. While it shows,
    // it takes the preview card's slot so the two never stack.
    let rail_hint_active = show_rail && !rail_hint_dismissed.get();
    if rail_hint_active && rail_hint_shown_at.get().is_none() {
        rail_hint_shown_at.set(Some(Instant::now()));
    }

    // The rail's hover card is the only consumer of the preview text, and it
    // shows one turn at a time. Snapshot just the turn under the cursor: doing
    // this for every turn materialized the whole conversation on every repaint
    // and made long sessions crawl while a run streamed.
    let hover_info: Option<(String, String)> = if show_rail {
        hovered_turn.get().and_then(|ix| {
            let messages = messages.borrow();
            // The hovered index is persistent panel state and can outlive a
            // transcript swap (session switch), so never index unguarded.
            let prompt = snippet(&messages.get(ix)?.text(), 100);
            let response = messages
                .get(ix + 1..)
                .unwrap_or(&[])
                .iter()
                .take_while(|message| !message.user)
                .find(|message| !message.text().trim().is_empty())
                .map(|message| snippet(&message.text(), 240))
                .unwrap_or_default();
            Some((prompt, response))
        })
    } else {
        None
    };

    // Devicons resolve once per frame: every file row in the list shares the
    // same Nerd Fonts family (or falls back to extension badges).
    let nerd = nerd_font_family(cx);
    let list_el = list(view.scroller.list_state(), move |ix, _window, cx| {
        let live = streaming.get() == Some(ix);
        // The rate's clock: prefer the turn's request start (`turn_start`),
        // the same basis as pi's `durationMs`; fall back to the stream start
        // when an older pi build never emitted one.
        let rate_clock = turn_started
            .get()
            .or_else(|| stream_started.get())
            .map(|started| started.elapsed());
        // The "Working for…" clock measures the whole turn, so it must not
        // share the rate's clock: `turn_start` re-arms on every provider
        // request inside an agentic turn (once per tool-call step), which
        // restarted the timer mid-run (#59). `stream_started` is armed once
        // per turn and matches the settled `worked_for` duration.
        let live_elapsed = if live {
            stream_started.get().map(|started| started.elapsed())
        } else {
            None
        };
        // Tokens/sec for the streaming row: the provider's cumulative
        // output (or a chars/4 estimate) over the turn's request clock.
        let live_tps = if live {
            messages.borrow().get(ix).and_then(|message| {
                streaming_tps(message, live_generation_clock(message, rate_clock))
            })
        } else {
            None
        };
        let fold_open = expanded_turns.borrow().contains(&ix);
        let copied_now = copied
            .borrow()
            .get(&ix)
            .is_some_and(|at| at.elapsed() < COPY_FEEDBACK);
        let row_count = messages.borrow().len();
        // The list's tail slot (ix == row_count) is the end-of-task
        // changed-files summary — pinned after the last message, in the
        // same centered column as every other row.
        if ix >= row_count {
            let theme = *theme::get(cx);
            // The prompt is out; the answer has not started. Show the same live
            // row the streaming turn will wear, so pressing Enter is answered by
            // the transcript itself and not only by the composer's Stop button.
            if awaiting_reply.get() && streaming.get().is_none() {
                let elapsed = stream_started
                    .get()
                    .map(|started| started.elapsed())
                    .unwrap_or_default();
                return div()
                    .id(ElementId::NamedInteger("transcript-row".into(), ix as u64))
                    .w_full()
                    .flex()
                    .justify_center()
                    .px(DynamicSpacing::Base20.px(&theme))
                    .pb(px(14.))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(CONTENT_MAX_WIDTH))
                            .min_w_0()
                            .child(render_working_indicator(elapsed, None, None, theme)),
                    )
                    .into_any_element();
            }
            let Some(files) = summary_files.as_ref() else {
                return div().into_any_element();
            };
            let card = render_changed_files(
                files,
                theme,
                nerd.as_ref(),
                ix,
                workspace.as_deref(),
                expanded_files.borrow().contains(&ix),
                expanded_files.clone(),
                scroller.clone(),
                review_changes.clone(),
            );
            // One run footer, after the changed-files card (turn order:
            // answer → files card → Copy): the copy affordance copies the
            // answer, with the settled time and the run's usage. The last
            // message's own footer is suppressed (see `suppress_footer`) so
            // these details are not shown twice.
            let last_ix = row_count.saturating_sub(1);
            let copy_text = messages
                .borrow()
                .get(last_ix)
                .filter(|message| message.is_assistant())
                .map(|message| message.text())
                .unwrap_or_default();
            let copied_last = copied
                .borrow()
                .get(&last_ix)
                .is_some_and(|at| at.elapsed() < COPY_FEEDBACK);
            let stamp = summary_finished_at
                .map(|millis| footer_time_stamp(summary_time_label(millis), theme));
            let summary_generation = messages
                .borrow()
                .get(last_ix)
                .and_then(|message| message.generation_time());
            let footer = div()
                .mt(px(10.))
                .px(DynamicSpacing::Base04.px(&theme))
                .child(render_message_footer(
                    copy_text,
                    last_ix,
                    stamp,
                    summary_usage.clone(),
                    summary_generation,
                    copied_last,
                    false,
                    theme,
                    copied.clone(),
                    hovered_usage.clone(),
                ));
            return div()
                .id(ElementId::NamedInteger("transcript-row".into(), ix as u64))
                .w_full()
                .flex()
                .justify_center()
                .px(DynamicSpacing::Base20.px(&theme))
                .pb(px(22.))
                .child(
                    div()
                        .w_full()
                        .max_w(px(CONTENT_MAX_WIDTH))
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(card)
                        .child(footer),
                )
                .into_any_element();
        }
        render_row(RowPaint {
            messages: messages.clone(),
            text_selection: text_selection.clone(),
            ix,
            row_count,
            theme: *theme::get(cx),
            nerd: nerd.clone(),
            live,
            live_elapsed,
            live_tps,
            fold_open,
            copied: copied_now,
            suppress_footer: suppress_message_footer(summary_files.is_some(), ix, row_count),
            expanded_turns: expanded_turns.clone(),
            expanded_activities: expanded_activities.clone(),
            copied_at: copied.clone(),
            scroller: scroller.clone(),
            expanded_tools: expanded_tools.clone(),
            copied_sections: copied_sections.clone(),
            expanded_sections: expanded_sections.clone(),
            expanded_blocks: expanded_blocks.clone(),
            thinking_scrolls: thinking_scrolls.clone(),
            collapsed_thoughts: collapsed_thoughts.clone(),
            thinking_detached: thinking_detached.clone(),
            hovered_usage: hovered_usage.clone(),
            image_opener: image_opener.clone(),
            search_hit: search_hits
                .as_ref()
                .is_some_and(|hits| hits.borrow().contains(&ix)),
            search_active: search_active
                .as_ref()
                .is_some_and(|active| active.get() == Some(ix)),
            table_breakout,
        })
        .into_any_element()
    })
    .w_full()
    .h_full();

    // Same height chain as the sessions sidebar: this panel is a `flex_1` +
    // `min_h_0` child of a column, so `h_full` on the list is a definite
    // height. Jump-to-latest and the bottom fade live on the scroller wrap.
    let scroller = message_scroller::render_scroller(view.scroller.clone(), theme, list_el);
    // Selection handlers ride a transparent canvas over the panel: its paint
    // callback registers global mouse listeners, so a drag that leaves the
    // panel keeps extending (clamped to the nearest text block) instead of
    // freezing at the edge.
    let events = {
        let state = events_selection;
        let messages = events_messages;
        canvas(
            // One hitbox for the whole panel: `is_hovered` respects occluding
            // overlays (the find bar, popovers), so presses there never start
            // a selection behind them.
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, _| {
                register_text_selection(state, messages, bounds, hitbox, window);
            },
        )
        .absolute()
        .inset_0()
        .size_full()
    };
    let mut panel = div()
        .id(ElementId::Name("transcript-panel".into()))
        .w_full()
        .min_w_0()
        .h_full()
        .min_h_0()
        .relative()
        .child(scroller)
        .child(events)
        .when(show_rail, |shell| {
            shell.child(render_navigation_rail(
                hover_info,
                user_turns.as_slice(),
                active_turn,
                hovered_turn.clone(),
                view.scroller.clone(),
                view.viewport_height,
                rail_scroll,
                rail_autoscroll,
                theme,
                rail_hint_active,
                rail_hint_dismissed,
                rail_hint_shown_at,
            ))
        });
    let menu = menu_selection.borrow().menu.clone();
    if let Some(menu) = menu {
        panel = panel.child(text_selection_menu(&menu, menu_selection, theme));
    }
    panel
}

/// The conversation rail: ticks per user turn, vertically centered,
/// capped at 80% of the viewport, wheel-scrollable with edge fades. Tick
/// widths fan out around the *hovered* turn only; the active turn is the
/// full-opacity tick while idle.
#[allow(clippy::too_many_arguments)]
fn render_navigation_rail(
    hover_info: Option<(String, String)>,
    user_turns: &[usize],
    active_turn: Option<usize>,
    hovered: Rc<Cell<Option<usize>>>,
    scroller: MessageScrollerState,
    viewport_height: Pixels,
    rail_scroll: ScrollHandle,
    rail_autoscroll: Rc<Cell<Option<usize>>>,
    theme: Theme,
    rail_hint_active: bool,
    rail_hint_dismissed: Rc<Cell<bool>>,
    rail_hint_shown_at: Rc<Cell<Option<Instant>>>,
) -> AnyElement {
    let pitch = px(NAVIGATION_RAIL_TURN_HEIGHT);
    let content_height = px(user_turns.len() as f32 * NAVIGATION_RAIL_TURN_HEIGHT);
    let rail_height = content_height.min(viewport_height * NAVIGATION_RAIL_MAX_HEIGHT);
    let scrollable = content_height > rail_height + px(1.);
    let scroll_offset = rail_scroll.offset().y;

    // Scroll the active tick into view whenever it changes.
    let active_pos =
        active_turn.and_then(|ix| user_turns.iter().position(|candidate| *candidate == ix));
    if scrollable {
        if let Some(pos) = active_pos {
            let top = px(pos as f32 * NAVIGATION_RAIL_TURN_HEIGHT);
            let visible_bottom = scroll_offset + rail_height;
            if rail_autoscroll.get() != Some(pos)
                && (top < scroll_offset || top + pitch > visible_bottom)
            {
                let target = (top + pitch / 2. - rail_height / 2.)
                    .clamp(px(0.), content_height - rail_height);
                rail_scroll.set_offset(point(px(0.), target));
            }
            rail_autoscroll.set(Some(pos));
        }
    }
    let scroll_offset = rail_scroll.offset().y;
    let at_top = !scrollable || scroll_offset <= px(0.5);
    let at_bottom = !scrollable || scroll_offset >= content_height - rail_height - px(0.5);

    // The emphasized (hovered) turn anchors the width fan-out; with nothing
    // hovered every tick rests at the 0.25 scale.
    let emphasized_turn = hovered.get();
    let emphasized_pos =
        emphasized_turn.and_then(|ix| user_turns.iter().position(|candidate| *candidate == ix));
    // The card's position comes from the hovered tick; its text was snapshotted
    // by the caller for that one turn only.
    let hover_info = match (emphasized_pos, hover_info) {
        (Some(pos), Some((prompt, response))) => Some((pos, prompt, response)),
        _ => None,
    };

    let ticks: Vec<(usize, f32, Hsla)> = user_turns
        .iter()
        .copied()
        .enumerate()
        .map(|(turn_pos, user_ix)| {
            let distance = emphasized_pos.map(|pos| pos.abs_diff(turn_pos));
            let scale = distance
                .and_then(|d| NAVIGATION_RAIL_EMPHASIS_SCALE.get(d))
                .copied()
                .unwrap_or(0.25);
            let prominent = emphasized_turn == Some(user_ix) || active_turn == Some(user_ix);
            let width = NAVIGATION_RAIL_TICK_WIDTH * scale;
            let color = if prominent {
                theme.text_primary
            } else {
                theme.text_muted.opacity(NAVIGATION_RAIL_INACTIVE_OPACITY)
            };
            (user_ix, width, color)
        })
        .collect();

    let shell_hover = hovered.clone();
    // The ticks map consumes one clone of the hint cells; the hint card at
    // the end of this function needs its own.
    let tick_hint_dismissed = rail_hint_dismissed.clone();
    let tick_hint_shown_at = rail_hint_shown_at.clone();
    let mut body = div()
        .relative()
        .w(px(NAVIGATION_RAIL_WIDTH))
        .h(rail_height)
        .child(
            div()
                .id(ElementId::Name("rail-ticks".into()))
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .overflow_y_scroll()
                .track_scroll(&rail_scroll)
                .flex()
                .flex_col()
                .children(ticks.into_iter().map(move |(user_ix, width, color)| {
                    let scroller = scroller.clone();
                    let hover_state = hovered.clone();
                    let tick_hint_dismissed = tick_hint_dismissed.clone();
                    let tick_hint_shown_at = tick_hint_shown_at.clone();
                    div()
                        .id(ElementId::NamedInteger("nav-tick".into(), user_ix as u64))
                        .w(px(NAVIGATION_RAIL_WIDTH))
                        .h(px(NAVIGATION_RAIL_TURN_HEIGHT))
                        .flex_none()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .on_hover({
                            let hover_state = hover_state.clone();
                            move |hovering: &bool, _, cx| {
                                // On leave, clear only when this tick
                                // owns the hover state (moving between
                                // ticks hands it to the next one).
                                let next = if *hovering {
                                    Some(user_ix)
                                } else if hover_state.get() == Some(user_ix) {
                                    None
                                } else {
                                    hover_state.get()
                                };
                                if hover_state.get() != next {
                                    hover_state.set(next);
                                    cx.refresh_windows();
                                }
                            }
                        })
                        .on_click({
                            move |_, _, cx| {
                                // Using the rail is the lesson — dismiss the
                                // one-time hint (persists per install).
                                crate::transcript::dismiss_rail_hint_state(
                                    &tick_hint_dismissed,
                                    &tick_hint_shown_at,
                                );
                                scroller.scroll_to_item(user_ix);
                                cx.refresh_windows();
                            }
                        })
                        .child(
                            div()
                                .id(ElementId::NamedInteger(
                                    "nav-tick-bar".into(),
                                    user_ix as u64,
                                ))
                                .h(px(NAVIGATION_RAIL_TICK_HEIGHT))
                                .w(px(width))
                                .rounded_full()
                                .bg(color)
                                .hover(|style| style.bg(theme.text_primary)),
                        )
                })),
        )
        .when(!at_top, |rail| rail.child(render_rail_fade(true, theme)))
        .when(!at_bottom, |rail| {
            rail.child(render_rail_fade(false, theme))
        });

    // Hover preview, clamped inside the rail body's vertical span so it stays
    // within the rail bounds. While the one-time hint is up it owns this slot,
    // so the two floating cards never stack.
    if let Some((pos, prompt, response)) = hover_info {
        let visible_center =
            px(pos as f32 * NAVIGATION_RAIL_TURN_HEIGHT) + pitch / 2. - scroll_offset;
        let preview_top = (visible_center - px(NAVIGATION_RAIL_PREVIEW_MAX_HEIGHT / 2.))
            .max(px(0.))
            .min((rail_height - px(NAVIGATION_RAIL_PREVIEW_MAX_HEIGHT)).max(px(0.)));
        body = body.child(render_rail_preview(
            &prompt,
            &response,
            theme,
            preview_top,
            pos + 1,
            user_turns.len(),
        ));
    } else if rail_hint_active {
        body = body.child(render_rail_hint(
            theme,
            rail_hint_dismissed,
            rail_hint_shown_at,
        ));
    }

    div()
        .id(ElementId::Name("conversation-navigation-rail".into()))
        .absolute()
        .top_0()
        .left(px(NAVIGATION_RAIL_LEFT))
        .w(px(NAVIGATION_RAIL_WIDTH))
        .h_full()
        .flex()
        .flex_col()
        .justify_center()
        // Belt and braces: leaving the rail entirely clears the preview even
        // if an individual tick's leave event was missed (e.g. scrolling).
        .on_hover({
            let hover_state = shell_hover;
            move |hovering: &bool, _, cx| {
                if !hovering && hover_state.get().is_some() {
                    hover_state.set(None);
                    cx.refresh_windows();
                }
            }
        })
        .child(body)
        .into_any_element()
}

/// Rail edge fade: a 20px gradient from the background so scrolling
/// ticks dissolve instead of clipping.
fn render_rail_fade(top: bool, theme: Theme) -> impl IntoElement {
    let base = div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(20.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme.background.opacity(0.), 0.),
            linear_color_stop(theme.background, 1.),
        ));
    if top {
        // Solid at the top edge, fading down: flip the stop order.
        base.top_0().bg(linear_gradient(
            180.,
            linear_color_stop(theme.background, 0.),
            linear_color_stop(theme.background.opacity(0.), 1.),
        ))
    } else {
        base.bottom_0()
    }
}

/// Rail hover card: the turn's number, the turn's prompt, and a
/// short response snippet, vertically positioned by the caller (clamped to
/// the rail's span) at a 60px left offset (rail width + gap).
fn render_rail_preview(
    prompt: &str,
    response: &str,
    theme: Theme,
    top: Pixels,
    turn_number: usize,
    turn_total: usize,
) -> impl IntoElement {
    div()
        .absolute()
        .left(px(NAVIGATION_RAIL_WIDTH + NAVIGATION_RAIL_CONTENT_GAP))
        .top(top)
        .w(px(NAVIGATION_RAIL_PREVIEW_WIDTH))
        .max_h(px(NAVIGATION_RAIL_PREVIEW_MAX_HEIGHT))
        .overflow_hidden()
        .elevation_2(&theme)
        .px(px(15.))
        .py(DynamicSpacing::Base12.px(&theme))
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .text_size(TextSize::XSmall.px(&theme))
                .line_height(theme.ui_px(14.))
                .text_color(theme.text_muted)
                .child(tr!(
                    "transcript_view.turn_of_total_hint",
                    current = turn_number,
                    total = turn_total
                )),
        )
        .child(
            div()
                .w_full()
                .truncate()
                .text_size(TextSize::Default.px(&theme))
                .line_height(theme.ui_px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(prompt.to_string()),
        )
        .when(!response.is_empty(), |card| {
            card.child(
                div()
                    .w_full()
                    .max_h(px(60.))
                    .overflow_hidden()
                    .whitespace_normal()
                    .text_size(TextSize::Default.px(&theme))
                    .line_height(theme.ui_px(20.))
                    .text_color(theme.text_muted)
                    .child(response.to_string()),
            )
        })
}

/// One-time affordance hint beside the rail: teaches what the ticks are,
/// that they jump, and the keyboard mirror — then never shows again on
/// this install (dismissed by use, click, or TTL).
fn render_rail_hint(
    theme: Theme,
    rail_hint_dismissed: Rc<Cell<bool>>,
    rail_hint_shown_at: Rc<Cell<Option<Instant>>>,
) -> impl IntoElement {
    div()
        .id(ElementId::Name("rail-hint".into()))
        .absolute()
        .left(px(NAVIGATION_RAIL_WIDTH + NAVIGATION_RAIL_CONTENT_GAP))
        .top_0()
        .w(px(216.))
        .elevation_2(&theme)
        .px(px(13.))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base03.px(&theme))
        .cursor_pointer()
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .line_height(theme.ui_px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(tr!("transcript_view.jump_between_turns")),
        )
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .line_height(theme.ui_px(16.))
                .text_color(theme.text_muted)
                .child(tr!(
                    "transcript_view.click_a_line_or_press_to_revisit_any_prompt"
                )),
        )
        .on_click(move |_, _, cx| {
            crate::transcript::dismiss_rail_hint_state(&rail_hint_dismissed, &rail_hint_shown_at);
            cx.refresh_windows();
        })
}

/// Whitespace-normalized grapheme snippet
/// (the presentation navigation previews use).
fn snippet(text: &str, max_graphemes: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut graphemes = normalized.graphemes(true);
    let head: String = graphemes.by_ref().take(max_graphemes).collect();
    if graphemes.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn render_row(paint: RowPaint) -> AnyElement {
    let messages = paint.messages.borrow();
    let Some(message) = messages.get(paint.ix) else {
        return div().into_any_element();
    };
    // Every text block built below registers with the row's selection scope
    // (thread-local, mirroring the block-parse memo), so the low-level text
    // builders stay unaware of the transcript panel's selection state.
    let _scope = TextScopeGuard::enter(TextScope {
        state: paint.text_selection.clone(),
        message_ix: paint.ix,
    });
    let followup = starts_followup_turn(&messages, paint.ix);
    let first = paint.ix == 0;
    let last = paint.ix + 1 == paint.row_count;

    let inner = if let Some(summary) = message.summary.as_ref() {
        render_summary_card(summary, &paint)
    } else if message.user {
        render_user_bubble(message, &paint).into_any_element()
    } else {
        render_assistant(message, &paint).into_any_element()
    };

    div()
        .id(ElementId::NamedInteger(
            "transcript-row".into(),
            paint.ix as u64,
        ))
        .w_full()
        .flex()
        .justify_center()
        .px(DynamicSpacing::Base20.px(&paint.theme))
        .py(DynamicSpacing::Base08.px(&paint.theme))
        .when(first, |row| row.pt(DynamicSpacing::Base24.px(&paint.theme)))
        .when(followup, |row| {
            row.pt(DynamicSpacing::Base32.px(&paint.theme))
        })
        .when(last, |row| row.pb(DynamicSpacing::Base24.px(&paint.theme)))
        // In-transcript find: matched rows get a quiet wash; the selected
        // hit is stronger. Painted on the full-width row so it reads as a
        // band, like a browser's find.
        .when(paint.search_hit, |row| {
            row.bg(paint.theme.overlay.opacity(0.5))
        })
        .when(paint.search_active, |row| {
            row.bg(paint.theme.accent.opacity(0.14))
        })
        .child(
            div()
                // Resolve the normal measure before any wide table is
                // measured; percentage widths can otherwise collapse during
                // GPUI's intrinsic sizing pass on a narrow pane.
                .w(paint.table_breakout.column_width)
                .flex_none()
                .min_w_0()
                .child(inner),
        )
        .into_any_element()
}

/// A context-boundary card: pi compacted the conversation, or summarized an
/// abandoned branch on `/tree` navigation. Collapsed by default (a summary
/// can be long); clicking the header toggles the markdown body — the same
/// per-row disclosure set the turn fold uses.
fn render_summary_card(row: &SummaryRow, paint: &RowPaint) -> AnyElement {
    let theme = paint.theme;
    let ix = paint.ix;
    let expanded = paint.fold_open;
    let (icon, label) = if row.branch {
        ("icons/branch.svg", tr!("transcript.branch_summary"))
    } else {
        ("icons/archive.svg", tr!("transcript.compaction"))
    };
    let detail = row
        .tokens_before
        .map(|tokens| {
            tr!(
                "transcript.compacted_from",
                tokens = crate::context_meter::format_tokens(tokens)
            )
        })
        .unwrap_or_default();
    let has_detail = !detail.is_empty();

    let header = div()
        .id(ElementId::NamedInteger("summary-head".into(), ix as u64))
        .h(DynamicSpacing::Base32.px(&theme))
        .px(DynamicSpacing::Base08.px(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base08.px(&theme))
        // Square hover fill over a rounded card otherwise; see the tool card.
        .rounded_t(Radius::XLarge.px(&theme))
        .when(!expanded, |header| {
            header.rounded_b(Radius::XLarge.px(&theme))
        })
        .cursor_pointer()
        .hover(|style| style.bg(theme.overlay_strong))
        .on_click({
            let expanded_turns = paint.expanded_turns.clone();
            let scroller = paint.scroller.clone();
            move |_, _, cx| {
                toggle_index(&expanded_turns, ix);
                scroller.remeasure_toggle(ix);
                cx.refresh_windows();
            }
        })
        .child(activity_badge(icon, theme.accent_text, theme))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_primary)
                .child(label),
        )
        .when(has_detail, |header| {
            header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(detail),
            )
        })
        .when(!has_detail, |header| header.child(div().flex_1()))
        .child(glyph(
            if expanded {
                "icons/chevron-down.svg"
            } else {
                "icons/chevron-right.svg"
            },
            IconSize::XSmall.px(&theme),
            theme.text_muted,
        ));

    let mut card = div()
        .id(ElementId::NamedInteger("summary-card".into(), ix as u64))
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.overlay)
        .child(header);
    if expanded {
        card = card.child(
            div()
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(theme.border)
                .px(DynamicSpacing::Base12.px(&theme))
                .py(DynamicSpacing::Base08.px(&theme))
                .child(render_prose(
                    &row.summary,
                    ix,
                    4096,
                    theme,
                    paint.copied_sections.clone(),
                    paint.expanded_blocks.clone(),
                    true,
                    paint.scroller.clone(),
                    Some(paint.table_breakout),
                )),
        );
    }
    card.into_any_element()
}

/// End-aligned user row: neutral raised bubble, persistent quiet
/// footer below. Attached images render as a tile grid above the text
/// bubble.
fn render_user_bubble(message: &ChatMessage, paint: &RowPaint) -> impl IntoElement {
    let theme = paint.theme;
    let ix = paint.ix;
    let text = message.text();
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_end()
        .gap(DynamicSpacing::Base04.px(&theme))
        // Attachment tiles (images queued with the prompt). Cover-cropped
        // squares; wrap when a message carries several.
        .when(!message.images.is_empty(), |column| {
            column.child(
                div()
                    .max_w(px(USER_BUBBLE_MAX_WIDTH))
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .children(message.images.iter().enumerate().map(|(image_ix, image)| {
                        let image = image.clone();
                        let opener = paint.image_opener.clone();
                        div()
                            .id(ElementId::NamedInteger(
                                "user-image".into(),
                                (ix as u64) << 20 | image_ix as u64,
                            ))
                            .size(px(112.))
                            .rounded(Radius::XLarge.px(&theme))
                            .border_1()
                            .border_color(theme.border)
                            .overflow_hidden()
                            .bg(theme.surface_elevated)
                            .when(opener.is_some(), |tile| {
                                tile.cursor_pointer()
                                    .hover(|s| s.border_color(theme.border_strong))
                            })
                            .child(
                                img(ImageSource::Image(image.clone()))
                                    .size_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                            .on_click(move |_, window, cx| {
                                if let Some(opener) = &opener {
                                    opener(image.clone(), window, cx);
                                }
                            })
                    })),
            )
        })
        .when(!text.is_empty(), |column| {
            column.child(
                div()
                    .max_w(px(USER_BUBBLE_MAX_WIDTH))
                    .rounded(Radius::XLarge.px(&theme))
                    .bg(theme.surface_elevated)
                    .text_color(theme.text_primary)
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base08.px(&theme))
                    .text_size(TextSize::Default.px(&theme))
                    .whitespace_normal()
                    .child(render_prose(
                        &text,
                        ix,
                        0,
                        theme,
                        paint.copied_sections.clone(),
                        paint.expanded_blocks.clone(),
                        true,
                        paint.scroller.clone(),
                        None,
                    )),
            )
        })
        .child(render_message_footer(
            text,
            ix,
            message
                .finished_at
                .and_then(format_time)
                .map(|time| footer_time_stamp(time, theme)),
            message.usage(),
            message.generation_time(),
            paint.copied,
            true,
            theme,
            paint.copied_at.clone(),
            paint.hovered_usage.clone(),
        ))
}

fn render_assistant(message: &ChatMessage, paint: &RowPaint) -> impl IntoElement {
    let theme = paint.theme;
    let ix = paint.ix;
    let mut content = div()
        // Resolve the reading width at the shared ancestor, not on individual
        // prose blocks: GPUI must measure Thinking cards at their actual width
        // rather than reserve their height cap during intrinsic layout.
        .w(paint.table_breakout.column_width)
        .max_w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_start()
        .gap(DynamicSpacing::Base08.px(&theme));

    // The turn fold precedes the run's first work (collapsed turns
    // hide the pre-answer work behind a single "Worked for" divider).
    if !paint.live && message.has_hidden_work() {
        content = content.child(render_turn_fold(
            ix,
            paint.fold_open,
            message.elapsed,
            theme,
            paint.expanded_turns.clone(),
            paint.scroller.clone(),
        ));
    }

    // Every thought and tool call of the turn collects into ONE collapsible
    // activity group: a tool-heavy turn reads as a single summary line
    // ("Ran 2 commands · 3 file reads · 4 thoughts") that expands into all
    // of its cards, instead of a stack of per-step "Ran …" rows.
    let answer_start = message
        .steps
        .iter()
        .position(|step| !step.text.trim().is_empty());
    let work_start = message
        .steps
        .iter()
        .position(|step| !step.thinking.is_empty() || !step.tools.is_empty());
    // First step index whose post-answer work is NOT yet absorbed into a
    // rendered activity group (see the in-sequence group block below).
    let mut covered_until = 0usize;

    for (step_ix, step) in message.steps.iter().enumerate() {
        // The group sits where the turn's first work happened, so any text
        // that preceded it stays above; the rest of the text follows below.
        if Some(step_ix) == work_start {
            let before_answer = answer_start.is_none_or(|answer| step_ix < answer);
            // Pre-answer work hides behind the turn fold; later work stays
            // visible as a collapsed, expandable group.
            let show_work = paint.live || paint.fold_open || !before_answer;
            if show_work {
                // The group covers the work up to and including the step
                // that produced the first answer text; work in later steps
                // gets its own group further down, so tool calls stay in
                // sequence with the text — live included: a turn that speaks
                // and then works again must not hoist that work above the
                // answer that preceded it.
                let group_end = answer_start.map_or(message.steps.len(), |answer| answer + 1);
                let group_live = activity_group_is_live(paint.live, group_end, &message.steps);
                let open = paint
                    .expanded_activities
                    .borrow()
                    .get(&(ix, 0))
                    .copied()
                    .unwrap_or(group_live);
                let group_has_work = message.steps[..group_end]
                    .iter()
                    .any(|step| !step.thinking.is_empty() || !step.tools.is_empty());
                if group_has_work {
                    content = content.child(render_activity_group(
                        ix,
                        0..group_end,
                        &message.steps,
                        open,
                        group_live,
                        theme,
                        paint.nerd.as_ref(),
                        paint.expanded_activities.clone(),
                        paint.expanded_tools.clone(),
                        paint.copied_sections.clone(),
                        paint.expanded_sections.clone(),
                        paint.scroller.clone(),
                        paint.thinking_scrolls.clone(),
                        paint.collapsed_thoughts.clone(),
                        paint.thinking_detached.clone(),
                    ));
                }
            }
        }

        if !step.text.is_empty() {
            // Live rows never collapse code blocks — a growing block's tail
            // edge must stay visible while it streams.
            let prose = step.text.as_str();
            content = content.child(
                div()
                    .w_full()
                    .min_w_0()
                    .pt(DynamicSpacing::Base04.px(&theme))
                    .child(render_prose(
                        prose,
                        ix,
                        (step_ix as u64 + 1) * 4096,
                        theme,
                        paint.copied_sections.clone(),
                        paint.expanded_blocks.clone(),
                        !paint.live,
                        paint.scroller.clone(),
                        Some(paint.table_breakout),
                    )),
            );
        }

        // Work in a step that FOLLOWS answer text (the assistant spoke,
        // then kept thinking/calling tools) renders as its own group here,
        // in sequence, instead of being pulled up into the first group.
        // The group absorbs every following work-only step, so a run of
        // tool calls between two text blocks is ONE group ("Ran 4
        // commands · 5 thoughts"), never a stack of per-step rows. While
        // live, the currently streaming work-only step is absorbed too,
        // so the group sits where the agent actually resumed working.
        if answer_start.is_some_and(|answer| step_ix > answer)
            && step_ix >= covered_until
            && (!step.thinking.is_empty() || !step.tools.is_empty())
        {
            let has_work = |j: usize| {
                let s = &message.steps[j];
                !s.thinking.is_empty() || !s.tools.is_empty()
            };
            let mut group_end = step_ix + 1;
            while group_end < message.steps.len()
                && message.steps[group_end].text.trim().is_empty()
                && has_work(group_end)
            {
                group_end += 1;
            }
            covered_until = group_end;
            let group_live = activity_group_is_live(paint.live, group_end, &message.steps);
            let open = paint
                .expanded_activities
                .borrow()
                .get(&(ix, step_ix))
                .copied()
                .unwrap_or(group_live);
            content = content.child(render_activity_group(
                ix,
                step_ix..group_end,
                &message.steps,
                open,
                group_live,
                theme,
                paint.nerd.as_ref(),
                paint.expanded_activities.clone(),
                paint.expanded_tools.clone(),
                paint.copied_sections.clone(),
                paint.expanded_sections.clone(),
                paint.scroller.clone(),
                paint.thinking_scrolls.clone(),
                paint.collapsed_thoughts.clone(),
                paint.thinking_detached.clone(),
            ));
        }
    }

    // Changed files render ONLY in the end-of-task summary card pinned
    // after the last row (once the run settles) — not per message.

    // A turn that ended in a provider/agent error (e.g. an unsupported
    // model). pi carries the message in `errorMessage`; render it so a
    // failed turn is never an empty row.
    if let Some(error) = &message.error {
        content = content.child(render_assistant_error(error, ix, theme));
    }

    // A cancelled turn keeps its partial content and says so, quietly —
    // "Stopped", never an error card (the user asked for the stop).
    if message.aborted && !paint.live {
        content = content.child(render_stopped_marker(theme));
    }

    if paint.live {
        let activity = message.steps.last().and_then(working_activity_label);
        content = content.child(render_working_indicator(
            paint.live_elapsed.unwrap_or(Duration::ZERO),
            activity,
            paint.live_tps,
            theme,
        ));
    }

    let mut row = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_start()
        .child(content);

    if !paint.live && !paint.suppress_footer && !message.text().is_empty() {
        row = row.child(render_message_footer(
            message.text(),
            ix,
            message
                .finished_at
                .and_then(format_time)
                .map(|time| footer_time_stamp(time, theme)),
            message.usage(),
            message.generation_time(),
            paint.copied,
            false,
            theme,
            paint.copied_at.clone(),
            paint.hovered_usage.clone(),
        ));
    }

    row
}

/// A settled assistant turn that failed: the provider/agent `errorMessage`
/// pi carries on the message, as a red card. Mirrors the app-level error
/// banner so the failure is visible in context too.
fn render_assistant_error(error: &str, ix: usize, theme: Theme) -> AnyElement {
    div()
        .w_full()
        .max_w_full()
        .min_w_0()
        .bg(theme.error.opacity(0.1))
        .border_1()
        .border_color(theme.error.opacity(0.45))
        .rounded(Radius::XLarge.px(&theme))
        .px(DynamicSpacing::Base12.px(&theme))
        .py(DynamicSpacing::Base08.px(&theme))
        .flex()
        .items_start()
        .gap_2p5()
        .child(glyph(
            "icons/info.svg",
            IconSize::Medium.px(&theme),
            theme.error,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(tr!("transcript_view.agent_error")),
                )
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .whitespace_normal()
                        .child({
                            let text: SharedString = error.to_string().into();
                            let key = ElementId::NamedInteger("agent-error".into(), ix as u64);
                            let scope = text_scope();
                            let range = scope
                                .as_ref()
                                .and_then(|scope| scope.state.borrow().range_for(&key, &text));
                            let runs = highlight_runs(
                                vec![code_run(text.len(), theme.text_secondary, &ui_font())],
                                range,
                                selection_color(theme),
                            );
                            SelectableText::new(
                                key,
                                StyledText::new(text.clone()).with_runs(runs),
                                text,
                                Vec::new(),
                                scope,
                            )
                        }),
                ),
        )
        .into_any_element()
}

/// The turn's activity summary: counts every tool kind and every thought
/// across the turn's steps — "Ran 7 commands · 4 thoughts", "Ran 1 file
/// read · 1 thought", "Ran 2 commands · 3 file reads · 1 file edit".
fn activity_title(steps: &[Step], live: bool) -> String {
    let mut parts: Vec<String> = Vec::new();
    let (mut commands, mut reads, mut edits, mut other, mut thoughts) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    for step in steps {
        for tool in &step.tools {
            match tool.name.as_str() {
                "bash" | "shell" | "terminal" | "exec" | "run" => commands += 1,
                "read" | "view" | "grep" | "find" | "glob" | "search" | "list" | "ls" | "tree" => {
                    reads += 1
                }
                "edit" | "write" => edits += 1,
                _ => other += 1,
            }
        }
        if !step.thinking.is_empty() {
            thoughts += 1;
        }
    }
    let unit = |count: usize, one: &str, other: &str| {
        if count == 1 {
            tr!(one, count = count)
        } else {
            tr!(other, count = count)
        }
    };
    if commands > 0 {
        parts.push(unit(
            commands,
            "transcript_view.ran_command_one",
            "transcript_view.ran_command_other",
        ));
    }
    if reads > 0 {
        parts.push(unit(
            reads,
            "transcript_view.ran_file_read_one",
            "transcript_view.ran_file_read_other",
        ));
    }
    if edits > 0 {
        parts.push(unit(
            edits,
            "transcript_view.ran_file_edit_one",
            "transcript_view.ran_file_edit_other",
        ));
    }
    if other > 0 {
        parts.push(unit(
            other,
            "transcript_view.ran_tool_one",
            "transcript_view.ran_tool_other",
        ));
    }
    if thoughts > 0 {
        parts.push(if live {
            tr!("transcript.thinking")
        } else {
            unit(
                thoughts,
                "transcript_view.thought_one",
                "transcript_view.thought_other",
            )
        });
    }
    if parts.is_empty() {
        return if live {
            tr!("transcript.working")
        } else {
            tr!("transcript.worked")
        };
    }
    parts.join(" · ")
}

/// A turn's activity group: a single collapsed summary line ("Ran 2
/// commands · 3 file reads · 4 thoughts") that expands into every thought
/// and tool card in `range`. Pre-answer work is one group keyed `(ix, 0)`;
/// work after the answer gets its own group per step so it stays in
/// sequence with the text.
#[allow(clippy::too_many_arguments)]
fn render_activity_group(
    ix: usize,
    range: std::ops::Range<usize>,
    all_steps: &[Step],
    open: bool,
    live: bool,
    theme: Theme,
    nerd: Option<&SharedString>,
    expanded_activities: ExpandedActivities,
    expanded_tools: ExpandedTools,
    copied_sections: CopiedSections,
    expanded_sections: ExpandedSections,
    scroller: MessageScrollerState,
    thinking_scrolls: ThinkingScrolls,
    collapsed_thoughts: CollapsedThoughts,
    thinking_detached: ThinkingDetached,
) -> impl IntoElement {
    let steps = &all_steps[range.clone()];
    let title = activity_title(steps, live);
    // Leading glyph cluster: one icon per distinct work kind (fixed
    // canonical order, the reasoning bulb last) so a folded group reads as
    // "what ran", not another line of prose.
    let icons = activity_group_icons(steps);
    let any_failed = steps
        .iter()
        .any(|step| step.tools.iter().any(|tool| tool.failed));
    let tool_base = all_steps[..range.start]
        .iter()
        .map(|step| step.tools.len())
        .sum::<usize>();
    // The tool that may still be running is the turn's newest call — not
    // merely this group's newest (a live turn can carry several groups).
    let last_tool = all_steps
        .iter()
        .map(|step| step.tools.len())
        .sum::<usize>()
        .saturating_sub(1);
    let key = (ix, range.start);
    // State color for the glyph cluster: danger when anything failed, accent
    // while the group streams. Settled clusters tint per work kind instead
    // (see `work_tint`).
    let cluster_state = if any_failed {
        Some(theme.error)
    } else if live {
        Some(theme.accent_text)
    } else {
        None
    };
    let mut group = div()
        .debug_selector(move || format!("activity-group-{ix}-{}", key.1))
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base04.px(&theme))
        // Let the summary chip hug its content so it reads as a control
        // instead of a full-width band across the answer column.
        .items_start()
        .child(
            button_frame(
                div().id(ElementId::NamedInteger(
                    "activity-toggle".into(),
                    ((ix as u64) << 20) | range.start as u64,
                )),
                &theme,
                ButtonSize::Medium,
            )
            .min_w_0()
            .max_w_full()
            // Raised fill + hairline = DESIGN.md's chip treatment: the
            // open state steps the border up for emphasis.
            .raised(theme.surface_elevated, &theme)
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .cursor_pointer()
            .hover(|style| {
                style
                    .raised(theme.surface_hover, &theme)
                    .text_color(theme.text_primary)
            })
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .children(icons.iter().copied().map(|icon| {
                        // Settled work is monochrome ink; only run/fail state
                        // takes a color (see `cluster_state`).
                        glyph(
                            icon,
                            IconSize::XSmall.px(&theme),
                            cluster_state.unwrap_or(theme.text_secondary),
                        )
                    })),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_secondary)
                    .child(title),
            )
            .child(glyph(
                if open {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                },
                IconSize::XSmall.px(&theme),
                theme.text_muted,
            ))
            .on_click({
                let expanded_activities = expanded_activities.clone();
                let scroller = scroller.clone();
                move |_, _, cx| {
                    let mut map = expanded_activities.borrow_mut();
                    let next = !map.get(&key).copied().unwrap_or(false);
                    map.insert(key, next);
                    scroller.remeasure_toggle(ix);
                    cx.refresh_windows();
                }
            }),
        );

    if open {
        let mut body = div()
            .w_full()
            .min_w_0()
            .ml(DynamicSpacing::Base06.px(&theme))
            .pl(DynamicSpacing::Base12.px(&theme))
            .pb(px(2.))
            .border_l_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme));
        let mut tool_base = tool_base;
        for (step_ix, step) in steps.iter().enumerate() {
            if !step.thinking.is_empty() {
                let duration = step_thinking_duration(all_steps, range.start + step_ix);
                // Only the turn's last step can still be streaming; earlier
                // thoughts are settled and must not chase the live edge.
                // Measured against ALL steps, not this group: a live group
                // can end before the turn's last (still-streaming) step.
                // The card also settles the moment its step moves on to
                // answer text or a tool call — pi sends no `thinking_end`, so
                // the turn's own end is too late to close it.
                let thinking_live =
                    thinking_is_live(live, range.start + step_ix + 1 == all_steps.len(), step);
                body = body.child(render_thinking_body(
                    &step.thinking,
                    thinking_live,
                    thinking_live,
                    duration,
                    theme,
                    (ix, range.start + step_ix),
                    scroller.clone(),
                    thinking_scrolls.clone(),
                    collapsed_thoughts.clone(),
                    thinking_detached.clone(),
                ));
            }
            body = body.children(step.tools.iter().enumerate().map(|(tool_ix, tool)| {
                let flat = tool_base + tool_ix;
                // Only the turn's newest call can still be running; every
                // earlier call is done and shows its check even while a
                // later step streams.
                let pulse = live && flat == last_tool;
                render_activity_card(
                    tool,
                    pulse,
                    !pulse,
                    theme,
                    nerd,
                    (ix, flat),
                    expanded_tools.borrow().contains(&(ix, flat)),
                    expanded_tools.clone(),
                    copied_sections.clone(),
                    expanded_sections.clone(),
                    scroller.clone(),
                )
            }));
            tool_base += step.tools.len();
        }
        group = group.child(body);
    }

    group
}

/// How long a step's reasoning took. Live runs carry the measured value on
/// the step; reloaded sessions have no timing, so estimate it from pi's
/// per-step timestamps — the gap to the next step's message covers this
/// step's LLM call (reasoning + call generation) plus the tools it ran.
fn step_thinking_duration(steps: &[Step], step_ix: usize) -> Option<Duration> {
    let step = steps.get(step_ix)?;
    if let Some(duration) = step.thinking_duration {
        return Some(duration);
    }
    if step.thinking.is_empty() {
        return None;
    }
    let start = step.timestamp?;
    let end = steps.get(step_ix + 1).and_then(|next| next.timestamp)?;
    let millis = end.checked_sub(start)?;
    (millis > 0).then(|| Duration::from_millis(millis as u64))
}

/// Whether a step's reasoning card is still the active phase: the row is
/// live, this is the turn's last step, and it has not moved on to answer
/// text or a tool call yet. pi emits no `thinking_end`, so that transition —
/// not the turn's end — is what settles the card.
fn thinking_is_live(live: bool, is_last_step: bool, step: &Step) -> bool {
    live && is_last_step && step.text.trim().is_empty() && step.tools.is_empty()
}

/// Whether an activity group is the one still streaming: the row must be
/// live, the group must include the turn's newest step, and that step must
/// not have moved on to answer text. Older groups settle as soon as a newer
/// step starts, instead of waiting for the whole turn to end.
fn activity_group_is_live(row_live: bool, group_end: usize, all_steps: &[Step]) -> bool {
    row_live
        && group_end == all_steps.len()
        && all_steps
            .last()
            .is_some_and(|step| step.text.trim().is_empty())
}

/// The reasoning card body inside a turn's activity group ("Thinking" live,
/// "Thought" settled). The header collapses/expands the reasoning; tall
/// reasoning scrolls inside a height-capped card so a long chain of thought
/// never pushes the answer off-screen.
#[allow(clippy::too_many_arguments)]
fn render_thinking_body(
    thinking: &str,
    live: bool,
    streaming: bool,
    duration: Option<Duration>,
    theme: Theme,
    key: (usize, usize),
    scroller: MessageScrollerState,
    thinking_scrolls: ThinkingScrolls,
    collapsed_thoughts: CollapsedThoughts,
    thinking_detached: ThinkingDetached,
) -> impl IntoElement {
    let label = if live {
        tr!("transcript.thinking")
    } else {
        tr!("transcript.thought")
    };
    // The label stays the bold accent anchor; the timing is secondary —
    // normal weight and muted, like the other metric text in the row.
    let timing = duration.map(|duration| {
        let elapsed = if live {
            format_working_elapsed(duration)
        } else {
            format_duration(duration)
        };
        tr!("transcript.for_duration", duration = elapsed)
    });
    let detail = cap_chars(thinking, THINKING_TEXT_CAP);
    let collapsed = collapsed_thoughts.borrow().contains(&key);
    let handle = thinking_scrolls
        .borrow_mut()
        .entry(key)
        .or_default()
        .clone();
    // While reasoning streams, keep the newest line in view: the card is
    // height-capped and would otherwise stay parked at the top while text
    // keeps growing below the fold. The reader's own scroll away from the
    // bottom detaches the card until they return (see
    // `chain_thinking_scroll`).
    if streaming && !thinking_detached.borrow().contains(&key) {
        handle.scroll_to_bottom();
    }
    let id = (key.0 as u64) << 16 | key.1 as u64;
    // The reasoning glyph: brand accent while the card streams (with a slow
    // opacity pulse, reduce-motion aware), muted once it settles.
    let thought_icon: AnyElement = if live && !theme.ui.reduce_motion {
        div()
            .flex_none()
            .child(glyph(
                "icons/tools/thinking.svg",
                IconSize::Small.px(&theme),
                theme.accent_text,
            ))
            .with_animation(
                ElementId::NamedInteger("thought-icon".into(), id),
                Animation::new(Duration::from_millis(1600)).repeat(),
                |el, delta| el.opacity(0.4 + 0.6 * (delta * std::f32::consts::TAU).sin().abs()),
            )
            .into_any_element()
    } else {
        glyph(
            "icons/tools/thinking.svg",
            IconSize::Small.px(&theme),
            if live {
                theme.accent_text
            } else {
                theme.text_muted
            },
        )
        .into_any_element()
    };
    let mut card = div()
        .debug_selector(move || format!("thought-card-{}-{}", key.0, key.1))
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.overlay)
        .px(DynamicSpacing::Base12.px(&theme))
        .py(DynamicSpacing::Base08.px(&theme))
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .id(ElementId::NamedInteger("thought-toggle".into(), id))
                .group(BUTTON_GROUP)
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .cursor_pointer()
                .text_size(TextSize::Default.px(&theme))
                .line_height(theme.ui_px(17.))
                .hover(|style| style.text_color(theme.text_primary))
                .child(thought_icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.accent_text)
                                .child(label),
                        )
                        .when_some(timing, |row, timing| {
                            row.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(theme.text_muted)
                                    .child(timing),
                            )
                        }),
                )
                .child(glyph(
                    if collapsed {
                        "icons/chevron-right.svg"
                    } else {
                        "icons/chevron-down.svg"
                    },
                    IconSize::XSmall.px(&theme),
                    theme.text_muted,
                ))
                .on_click({
                    let collapsed_thoughts = collapsed_thoughts.clone();
                    let scroller = scroller.clone();
                    move |_, _, cx| {
                        let mut set = collapsed_thoughts.borrow_mut();
                        if !set.remove(&key) {
                            set.insert(key);
                        }
                        drop(set);
                        scroller.remeasure_toggle(key.0);
                        cx.refresh_windows();
                    }
                }),
        );
    if !collapsed {
        card = card.child(
            div()
                // A stable id lets GPUI remember the scroll offset across
                // re-renders (element-state keyed scrolling).
                .id(ElementId::NamedInteger("thinking-scroll".into(), id))
                .w_full()
                .min_w_0()
                .max_h(px(THINKING_MAX_HEIGHT))
                .overflow_y_scroll()
                // A tracked handle lets `chain_thinking_scroll` read the
                // card's offset and limit once it bottoms out.
                .track_scroll(&handle)
                // The transcript is a virtualized list whose wheel handler
                // bubbles and claims the event whenever its hitbox is under
                // the cursor — so a long thought would scroll the page
                // instead of the card. `occlude` blocks hitboxes behind it,
                // which flips the list's `should_handle_scroll` to false and
                // leaves the wheel to this card.
                .occlude()
                // `occlude` also stops the page from ever scrolling while
                // the card is at its end, so hand the leftover wheel back to
                // the transcript (a card too short to scroll forwards the
                // whole delta, so the page still moves normally).
                .on_scroll_wheel({
                    let handle = handle.clone();
                    let scroller = scroller.clone();
                    let thinking_detached = thinking_detached.clone();
                    move |event, window, cx| {
                        chain_thinking_scroll(
                            &handle,
                            &scroller,
                            &thinking_detached,
                            key,
                            event,
                            window.line_height(),
                            cx,
                        );
                    }
                })
                .font_family(theme::code_font_family())
                .text_size(theme.code_px(12.))
                .line_height(theme.code_px(18.))
                .text_color(theme.text_secondary)
                .whitespace_normal()
                .child({
                    let body_key = ElementId::NamedInteger(
                        "thinking-body".into(),
                        ((key.0 as u64) << 32) | key.1 as u64,
                    );
                    let scope = text_scope();
                    let range = scope
                        .as_ref()
                        .and_then(|scope| scope.state.borrow().range_for(&body_key, &detail));
                    let runs = highlight_runs(
                        vec![code_run(detail.len(), theme.text_secondary, &mono_font())],
                        range,
                        selection_color(theme),
                    );
                    SelectableText::new(
                        body_key,
                        StyledText::new(detail.clone()).with_runs(runs),
                        detail.into(),
                        Vec::new(),
                        scope,
                    )
                }),
        );
    }
    card
}

/// Chain a wheel event from a "Thought" card to the transcript once the card
/// has scrolled as far as it can. The card's own `overflow_y_scroll` listener
/// is registered *after* this one and GPUI dispatches bubble listeners in
/// reverse, so it runs first and adds the raw delta to the handle
/// *unclamped* — the offset read here is therefore the pre-clamp value. The
/// part a clamp would discard is exactly what the transcript should consume.
fn chain_thinking_scroll(
    handle: &ScrollHandle,
    scroller: &MessageScrollerState,
    thinking_detached: &ThinkingDetached,
    key: (usize, usize),
    event: &ScrollWheelEvent,
    line_height: Pixels,
    cx: &mut App,
) {
    let raw = event.delta.pixel_delta(line_height);
    // The card only scrolls on Y; mirror the built-in listener's fallback (a
    // purely horizontal delta over a vertical scroller is applied to Y).
    let delta_y = if raw.y != px(0.) { raw.y } else { raw.x };
    if delta_y == px(0.) {
        return;
    }
    let offset = handle.offset();
    let max = handle.max_offset().height;
    // The built-in listener already added `delta_y` without clamping, so the
    // pre-event offset is this event's offset minus the delta it applied.
    let previous = offset.y - delta_y;
    let (clamped, residual) = split_thinking_scroll(previous, max, delta_y);
    // Track whether the reader is following the stream or has scrolled away
    // from the newest line: reaching the bottom re-arms following, an upward
    // wheel detaches, and a downward wheel short of the bottom leaves the
    // current choice alone.
    match thinking_follow_after(clamped, max, delta_y) {
        Some(true) => {
            thinking_detached.borrow_mut().remove(&key);
        }
        Some(false) => {
            thinking_detached.borrow_mut().insert(key);
        }
        None => {}
    }
    if clamped != offset.y {
        handle.set_offset(point(offset.x, clamped));
        cx.refresh_windows();
    }
    if residual != px(0.) {
        // `ListState::scroll_by` is positive towards the live edge; the wheel
        // delta is negative in that direction.
        scroller.scroll_by(-residual);
        cx.refresh_windows();
    }
}

/// Split a wheel delta between a "Thought" card and the transcript: clamp the
/// card to its scrollable range and return `(card_offset, residual)`, where
/// `residual` is the part the card could not consume and the transcript
/// should. `previous` is the card's offset before this event; `max` is its
/// scrollable extent (positive pixels).
fn split_thinking_scroll(previous: Pixels, max: Pixels, delta: Pixels) -> (Pixels, Pixels) {
    let unclamped = previous + delta;
    let clamped = unclamped.clamp(-max, px(0.));
    (clamped, unclamped - clamped)
}

/// The follow state a live "Thought" card should carry after a wheel event:
/// `Some(true)` to follow the newest line (the card reached its bottom, or is
/// too short to scroll), `Some(false)` to detach (the reader wheeled up), and
/// `None` to leave the current state alone (a downward wheel still short of
/// the bottom). `clamped` is the card's offset after the event, `max` its
/// scrollable extent, and `delta_y` the wheel delta (positive scrolls up).
fn thinking_follow_after(clamped: Pixels, max: Pixels, delta_y: Pixels) -> Option<bool> {
    // Offsets run from `0` (top) to `-max` (bottom), so hitting the floor is
    // `clamped <= -max`, not `>=`.
    if max <= px(0.) || clamped <= -max {
        Some(true)
    } else if delta_y > px(0.) {
        Some(false)
    } else {
        None
    }
}

/// The `ask_user_question` card: a compact question record for the transcript.
/// While the tool waits the options read as a plain numbered list; once it
/// answers, the chosen option is marked (or the typed custom answer shown).
/// The live, answerable version is the inline panel above the composer.
fn render_ask_card(tool: &ToolCall, theme: Theme, key: (usize, usize)) -> AnyElement {
    use crate::ask;

    let questions = ask::questions_from_args(tool.args.as_ref());
    let answered = ask::answer_envelope(tool.output.as_ref());
    let declined = ask::is_declined(tool.output.as_ref());
    let waiting = tool.output.is_none() && !tool.failed;

    let mut card = div()
        .id(ElementId::NamedInteger(
            "ask-card".into(),
            (key.0 as u64) << 16 | key.1 as u64,
        ))
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.overlay)
        .flex()
        .flex_col();

    // ── header ──
    let mut header = div()
        .h(DynamicSpacing::Base32.px(&theme))
        .px(DynamicSpacing::Base08.px(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base08.px(&theme))
        .text_size(TextSize::Default.px(&theme))
        .line_height(BufferLineHeight::Standard.relative())
        .child(glyph(
            "icons/task.svg",
            IconSize::Small.px(&theme),
            theme.text_muted,
        ))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.accent_text)
                .child(tr!("transcript_view.question")),
        );
    if questions.len() > 1 {
        header = header.child(
            div()
                .flex_none()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("transcript.asked", count = questions.len())),
        );
    }
    if declined {
        header = header.child(
            div()
                .flex_1()
                .flex()
                .justify_end()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("transcript_view.no_answer")),
        );
    } else if tool.failed {
        header = header.child(div().flex_1().flex().justify_end().child(glyph(
            "icons/stop.svg",
            IconSize::XSmall.px(&theme),
            theme.error,
        )));
    }
    card = card.child(header);

    // ── body: one block per question ──
    let mut body = div().px(px(10.)).pb(px(10.)).flex().flex_col().gap(px(10.));
    for question in &questions {
        let mut block = div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base04.px(&theme));
        let mut head = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme));
        if !question.header.trim().is_empty() {
            head = head.child(
                div()
                    .flex_none()
                    .px(DynamicSpacing::Base06.px(&theme))
                    .py(px(1.))
                    .rounded(Radius::Small.px(&theme))
                    .bg(theme.overlay_strong)
                    .text_size(TextSize::XSmall.px(&theme))
                    .line_height(theme.ui_px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(question.header.clone())),
            );
        }
        head = head.child(
            div()
                .min_w_0()
                .flex_1()
                .whitespace_normal()
                .text_size(TextSize::Default.px(&theme))
                .line_height(theme.ui_px(18.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(SharedString::from(question.question.clone())),
        );
        block = block.child(head);

        // The answer, when the tool completed with one. A selection is marked
        // in the list below; a typed custom answer gets its own row.
        let answer =
            answered.and_then(|envelope| ask::envelope_answer(envelope, &question.question));
        let custom = answer.filter(|answer| !ask::answer_is_option_selection(question, answer));

        let mut list = div().flex().flex_col().gap(px(1.));
        for (ix, option) in question.options.iter().enumerate() {
            let chosen =
                answer.is_some_and(|answer| ask::answer_includes_label(answer, &option.label));
            let mut row = div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .px(DynamicSpacing::Base08.px(&theme))
                .py(DynamicSpacing::Base04.px(&theme))
                .rounded(Radius::Medium.px(&theme))
                .when(chosen, |row| row.bg(theme.selection));
            // A plain number keeps the list readable at a glance.
            row = row.child(
                div()
                    .flex_none()
                    .text_size(TextSize::Small.px(&theme))
                    .line_height(theme.ui_px(17.))
                    .text_color(if chosen {
                        theme.accent_text
                    } else {
                        theme.text_muted
                    })
                    .child(format!("{}.", ix + 1)),
            );
            row = row.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .whitespace_normal()
                    .text_size(TextSize::Small.px(&theme))
                    .line_height(theme.ui_px(17.))
                    .text_color(if chosen {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    })
                    .child(SharedString::from(option.label.clone())),
            );
            if chosen {
                row = row.child(glyph(
                    "icons/check.svg",
                    IconSize::XSmall.px(&theme),
                    theme.accent_text,
                ));
            }
            list = list.child(row);
        }
        if let Some(answer) = custom {
            list = list.child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base04.px(&theme))
                    .rounded(Radius::Medium.px(&theme))
                    .bg(theme.accent.opacity(0.10))
                    .child(
                        div()
                            .flex_none()
                            .text_size(TextSize::Small.px(&theme))
                            .line_height(theme.ui_px(17.))
                            .text_color(theme.accent_text)
                            .child("A."),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .whitespace_normal()
                            .text_size(TextSize::Small.px(&theme))
                            .line_height(theme.ui_px(17.))
                            .text_color(theme.text_primary)
                            .child(SharedString::from(answer.to_string())),
                    ),
            );
        }
        block = block.child(list);

        if waiting {
            block = block.child(
                div()
                    .px(DynamicSpacing::Base08.px(&theme))
                    .text_size(TextSize::Small.px(&theme))
                    .line_height(theme.ui_px(15.))
                    .text_color(theme.text_muted)
                    .child(tr!("transcript_view.waiting_for_an_answer")),
            );
        }
        body = body.child(block);
    }
    card = card.child(body);
    card.into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_activity_card(
    tool: &ToolCall,
    pulse: bool,
    complete: bool,
    theme: Theme,
    nerd: Option<&SharedString>,
    key: (usize, usize),
    tool_open: bool,
    expanded_tools: ExpandedTools,
    copied_sections: CopiedSections,
    expanded_sections: ExpandedSections,
    scroller: MessageScrollerState,
) -> AnyElement {
    // A structured questionnaire gets its own card — the raw arguments JSON
    // and the generic "Command/Arguments" detail would bury the question.
    if crate::ask::is_ask_tool(&tool.name)
        && !crate::ask::questions_from_args(tool.args.as_ref()).is_empty()
    {
        return render_ask_card(tool, theme, key);
    }
    let mcp = mcp_tool_parts(&tool.name);
    let action = match mcp {
        Some((server, _)) => tr!("mcp.card_action", server = server),
        None => activity_action_label(&tool.name),
    };
    let detail = match mcp {
        Some((_, tool_name)) => tool_name.to_string(),
        None => activity_preview(tool),
    };
    let icon = activity_icon(&tool.name);
    // The glyph tone: state color while the call runs or once it failed;
    // settled work is monochrome ink. The glyph shape names the kind, so a
    // per-kind hue was decoration that spent the accent budget and turned a
    // long transcript into a field of tinted badges.
    let tone = if tool.failed {
        theme.error
    } else if pulse {
        theme.accent_text
    } else {
        theme.text_secondary
    };
    let is_command = tool_command(tool).is_some();
    let has_diff = tool.added > 0 || tool.removed > 0;
    let added = tool.added;
    let removed = tool.removed;
    // Expandable activity rows: full arguments and, when captured
    // live, the tool result.
    let has_detail =
        tool.args.as_ref().is_some_and(|args| !args.is_null()) || tool.output.is_some();
    // An edit/write tool carries its own change; the header copy button and
    // the expanded body both read from it.
    let diff = edit_diff(tool);
    // File-content tools get their file's devicon in the header; the
    // devicons palette follows the theme.
    let file_path = file_preview_path(tool);
    let dark = theme.mode == ThemeMode::Dark;

    // gpui 0.2.2's `overflow_hidden` clips a rectangle, not the border radius
    // (`Style::overflow_mask` builds the `ContentMask` from `bounds` alone), so
    // a child background reaching the card's edge paints square corners over
    // the rounded border. The first and last children carry the card's own
    // radius instead.
    let detail_shown = tool_open && has_detail;
    let header_is_only_child = !detail_shown && !tool.failed;
    let mut card = div()
        .id(ElementId::NamedInteger(
            "activity-card".into(),
            (key.0 as u64) << 16 | key.1 as u64,
        ))
        .debug_selector(move || format!("tool-card-{}-{}", key.0, key.1))
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.overlay)
        .child(
            div()
                .h(DynamicSpacing::Base32.px(&theme))
                .px(DynamicSpacing::Base08.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .text_size(TextSize::Default.px(&theme))
                .line_height(BufferLineHeight::Standard.relative())
                .rounded_t(Radius::XLarge.px(&theme))
                .when(header_is_only_child, |row| {
                    row.rounded_b(Radius::XLarge.px(&theme))
                })
                .when(has_detail, |row| row.cursor_pointer())
                .hover(|style| style.bg(theme.overlay_strong))
                .child(activity_badge(icon, tone, theme))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_primary)
                        .child(action),
                )
                .when(!detail.is_empty(), |row| {
                    if is_command {
                        // Terminal preview: a muted prompt sigil then the
                        // command in the code face, syntax-colored exactly
                        // like the expanded detail.
                        let tokens = highlight::tokenize_cached(highlight::Lang::Shell, &detail);
                        row.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .flex()
                                .items_center()
                                .gap(DynamicSpacing::Base06.px(&theme))
                                .font_family(theme::code_font_family())
                                .text_size(theme.code_px(12.))
                                .child(div().flex_none().text_color(theme.text_muted).child("$"))
                                .child(
                                    div().min_w_0().overflow_hidden().whitespace_nowrap().child(
                                        syntax_styled(
                                            &detail,
                                            tokens.first().map(Vec::as_slice),
                                            theme.text_secondary,
                                            theme,
                                        ),
                                    ),
                                ),
                        )
                    } else if let Some(url) = tool_url(tool) {
                        // A fetch tool's preview is the URL itself: a link
                        // that opens the browser without toggling the card
                        // (the row's own click expands the detail).
                        row.child(
                            div()
                                .id(ElementId::NamedInteger(
                                    "tool-link".into(),
                                    (key.0 as u64) << 16 | key.1 as u64,
                                ))
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap(DynamicSpacing::Base04.px(&theme))
                                .cursor_pointer()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(theme.accent_text)
                                        .underline()
                                        .child(display_url(&url).to_string()),
                                )
                                .child(glyph(
                                    "icons/arrow-up-right.svg",
                                    IconSize::XSmall.px(&theme),
                                    theme.accent_text,
                                ))
                                .on_click(move |_, _, cx| {
                                    cx.open_url(&url);
                                    cx.stop_propagation();
                                }),
                        )
                    } else {
                        let mut preview = div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(DynamicSpacing::Base06.px(&theme));
                        if let Some(path) = file_path {
                            preview = preview.child(file_glyph(
                                path,
                                dark,
                                nerd,
                                IconSize::Small.px(&theme),
                                file_badge(path, theme),
                            ));
                        }
                        row.child(
                            preview.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.text_secondary)
                                    .child(detail),
                            ),
                        )
                    }
                })
                .when_some(truncation_chip(&tool.facts, theme), |row, chip| {
                    row.child(chip)
                })
                .when(has_diff, |row| {
                    row.child(render_line_delta(added, removed, theme, 12.5))
                })
                .when(pulse, |row| {
                    row.child(crate::app::spinner(
                        ElementId::NamedInteger(
                            "activity-spin".into(),
                            (key.0 as u64) << 16 | key.1 as u64,
                        ),
                        IconSize::XSmall.px(&theme),
                        theme.accent_text,
                        theme,
                    ))
                })
                .when(tool.failed, |row| {
                    row.child(glyph(
                        "icons/stop.svg",
                        IconSize::XSmall.px(&theme),
                        theme.error,
                    ))
                })
                .when(complete && !tool.failed, |row| {
                    row.child(glyph(
                        "icons/check.svg",
                        IconSize::XSmall.px(&theme),
                        theme.text_muted,
                    ))
                })
                .when_some(diff.clone(), |row, rows| {
                    let copied = copied_sections
                        .borrow()
                        .get(&(key.0, key.1, EDIT_DIFF_COPY_SECTION))
                        .is_some_and(|at| at.elapsed() < COPY_FEEDBACK);
                    let patch = edit_patch_text(&rows);
                    let copied_sections = copied_sections.clone();
                    row.child(
                        icon_button_frame(
                            div().id(ElementId::NamedInteger(
                                "copy-edit-diff".into(),
                                (key.0 as u64) << 16 | key.1 as u64,
                            )),
                            &theme,
                            ButtonSize::Compact,
                        )
                        .tip(tr!("transcript.copy_diff"))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.overlay_strong))
                        .child(glyph(
                            if copied {
                                "icons/check.svg"
                            } else {
                                "icons/copy.svg"
                            },
                            ButtonSize::Compact.icon_size().px(&theme),
                            if copied {
                                theme.success
                            } else {
                                theme.text_muted
                            },
                        ))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(patch.clone()));
                            copied_sections
                                .borrow_mut()
                                .insert((key.0, key.1, EDIT_DIFF_COPY_SECTION), Instant::now());
                            cx.stop_propagation();
                            cx.refresh_windows();
                        }),
                    )
                })
                .child(glyph(
                    if has_detail && tool_open {
                        "icons/chevron-down.svg"
                    } else if has_detail {
                        "icons/chevron-right.svg"
                    } else {
                        // Placeholder keeps the row height stable either way.
                        "icons/chevron-right.svg"
                    },
                    IconSize::XSmall.px(&theme),
                    if has_detail {
                        theme.text_muted
                    } else {
                        theme.text_muted.opacity(0.)
                    },
                )),
        );
    // A failed run carries its first error line right on the card — the
    // fact a user scanning the turn needs, without opening the detail.
    if tool.failed {
        card = card.child(render_tool_error_strip(tool, !detail_shown, theme));
    }
    card = card.when(has_detail, |row| {
        let scroller = scroller.clone();
        row.on_click(move |_, _, cx| {
            let mut open = expanded_tools.borrow_mut();
            if !open.remove(&key) {
                open.insert(key);
            }
            drop(open);
            scroller.remeasure_toggle(key.0);
            cx.refresh_windows();
        })
    });

    if tool_open && has_detail {
        card = card.child(render_tool_detail(
            tool,
            key,
            copied_sections,
            expanded_sections,
            scroller.clone(),
            theme,
        ));
    }
    card.into_any_element()
}

/// The header label for a capped result: "truncated" alone, or
/// "truncated · 1,172/1,303" when pi reported the line budget. `None` for an
/// uncapped tool.
fn truncation_label(facts: &ToolFacts) -> Option<String> {
    if !facts.truncated {
        return None;
    }
    Some(match (facts.output_lines, facts.total_lines) {
        (Some(shown), Some(total)) if total > shown => {
            format!("{} · {shown}/{total}", tr!("transcript.truncated"))
        }
        _ => tr!("transcript.truncated"),
    })
}

/// A compact header chip for a tool whose result pi capped. Warn-colored —
/// the agent saw only part of the data — so the reader learns it without
/// expanding the output.
fn truncation_chip(facts: &ToolFacts, theme: Theme) -> Option<AnyElement> {
    let label = truncation_label(facts)?;
    Some(
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base03.px(&theme))
            .text_size(TextSize::Small.px(&theme))
            .line_height(theme.ui_px(15.))
            .text_color(theme.warning)
            .child(glyph(
                "icons/tools/truncated.svg",
                IconSize::XSmall.px(&theme),
                theme.warning,
            ))
            .child(label)
            .into_any_element(),
    )
}

/// First human-readable error line from a failed tool's captured output.
fn first_error_line(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|line| cap_chars(line, 200)),
        Value::Array(items) => items.iter().find_map(first_error_line),
        Value::Object(map) => map.values().find_map(first_error_line),
        _ => None,
    }
}

/// One-line error strip on a failed tool card: red-washed, drawn stop
/// glyph, and the actual error text (never invented — the fallback says
/// when nothing was captured).
fn render_tool_error_strip(tool: &ToolCall, round_bottom: bool, theme: Theme) -> impl IntoElement {
    let snippet = tool
        .output
        .as_ref()
        .and_then(first_error_line)
        .unwrap_or_else(|| tr!("transcript.run_failed"));
    div()
        .w_full()
        .border_t_1()
        .border_color(theme.border)
        .bg(theme.error.opacity(0.07))
        .when(round_bottom, |s| s.rounded_b(Radius::XLarge.px(&theme)))
        .px(DynamicSpacing::Base08.px(&theme))
        .py(DynamicSpacing::Base04.px(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .flex_none()
        .child(glyph(
            "icons/stop.svg",
            IconSize::XSmall.px(&theme),
            theme.error,
        ))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(TextSize::Small.px(&theme))
                .line_height(BufferLineHeight::Standard.relative())
                .text_color(theme.error)
                .child(snippet),
        )
}

/// The expandable detail card: a Command (terminal-highlighted shell), or
/// Arguments (JSON) when the tool is not a shell command, then the Output.
/// Each section carries its own copy button; tall sections collapse to a
/// preview so a long log never buries the answer that follows it.
fn render_tool_detail(
    tool: &ToolCall,
    key: (usize, usize),
    copied_sections: CopiedSections,
    expanded_sections: ExpandedSections,
    scroller: MessageScrollerState,
    theme: Theme,
) -> impl IntoElement {
    // An edit/write tool shows its change as an inline diff — the arguments
    // JSON would just be the same text, unread. Everything else: a command
    // tool shows its shell command as terminal input, and the rest show the
    // JSON arguments. The result is JSON-highlighted when the tool returned
    // structured data, plain otherwise.
    let diff = edit_diff(tool);
    let first = match tool_command(tool) {
        Some(command) => Some((
            tr!("transcript.command"),
            command,
            Some(highlight::Lang::Shell),
            true,
        )),
        None if diff.is_some() => None,
        None => tool.args.as_ref().map(|args| {
            (
                tr!("transcript.arguments"),
                display_value_capped(args, DETAIL_TEXT_CAP),
                section_lang(Some(args)),
                false,
            )
        }),
    };
    let second = tool.output.as_ref().map(|output| {
        (
            tr!("transcript.output"),
            display_value_capped(output, OUTPUT_TEXT_CAP),
            section_lang(Some(output)),
            false,
        )
    });
    let sections: Vec<_> = [
        first.map(|(label, content, lang, prompt)| (0u8, label, content, lang, prompt)),
        second.map(|(label, content, lang, prompt)| (1u8, label, content, lang, prompt)),
    ]
    .into_iter()
    .flatten()
    .filter(|(_, _, content, _, _)| !content.trim().is_empty())
    .collect();
    let has_sections = !sections.is_empty();

    let mut container = div().w_full().min_w_0().flex().flex_col();
    if let Some(rows) = diff.as_ref() {
        container = container.child(
            div()
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(theme.border)
                // With no sections below it, the diff is the card's last
                // child and carries its bottom radius.
                .when(!has_sections, |d| d.rounded_b(Radius::XLarge.px(&theme)))
                .child(render_edit_diff(
                    rows,
                    key,
                    expanded_sections.clone(),
                    scroller.clone(),
                    theme,
                )),
        );
    }
    if has_sections {
        // One recessed terminal surface holds every section, divided by a
        // hairline: a command reads top-to-bottom — input, then output — the
        // way a shell transcript does, instead of two blocks whose only
        // boundary is a gap. The surface is a code plane (`code_bg`), the
        // same treatment a markdown code block takes.
        let mut surface = div()
            .w_full()
            .min_w_0()
            .bg(theme.code_bg)
            .flex()
            .flex_col()
            // The surface is always the card's last child, so it carries the
            // card's bottom radius (gpui clips children to a rectangle).
            .rounded_b(Radius::XLarge.px(&theme))
            // Without a diff above it, the surface owns the rule that
            // separates it from the card header.
            .when(diff.is_none(), |s| {
                s.border_t_1().border_color(theme.border)
            });
        for (section_ix, (section, label, content, lang, prompt)) in
            sections.into_iter().enumerate()
        {
            if section_ix > 0 {
                surface =
                    surface.child(div().w_full().h(BORDER_WIDTH).flex_none().bg(theme.border));
            }
            surface = surface.child(render_detail_section(
                key,
                section,
                &label,
                content,
                lang,
                prompt,
                copied_sections.clone(),
                expanded_sections.clone(),
                scroller.clone(),
                theme,
            ));
        }
        container = container.child(surface);
    }
    container
}

/// A structured (non-string) value highlights as JSON; strings and scalars
/// stay plain so raw command output is not mislabeled as a document.
fn section_lang(value: Option<&Value>) -> Option<highlight::Lang> {
    match value {
        Some(Value::String(_)) | None => None,
        Some(_) => Some(highlight::Lang::Json),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_detail_section(
    key: (usize, usize),
    section: u8,
    label: &str,
    content: String,
    lang: Option<highlight::Lang>,
    prompt: bool,
    copied_sections: CopiedSections,
    expanded_sections: ExpandedSections,
    scroller: MessageScrollerState,
    theme: Theme,
) -> impl IntoElement {
    let copied = copied_sections
        .borrow()
        .get(&(key.0, key.1, section))
        .is_some_and(|at| at.elapsed() < COPY_FEEDBACK);
    let copy_content = content.clone();
    // Tall sections (a test run's log, a big edit's arguments) collapse to a
    // prefix preview; the toggle remeasures the row. Copy always carries the
    // full captured text, folded or not.
    let lines: Vec<&str> = content.split('\n').collect();
    let foldable = lines.len() > OUTPUT_COLLAPSE_LINES;
    let expanded = foldable
        && expanded_sections
            .borrow()
            .contains(&(key.0, key.1, section));
    let visible: &[&str] = if foldable && !expanded {
        &lines[..OUTPUT_PREVIEW_LINES]
    } else if lines.len() > OUTPUT_EXPANDED_PAINT_LINES {
        &lines[..OUTPUT_EXPANDED_PAINT_LINES]
    } else {
        &lines
    };
    let paint_clipped = expanded && lines.len() > OUTPUT_EXPANDED_PAINT_LINES;
    let card = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        // The label row: a quiet heading on the terminal surface, with the
        // copy control pinned right so every section's action shares one
        // axis.
        .child(
            div()
                .w_full()
                .min_w_0()
                .h(ButtonSize::Default.height(&theme))
                .px(DynamicSpacing::Base12.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(TextSize::Small.px(&theme))
                        .line_height(theme.ui_px(15.))
                        .text_color(theme.text_muted)
                        .child(label.to_string()),
                )
                .child(
                    icon_button_frame(
                        div().id(ElementId::NamedInteger(
                            "copy-activity-section".into(),
                            ((key.0 as u64) << 16 | key.1 as u64) << 8 | section as u64,
                        )),
                        &theme,
                        ButtonSize::Default,
                    )
                    .tip(tr!("transcript.copy_section"))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.overlay_strong))
                    .child(glyph(
                        if copied {
                            "icons/check.svg"
                        } else {
                            "icons/copy.svg"
                        },
                        ButtonSize::Default.icon_size().px(&theme),
                        if copied {
                            theme.success
                        } else {
                            theme.text_muted
                        },
                    ))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_content.clone()));
                        copied_sections
                            .borrow_mut()
                            .insert((key.0, key.1, section), Instant::now());
                        cx.refresh_windows();
                    }),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .px(DynamicSpacing::Base12.px(&theme))
                .pb(DynamicSpacing::Base08.px(&theme))
                .flex()
                .items_start()
                .gap(DynamicSpacing::Base08.px(&theme))
                .when(prompt, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .font_family(theme::code_font_family())
                            .text_size(theme.term_px(13.))
                            .line_height(theme.term_px(20.))
                            .text_color(theme.text_muted)
                            .child("$"),
                    )
                })
                .child(render_detail_body(
                    &content, visible, lang, key, section, theme,
                )),
        );
    if foldable {
        let total = lines.len();
        let toggle_label = if expanded {
            tr!("transcript.show_less")
        } else {
            // The key's placeholder is `%{count}` — binding `total` left the
            // literal `%{count}` printed in the fold label.
            tr!("transcript.show_all_lines", count = total)
        };
        card.child(
            button_frame(
                div().id(ElementId::NamedInteger(
                    "section-fold".into(),
                    ((key.0 as u64) << 16 | key.1 as u64) << 8 | section as u64,
                )),
                &theme,
                ButtonSize::Default,
            )
            // Full width with its label at the start, so the paint-cap note
            // can fill the rest of the row.
            .w_full()
            .justify_start()
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .hover(|style| style.text_color(theme.text_primary))
            .child(glyph(
                if expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                },
                IconSize::XSmall.px(&theme),
                theme.text_muted,
            ))
            .child(toggle_label)
            .when(paint_clipped, |toggle| {
                toggle.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.text_muted)
                        .child(tr!(
                            "transcript.showing_first_output",
                            count = OUTPUT_EXPANDED_PAINT_LINES
                        )),
                )
            })
            .on_click(move |_, _, cx| {
                let fold_key = (key.0, key.1, section);
                let mut open = expanded_sections.borrow_mut();
                if !open.remove(&fold_key) {
                    open.insert(fold_key);
                }
                drop(open);
                scroller.remeasure_toggle(key.0);
                cx.refresh_windows();
            }),
        )
    } else {
        card
    }
}

/// Highlighted, wrapping tool text: one line per row so a long command or
/// blob wraps inside the card rather than forcing it wide. `visible` is the
/// prefix slice actually painted (collapse preview / paint cap); token
/// ranges index lines absolutely, and a prefix slice keeps them aligned.
fn render_detail_body(
    content: &str,
    visible: &[&str],
    lang: Option<highlight::Lang>,
    key: (usize, usize),
    section: u8,
    theme: Theme,
) -> AnyElement {
    let tokens = lang.map(|lang| highlight::tokenize_cached(lang, content));
    div()
        .flex_1()
        .min_w_0()
        .font_family(theme::code_font_family())
        .text_size(theme.term_px(13.))
        .line_height(theme.term_px(20.))
        .text_color(theme.code_text)
        .whitespace_normal()
        .flex()
        .flex_col()
        .children(visible.iter().enumerate().map(move |(line_ix, line)| {
            let line_id = ElementId::NamedInteger(
                "detail-line".into(),
                ((key.0 as u64) << 34)
                    | ((key.1 as u64) << 18)
                    | ((section as u64) << 12)
                    | line_ix as u64,
            );
            div().w_full().min_w_0().child(syntax_line(
                line,
                tokens
                    .as_deref()
                    .and_then(|lines| lines.get(line_ix))
                    .map(Vec::as_slice),
                line_id,
                theme.code_text,
                theme,
            ))
        }))
        .into_any_element()
}

// ── inline edit diff ───────────────────────────────────────────────────────

/// Kind of one row in an inline edit diff. `Break` separates two edits that
/// arrived in the same tool call so their hunks don't run together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffRowKind {
    Context,
    Addition,
    Deletion,
    Break,
}

/// One painted row: the text plus the syntax tokens resolved for its line.
#[derive(Clone)]
struct DiffRow {
    kind: DiffRowKind,
    text: String,
    tokens: Option<Vec<Token>>,
}

type EditDiffCache = HashMap<u64, Rc<Vec<DiffRow>>>;

thread_local! {
    /// Bounded per-tool edit-diff memo. A card repaints every frame (and on
    /// every streaming tick); the line diff and token pass run only when the
    /// tool's text actually changes.
    static EDIT_DIFF_CACHE: RefCell<EditDiffCache> = RefCell::new(HashMap::new());
}

/// The inline diff for an `edit` / `write` tool, or `None` when its arguments
/// carry no text to diff. Content comes only from the tool itself.
fn edit_diff(tool: &ToolCall) -> Option<Rc<Vec<DiffRow>>> {
    let key = {
        let mut hasher = DefaultHasher::new();
        tool.name.hash(&mut hasher);
        tool.path.hash(&mut hasher);
        if let Some(args) = tool.args.as_ref() {
            if let Some(edits) = args.get("edits").and_then(Value::as_array) {
                for edit in edits {
                    edit.get("oldText")
                        .and_then(Value::as_str)
                        .hash(&mut hasher);
                    edit.get("newText")
                        .and_then(Value::as_str)
                        .hash(&mut hasher);
                }
            }
            if let Some(content) = args.get("content").and_then(Value::as_str) {
                content.hash(&mut hasher);
            }
        }
        hasher.finish()
    };
    EDIT_DIFF_CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(&key) {
            return Some(hit.clone());
        }
        let rows = Rc::new(build_edit_diff(tool)?);
        let mut cache = cache.borrow_mut();
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(key, rows.clone());
        Some(rows)
    })
}

fn build_edit_diff(tool: &ToolCall) -> Option<Vec<DiffRow>> {
    let lang = tool.path.as_deref().and_then(path_language);
    match tool.name.as_str() {
        "edit" => {
            let edits = tool.args.as_ref()?.get("edits")?.as_array()?;
            let mut rows: Vec<DiffRow> = Vec::new();
            for edit in edits {
                let (Some(old), Some(new)) = (
                    edit.get("oldText").and_then(Value::as_str),
                    edit.get("newText").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if !rows.is_empty() {
                    rows.push(DiffRow {
                        kind: DiffRowKind::Break,
                        text: String::new(),
                        tokens: None,
                    });
                }
                rows.extend(line_diff(old, new, lang));
            }
            (!rows.is_empty()).then_some(rows)
        }
        "write" => {
            let content = tool.args.as_ref()?.get("content")?.as_str()?;
            let lines = split_diff_lines(content);
            let tokens = lang.map(|lang| highlight::tokenize(lang, content));
            Some(
                lines
                    .iter()
                    .enumerate()
                    .map(|(ix, text)| DiffRow {
                        kind: DiffRowKind::Addition,
                        text: (*text).to_string(),
                        tokens: tokens.as_ref().and_then(|t| t.get(ix)).cloned(),
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// Language for a file path's extension, when the lexer knows it.
fn path_language(path: &str) -> Option<highlight::Lang> {
    let ext = Path::new(path).extension()?.to_str()?;
    highlight::lang_for_tag(ext)
}

/// Split a code block into lines, dropping a single trailing newline so a
/// file that ends `\n` doesn't paint a phantom empty row.
fn split_diff_lines(text: &str) -> Vec<&str> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    }
}

/// Longest-common-subsequence line diff: unchanged lines become context, the
/// rest are deletions (old only) and additions (new only). Oversized edits
/// fall back to a whole-block replacement so the paint stays bounded.
fn line_diff(old: &str, new: &str, lang: Option<highlight::Lang>) -> Vec<DiffRow> {
    let old_lines = split_diff_lines(old);
    let new_lines = split_diff_lines(new);
    let old_tokens = lang.map(|lang| highlight::tokenize(lang, old));
    let new_tokens = lang.map(|lang| highlight::tokenize(lang, new));
    let (n, m) = (old_lines.len(), new_lines.len());
    let mut ops: Vec<(DiffRowKind, Option<usize>, Option<usize>)> = Vec::new();
    if n.saturating_mul(m) > 250_000 {
        ops.extend((0..n).map(|i| (DiffRowKind::Deletion, Some(i), None)));
        ops.extend((0..m).map(|j| (DiffRowKind::Addition, None, Some(j))));
    } else {
        let mut lcs = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i][j] = if old_lines[i] == new_lines[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < n && j < m {
            if old_lines[i] == new_lines[j] {
                ops.push((DiffRowKind::Context, Some(i), Some(j)));
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                ops.push((DiffRowKind::Deletion, Some(i), None));
                i += 1;
            } else {
                ops.push((DiffRowKind::Addition, None, Some(j)));
                j += 1;
            }
        }
        while i < n {
            ops.push((DiffRowKind::Deletion, Some(i), None));
            i += 1;
        }
        while j < m {
            ops.push((DiffRowKind::Addition, None, Some(j)));
            j += 1;
        }
    }
    ops.into_iter()
        .map(|(kind, old_ix, new_ix)| {
            let (text, tokens) = match kind {
                DiffRowKind::Deletion => (
                    old_ix.and_then(|ix| old_lines.get(ix)).copied(),
                    old_tokens
                        .as_ref()
                        .and_then(|t| old_ix.and_then(|ix| t.get(ix))),
                ),
                _ => (
                    new_ix.and_then(|ix| new_lines.get(ix)).copied(),
                    new_tokens
                        .as_ref()
                        .and_then(|t| new_ix.and_then(|ix| t.get(ix))),
                ),
            };
            DiffRow {
                kind,
                text: text.unwrap_or("").to_string(),
                tokens: tokens.cloned(),
            }
        })
        .collect()
}

/// Unified-patch text for the copy button — `+`/`-`/space prefixes, minus the
/// cosmetic break rows.
fn edit_patch_text(rows: &[DiffRow]) -> String {
    let mut out = String::new();
    for row in rows {
        let marker = match row.kind {
            DiffRowKind::Addition => Some('+'),
            DiffRowKind::Deletion => Some('-'),
            DiffRowKind::Context => Some(' '),
            DiffRowKind::Break => None,
        };
        if let Some(marker) = marker {
            out.push(marker);
            out.push_str(&row.text);
            out.push('\n');
        }
    }
    out
}

/// The diff body inside an expanded edit card, full-bleed like the Review
/// pane's rows. Long diffs fold to a preview; copy always carries the whole
/// patch.
fn render_edit_diff(
    rows: &[DiffRow],
    key: (usize, usize),
    expanded_sections: ExpandedSections,
    scroller: MessageScrollerState,
    theme: Theme,
) -> AnyElement {
    let total = rows.len();
    let foldable = total > EDIT_DIFF_COLLAPSE_LINES;
    let expanded = foldable
        && expanded_sections
            .borrow()
            .contains(&(key.0, key.1, EDIT_DIFF_COPY_SECTION));
    let visible = if foldable && !expanded {
        EDIT_DIFF_PREVIEW_LINES.min(total)
    } else {
        EDIT_DIFF_PAINT_LINES.min(total)
    };
    let clipped = expanded && total > EDIT_DIFF_PAINT_LINES;
    let mut body = div()
        .w_full()
        .min_w_0()
        .py(DynamicSpacing::Base04.px(&theme))
        .flex()
        .flex_col()
        .children(
            rows.iter()
                .take(visible)
                .enumerate()
                .map(|(line_ix, row)| render_edit_diff_row(row, line_ix, key, theme)),
        );
    if foldable {
        let label = if expanded {
            tr!("transcript.show_less")
        } else {
            tr!("transcript.show_all_lines", count = total)
        };
        body = body.child(
            button_frame(
                div().id(ElementId::NamedInteger(
                    "edit-diff-fold".into(),
                    ((key.0 as u64) << 16 | key.1 as u64) << 8 | EDIT_DIFF_COPY_SECTION as u64,
                )),
                &theme,
                ButtonSize::Default,
            )
            // Full width with its label at the start, so the paint-cap note
            // can fill the rest of the row.
            .w_full()
            .justify_start()
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .hover(|style| style.text_color(theme.text_primary))
            .child(glyph(
                if expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                },
                IconSize::XSmall.px(&theme),
                theme.text_muted,
            ))
            .child(label)
            .when(clipped, |toggle| {
                toggle.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.text_muted)
                        .child(tr!(
                            "transcript.showing_first_diff",
                            count = EDIT_DIFF_PAINT_LINES
                        )),
                )
            })
            .on_click(move |_, _, cx| {
                let fold_key = (key.0, key.1, EDIT_DIFF_COPY_SECTION);
                let mut open = expanded_sections.borrow_mut();
                if !open.remove(&fold_key) {
                    open.insert(fold_key);
                }
                drop(open);
                scroller.remeasure_toggle(key.0);
                // The fold is not a card toggle — don't collapse the card
                // out from under the click.
                cx.stop_propagation();
                cx.refresh_windows();
            }),
        );
    }
    body.into_any_element()
}

fn render_edit_diff_row(
    row: &DiffRow,
    line_ix: usize,
    key: (usize, usize),
    theme: Theme,
) -> AnyElement {
    if row.kind == DiffRowKind::Break {
        return div()
            .w_full()
            .flex_none()
            .h(px(9.))
            .border_t_1()
            .border_color(theme.border)
            .into_any_element();
    }
    let line_id = ElementId::NamedInteger(
        "diff-line".into(),
        ((key.0 as u64) << 34) | ((key.1 as u64) << 18) | line_ix as u64,
    );
    let (body_bg, gutter_bg, edge, marker_color, marker) = match row.kind {
        DiffRowKind::Addition => (
            Some(theme.success.opacity(diff_body_wash(theme))),
            Some(theme.success.opacity(diff_gutter_wash(theme))),
            Some(theme.success),
            theme.success,
            "+",
        ),
        DiffRowKind::Deletion => (
            Some(theme.error.opacity(diff_body_wash(theme))),
            Some(theme.error.opacity(diff_gutter_wash(theme))),
            Some(theme.error),
            theme.error,
            "-",
        ),
        _ => (None, None, None, theme.text_muted, ""),
    };
    div()
        .w_full()
        .min_w_0()
        .min_h(px(EDIT_DIFF_ROW_HEIGHT))
        .flex()
        .font_family(theme::code_font_family())
        .text_size(theme.code_px(EDIT_DIFF_TEXT_SIZE))
        .line_height(theme.code_px(18.))
        .when_some(edge, |row, edge| row.border_l_2().border_color(edge))
        .child(
            div()
                .w(px(24.))
                .flex_none()
                .flex()
                .items_start()
                .justify_center()
                .text_color(marker_color)
                .when_some(gutter_bg, |gutter, bg| gutter.bg(bg))
                .child(marker),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .pr(DynamicSpacing::Base12.px(&theme))
                .overflow_hidden()
                // A long line wraps to the next row instead of being clipped —
                // the diff must stay readable at any pane width.
                .whitespace_normal()
                .when_some(body_bg, |body, bg| body.bg(bg))
                .text_color(theme.code_text)
                .child(diff_row_text(row, line_id, theme)),
        )
        .into_any_element()
}

fn diff_body_wash(theme: Theme) -> f32 {
    if theme.mode == ThemeMode::Dark {
        0.18
    } else {
        0.12
    }
}

fn diff_gutter_wash(theme: Theme) -> f32 {
    if theme.mode == ThemeMode::Dark {
        0.24
    } else {
        0.15
    }
}

/// Syntax-colored text for one diff row (tokens are already per-line); the
/// row's text (with its `+`/`-` marker) is what a selection copies.
fn diff_row_text(row: &DiffRow, key: ElementId, theme: Theme) -> SelectableText {
    let (display, runs) = syntax_runs(&row.text, row.tokens.as_deref(), theme.code_text, theme);
    let scope = text_scope();
    let range = scope
        .as_ref()
        .and_then(|scope| scope.state.borrow().range_for(&key, &display));
    let runs = highlight_runs(runs, range, selection_color(theme));
    SelectableText::new(
        key,
        StyledText::new(display).with_runs(runs),
        row.text.clone().into(),
        Vec::new(),
        scope,
    )
}

/// Arguments / result text: strings as-is, everything else pretty-printed.
fn display_value_capped(value: &Value, cap: usize) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };
    cap_chars(&text, cap)
}

fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    } else {
        text.to_string()
    }
}

/// The quiet timestamp chip in a message/run footer.
fn footer_time_stamp(text: String, theme: Theme) -> AnyElement {
    div()
        .flex_none()
        .h(FOOTER_BUTTON.height(&theme))
        .px(DynamicSpacing::Base04.px(&theme))
        .flex()
        .items_center()
        .text_size(TextSize::Small.px(&theme))
        .line_height(BufferLineHeight::Standard.relative())
        .text_color(theme.text_muted)
        .child(text)
        .into_any_element()
}

/// Message footer: persistent quiet copy button + timestamp stamp and, when
/// pi reports usage, a compact `↑in ↓out · $cost` metric. Hovering the metric
/// opens the full token/cache/cost breakdown.
#[allow(clippy::too_many_arguments)]
fn render_message_footer(
    copy_text: String,
    ix: usize,
    stamp: Option<AnyElement>,
    usage: Option<MessageUsage>,
    generation: Option<Duration>,
    copied: bool,
    align_right: bool,
    theme: Theme,
    copied_at: Rc<RefCell<HashMap<usize, Instant>>>,
    hovered_usage: Rc<Cell<Option<usize>>>,
) -> impl IntoElement {
    let button = icon_button_frame(
        div().id(ElementId::NamedInteger("copy-response".into(), ix as u64)),
        &theme,
        FOOTER_BUTTON,
    )
    .tip(tr!("transcript.copy_message"))
    .cursor_pointer()
    .hover(|style| style.bg(theme.overlay_strong))
    .child(glyph(
        if copied {
            "icons/check.svg"
        } else {
            "icons/copy.svg"
        },
        FOOTER_BUTTON.icon_size().px(&theme),
        if copied {
            theme.success
        } else {
            theme.text_muted
        },
    ))
    .on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
        copied_at.borrow_mut().insert(ix, Instant::now());
        cx.refresh_windows();
    });
    let mut footer = div()
        .h(FOOTER_BUTTON.height(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base01.px(&theme))
        .when(align_right, |row| row.justify_end());
    if align_right {
        // Right-aligned footer: timestamp first, then actions.
        if let Some(stamp) = stamp {
            footer = footer.child(stamp);
        }
        footer = footer.child(button);
    } else {
        footer = footer.child(button);
        if let Some(stamp) = stamp {
            footer = footer.child(stamp);
        }
    }
    if let Some(metric) = usage_metric(usage, generation, ix, theme, hovered_usage) {
        footer = footer.child(metric);
    }
    footer
}

/// Compact `↑in ↓out · $cost` footer metric with a hover breakdown card.
/// `None` when the turn has no reported usage, so the footer stays clean.
fn usage_metric(
    usage: Option<MessageUsage>,
    generation: Option<Duration>,
    ix: usize,
    theme: Theme,
    hovered_usage: Rc<Cell<Option<usize>>>,
) -> Option<AnyElement> {
    let usage = usage?;
    let hovered = hovered_usage.get() == Some(ix);
    let label = {
        let mut label = format!(
            "↑{} ↓{}",
            format_tokens(usage.input),
            format_tokens(usage.output)
        );
        if usage.cache_read > 0 {
            if let Some(percent) = usage.cache_read_percent() {
                label.push_str(&tr!(
                    "transcript_view.cached_share",
                    percent = hit_percent_label(percent)
                ));
            }
        }
        if let Some(cost) = usage.cost.filter(|cost| *cost > 0.0) {
            label.push_str(" · ");
            label.push_str(&format_message_cost(cost));
        }
        if let Some(tps) = generation_tps(&usage, generation) {
            label.push_str(" · ");
            label.push_str(&tr!("transcript_view.tps_value", rate = format_tps(tps)));
        }
        label
    };
    let mut metric = div()
        .id(ElementId::NamedInteger("usage-metric".into(), ix as u64))
        .relative()
        .ml(DynamicSpacing::Base06.px(&theme))
        .h(FOOTER_BUTTON.height(&theme))
        .flex()
        .items_center()
        .text_size(TextSize::Small.px(&theme))
        .line_height(BufferLineHeight::Standard.relative())
        .text_color(theme.text_muted)
        .child(label)
        .on_hover(move |is_hovered, _, cx| {
            let next = if *is_hovered { Some(ix) } else { None };
            if hovered_usage.get() != next {
                hovered_usage.set(next);
                cx.refresh_windows();
            }
        });
    if hovered {
        metric = metric.child(
            div()
                .absolute()
                .bottom_full()
                .left_0()
                .mb(popover::MENU_OFFSET)
                .child(deferred(usage_breakdown_card(&usage, generation, theme))),
        );
    }
    Some(metric.into_any_element())
}

/// Hover card: the full per-turn token/cache/cost breakdown behind the
/// compact footer metric.
fn usage_breakdown_card(
    usage: &MessageUsage,
    generation: Option<Duration>,
    theme: Theme,
) -> AnyElement {
    let cost = usage
        .cost
        .map(format_message_cost)
        .unwrap_or_else(|| "—".into());
    div()
        .w(px(224.))
        .elevation_2(&theme)
        .px(DynamicSpacing::Base12.px(&theme))
        .py(DynamicSpacing::Base08.px(&theme))
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base06.px(&theme))
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("transcript_view.message_usage")),
        )
        .child(usage_metric_row(
            "icons/usage-input.svg",
            tr!("transcript.input"),
            format_tokens(usage.input),
            theme,
        ))
        .child(usage_metric_row(
            "icons/usage-output.svg",
            tr!("transcript.output"),
            format_tokens(usage.output),
            theme,
        ))
        .child(usage_metric_row(
            "icons/cache-read.svg",
            tr!("transcript.cache_read"),
            cache_read_label(usage),
            theme,
        ))
        .child(usage_metric_row(
            "icons/cache-write.svg",
            tr!("transcript.cache_write"),
            format_tokens(usage.cache_write),
            theme,
        ))
        .child(div().w_full().h(px(1.)).bg(theme.border))
        .child(usage_metric_row(
            "icons/usage-total.svg",
            tr!("transcript.total"),
            format_tokens(usage.total),
            theme,
        ))
        .children(generation_tps(usage, generation).map(|tps| {
            usage_metric_row(
                "icons/usage-output.svg",
                tr!("transcript_view.rate"),
                tr!("transcript_view.tps_value", rate = format_tps(tps)),
                theme,
            )
        }))
        .child(usage_metric_row(
            "icons/usage-cost.svg",
            "Cost",
            cost,
            theme,
        ))
        .into_any_element()
}

fn usage_metric_row(
    icon_path: &'static str,
    label: impl Into<SharedString>,
    value: String,
    theme: Theme,
) -> impl IntoElement {
    let label = label.into();
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .child(glyph(
            icon_path,
            IconSize::Small.px(&theme),
            theme.text_muted,
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .whitespace_nowrap()
                .child(label),
        )
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_primary)
                .whitespace_nowrap()
                .child(value),
        )
}

/// `40.0K · 89% hit` for the cache-read row: the token count plus the
/// provider cache hit rate over prompt tokens (`None` when the prompt was
/// empty, so no meaningless `0%`).
fn cache_read_label(usage: &MessageUsage) -> String {
    let tokens = format_tokens(usage.cache_read);
    match usage.cache_read_percent() {
        Some(percent) => tr!(
            "transcript_view.hit_share",
            tokens = tokens,
            percent = hit_percent_label(percent)
        ),
        None => tokens,
    }
}

/// Per-message costs are usually fractions of a cent, so show four decimals
/// below one cent rather than collapsing every row to `<$0.01`.
fn format_message_cost(cost: f64) -> String {
    if !cost.is_finite() || cost <= 0.0 {
        "$0.00".into()
    } else if cost < 0.01 {
        format!("${cost:.4}")
    } else if cost < 100.0 {
        format!("${cost:.2}")
    } else {
        format!("${cost:.0}")
    }
}

/// Local `HH:MM` for an epoch-millis stamp; `None` when it cannot be resolved.
fn format_time(millis: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

fn glyph(path: &'static str, size: impl Into<Pixels>, color: Hsla) -> impl IntoElement {
    // The shared icon carries the button hover ink-lift, so every control in
    // the transcript that opts into `BUTTON_GROUP` brightens its glyph.
    crate::app::icon(path, size, color)
}

fn fold_label(elapsed: Option<Duration>) -> String {
    match elapsed {
        Some(duration) => tr!(
            "transcript_view.worked_for",
            duration = format_duration(duration)
        ),
        None => tr!("transcript.worked"),
    }
}

/// Spoken duration units: `5 minutes 41 seconds`.
fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs().max(1);
    if secs < 60 {
        return spoken_unit(secs, "s");
    }
    if secs < 3_600 {
        let minutes = secs / 60;
        let remaining = secs % 60;
        let first = spoken_unit(minutes, "m");
        return if remaining > 0 {
            format!("{} {}", first, spoken_unit(remaining, "s"))
        } else {
            first
        };
    }
    let hours = secs / 3_600;
    let minutes = (secs % 3_600) / 60;
    let first = spoken_unit(hours, "h");
    if minutes > 0 {
        format!("{} {}", first, spoken_unit(minutes, "m"))
    } else {
        first
    }
}

/// One spoken duration unit in the active locale: "1 second" / "7 minutes".
fn spoken_unit(count: u64, kind: &str) -> String {
    let word = if count == 1 {
        match kind {
            "s" => tr!("transcript_view.second"),
            "m" => tr!("transcript_view.minute"),
            _ => tr!("transcript_view.hour"),
        }
    } else {
        match kind {
            "s" => tr!("transcript_view.seconds"),
            "m" => tr!("transcript_view.minutes"),
            _ => tr!("transcript_view.hours"),
        }
    };
    format!("{} {}", count, word)
}

fn render_turn_fold(
    ix: usize,
    expanded: bool,
    elapsed: Option<Duration>,
    theme: Theme,
    expanded_turns: Rc<RefCell<HashSet<usize>>>,
    scroller: MessageScrollerState,
) -> impl IntoElement {
    let label = fold_label(elapsed);
    div()
        .w_full()
        .h(px(24.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().h(px(1.)).flex_1().bg(theme.border))
        .child(
            button_frame(
                div().id(ElementId::NamedInteger("turn-fold".into(), ix as u64)),
                &theme,
                ButtonSize::Default,
            )
            .group(BUTTON_GROUP)
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .hover(|style| style.text_color(theme.text_secondary))
            .child(label)
            .child(glyph(
                if expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                },
                IconSize::XSmall.px(&theme),
                theme.text_muted,
            ))
            .on_click(move |_, _, cx| {
                toggle_index(&expanded_turns, ix);
                scroller.remeasure_toggle(ix);
                cx.refresh_windows();
            }),
        )
        .child(div().h(px(1.)).flex_1().bg(theme.border))
}

fn working_wave_dots(theme: Theme, elapsed_ms: u128) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base03.px(&theme))
        .children((0..3).map(move |i| {
            let phase = ((elapsed_ms / 180) + i as u128 * 2) % 6;
            let on = phase < 3;
            div()
                .size(px(4.))
                .rounded_full()
                .bg(if on { theme.accent } else { theme.border })
        }))
}

/// What the agent is doing right now, for the live indicator: "Running
/// cargo test", "Reading src/auth.rs", "Thinking…". Derived from the live
/// step's latest tool so the status names real work, never a bare spinner.
fn working_activity_label(step: &Step) -> Option<String> {
    if let Some(tool) = step.tools.last() {
        let detail = activity_preview(tool);
        let verb = match tool.name.as_str() {
            "bash" | "shell" | "terminal" | "exec" | "run" => {
                tr!("transcript_view.verb_running")
            }
            "read" | "view" => tr!("transcript_view.verb_reading"),
            "grep" | "find" | "glob" | "search" => tr!("transcript_view.verb_searching"),
            "edit" | "write" => tr!("transcript_view.verb_editing"),
            other => {
                let action = activity_action_label(other);
                return Some(if detail.is_empty() {
                    tr!("transcript_view.using_tool", action = action)
                } else {
                    format!("{action} {detail}")
                });
            }
        };
        return Some(if detail.is_empty() {
            verb
        } else {
            format!("{verb} {detail}")
        });
    }
    // Reasoning is only the current activity while the step has no answer
    // text yet; once the model is writing, the thinking phase is over.
    if !step.thinking.is_empty() && step.text.trim().is_empty() {
        return Some(tr!("transcript_view.thinking_ellipsis"));
    }
    None
}

/// Quiet end-of-turn marker for a cancelled generation: a stop glyph and
/// "Stopped" in the meta color. Sits where the working indicator was.
fn render_stopped_marker(theme: Theme) -> impl IntoElement {
    div()
        .h(px(22.))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base08.px(&theme))
        .child(glyph(
            "icons/stop.svg",
            IconSize::XSmall.px(&theme),
            theme.text_muted,
        ))
        .child(
            div()
                .text_size(TextSize::Default.px(&theme))
                .line_height(theme.ui_px(18.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(tr!("transcript_view.stopped")),
        )
}

fn render_working_indicator(
    elapsed: Duration,
    activity: Option<String>,
    tps: Option<f64>,
    theme: Theme,
) -> impl IntoElement {
    // Name the current activity when we know it; the elapsed time stays so
    // a long-running command still reads as progress, not a hang. When the
    // row is producing output, the rate rides alongside as `· 42 tok/s`.
    let timing = match tps {
        Some(tps) => tr!(
            "transcript_view.working_for_with_rate",
            duration = format_working_elapsed(elapsed),
            rate = format_tps(tps)
        ),
        None => tr!(
            "transcript_view.working_for",
            duration = format_working_elapsed(elapsed)
        ),
    };
    let label = match activity {
        Some(activity) => format!("{activity} · {timing}"),
        None => timing,
    };
    let label_size = theme.ui_px(13.5);
    let label_line = theme.ui_px(18.);
    // The live label carries the sidebar's running-session shimmer — the same
    // accent band sweeping across the ink. Reduce-motion keeps the accent
    // signal without the perpetual sweep.
    let label_el: AnyElement = if theme.ui.reduce_motion {
        div()
            .min_w_0()
            .truncate()
            .text_size(label_size)
            .line_height(label_line)
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.accent_text)
            .child(label)
            .into_any_element()
    } else {
        ShimmerText::new(label, theme.text_muted, theme.accent)
            .with_animation(
                ElementId::NamedInteger("working-shimmer".into(), 0),
                Animation::new(Duration::from_millis(2000)).repeat(),
                |mut text, delta| {
                    text.phase = delta;
                    text
                },
            )
            .into_any_element()
    };
    div()
        // The row needs a definite width: the shimmer paints at `width: 100%`
        // of its wrapper, so an indefinite (shrink-to-fit) row would collapse
        // the label — timer and all — to nothing.
        .debug_selector(|| "working-indicator".to_string())
        .w_full()
        .h(px(22.))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base08.px(&theme))
        .child(working_wave_dots(theme, elapsed.as_millis()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(label_size)
                .line_height(label_line)
                .font_weight(FontWeight::MEDIUM)
                .child(label_el),
        )
}

/// Compact working-elapsed form: `12s` / `5m 41s` / `1h 2m`.
fn format_working_elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3_600 {
        let minutes = secs / 60;
        let remaining = secs % 60;
        return if remaining > 0 {
            format!("{minutes}m {remaining}s")
        } else {
            format!("{minutes}m")
        };
    }
    let hours = secs / 3_600;
    let minutes = (secs % 3_600) / 60;
    if minutes > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{hours}h")
    }
}

/// Output tokens a streaming row has produced so far: the provider's latest
/// cumulative `usage.output` when it reports one, else a `chars/4` estimate
/// over the live text (reasoning included — reasoning is a subset of the
/// provider's output).
fn streamed_output_tokens(message: &ChatMessage) -> u64 {
    if let Some(usage) = message.usage() {
        if usage.output > 0 {
            return usage.output;
        }
    }
    let chars: usize = message
        .steps
        .iter()
        .map(|step| step.text.chars().count() + step.thinking.chars().count())
        .sum();
    (chars / 4) as u64
}

/// The denominator for a streaming turn's rate. Settled steps contribute
/// pi's `durationMs` (or their client fallback); the step still streaming
/// contributes the time since its own request began. The numerator
/// ([`streamed_output_tokens`]) is turn-cumulative, so the clock must be too
/// — otherwise a step after a tool call would divide the whole run's tokens
/// by only its own elapsed time.
fn live_generation_clock(
    message: &ChatMessage,
    streaming_elapsed: Option<Duration>,
) -> Option<Duration> {
    let last = message.steps.len().checked_sub(1)?;
    let mut total = Duration::ZERO;
    let mut any = false;
    for (ix, step) in message.steps.iter().enumerate() {
        let clock = if ix == last {
            streaming_elapsed.or(step.generation_time)
        } else {
            step.duration.or(step.generation_time)
        };
        if let Some(clock) = clock {
            total += clock;
            any = true;
        }
    }
    any.then_some(total)
}

/// Tokens per second for a streaming row, or `None` before a rate is
/// meaningful (under a second of the turn elapsed, or no output yet). The
/// caller passes the turn's request clock (`turn_start`) — the same basis as
/// pi's settled `durationMs` — so the two figures agree.
fn streaming_tps(message: &ChatMessage, elapsed: Option<Duration>) -> Option<f64> {
    let elapsed = elapsed?;
    let secs = elapsed.as_secs_f64();
    if secs < 1.0 {
        return None;
    }
    let tokens = streamed_output_tokens(message);
    (tokens > 0).then(|| tokens as f64 / secs)
}

/// Settled output rate: output tokens over pi's measured call duration
/// (its `durationMs`, or the client-measured fallback) — prefill plus
/// decode. `None` when there is no measurable rate.
fn generation_tps(usage: &MessageUsage, generation: Option<Duration>) -> Option<f64> {
    let secs = generation?.as_secs_f64();
    if secs < 1.0 || usage.output == 0 {
        return None;
    }
    Some(usage.output as f64 / secs)
}

/// Compact rate label: whole numbers at 10+ (`42`), one decimal below
/// (`7.4`).
fn format_tps(tps: f64) -> String {
    if !tps.is_finite() || tps <= 0.0 {
        return "0".into();
    }
    if tps >= 10.0 {
        format!("{tps:.0}")
    } else {
        format!("{tps:.1}")
    }
}

/// The URL a fetch-style tool was invoked with. `None` for other tools.
fn tool_url(tool: &ToolCall) -> Option<String> {
    if !matches!(
        tool.name.as_str(),
        "web_fetch" | "webfetch" | "fetch" | "open_url" | "browse"
    ) {
        return None;
    }
    tool.args
        .as_ref()?
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The query a web-search tool was invoked with. `None` for other tools.
fn tool_search_query(tool: &ToolCall) -> Option<String> {
    if !matches!(
        tool.name.as_str(),
        "web_search" | "websearch" | "search_web" | "browse"
    ) {
        return None;
    }
    tool.args
        .as_ref()?
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A URL as it reads in a card header: scheme and a leading `www.` dropped.
fn display_url(url: &str) -> &str {
    let url = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    url.strip_prefix("www.").unwrap_or(url)
}

/// File-content tools whose header preview is one file path — the tools
/// that get the file's devicon.
fn file_preview_path(tool: &ToolCall) -> Option<&str> {
    match tool.name.as_str() {
        "read" | "view" | "edit" | "write" => tool.path.as_deref(),
        _ => None,
    }
}

fn activity_preview(tool: &ToolCall) -> String {
    if let Some(path) = &tool.path {
        return path.clone();
    }
    // A fetch tool's arguments JSON would show `{"url":"…"}` and a search
    // tool's `{"query":"…"}`; the URL/query itself is the fact to show.
    if let Some(url) = tool_url(tool) {
        return display_url(&url).to_string();
    }
    if let Some(query) = tool_search_query(tool) {
        return query;
    }
    // The codemode tool takes a JavaScript source string; show its first
    // non-empty line like a command preview instead of the raw JSON args.
    if tool.name == "codemode" {
        if let Some(code) = tool
            .args
            .as_ref()
            .and_then(|args| args.get("code"))
            .and_then(Value::as_str)
        {
            let first = code
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            return snippet(first.trim(), 72);
        }
    }
    // A command tool's pi summary is the raw JSON arguments; show the shell
    // command itself in the header instead of `{"command":"…"}`.
    let summary = tool_command(tool).unwrap_or_else(|| tool.summary.clone());
    // Headers are single-line. A literal newline in a heredoc would make
    // the fixed-height preview paint several clipped lines; full arguments
    // and copy still use the original command.
    snippet(&summary, 72)
}

/// The shell command a command-running tool was invoked with, when the tool
/// carries one in its arguments. `None` for edit/read/other tools.
fn tool_command(tool: &ToolCall) -> Option<String> {
    if !matches!(
        tool.name.as_str(),
        "bash" | "shell" | "terminal" | "exec" | "run"
    ) {
        return None;
    }
    tool.args
        .as_ref()?
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn render_line_delta(added: u64, removed: u64, theme: Theme, size: f32) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .text_size(theme.ui_px(size))
        .line_height(theme.ui_px(size + 3.))
        .child(
            div()
                .flex_none()
                .text_color(theme.success)
                .child(format!("+{added}")),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme.error)
                .child(format!("-{removed}")),
        )
}

// ── Markdown ────────────────────────────────────────────────────────────────
// Markdown styling: 14px/22px body, 0.9rem rhythm between blocks,
// h1/h2/h3 at 20/18/16px semibold, `ml-5` lists with hanging indents,
// `border-l-2` blockquotes, mono inline-code chips, rounded pre blocks,
// and clickable underlined links.

/// Flattened inline runs: `(text, runs, links, autolink marks)`. Each link
/// is a `(byte range, url)` pair into the body; each mark is the byte range
/// of an autolink's arrow glyph.
type InlineRuns = (
    SharedString,
    Vec<TextRun>,
    Vec<(Range<usize>, String)>,
    Vec<Range<usize>>,
);

/// One styled inline span produced by [`parse_inline`].
#[derive(Clone, Default)]
struct InlineSpan {
    text: String,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
    link: Option<String>,
    /// A bare URL the parser recognized, as opposed to a `[label](url)`
    /// link. Autolinks get an external-link mark in the rendered text.
    autolink: bool,
}

fn flush_span(spans: &mut Vec<InlineSpan>, current: &mut InlineSpan) {
    if !current.text.is_empty() {
        spans.push(std::mem::take(current));
    }
}

/// The end index (exclusive) of a bare `http://` or `https://` URL starting
/// at `start`, or `None` when the text there is not a URL. The scan stops at
/// whitespace, `<`, and `>`; trailing sentence punctuation, emphasis markers,
/// and closing brackets with no opener inside the URL are left for the prose.
fn autolink_end(chars: &[char], start: usize) -> Option<usize> {
    let tail = &chars[start..];
    if !tail.starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/'])
        && !tail.starts_with(&['h', 't', 't', 'p', ':', '/', '/'])
    {
        return None;
    }
    let mut end = start;
    while end < chars.len() && !chars[end].is_whitespace() && chars[end] != '<' && chars[end] != '>'
    {
        end += 1;
    }
    while end > start {
        let last = chars[end - 1];
        let trim = matches!(
            last,
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' | '_' | '~'
        ) || (matches!(last, ')' | ']' | '}') && {
            let opener = match last {
                ')' => '(',
                ']' => '[',
                _ => '{',
            };
            let opens = chars[start..end].iter().filter(|&&c| c == opener).count();
            let closes = chars[start..end].iter().filter(|&&c| c == last).count();
            closes > opens
        });
        if !trim {
            break;
        }
        end -= 1;
    }
    (end > start).then_some(end)
}

/// Parse GFM inline syntax: `` `code` ``, `**bold**`, `*italic*`, `_italic_`,
/// `~~strike~~`, `[links](url)`, and bare `http(s)://` URLs. `\*`-style
/// escapes render literally.
fn parse_inline(text: &str) -> Vec<InlineSpan> {
    let mut spans: Vec<InlineSpan> = Vec::new();
    let mut current = InlineSpan::default();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '\\' if i + 1 < chars.len() && "*_`~[]".contains(chars[i + 1]) => {
                current.text.push(chars[i + 1]);
                i += 2;
            }
            '`' => match (i + 1..chars.len()).find(|&j| chars[j] == '`') {
                Some(close) => {
                    flush_span(&mut spans, &mut current);
                    spans.push(InlineSpan {
                        text: chars[i + 1..close].iter().collect(),
                        code: true,
                        bold: current.bold,
                        ..InlineSpan::default()
                    });
                    i = close + 1;
                }
                None => {
                    current.text.push('`');
                    i += 1;
                }
            },
            '*' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                flush_span(&mut spans, &mut current);
                current.bold = !current.bold;
                i += 2;
            }
            '_' if i + 1 < chars.len()
                && chars[i + 1] == '_'
                && (i == 0 || !chars[i - 1].is_alphanumeric()) =>
            {
                flush_span(&mut spans, &mut current);
                current.bold = !current.bold;
                i += 2;
            }
            '~' if i + 1 < chars.len() && chars[i + 1] == '~' => {
                flush_span(&mut spans, &mut current);
                current.strikethrough = !current.strikethrough;
                i += 2;
            }
            '*' => {
                // Italic: an opening `*` needs a non-space next char, a
                // closing one a non-space previous char.
                let opens = i + 1 < chars.len() && !chars[i + 1].is_whitespace();
                let closes = i > 0 && !chars[i - 1].is_whitespace();
                if (current.italic && closes) || (!current.italic && opens) {
                    flush_span(&mut spans, &mut current);
                    current.italic = !current.italic;
                } else {
                    current.text.push('*');
                }
                i += 1;
            }
            '_' if i == 0 || !chars[i - 1].is_alphanumeric() => {
                let opens = i + 1 < chars.len() && !chars[i + 1].is_whitespace();
                let closes = i > 0 && !chars[i - 1].is_whitespace();
                if (current.italic && closes) || (!current.italic && opens) {
                    flush_span(&mut spans, &mut current);
                    current.italic = !current.italic;
                } else {
                    current.text.push('_');
                }
                i += 1;
            }
            '[' => {
                // Try `[label](url)` — fall back to a literal `[`.
                let mut label_end = None;
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '\n' {
                    if chars[j] == ']' && j + 1 < chars.len() && chars[j + 1] == '(' {
                        label_end = Some(j);
                        break;
                    }
                    j += 1;
                }
                let mut matched = false;
                if let Some(label_end) = label_end {
                    if let Some(close) = (label_end + 2..chars.len())
                        .take_while(|&k| chars[k] != '\n')
                        .find(|&k| chars[k] == ')')
                    {
                        flush_span(&mut spans, &mut current);
                        let mut span = current.clone();
                        span.text = chars[i + 1..label_end].iter().collect();
                        span.link = Some(chars[label_end + 2..close].iter().collect());
                        spans.push(span);
                        i = close + 1;
                        matched = true;
                    }
                }
                if !matched {
                    current.text.push('[');
                    i += 1;
                }
            }
            'h' => {
                // A bare URL is a link even without `[label](url)` syntax.
                if let Some(end) = autolink_end(&chars, i) {
                    // Capture the surrounding style before the flush resets
                    // it, so a URL inside `**bold**` stays bold.
                    let mut span = current.clone();
                    flush_span(&mut spans, &mut current);
                    span.text = chars[i..end].iter().collect();
                    span.link = Some(span.text.clone());
                    span.autolink = true;
                    spans.push(span);
                    i = end;
                } else {
                    current.text.push(ch);
                    i += 1;
                }
            }
            other => {
                current.text.push(other);
                i += 1;
            }
        }
    }
    flush_span(&mut spans, &mut current);
    spans
}

/// Build `(text, runs, links)` for a [`StyledText`] from inline spans. Each
/// link is a `(byte range, url)` pair into the flattened body.
fn inline_runs(
    spans: &[InlineSpan],
    base_weight: FontWeight,
    base_color: Hsla,
    theme: Theme,
) -> InlineRuns {
    let mut body = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    let mut marks: Vec<Range<usize>> = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let start = body.len();
        body.push_str(&span.text);
        // A bare URL wears an external-link mark so it reads as a link at a
        // glance: a gap, then the arrow the mark run carries. The whole mark
        // rides the link run, so clicking it opens the URL too. The arrow
        // gets its own run so the transcript can blank the font glyph and
        // paint the hugeicon arrow over it.
        if span.autolink {
            body.push(' ');
        }
        let len = body.len() - start;
        let mut font = ui_font();
        if span.code {
            font.family = theme::code_font_family();
            font.weight = FontWeight::NORMAL;
            font.style = FontStyle::Normal;
        } else {
            font.weight = if span.bold {
                FontWeight::BOLD
            } else {
                base_weight
            };
            font.style = if span.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            };
        }
        // Only links take the accent — they are the interactive token. Inline
        // code keeps the body ink and is marked by the mono face and its wash
        // instead; tinting it too made a code-heavy answer a field of accent
        // (violating DESIGN.md's One Accent Rule) and blurred it into links.
        let color = if span.link.is_some() {
            theme.accent_text
        } else {
            base_color
        };
        let run = TextRun {
            len,
            font,
            color,
            background_color: span.code.then_some(theme.inline_code_bg),
            underline: span.link.is_some().then(|| UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            }),
            strikethrough: span.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
        };
        if span.autolink {
            let mark_start = body.len();
            body.push_str(AUTOLINK_ARROW);
            marks.push(mark_start..body.len());
            runs.push(run.clone());
            runs.push(TextRun {
                len: body.len() - mark_start,
                ..run
            });
        } else {
            runs.push(run);
        }
        if let Some(url) = &span.link {
            links.push((start..body.len(), url.clone()));
        }
    }
    (body.into(), runs, links, marks)
}

/// Blank the font glyphs the autolink mark runs carry; [`paint_link_mark`]
/// draws the hugeicon arrow over that space instead. The run stays in the
/// text, so the click target and copied selection still cover the mark.
fn hide_autolink_marks(runs: Vec<TextRun>, marks: &[Range<usize>]) -> Vec<TextRun> {
    if marks.is_empty() {
        return runs;
    }
    let mut offset = 0usize;
    runs.into_iter()
        .map(|mut run| {
            let start = offset;
            offset += run.len;
            if marks
                .iter()
                .any(|mark| mark.start < offset && mark.end > start)
            {
                run.color = Hsla::transparent_black();
                run.underline = None;
            }
            run
        })
        .collect()
}

/// The north-east arrow a bare URL reserves after its gap. Kept as plain
/// Unicode in the text — the copied selection stays readable, unlike a
/// private-use glyph — while the transcript blanks the font's glyph and
/// paints the hugeicon arrow over it.
const AUTOLINK_ARROW: &str = "\u{2197}";

/// The window's default UI face (Zed's IBM Plex Sans), explicit for
/// [`TextRun`] construction.
fn ui_font() -> Font {
    Font {
        family: theme::ui_font_family(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// A styled, word-wrapping paragraph. In the transcript the text is
/// selectable and links resolve through the panel's mouse handlers; outside
/// it (the skills viewer) the plain clickable [`InteractiveText`] is kept.
///
/// Line height must clear inline-code backgrounds: a tight 14/22 measure let
/// the next wrapped line (and the next block) paint through the previous one.
fn paragraph_text(
    text: &str,
    size: f32,
    line_height: f32,
    weight: FontWeight,
    color: Hsla,
    key: ElementId,
    theme: Theme,
) -> impl IntoElement {
    let spans = parse_inline(text);
    let (body, runs, links, marks) = inline_runs(&spans, weight, color, theme);
    let wrapper = div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_size(theme.ui_px(size))
        .line_height(theme.ui_px(line_height))
        .text_color(color);
    if let Some(scope) = text_scope() {
        // The font's arrow would pick a fallback/emoji face; the hugeicon
        // paints crisper and on-theme (see `paint_link_mark`).
        let runs = hide_autolink_marks(runs, &marks);
        let range = scope.state.borrow().range_for(&key, body.as_ref());
        let runs = highlight_runs(runs, range, selection_color(theme));
        let plain = body.clone();
        wrapper
            .child(
                SelectableText::new(
                    key,
                    StyledText::new(body).with_runs(runs),
                    plain,
                    links,
                    Some(scope),
                )
                .with_marks(marks, theme.accent_text, IconSize::Small.px(&theme)),
            )
            .into_any_element()
    } else {
        let ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
        wrapper
            .child(
                InteractiveText::new(key, StyledText::new(body).with_runs(runs)).on_click(
                    ranges,
                    move |range_ix: usize, _, cx| {
                        if let Some((_, url)) = links.get(range_ix) {
                            cx.open_url(url);
                        }
                    },
                ),
            )
            .into_any_element()
    }
}

fn md_id(ix: usize, salt: u64, block_ix: usize, sub: usize) -> ElementId {
    ElementId::NamedInteger(
        "md".into(),
        ((ix as u64) << 40) | ((salt & 0xffff) << 24) | ((block_ix as u64) << 8) | sub as u64,
    )
}

// ── block model ─────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Block {
    Paragraph(Vec<String>),
    Heading(u8, String),
    Code(Option<String>, Vec<String>),
    List(Vec<ListItem>),
    Quote(Vec<String>),
    /// GitHub-style alert (`> [!NOTE]` …): a labeled, tinted callout.
    Alert(AlertKind, Vec<String>),
    Rule,
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        aligns: Vec<TableAlign>,
    },
    /// A standalone image: a Markdown `![alt](url)` line or an HTML
    /// `<img src="…">` tag. GitHub issue screenshots usually land on their
    /// own line, so the block model is enough to render them inline.
    Image {
        alt: String,
        url: String,
    },
}

/// GFM column alignment parsed from a table's delimiter row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TableAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl TableAlign {
    fn text_align(self) -> TextAlign {
        match self {
            Self::Left => TextAlign::Left,
            Self::Center => TextAlign::Center,
            Self::Right => TextAlign::Right,
        }
    }
}

/// Kinds of GitHub alert blockquote (`> [!WARNING]` …). Each maps to one
/// semantic token color and one icon, so the family stays consistent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AlertKind {
    fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Note | Self::Important | Self::Warning => "icons/info.svg",
            Self::Tip => "icons/spark.svg",
            Self::Caution => "icons/stop.svg",
        }
    }

    fn color(self, theme: Theme) -> Hsla {
        match self {
            Self::Note | Self::Important => theme.accent_text,
            Self::Tip => theme.success,
            Self::Warning => theme.warning,
            Self::Caution => theme.error,
        }
    }
}

#[derive(Clone)]
struct ListItem {
    depth: usize,
    ordered: bool,
    number: u64,
    /// GFM task-list state: `Some(true)` checked, `Some(false)` open, `None`
    /// a plain item. Task items drop the bullet/number for a checkbox.
    checked: Option<bool>,
    text: String,
}

fn flush_paragraph(paragraph: &mut Vec<String>, blocks: &mut Vec<Block>) {
    if !paragraph.is_empty() {
        blocks.push(Block::Paragraph(std::mem::take(paragraph)));
    }
}

fn heading_line(trimmed: &str) -> Option<(u8, String)> {
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&level) {
        if let Some(body) = trimmed[level..].strip_prefix(' ') {
            let body = body.trim();
            if !body.is_empty() {
                // Keep the real level; the renderer gives h4+ a smaller step
                // rather than flattening everything past h3 to one size.
                return Some((level as u8, body.to_string()));
            }
        }
    }
    None
}

fn is_rule(trimmed: &str) -> bool {
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    match compact.chars().next() {
        Some(first @ ('-' | '*' | '_')) => {
            compact.len() >= 3 && compact.chars().all(|c| c == first)
        }
        _ => false,
    }
}

/// Parse a line that is solely an image into `(alt, url)`.
///
/// Handles Markdown `![alt](url "title")` (with optional `<…>` wrapping) and
/// the HTML `<img src="…">` form GitHub sometimes stores. Returns `None` for
/// a line that merely contains an image among other text, so prose is never
/// split mid-sentence.
fn image_line(trimmed: &str) -> Option<(String, String)> {
    let t = trimmed.trim();
    if let Some(rest) = t.strip_prefix("![") {
        let close = rest.find("](")?;
        let alt = rest[..close].to_string();
        let after = &rest[close + 2..];
        let end = after.find(')')?;
        // A title follows the URL after whitespace; the URL itself may be
        // wrapped in angle brackets (Markdown's escape for spaces).
        let raw = after[..end].trim();
        let url = raw
            .strip_prefix('<')
            .and_then(|rest| rest.split('>').next())
            .unwrap_or_else(|| raw.split_whitespace().next().unwrap_or(""));
        if !url.is_empty() {
            return Some((alt, normalize_image_url(url)));
        }
        return None;
    }
    if t.starts_with("<img") {
        let src = html_attr(t, "src")?;
        if !src.is_empty() {
            return Some((
                html_attr(t, "alt").unwrap_or_default(),
                normalize_image_url(&src),
            ));
        }
    }
    None
}

/// Rewrite GitHub's signed `private-user-images` CDN URL to the durable
/// `user-attachments` URL for the same asset.
///
/// A screenshot pasted into a comment is stored as
/// `private-user-images.githubusercontent.com/<user>/<asset>-<uuid>.<ext>?jwt=…`.
/// That JWT expires within minutes, and GitHub re-signs the URL only when it
/// renders the page itself — so the stored URL 404s for Orbit and the reply
/// shows the alt-text fallback. The durable URL is the same asset and never
/// expires: it is the form the issue/PR body itself uses.
fn normalize_image_url(url: &str) -> String {
    const SIGNED_HOST: &str = "https://private-user-images.githubusercontent.com/";
    let Some(rest) = url.strip_prefix(SIGNED_HOST) else {
        return url.to_string();
    };
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    let file = path.rsplit('/').next().unwrap_or(path);
    // `<asset id>-<uuid>.<ext>`: the UUID half names the durable asset.
    let Some((_, stem)) = file.split_once('-') else {
        return url.to_string();
    };
    let uuid = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
    if !is_uuid(uuid) {
        return url.to_string();
    }
    format!("https://github.com/user-attachments/assets/{uuid}")
}

/// `8-4-4-4-12` hex, the shape of GitHub's asset IDs.
fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(ix, byte)| match ix {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// Read `name="value"` (single or double quoted) from a tag's source text.
fn html_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key = format!("{name}=");
    let start = lower.find(&key)? + key.len();
    let rest = &tag[start..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn list_item_line(line: &str) -> Option<(usize, bool, u64, Option<bool>, String)> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    let (ordered, number, body) = if let Some(rest) = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
    {
        (false, 0u64, rest)
    } else {
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let after = t[digits..]
            .strip_prefix(". ")
            .or_else(|| t[digits..].strip_prefix(") "))?;
        (true, t[..digits].parse().ok()?, after)
    };
    let (checked, body) = match task_status(body) {
        Some((checked, rest)) => (Some(checked), rest),
        None => (None, body),
    };
    Some((
        indent.div_ceil(2).min(2),
        ordered,
        number,
        checked,
        body.to_string(),
    ))
}

/// Strip a leading GFM task marker (`[ ] `, `[x] `, `[X] `) from an item's
/// body, returning its state and the remaining text.
fn task_status(body: &str) -> Option<(bool, &str)> {
    let rest = body.strip_prefix('[')?;
    let mut chars = rest.chars();
    let checked = match chars.next()? {
        ' ' => false,
        'x' | 'X' => true,
        _ => return None,
    };
    let after = &rest[1..];
    let after = after.strip_prefix(']')?;
    Some((checked, after.strip_prefix(' ').unwrap_or(after)))
}

fn split_table_row(line: &str) -> Vec<String> {
    let s = line.trim();
    let s = s.strip_prefix('|').unwrap_or(s);
    let s = s.strip_suffix('|').unwrap_or(s);
    s.split('|').map(|cell| cell.trim().to_string()).collect()
}

/// Parse a GFM delimiter row (`| --- | :--: | ---: |`) into per-column
/// alignment; `None` when the line is not a delimiter row.
fn table_aligns(trimmed: &str) -> Option<Vec<TableAlign>> {
    if !trimmed.contains('-') || !trimmed.contains('|') {
        return None;
    }
    if !trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
        return None;
    }
    let cells = split_table_row(trimmed);
    if cells.is_empty() {
        return None;
    }
    Some(
        cells
            .iter()
            .map(|cell| {
                let left = cell.starts_with(':');
                let right = cell.ends_with(':');
                match (left, right) {
                    (true, true) => TableAlign::Center,
                    (false, true) => TableAlign::Right,
                    _ => TableAlign::Left,
                }
            })
            .collect(),
    )
}

/// GitHub alert marker on the first quoted line, e.g. `[!WARNING]`.
fn alert_marker(body: &[String]) -> Option<AlertKind> {
    let first = body.first()?.trim();
    let inner = first.strip_prefix("[!")?.strip_suffix(']')?;
    Some(match inner.trim().to_ascii_uppercase().as_str() {
        "NOTE" => AlertKind::Note,
        "TIP" => AlertKind::Tip,
        "IMPORTANT" => AlertKind::Important,
        "WARNING" => AlertKind::Warning,
        "CAUTION" => AlertKind::Caution,
        _ => return None,
    })
}

fn parse_blocks(text: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush_paragraph(&mut paragraph, &mut blocks);
            let marker = &trimmed[..trimmed.len().min(3)];
            // The fence info string's first token is the language hint
            // (```rust, ```text, …) — it becomes the code block's chip.
            let language = trimmed[3..]
                .split_whitespace()
                .next()
                .filter(|lang| !lang.is_empty())
                .map(str::to_string);
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(marker) {
                body.push(lines[i].to_string());
                i += 1;
            }
            i += 1; // closing fence (or EOF)
            blocks.push(Block::Code(language, body));
            continue;
        }

        if trimmed.is_empty() {
            flush_paragraph(&mut paragraph, &mut blocks);
            i += 1;
            continue;
        }

        if let Some((level, body)) = heading_line(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Heading(level, body));
            i += 1;
            continue;
        }

        if is_rule(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }

        // A line that is only an image becomes its own block, so a screenshot
        // pasted under a heading actually paints instead of rendering as a
        // stray `!` and a link.
        if let Some((alt, url)) = image_line(trimmed) {
            flush_paragraph(&mut paragraph, &mut blocks);
            blocks.push(Block::Image { alt, url });
            i += 1;
            continue;
        }

        if trimmed.contains('|') {
            if let Some(aligns) = lines
                .get(i + 1)
                .and_then(|separator| table_aligns(separator.trim()))
            {
                flush_paragraph(&mut paragraph, &mut blocks);
                let header = split_table_row(trimmed);
                i += 2;
                let mut rows = Vec::new();
                while i < lines.len() {
                    let row_line = lines[i].trim();
                    if row_line.is_empty() || !row_line.contains('|') {
                        break;
                    }
                    rows.push(split_table_row(row_line));
                    i += 1;
                }
                blocks.push(Block::Table {
                    header,
                    rows,
                    aligns,
                });
                continue;
            }
        }

        if trimmed.starts_with('>') {
            flush_paragraph(&mut paragraph, &mut blocks);
            let mut body = Vec::new();
            while i < lines.len() {
                match lines[i].trim_start().strip_prefix('>') {
                    Some(rest) => {
                        body.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                        i += 1;
                    }
                    None => break,
                }
            }
            match alert_marker(&body) {
                Some(kind) => {
                    body.remove(0);
                    // Drop the blank separator line GitHub allows after the
                    // marker so the callout body starts on real content.
                    while body.first().is_some_and(|line| line.trim().is_empty()) {
                        body.remove(0);
                    }
                    blocks.push(Block::Alert(kind, body));
                }
                None => blocks.push(Block::Quote(body)),
            }
            continue;
        }

        if let Some((depth, ordered, number, checked, body)) = list_item_line(line) {
            flush_paragraph(&mut paragraph, &mut blocks);
            let mut items = Vec::new();
            let mut pending = ListItem {
                depth,
                ordered,
                number,
                checked,
                text: body,
            };
            i += 1;
            while i < lines.len() {
                let l = lines[i];
                if l.trim().is_empty() {
                    // A blank line keeps the list open only when another
                    // item follows (loose list).
                    let mut look = i + 1;
                    while look < lines.len() && lines[look].trim().is_empty() {
                        look += 1;
                    }
                    if look < lines.len() && list_item_line(lines[look]).is_some() {
                        i = look;
                        continue;
                    }
                    break;
                }
                if let Some((depth, ordered, number, checked, body)) = list_item_line(l) {
                    // A marker-type change starts a new list.
                    if ordered != pending.ordered {
                        break;
                    }
                    items.push(std::mem::replace(
                        &mut pending,
                        ListItem {
                            depth,
                            ordered,
                            number,
                            checked,
                            text: body,
                        },
                    ));
                    i += 1;
                } else if l.starts_with("  ") || l.starts_with('\t') {
                    // Indented continuation of the pending item.
                    pending.text.push(' ');
                    pending.text.push_str(l.trim());
                    i += 1;
                } else {
                    break;
                }
            }
            items.push(pending);
            blocks.push(Block::List(items));
            continue;
        }

        paragraph.push(line.trim().to_string());
        i += 1;
    }
    flush_paragraph(&mut paragraph, &mut blocks);
    blocks
}

thread_local! {
    /// Memoized markdown block parses, keyed by content hash. Every visible
    /// row re-renders on each heartbeat while a run streams (the live clock
    /// alone repaints at ~11 Hz); parsing is pure over the source text, so
    /// each distinct text parses once and rows share the blocks.
    static BLOCK_PARSE_CACHE: RefCell<HashMap<u64, Rc<Vec<Block>>>> =
        RefCell::new(HashMap::new());
}

/// Parse markdown into blocks, memoized by content. The cache is small and
/// cleared when full — a stale-entry miss costs one reparse, never a wrong
/// render (keyed by the text's full 64-bit hash).
fn parse_blocks_cached(text: &str) -> Rc<Vec<Block>> {
    let key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(text, &mut hasher);
        std::hash::Hasher::finish(&hasher)
    };
    BLOCK_PARSE_CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(&key) {
            return hit.clone();
        }
        let blocks = Rc::new(parse_blocks(text));
        let mut cache = cache.borrow_mut();
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(key, blocks.clone());
        blocks
    })
}

/// A table-only breakout from the unchanged, centered message column.
/// Definite widths avoid remeasuring normal prose and activity cards against
/// a wider ancestor. Relative positioning centers the wider table without
/// negative layout margins affecting its siblings' intrinsic measurements.
#[derive(Clone, Copy)]
struct TableBreakout {
    max_width: Pixels,
    column_width: Pixels,
}

/// Markdown: 14px/22px body. Spacing is graded by block pair rather
/// than one uniform gap, so a heading reads as a section start, a list hugs
/// the paragraph that introduces it, and consecutive paragraphs breathe.
///
/// In the transcript, only tables escape the normal content column; the
/// other blocks keep their existing measure. `collapsible` is false for live
/// rows — streaming code blocks
/// never collapse out from under their own growing edge.
#[allow(clippy::too_many_arguments)]
fn render_prose(
    text: &str,
    ix: usize,
    salt: u64,
    theme: Theme,
    copied_sections: CopiedSections,
    expanded_blocks: ExpandedBlocks,
    collapsible: bool,
    scroller: MessageScrollerState,
    table_breakout: Option<TableBreakout>,
) -> impl IntoElement + use<> {
    let blocks = parse_blocks_cached(text);
    let gaps: Vec<f32> = blocks
        .iter()
        .enumerate()
        .map(|(i, block)| {
            if i == 0 {
                0.0
            } else {
                block_gap(&blocks[i - 1], block)
            }
        })
        .collect();
    div().w_full().min_w_0().flex().flex_col().children(
        blocks
            .iter()
            .enumerate()
            .map(move |(block_ix, block)| {
                let gap = gaps[block_ix];
                div()
                    .w_full()
                    .min_w_0()
                    .when_some(table_breakout, |node, breakout| {
                        if let Block::Table { header, rows, .. } = block {
                            // Fit the capped column widths, not the strip or
                            // pane. Small tables align with prose; larger ones
                            // extend equally into its margins, up to the pane.
                            let natural_width =
                                px(table_column_widths(header, rows).iter().sum::<f32>() + 2.);
                            let width = natural_width.min(breakout.max_width);
                            let left = ((breakout.column_width - width) / 2.).min(px(0.));
                            node.flex_none().w(width).relative().left(left)
                        } else {
                            // Only tables override their width; ordinary prose
                            // inherits the assistant's reading column.
                            node
                        }
                    })
                    .when(gap > 0.0, |node| node.mt(px(gap)))
                    .child(render_block(
                        block,
                        ix,
                        salt,
                        block_ix,
                        theme,
                        copied_sections.clone(),
                        expanded_blocks.clone(),
                        collapsible,
                        scroller.clone(),
                    ))
            })
            .collect::<Vec<_>>(),
    )
}

/// Render a standalone markdown document — a skill's `SKILL.md` body — with
/// the transcript's block renderer and no transcript-specific state, so the
/// two surfaces can never drift.
pub(crate) fn render_markdown_document(text: &str, theme: Theme) -> impl IntoElement + use<> {
    let copied: CopiedSections = Rc::new(RefCell::new(HashMap::new()));
    let expanded: ExpandedBlocks = Rc::new(RefCell::new(HashSet::new()));
    // Not collapsible: the page has no persistent fold state for a document
    // preview, and a fold control that could not remember its state would be
    // a broken affordance.
    render_prose(
        text,
        0,
        0,
        theme,
        copied,
        expanded,
        false,
        MessageScrollerState::new(1),
        None,
    )
}

/// Vertical space to place *above* `next`, given `prev`. More room above a
/// heading than below it, tight joins for lists and their introducer, and a
/// clear break around code and tables (the brief's rhythm table).
fn block_gap(prev: &Block, next: &Block) -> f32 {
    use Block::{Alert, Code, Heading, Image, List, Paragraph, Quote, Rule, Table};
    match (prev, next) {
        (_, Heading(..)) => 20.0,
        (Heading(..), _) => 8.0,
        (Paragraph(..), Paragraph(..)) => 14.0,
        (_, Code(..)) => 12.0,
        (Code(..), _) => 14.0,
        (_, List(..)) => 10.0,
        (List(..), _) => 14.0,
        (_, Quote(..)) | (Quote(..), _) => 12.0,
        (_, Alert(..)) | (Alert(..), _) => 12.0,
        (_, Table { .. }) | (Table { .. }, _) => 14.0,
        (Rule, _) => 14.0,
        (_, Image { .. }) | (Image { .. }, _) => 14.0,
        _ => 10.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_block(
    block: &Block,
    ix: usize,
    salt: u64,
    block_ix: usize,
    theme: Theme,
    copied_sections: CopiedSections,
    expanded_blocks: ExpandedBlocks,
    collapsible: bool,
    scroller: MessageScrollerState,
) -> AnyElement {
    match block {
        Block::Paragraph(lines) => paragraph_text(
            &lines.join(" "),
            14.,
            26.,
            FontWeight::NORMAL,
            theme.assistant_text,
            md_id(ix, salt, block_ix, 0),
            theme,
        )
        .into_any_element(),
        Block::Heading(level, text) => {
            let (size, line_height, color) = match level {
                1 => (20., 30., theme.text_primary),
                2 => (18., 28., theme.text_primary),
                3 => (16., 26., theme.text_primary),
                4 => (15., 24., theme.text_primary),
                _ => (14., 24., theme.text_secondary),
            };
            paragraph_text(
                text,
                size,
                line_height,
                FontWeight::SEMIBOLD,
                color,
                md_id(ix, salt, block_ix, 0),
                theme,
            )
            .into_any_element()
        }
        Block::Code(language, lines) => render_code_block(
            language.as_deref(),
            lines,
            ix,
            salt,
            block_ix,
            theme,
            copied_sections,
            expanded_blocks,
            collapsible,
            scroller,
        )
        .into_any_element(),
        Block::Rule => div().w_full().h(px(1.)).bg(theme.border).into_any_element(),
        Block::Alert(kind, lines) => {
            render_alert(*kind, lines, ix, salt, block_ix, theme).into_any_element()
        }
        Block::Quote(lines) => div()
            .w_full()
            .min_w_0()
            .border_l_2()
            .border_color(theme.text_muted)
            .bg(theme.overlay)
            .rounded_r(Radius::Large.px(&theme))
            .pl(px(14.))
            .pr(DynamicSpacing::Base12.px(&theme))
            .py(DynamicSpacing::Base08.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base04.px(&theme))
            .children(lines.iter().enumerate().map(move |(sub, line)| {
                render_container_line(
                    line,
                    ix,
                    salt,
                    block_ix,
                    sub,
                    14.,
                    26.,
                    FontWeight::NORMAL,
                    theme.text_secondary,
                    theme,
                )
            }))
            .into_any_element(),
        Block::List(items) => div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .children(
                items
                    .iter()
                    .enumerate()
                    .map(move |(sub, item)| render_list_item(item, ix, salt, block_ix, sub, theme)),
            )
            .into_any_element(),
        Block::Table {
            header,
            rows,
            aligns,
        } => render_table(header, rows, aligns, ix, salt, block_ix, theme).into_any_element(),
        Block::Image { alt, url } => {
            render_markdown_image(alt, url, ix, salt, block_ix, 0, theme).into_any_element()
        }
    }
}

/// A block-level image. Remote sources (the GitHub issue screenshots) load
/// through gpui's image cache; a load failure falls back to the alt text or a
/// quiet "image unavailable" note instead of leaving a hole. Clicking opens
/// the source in the browser so a private/expired URL is still reachable.
fn render_markdown_image(
    alt: &str,
    url: &str,
    ix: usize,
    salt: u64,
    block_ix: usize,
    sub: usize,
    theme: Theme,
) -> impl IntoElement {
    let url = url.to_string();
    let open = url.clone();
    let fallback_alt = alt.to_string();
    div().w_full().min_w_0().flex().child(
        img(url)
            .id(md_id(ix, salt, block_ix, sub))
            // `min_w_0` is load-bearing: a replaced element's automatic minimum
            // width is its intrinsic width, so without it `max_w_full` loses
            // and a large screenshot overflows the column and the rail.
            .min_w_0()
            .max_w_full()
            // Leave the height auto so gpui derives it from the decoded image
            // and taffy re-derives it from the aspect ratio once
            // `max_w_full` clamps the width. A definite height (including 0)
            // makes gpui compute the width *from* it instead — 0 × ratio —
            // and the loaded image collapses to nothing.
            .rounded(Radius::XLarge.px(&theme))
            .border_1()
            .border_color(theme.border)
            .object_fit(ObjectFit::Contain)
            .cursor_pointer()
            .with_fallback(move || markdown_image_fallback(&fallback_alt, theme))
            .on_click(move |_, _, cx| cx.open_url(&open)),
    )
}

/// The fallback shown when a markdown image cannot load: the alt text when it
/// says something, otherwise a muted "image unavailable" note.
fn markdown_image_fallback(alt: &str, theme: Theme) -> AnyElement {
    let label = if alt.trim().is_empty() {
        tr!("markdown.image_unavailable")
    } else {
        alt.to_string()
    };
    div()
        .px(DynamicSpacing::Base08.px(&theme))
        .py(DynamicSpacing::Base08.px(&theme))
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.overlay)
        .text_size(TextSize::Small.px(&theme))
        .text_color(theme.text_muted)
        .child(label)
        .into_any_element()
}

/// A GitHub-style callout: a tinted, rounded container with a semantic
/// icon + label and readable body copy. Deliberately no accent bar — the
/// tint, icon, and label carry the meaning without a heavy left edge.
fn render_alert(
    kind: AlertKind,
    lines: &[String],
    ix: usize,
    salt: u64,
    block_ix: usize,
    theme: Theme,
) -> impl IntoElement {
    let color = kind.color(theme);
    div()
        .w_full()
        .min_w_0()
        .rounded(Radius::XLarge.px(&theme))
        .bg(color.opacity(0.08))
        .px(DynamicSpacing::Base12.px(&theme))
        .py(px(10.))
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base06.px(&theme))
        .child(
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(glyph(kind.icon(), IconSize::Small.px(&theme), color))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .line_height(theme.ui_px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color)
                        .child(kind.label()),
                ),
        )
        .children(lines.iter().enumerate().map(move |(sub, line)| {
            render_container_line(
                line,
                ix,
                salt,
                block_ix,
                sub,
                14.,
                26.,
                FontWeight::NORMAL,
                theme.assistant_text,
                theme,
            )
        }))
}

/// Render one line inside a quote or alert. A standalone image line (GitHub
/// quotes a bug report whose screenshot was pasted as a raw `<img>` tag) paints
/// as an image instead of showing the tag as text.
#[allow(clippy::too_many_arguments)]
fn render_container_line(
    line: &str,
    ix: usize,
    salt: u64,
    block_ix: usize,
    sub: usize,
    size: f32,
    line_height: f32,
    weight: FontWeight,
    color: Hsla,
    theme: Theme,
) -> AnyElement {
    if let Some((alt, url)) = image_line(line) {
        return render_markdown_image(&alt, &url, ix, salt, block_ix, sub, theme)
            .into_any_element();
    }
    paragraph_text(
        line,
        size,
        line_height,
        weight,
        color,
        md_id(ix, salt, block_ix, sub),
        theme,
    )
    .into_any_element()
}

/// `ml-5` bullet / numbered item with a hanging indent on wrap. GFM task
/// items swap the marker for a checkbox and mute checked text.
fn render_list_item(
    item: &ListItem,
    ix: usize,
    salt: u64,
    block_ix: usize,
    sub: usize,
    theme: Theme,
) -> AnyElement {
    let marker: AnyElement = match item.checked {
        Some(checked) => task_checkbox(checked, theme),
        None => {
            let text = if item.ordered {
                format!("{}.", item.number)
            } else {
                "•".to_string()
            };
            div()
                .text_size(TextSize::Default.px(&theme))
                .line_height(theme.ui_px(26.))
                .text_color(if item.ordered {
                    theme.text_primary
                } else {
                    theme.text_muted
                })
                .child(text)
                .into_any_element()
        }
    };
    // Completed tasks recede but stay above the 4.5:1 body-contrast floor.
    let text_color = if item.checked == Some(true) {
        theme.text_secondary
    } else {
        theme.assistant_text
    };
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .pl(px(20. + 20. * item.depth as f32))
        .child(
            div()
                .flex_none()
                .w(px(20.))
                .flex()
                .items_center()
                .justify_start()
                .child(marker),
        )
        .child(div().min_w_0().flex_1().child(render_container_line(
            &item.text,
            ix,
            salt,
            block_ix,
            sub,
            14.,
            26.,
            FontWeight::NORMAL,
            text_color,
            theme,
        )))
        .into_any_element()
}

/// GFM task-list checkbox: accent-filled check when done, hairline empty box
/// when open. The `action_fg` check color stays legible on the fill in both
/// appearances.
fn task_checkbox(checked: bool, theme: Theme) -> AnyElement {
    let size = 14.0;
    let mut box_ = div()
        .flex_none()
        .size(px(size))
        .rounded(Radius::Small.px(&theme))
        .flex()
        .items_center()
        .justify_center();
    if checked {
        box_ = box_.bg(theme.accent).child(glyph(
            "icons/check.svg",
            IconSize::Indicator.px(&theme),
            theme.accent_fg,
        ));
    } else {
        box_ = box_.border_1().border_color(theme.border_strong);
    }
    div()
        .h(px(26.))
        .flex()
        .items_center()
        .child(box_)
        .into_any_element()
}

/// A first-class code block: a title strip naming the language with a copy
/// affordance, then syntax-highlighted monospace lines that scroll
/// horizontally rather than wrapping or clipping. Highlight colors come from
/// [`Theme::token_color`], the same map the review diff uses, so a snippet
/// reads identically in the transcript and the side pane.
///
/// Settled blocks taller than [`CODE_COLLAPSE_LINES`] show a
/// [`CODE_PREVIEW_LINES`]-line preview with a "Show remaining N lines" bar;
/// copy always carries the complete code regardless of the fold.
#[allow(clippy::too_many_arguments)]
fn render_code_block(
    language: Option<&str>,
    lines: &[String],
    ix: usize,
    salt: u64,
    block_ix: usize,
    theme: Theme,
    copied_sections: CopiedSections,
    expanded_blocks: ExpandedBlocks,
    collapsible: bool,
    scroller: MessageScrollerState,
) -> impl IntoElement {
    let code = lines.join("\n");
    let tokens = language
        .and_then(highlight::lang_for_tag)
        .map(|lang| highlight::tokenize_cached(lang, &code));
    let label = code_block_label(language);
    // Block identity within the message: the prose salt scopes the index to
    // the step/user run it was parsed from, so two steps' third blocks
    // never share copy feedback, element state, or fold state.
    let block_key = salt as usize + block_ix;
    let copied = copied_sections
        .borrow()
        .get(&(ix, block_key, CODE_COPY_SECTION))
        .is_some_and(|at| at.elapsed() < COPY_FEEDBACK);
    let copy_button = icon_button_frame(
        div().id(ElementId::NamedInteger(
            "copy-code".into(),
            (ix as u64) << 32 | block_key as u64,
        )),
        &theme,
        CODE_COPY_BUTTON,
    )
    .tip(tr!("transcript.copy_code"))
    .cursor_pointer()
    .hover(|style| style.bg(theme.overlay_strong))
    .child(glyph(
        if copied {
            "icons/check.svg"
        } else {
            "icons/copy.svg"
        },
        CODE_COPY_BUTTON.icon_size().px(&theme),
        if copied {
            theme.success
        } else {
            theme.text_muted
        },
    ))
    .on_click(move |_, _, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
        copied_sections
            .borrow_mut()
            .insert((ix, block_key, CODE_COPY_SECTION), Instant::now());
        cx.refresh_windows();
    });
    // Title strip: the language name and the copy affordance in one row, so
    // context and action sit in the same place on every code block.
    let header = div()
        .w_full()
        .min_w_0()
        .h(px(34.))
        .px(DynamicSpacing::Base12.px(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base08.px(&theme))
        .bg(theme.overlay)
        // The title strip is the card's first child; gpui clips children to a
        // rectangle, so the strip carries the card's top radius itself.
        .rounded_t(Radius::XLarge.px(&theme))
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(TextSize::Small.px(&theme))
                .line_height(theme.ui_px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(label),
        )
        .child(copy_button);

    // Long settled blocks fold to a prefix preview; the fold state is keyed
    // by (message, prose run, block) so steps never collide.
    let folded = collapsible && lines.len() > CODE_COLLAPSE_LINES;
    let expanded = folded && expanded_blocks.borrow().contains(&(ix, salt, block_ix));
    let visible = if folded && !expanded {
        &lines[..CODE_PREVIEW_LINES]
    } else {
        lines
    };

    let mut card = div()
        .w_full()
        .min_w_0()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.code_bg)
        .overflow_hidden()
        .child(header)
        .child(
            div()
                .id(ElementId::NamedInteger(
                    "code-scroll".into(),
                    (ix as u64) << 32 | block_key as u64,
                ))
                .w_full()
                .min_w_0()
                .flex()
                .items_start()
                .overflow_x_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .px(DynamicSpacing::Base16.px(&theme))
                        .pt(DynamicSpacing::Base08.px(&theme))
                        .pb(DynamicSpacing::Base12.px(&theme))
                        .font_family(theme::code_font_family())
                        .text_size(theme.code_px(13.))
                        .line_height(theme.code_px(21.))
                        .text_color(theme.code_text)
                        .children(visible.iter().enumerate().map(|(line_ix, line)| {
                            let line_id = ElementId::NamedInteger(
                                "code-line".into(),
                                code_line_id(ix, salt, block_ix, line_ix),
                            );
                            code_line(
                                line,
                                tokens
                                    .as_deref()
                                    .and_then(|lines| lines.get(line_ix))
                                    .map(Vec::as_slice),
                                line_id,
                                theme,
                            )
                        })),
                ),
        );

    if folded {
        let hidden = lines.len() - CODE_PREVIEW_LINES;
        let label = if expanded {
            tr!("transcript.show_less")
        } else {
            tr!("transcript_view.show_remaining_lines", count = hidden)
        };
        card = card.child(
            div()
                .id(ElementId::NamedInteger(
                    "code-fold".into(),
                    (ix as u64) << 32 | block_key as u64,
                ))
                .w_full()
                .h(px(30.))
                .px(DynamicSpacing::Base12.px(&theme))
                .border_t_1()
                .border_color(theme.border)
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .cursor_pointer()
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_secondary)
                .hover(|style| style.bg(theme.surface_hover).text_color(theme.text_primary))
                .child(label)
                .child(div().flex_1())
                .child(glyph(
                    if expanded {
                        "icons/chevron-down.svg"
                    } else {
                        "icons/chevron-right.svg"
                    },
                    IconSize::XSmall.px(&theme),
                    theme.text_muted,
                ))
                .on_click(move |_, _, cx| {
                    let key = (ix, salt, block_ix);
                    let mut open = expanded_blocks.borrow_mut();
                    if !open.remove(&key) {
                        open.insert(key);
                    }
                    drop(open);
                    scroller.remeasure_toggle(ix);
                    cx.refresh_windows();
                }),
        );
    }

    card
}

/// Display label for a code block's title strip: a friendly language name
/// for known fences, the raw tag when the language is unrecognized, and
/// `Plain text` for a bare fence.
fn code_block_label(language: Option<&str>) -> String {
    match language {
        // D2: no native diagram renderer. A mermaid fence stays a copyable
        // code block — the label says so instead of pretending to preview.
        Some(tag) if tag.eq_ignore_ascii_case("mermaid") => {
            tr!("transcript_view.mermaid_source")
        }
        Some(tag) => highlight::language_label(tag)
            .map(str::to_string)
            .unwrap_or_else(|| tag.to_string()),
        None => tr!("transcript_view.plain_text"),
    }
}

/// One code line as syntax-colored `TextRun`s over the monospace face.
/// `whitespace_nowrap` keeps indentation exact and lets the block scroll
/// instead of wrap; an empty line carries a space so its row keeps height.
fn code_line(text: &str, tokens: Option<&[Token]>, key: ElementId, theme: Theme) -> AnyElement {
    div()
        .flex_none()
        .whitespace_nowrap()
        .child(syntax_line(text, tokens, key, theme.code_text, theme))
        .into_any_element()
}

/// A selectable syntax-highlighted line; the selection wash rides the line's
/// runs, so wrapped/folded lines highlight exactly with the text.
fn syntax_line(
    text: &str,
    tokens: Option<&[Token]>,
    key: ElementId,
    base: Hsla,
    theme: Theme,
) -> SelectableText {
    let (display, runs) = syntax_runs(text, tokens, base, theme);
    let plain: SharedString = text.to_string().into();
    let scope = text_scope();
    let range = scope
        .as_ref()
        .and_then(|scope| scope.state.borrow().range_for(&key, &display));
    let runs = highlight_runs(runs, range, selection_color(theme));
    SelectableText::new(
        key,
        StyledText::new(display).with_runs(runs),
        plain,
        Vec::new(),
        scope,
    )
}

/// Build a line of syntax-colored text: plain gaps and token spans over the
/// monospace face, with an empty line carrying a space so its row keeps
/// height. Shared by code blocks (no-wrap) and tool detail (wrapping).
fn syntax_styled(text: &str, tokens: Option<&[Token]>, base: Hsla, theme: Theme) -> StyledText {
    let (display, runs) = syntax_runs(text, tokens, base, theme);
    StyledText::new(display).with_runs(runs)
}

fn syntax_runs(
    text: &str,
    tokens: Option<&[Token]>,
    base: Hsla,
    theme: Theme,
) -> (SharedString, Vec<TextRun>) {
    let font = mono_font();
    let display: SharedString = if text.is_empty() {
        " ".into()
    } else {
        text.to_string().into()
    };
    let mut runs: Vec<TextRun> = Vec::new();
    if let Some(spans) = tokens {
        let mut offset = 0usize;
        for token in spans {
            let start = token.range.start.min(text.len());
            let end = token.range.end.min(text.len());
            if start > offset {
                runs.push(code_run(start - offset, base, &font));
            }
            if end > start {
                runs.push(code_run(end - start, theme.token_color(token.class), &font));
            }
            offset = offset.max(end);
        }
        if offset < text.len() {
            runs.push(code_run(text.len() - offset, base, &font));
        }
    }
    if runs.is_empty() {
        runs.push(code_run(display.len(), base, &font));
    }
    (display, runs)
}

/// Element id for one line inside a code block.
fn code_line_id(ix: usize, salt: u64, block_ix: usize, line_ix: usize) -> u64 {
    ((ix as u64) << 40) | ((salt & 0xff_ffff) << 16) | ((block_ix as u64) << 10) | line_ix as u64
}

fn code_run(len: usize, color: Hsla, font: &Font) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
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

/// Shared sizing policy for both the cells and the strip-breakout decision.
/// Columns follow their content, with a readable baseline and a wrapping cap.
fn table_column_widths(header: &[String], rows: &[Vec<String>]) -> Vec<f32> {
    let columns = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    let mut widths = vec![96f32; columns];
    for (col, width) in widths.iter_mut().enumerate() {
        let mut longest = 4usize;
        if let Some(cell) = header.get(col) {
            longest = longest.max(cell.chars().count());
        }
        for row in rows {
            if let Some(cell) = row.get(col) {
                longest = longest.max(cell.chars().count());
            }
        }
        *width = (longest as f32 * 6.8 + 28.).clamp(96., 340.);
    }
    widths
}

/// GFM table: outer border, semibold header, dividers, content-weighted
/// columns (`th`/`td` markdown-table styling).
fn render_table(
    header: &[String],
    rows: &[Vec<String>],
    aligns: &[TableAlign],
    ix: usize,
    salt: u64,
    block_ix: usize,
    theme: Theme,
) -> AnyElement {
    let widths = table_column_widths(header, rows);
    let columns = widths.len();
    if columns == 0 {
        return div().into_any_element();
    }
    let table_width: f32 = widths.iter().sum();
    let make_cell = |text: &str,
                     col: usize,
                     strong: bool,
                     align: TableAlign,
                     sub: usize,
                     salt: u64,
                     theme: Theme| {
        div()
            .flex_none()
            .w(px(widths[col]))
            .min_w_0()
            .px(DynamicSpacing::Base12.px(&theme))
            .py(DynamicSpacing::Base08.px(&theme))
            .text_align(align.text_align())
            .child(paragraph_text(
                text,
                13.,
                22.,
                if strong {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                },
                theme.assistant_text,
                // Table cells get their own id namespace: `md_id`'s 8-bit
                // `sub` overflows for taller tables and would collide with
                // other blocks (selection keys depend on unique ids).
                ElementId::NamedInteger(
                    "md-table".into(),
                    ((ix as u64) << 40)
                        | ((salt & 0xffff) << 24)
                        | ((block_ix as u64) << 16)
                        | (sub as u64 & 0xffff),
                ),
                theme,
            ))
    };
    let align_at = |col: usize| aligns.get(col).copied().unwrap_or_default();
    let mut inner = div()
        .flex_none()
        // Keep the actual content extent for horizontal scrolling. Neither
        // the table nor its last column should grow into unused space.
        .w(px(table_width))
        .flex()
        .flex_col();
    inner = inner.child(
        div()
            .w_full()
            .flex_none()
            .flex()
            .bg(theme.overlay)
            .children((0..columns).map(|col| {
                make_cell(
                    header.get(col).map(String::as_str).unwrap_or_default(),
                    col,
                    true,
                    align_at(col),
                    col,
                    salt,
                    theme,
                )
            })),
    );
    for (row_ix, row) in rows.iter().enumerate() {
        inner = inner.child(
            div()
                .w_full()
                .flex_none()
                .flex()
                .border_t_1()
                .border_color(theme.border)
                .children((0..columns).map(|col| {
                    make_cell(
                        row.get(col).map(String::as_str).unwrap_or_default(),
                        col,
                        false,
                        align_at(col),
                        64 + (row_ix * 64) + col,
                        salt,
                        theme,
                    )
                })),
        );
    }
    div()
        .w(px(table_width + 2.))
        .max_w_full()
        .min_w_0()
        .overflow_hidden()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .id(ElementId::NamedInteger(
                    "table-scroll".into(),
                    ((ix as u64) << 40) | ((salt & 0xffff) << 24) | ((block_ix as u64) << 8),
                ))
                .w_full()
                .min_w_0()
                .overflow_x_scroll()
                .child(inner),
        )
        .into_any_element()
}
/// Footer stamp for the changed-files summary: `Today 1:15 PM`,
/// `Yesterday 6:07 PM`, then a short date (`Sep 6`) once past yesterday.
fn summary_time_label(millis: i64) -> String {
    summary_time_label_at(millis, chrono::Local::now())
}

fn summary_time_label_at(millis: i64, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::{DateTime, Local, TimeZone};
    let Some(utc) = DateTime::from_timestamp_millis(millis) else {
        return String::new();
    };
    let dt: DateTime<Local> = Local.from_utc_datetime(&utc.naive_local());
    let days = (now.date_naive() - dt.date_naive()).num_days();
    let clock = dt.format("%-I:%M %p");
    match days {
        0 => tr!("transcript_view.today_clock", clock = clock.to_string()),
        1 => tr!("transcript_view.yesterday_clock", clock = clock.to_string()),
        _ => dt.format("%b %-d").to_string(),
    }
}

fn changed_files_title(count: usize) -> String {
    if count == 1 {
        tr!("transcript_view.changed_file_one", count = count)
    } else {
        tr!("transcript_view.changed_file_other", count = count)
    }
}

/// Show a changed-file path relative to the session workspace when possible.
fn workspace_relative_path(path: &str, workspace: Option<&Path>) -> String {
    let path = Path::new(path);
    if let Some(root) = workspace {
        if path.is_absolute() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel = rel.to_string_lossy();
                let rel = rel.trim_start_matches(['/', '\\']);
                if !rel.is_empty() {
                    return rel.replace('\\', "/");
                }
            }
        }
    }
    path.to_string_lossy().replace('\\', "/")
}

/// End-of-task changed-files summary: a hairlined group on the transcript
/// canvas (canvas fill — not a raised/shadowed slab), "Changed N files"
/// with a ±delta underneath, a Review chip, and roomy file rows. Shows 3
/// rows; expanded shows up to 12 with a clip note.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_changed_files(
    files: &[(String, u64, u64)],
    theme: Theme,
    nerd: Option<&SharedString>,
    message_ix: usize,
    workspace: Option<&Path>,
    expanded: bool,
    expanded_files: Rc<RefCell<HashSet<usize>>>,
    scroller: MessageScrollerState,
    review_changes: Option<ReviewOpener>,
) -> impl IntoElement {
    const EXPANDED_PREVIEW_LIMIT: usize = 12;
    let additions: u64 = files.iter().map(|(_, added, _)| *added).sum();
    let deletions: u64 = files.iter().map(|(_, _, removed)| *removed).sum();
    let title = changed_files_title(files.len());
    let can_expand = files.len() > CHANGED_FILES_PREVIEW_LIMIT;
    let visible = if expanded {
        files.len().min(EXPANDED_PREVIEW_LIMIT)
    } else {
        files.len().min(CHANGED_FILES_PREVIEW_LIMIT)
    };

    let mut rows = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(theme.border_subtle);
    for (path, added, removed) in files.iter().take(visible) {
        let label = workspace_relative_path(path, workspace);
        rows = rows.child(
            div()
                .h(px(32.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(file_glyph(
                    path,
                    theme.mode == ThemeMode::Dark,
                    nerd,
                    IconSize::Small.px(&theme),
                    file_badge(path, theme),
                ))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_size(TextSize::Small.px(&theme))
                        .line_height(theme.ui_px(16.))
                        .text_color(theme.text_secondary)
                        .child(label),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(TextSize::XSmall.px(&theme))
                        .text_color(theme.success)
                        .child(format!("+{added}")),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(TextSize::XSmall.px(&theme))
                        .text_color(theme.error)
                        .child(format!("-{removed}")),
                ),
        );
    }

    // Review affordance: opens the changed-files diff in the right side
    // pane's Review tab (no external editor hop).
    let review = review_changes.map(|review| {
        button_frame(
            div().id(ElementId::NamedInteger(
                "review-changes".into(),
                message_ix as u64,
            )),
            &theme,
            ButtonSize::Medium,
        )
        .border_1()
        .border_color(theme.border)
        .raised(theme.surface_elevated, &theme)
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_secondary)
        .hover(|style| {
            style
                .raised(theme.surface_hover, &theme)
                .text_color(theme.text_primary)
        })
        .child(glyph(
            "icons/file-diff.svg",
            ButtonSize::Medium.icon_size().px(&theme),
            theme.text_muted,
        ))
        .child(tr!("transcript_view.review"))
        .on_click(move |_, window, cx| review(window, cx))
    });

    let mut header = div()
        .min_h(px(56.))
        .px(px(14.))
        .py(px(10.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(28.))
                .flex_none()
                .rounded(Radius::Large.px(&theme))
                .bg(theme.surface_elevated)
                .border_1()
                .border_color(theme.border)
                .flex()
                .items_center()
                .justify_center()
                .child(glyph(
                    "icons/file-diff.svg",
                    IconSize::Small.px(&theme),
                    theme.text_secondary,
                )),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_size(TextSize::Default.px(&theme))
                        .line_height(theme.ui_px(17.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .mt(px(2.))
                        .flex()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .text_size(TextSize::Small.px(&theme))
                        .line_height(theme.ui_px(14.))
                        .child(
                            div()
                                .text_color(theme.success)
                                .child(format!("+{additions}")),
                        )
                        .child(div().text_color(theme.error).child(format!("-{deletions}"))),
                ),
        );
    if let Some(review) = review {
        header = header.child(review);
    }

    let mut card = div()
        .w_full()
        .min_w_0()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .overflow_hidden()
        .child(header)
        .child(rows);

    if can_expand {
        let remaining = files.len() - CHANGED_FILES_PREVIEW_LIMIT;
        let label = if expanded {
            tr!("transcript_view.show_fewer_files")
        } else if remaining == 1 {
            tr!("transcript_view.show_one_more_file")
        } else {
            tr!("transcript_view.show_more_files", count = remaining)
        };
        let clipped = expanded && files.len() > EXPANDED_PREVIEW_LIMIT;
        let mut toggle = div()
            .id(ElementId::NamedInteger(
                "changed-files-toggle".into(),
                message_ix as u64,
            ))
            .h(px(34.))
            .px(px(14.))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base06.px(&theme))
            .cursor_pointer()
            .text_size(TextSize::Small.px(&theme))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_secondary)
            .hover(|style| style.bg(theme.surface_hover).text_color(theme.text_primary))
            .child(label);
        if clipped {
            toggle = toggle.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .font_weight(FontWeight::NORMAL)
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(tr!(
                        "transcript_view.showing_first_of",
                        limit = EXPANDED_PREVIEW_LIMIT,
                        total = files.len()
                    )),
            );
        }
        card = card.child(
            toggle
                .child(div().flex_1())
                .child(glyph(
                    if expanded {
                        "icons/chevron-down.svg"
                    } else {
                        "icons/chevron-right.svg"
                    },
                    IconSize::XSmall.px(&theme),
                    theme.text_muted,
                ))
                .on_click(move |_, _, cx| {
                    toggle_index(&expanded_files, message_ix);
                    scroller.remeasure_toggle(message_ix);
                    cx.refresh_windows();
                }),
        );
    }

    card
}

fn toggle_index(set: &Rc<RefCell<HashSet<usize>>>, ix: usize) {
    let mut set = set.borrow_mut();
    if !set.remove(&ix) {
        set.insert(ix);
    }
}

/// The leading glyph for a tool card — a HugeIcons stroke-rounded SVG from
/// `assets/icons/tools/`. Tools are grouped by the kind of work so a turn
/// scans by shape: explore (read/search/find/list), mutate (edit/write),
/// execute (bash), delegate (task/skill/mcp/web), and a wrench fallback so
/// an unknown tool still renders a real mark.
pub(crate) fn activity_icon(name: &str) -> &'static str {
    match name {
        "edit" => "icons/tools/edit.svg",
        "write" => "icons/tools/write.svg",
        "read" | "view" => "icons/tools/read.svg",
        "grep" | "search" => "icons/tools/search.svg",
        "find" | "glob" => "icons/tools/find.svg",
        "list" | "ls" | "tree" => "icons/tools/list.svg",
        "bash" | "shell" | "terminal" | "exec" | "run" => "icons/tools/bash.svg",
        "task" | "agent" | "subagent" => "icons/tools/task.svg",
        "web_search" | "websearch" | "search_web" | "browse" => "icons/tools/web.svg",
        "web_fetch" | "webfetch" | "fetch" | "open_url" => "icons/tools/fetch.svg",
        "skill" => "icons/tools/skill.svg",
        "ask" | "ask_user" | "question" | "elicit" => "icons/tools/ask.svg",
        "todo" | "todo_write" | "plan" | "update_plan" => "icons/tools/todo.svg",
        "notebook" | "eval" | "execute_code" | "codemode" => "icons/tools/code.svg",
        "tool_search" => "icons/tools/search.svg",
        name if name.starts_with("mcp") || name.contains("mcp_resource") => "icons/tools/mcp.svg",
        _ => "icons/tools/tool.svg",
    }
}

/// One glyph per distinct work kind in a group, deduped and ordered
/// canonically (run → read → edit → web → other), the reasoning bulb last
/// when any step thought. The cluster is an index of what ran; the counts
/// in the title stay the truth.
fn activity_group_icons(steps: &[Step]) -> Vec<&'static str> {
    let mut icons: Vec<&'static str> = Vec::new();
    for step in steps {
        for tool in &step.tools {
            let icon = activity_icon(&tool.name);
            if !icons.contains(&icon) {
                icons.push(icon);
            }
        }
    }
    icons.sort_unstable_by_key(|icon| activity_icon_rank(icon));
    if steps.iter().any(|step| !step.thinking.is_empty()) {
        icons.push("icons/tools/thinking.svg");
    }
    // Six glyphs is all a compact summary chip can afford before the cluster
    // starts competing with the title.
    icons.truncate(6);
    icons
}

/// Canonical cluster order: run, then explore, then mutate, then web, then
/// everything else (task/skill/ask/mcp/wrench).
fn activity_icon_rank(icon: &'static str) -> usize {
    match icon {
        "icons/tools/bash.svg" => 0,
        "icons/tools/read.svg"
        | "icons/tools/search.svg"
        | "icons/tools/find.svg"
        | "icons/tools/list.svg" => 1,
        "icons/tools/edit.svg" | "icons/tools/write.svg" => 2,
        "icons/tools/web.svg" | "icons/tools/fetch.svg" => 3,
        _ => 4,
    }
}

/// A tool glyph in its category badge: a 22px rounded square washed at a low
/// alpha of the tone, the glyph at full strength. The wash lifts the icon out
/// of the prose column so a tool row reads as a control at a glance, in every
/// palette. `tone` is the state color while the call runs/fails, the work
/// kind's tint once it settles (see `work_tint`).
fn activity_badge(icon: &'static str, tone: Hsla, theme: Theme) -> impl IntoElement {
    let wash = match theme.mode {
        ThemeMode::Dark => 0.16,
        ThemeMode::Light => 0.12,
    };
    div()
        .flex_none()
        .size(px(22.))
        .rounded(Radius::Medium.px(&theme))
        .bg(tone.opacity(wash))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph(icon, IconSize::Small.px(&theme), tone))
}

/// Split a Pi MCP tool name (`mcp__<server>__<tool>`) into `(server, tool)`
/// for the card header. Pi sanitizes both halves to `[A-Za-z0-9_-]` and joins
/// them with `__`; a server name that itself contains `__` would split early,
/// which is the one lossy case — the full name still reaches the model and the
/// expanded args.
pub(crate) fn mcp_tool_parts(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// Human label for a tool card header. Known pi tools get a short, scannable
/// verb ("Run", "Read", "Find files"); anything else falls back to the
/// capitalized tool name so an unknown tool never renders blank.
fn activity_action_label(name: &str) -> String {
    let known = match name {
        "bash" | "shell" | "terminal" | "exec" | "run" => Some("Run"),
        "read" | "view" => Some("Read"),
        "edit" => Some("Edit"),
        "write" => Some("Write"),
        "grep" | "search" => Some("Search"),
        "find" | "glob" => Some("Find files"),
        "list" | "ls" | "tree" => Some("List"),
        "task" | "agent" | "subagent" => Some("Task"),
        "web_search" | "websearch" | "search_web" | "browse" => Some("Web search"),
        "web_fetch" | "webfetch" | "fetch" | "open_url" => Some("Fetch"),
        "skill" => Some("Skill"),
        "ask" | "ask_user" | "question" | "elicit" => Some("Ask"),
        "todo" | "todo_write" | "plan" | "update_plan" => Some("Plan"),
        "notebook" | "eval" | "execute_code" | "codemode" => Some("Run code"),
        "tool_search" => Some("Find tools"),
        "list_mcp_resources" | "list_mcp_resource_templates" => Some("List resources"),
        "read_mcp_resource" => Some("Read resource"),
        _ => None,
    };
    if let Some(label) = known {
        return label.to_string();
    }
    if name.starts_with("mcp") {
        return "MCP".to_string();
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Tool".to_string(),
    }
}

pub(crate) fn active_user_index(
    messages: &[ChatMessage],
    streaming: Option<usize>,
    viewport_hint: Option<usize>,
) -> Option<usize> {
    if messages.is_empty() {
        return None;
    }
    // The reader scrolled away from the live edge: the active tick is the
    // turn the viewport is reading — the newest user turn at or above the
    // first visible row — not the latest turn in the transcript. This holds
    // while a run streams too: scrolling up to read history must move the
    // rail, and clicking a tick must be able to leave the streaming turn.
    if let Some(first_visible) = viewport_hint {
        let clamped = first_visible.min(messages.len() - 1);
        if let Some(ix) = messages[..clamped + 1]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, message)| message.user)
            .map(|(ix, _)| ix)
        {
            return Some(ix);
        }
    }
    let end = streaming
        .unwrap_or(messages.len() - 1)
        .min(messages.len() - 1);
    messages[..end + 1]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| message.user)
        .map(|(ix, _)| ix)
}

fn starts_followup_turn(messages: &[ChatMessage], ix: usize) -> bool {
    ix > 0 && messages[ix].user && !messages[ix - 1].user
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_label_falls_back_without_elapsed() {
        assert_eq!(fold_label(None), "Worked");
        assert_eq!(
            fold_label(Some(Duration::from_secs(341))),
            "Worked for 5 minutes 41 seconds"
        );
    }

    #[test]
    fn step_thinking_duration_prefers_live_and_estimates_on_reload() {
        let step = |thinking: &str, timestamp: Option<i64>, duration: Option<Duration>| Step {
            thinking: thinking.into(),
            timestamp,
            thinking_duration: duration,
            ..Step::default()
        };
        // A measured duration always wins over the timestamp estimate.
        let live = vec![
            step("a", Some(1_000), Some(Duration::from_millis(2_500))),
            step("b", Some(9_999), None),
        ];
        assert_eq!(
            step_thinking_duration(&live, 0),
            Some(Duration::from_millis(2_500))
        );
        // Reload: estimated from the gap to the next step's timestamp.
        let reload = vec![step("a", Some(1_000), None), step("b", Some(4_500), None)];
        assert_eq!(
            step_thinking_duration(&reload, 0),
            Some(Duration::from_millis(3_500))
        );
        // The last step has no following timestamp to measure against.
        assert_eq!(step_thinking_duration(&reload, 1), None);
        // A step without reasoning has no duration at all.
        let no_think = vec![step("", Some(1_000), None), step("b", Some(2_000), None)];
        assert_eq!(step_thinking_duration(&no_think, 0), None);
    }

    #[test]
    fn split_thinking_scroll_chains_only_past_the_edges() {
        // Mid-card: the full delta stays on the card, nothing forwarded.
        assert_eq!(
            split_thinking_scroll(px(-40.), px(100.), px(-10.)),
            (px(-50.), px(0.))
        );
        // At the bottom: the overshoot is clamped off and forwarded whole.
        assert_eq!(
            split_thinking_scroll(px(-95.), px(100.), px(-10.)),
            (px(-100.), px(-5.))
        );
        // At the top scrolling up: the overshoot goes back to the transcript.
        assert_eq!(
            split_thinking_scroll(px(-5.), px(100.), px(10.)),
            (px(0.), px(5.))
        );
        // A card that cannot scroll forwards everything (short content).
        assert_eq!(
            split_thinking_scroll(px(0.), px(0.), px(-10.)),
            (px(0.), px(-10.))
        );
    }

    #[test]
    fn thinking_follow_only_detaches_on_an_upward_wheel() {
        // At the card's bottom (or too short to scroll): follow the stream.
        assert_eq!(
            thinking_follow_after(px(-100.), px(100.), px(-10.)),
            Some(true)
        );
        assert_eq!(thinking_follow_after(px(0.), px(0.), px(-10.)), Some(true));
        assert_eq!(thinking_follow_after(px(0.), px(0.), px(10.)), Some(true));
        // Mid-card, a wheel up detaches; a wheel down leaves the choice be.
        assert_eq!(
            thinking_follow_after(px(-50.), px(100.), px(10.)),
            Some(false)
        );
        assert_eq!(thinking_follow_after(px(-50.), px(100.), px(-10.)), None);
        // A wheel up from the bottom moves off it and detaches.
        assert_eq!(
            thinking_follow_after(px(-90.), px(100.), px(10.)),
            Some(false)
        );
    }

    #[test]
    fn activity_title_aggregates_every_step() {
        let tool = |name: &str| ToolCall {
            name: name.to_string(),
            summary: String::new(),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: None,
            output: None,
            failed: false,
            facts: Default::default(),
        };
        // Two steps' worth of work lands on one summary line — counts span
        // every step instead of one "Ran …" row per step.
        let steps = vec![
            Step {
                thinking: "first".into(),
                tools: vec![tool("edit")],
                ..Step::default()
            },
            Step {
                thinking: "second".into(),
                tools: vec![tool("bash"), tool("read")],
                ..Step::default()
            },
        ];
        assert_eq!(
            activity_title(&steps, false),
            "Ran 1 command · Ran 1 file read · Ran 1 file edit · 2 thoughts"
        );
        assert_eq!(
            activity_title(&steps, true),
            "Ran 1 command · Ran 1 file read · Ran 1 file edit · Thinking"
        );
        assert_eq!(activity_title(&[], false), "Worked");
    }

    #[test]
    fn activity_group_icons_dedupe_order_and_cap() {
        let step = |thinking: &str, tools: &[&str]| Step {
            thinking: thinking.into(),
            tools: tools
                .iter()
                .map(|name| ToolCall {
                    name: name.to_string(),
                    summary: String::new(),
                    path: None,
                    added: 0,
                    removed: 0,
                    id: None,
                    args: None,
                    output: None,
                    failed: false,
                    facts: Default::default(),
                })
                .collect(),
            ..Step::default()
        };
        // Canonical order regardless of arrival order, deduped, bulb last.
        let icons = activity_group_icons(&[
            step("", &["read", "bash", "edit", "read", "bash"]),
            step("hmm", &["bash"]),
        ]);
        assert_eq!(
            icons,
            vec![
                "icons/tools/bash.svg",
                "icons/tools/read.svg",
                "icons/tools/edit.svg",
                "icons/tools/thinking.svg",
            ]
        );
        // A thought-only group carries just the bulb; a tools-only group
        // carries none.
        assert_eq!(
            activity_group_icons(&[step("hmm", &[])]),
            vec!["icons/tools/thinking.svg"]
        );
        assert_eq!(
            activity_group_icons(&[step("", &["bash", "shell"])]),
            vec!["icons/tools/bash.svg"]
        );
    }

    #[test]
    fn duration_format_matches_spoken_forms() {
        assert_eq!(format_duration(Duration::from_secs(1)), "1 second");
        assert_eq!(format_duration(Duration::from_secs(12)), "12 seconds");
        assert_eq!(format_duration(Duration::from_secs(60)), "1 minute");
        assert_eq!(
            format_duration(Duration::from_secs(100)),
            "1 minute 40 seconds"
        );
        assert_eq!(
            format_duration(Duration::from_secs(341)),
            "5 minutes 41 seconds"
        );
        assert_eq!(format_duration(Duration::from_secs(3_600)), "1 hour");
        assert_eq!(
            format_duration(Duration::from_secs(3_721)),
            "1 hour 2 minutes"
        );
    }

    #[test]
    fn working_elapsed_uses_short_form() {
        assert_eq!(format_working_elapsed(Duration::from_secs(0)), "0s");
        assert_eq!(format_working_elapsed(Duration::from_secs(12)), "12s");
        assert_eq!(format_working_elapsed(Duration::from_secs(60)), "1m");
        assert_eq!(format_working_elapsed(Duration::from_secs(341)), "5m 41s");
        assert_eq!(format_working_elapsed(Duration::from_secs(3_600)), "1h");
        assert_eq!(format_working_elapsed(Duration::from_secs(3_721)), "1h 2m");
    }

    #[test]
    fn streaming_tps_prefers_reported_output_and_guards_early() {
        let message = |output: u64, text: &str| ChatMessage {
            user: false,
            summary: None,
            steps: vec![Step {
                text: text.into(),
                usage: (output > 0).then_some(MessageUsage {
                    input: 0,
                    output,
                    cache_read: 0,
                    cache_write: 0,
                    total: output,
                    cost: None,
                }),
                ..Step::default()
            }],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        };

        // Provider-reported output wins over the char estimate.
        let reported = message(80, "short");
        assert_eq!(streamed_output_tokens(&reported), 80);
        assert_eq!(
            streaming_tps(&reported, Some(Duration::from_secs(2))),
            Some(40.0)
        );
        // Under a second, and with no clock, there is no rate yet.
        assert_eq!(
            streaming_tps(&reported, Some(Duration::from_millis(800))),
            None
        );
        assert_eq!(streaming_tps(&reported, None), None);

        // Without provider usage the estimate is chars/4: 40 chars -> 10
        // tokens over 2s -> 5.0.
        let estimated = message(0, &"a".repeat(40));
        assert_eq!(streamed_output_tokens(&estimated), 10);
        assert_eq!(
            streaming_tps(&estimated, Some(Duration::from_secs(2))),
            Some(5.0)
        );

        // No text and no usage: no rate.
        let empty = message(0, "");
        assert_eq!(streaming_tps(&empty, Some(Duration::from_secs(2))), None);
    }

    /// A streaming turn's clock sums the settled steps' durations plus the
    /// live step's elapsed time, so a step that follows a tool call is not
    /// measured against the whole run's token count.
    #[test]
    fn live_generation_clock_spans_settled_and_streaming_steps() {
        let step = |duration: Option<Duration>, generation_time: Option<Duration>| Step {
            duration,
            generation_time,
            ..Step::default()
        };
        let message = |steps: Vec<Step>| ChatMessage {
            user: false,
            summary: None,
            steps,
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        };

        // A single streaming step uses just its own request clock.
        let one = message(vec![step(None, None)]);
        assert_eq!(
            live_generation_clock(&one, Some(Duration::from_secs(5))),
            Some(Duration::from_secs(5))
        );

        // A settled step's pi duration plus the live step's elapsed time.
        let two = message(vec![
            step(Some(Duration::from_secs(3)), None),
            step(None, None),
        ]);
        assert_eq!(
            live_generation_clock(&two, Some(Duration::from_secs(2))),
            Some(Duration::from_secs(5))
        );

        // Without a live clock, the step's client measurement is the fallback.
        let fallback = message(vec![step(None, Some(Duration::from_secs(4)))]);
        assert_eq!(
            live_generation_clock(&fallback, None),
            Some(Duration::from_secs(4))
        );

        // No steps: no clock at all.
        assert_eq!(
            live_generation_clock(&message(Vec::new()), Some(Duration::from_secs(1))),
            None
        );
    }

    #[test]
    fn format_tps_reads_compactly() {
        assert_eq!(format_tps(42.4), "42");
        assert_eq!(format_tps(7.44), "7.4");
        assert_eq!(format_tps(0.0), "0");
        assert_eq!(format_tps(f64::NAN), "0");
    }

    #[test]
    fn generation_tps_uses_output_over_active_decode_time() {
        let usage = MessageUsage {
            input: 100,
            output: 90,
            cache_read: 0,
            cache_write: 0,
            total: 190,
            cost: None,
        };
        assert_eq!(
            generation_tps(&usage, Some(Duration::from_secs(3))),
            Some(30.0)
        );
        // No clock, under a second, or no output: not a rate yet.
        assert_eq!(generation_tps(&usage, None), None);
        assert_eq!(
            generation_tps(&usage, Some(Duration::from_millis(900))),
            None
        );
        let no_output = MessageUsage {
            output: 0,
            ..usage.clone()
        };
        assert_eq!(
            generation_tps(&no_output, Some(Duration::from_secs(3))),
            None
        );
    }

    #[test]
    fn changed_files_title_uses_screenshot_copy() {
        assert_eq!(changed_files_title(1), "Changed 1 file");
        assert_eq!(changed_files_title(2), "Changed 2 files");
    }

    /// The run footer (copy + time + usage) lives once, on the tail summary;
    /// the last message's own footer steps aside so the details don't repeat.
    #[test]
    fn tail_summary_owns_the_single_footer() {
        // No summary: every message keeps its footer.
        assert!(!suppress_message_footer(false, 0, 3));
        assert!(!suppress_message_footer(false, 2, 3));
        // Summary present: only the last message's footer is suppressed.
        assert!(!suppress_message_footer(true, 0, 3));
        assert!(!suppress_message_footer(true, 1, 3));
        assert!(suppress_message_footer(true, 2, 3));
    }

    #[test]
    fn workspace_relative_path_strips_workspace_prefix() {
        // `Path::is_absolute` needs a drive prefix on Windows, so the root is
        // built from a path that is absolute on whatever host runs the test.
        let root = crate::platform::home_dir().join("orbit-ws");
        let file = root.join("src").join("main.rs");
        assert_eq!(
            workspace_relative_path(&file.to_string_lossy(), Some(&root)),
            "src/main.rs"
        );
        assert_eq!(
            workspace_relative_path("src/main.rs", Some(&root)),
            "src/main.rs"
        );
    }

    #[test]
    fn message_cost_keeps_sub_cent_precision() {
        assert_eq!(format_message_cost(0.0), "$0.00");
        assert_eq!(format_message_cost(0.0012), "$0.0012");
        assert_eq!(format_message_cost(0.45), "$0.45");
        assert_eq!(format_message_cost(12.345), "$12.35");
        assert_eq!(format_message_cost(250.0), "$250");
        assert_eq!(format_message_cost(f64::NAN), "$0.00");
    }

    #[test]
    fn summary_time_label_matches_screenshot_copy() {
        use chrono::{Duration, TimeZone, Timelike, Utc};
        let at = |y: i32, m: u32, d: u32, h: u32, min: u32| {
            Utc.with_ymd_and_hms(y, m, d, h, min, 0)
                .single()
                .unwrap()
                .with_timezone(&chrono::Local)
        };
        // Bucket boundaries are relative to `now`, so the test is
        // timezone-independent.
        let now = at(2026, 9, 8, 16, 0);
        let millis = |dt: chrono::DateTime<chrono::Local>| dt.timestamp_millis();
        // 18:07 local yesterday — the screenshot's "Yesterday 6:07 PM".
        let yesterday = (now - Duration::days(1))
            .with_hour(18)
            .unwrap()
            .with_minute(7)
            .unwrap();
        assert_eq!(
            summary_time_label_at(millis(yesterday), now),
            "Yesterday 6:07 PM"
        );
        // Same day → Today + 12-hour clock, no leading zero on the hour.
        let today = now.with_hour(13).unwrap().with_minute(5).unwrap();
        assert_eq!(summary_time_label_at(millis(today), now), "Today 1:05 PM");
        // Older than yesterday falls back to a short date.
        let older = now - Duration::days(3);
        assert_eq!(
            summary_time_label_at(millis(older), now),
            older.format("%b %-d").to_string()
        );
    }

    #[test]
    fn activity_icon_maps_pi_tools() {
        assert_eq!(activity_icon("edit"), "icons/tools/edit.svg");
        assert_eq!(activity_icon("write"), "icons/tools/write.svg");
        assert_eq!(activity_icon("read"), "icons/tools/read.svg");
        assert_eq!(activity_icon("bash"), "icons/tools/bash.svg");
        assert_eq!(activity_icon("grep"), "icons/tools/search.svg");
        assert_eq!(activity_icon("glob"), "icons/tools/find.svg");
        assert_eq!(activity_icon("mcp"), "icons/tools/mcp.svg");
        assert_eq!(activity_icon("mcp__filesystem"), "icons/tools/mcp.svg");
        assert_eq!(activity_icon("codemode"), "icons/tools/code.svg");
        assert_eq!(activity_icon("tool_search"), "icons/tools/search.svg");
        assert_eq!(activity_icon("read_mcp_resource"), "icons/tools/mcp.svg");
        assert_eq!(activity_icon("list_mcp_resources"), "icons/tools/mcp.svg");
        // An unknown tool keeps a real mark instead of a blank slot.
        assert_eq!(activity_icon("frobnicate"), "icons/tools/tool.svg");
    }

    #[test]
    fn activity_action_label_reads_as_a_verb() {
        assert_eq!(activity_action_label("bash"), "Run");
        assert_eq!(activity_action_label("read"), "Read");
        assert_eq!(activity_action_label("glob"), "Find files");
        assert_eq!(activity_action_label("mcp__github"), "MCP");
        assert_eq!(activity_action_label("codemode"), "Run code");
        assert_eq!(activity_action_label("tool_search"), "Find tools");
        assert_eq!(activity_action_label("read_mcp_resource"), "Read resource");
        // Unknown tools still get a readable, never-blank label.
        assert_eq!(activity_action_label("frobnicate"), "Frobnicate");
        assert_eq!(activity_action_label(""), "Tool");
    }

    #[test]
    fn mcp_tool_names_split_into_server_and_tool() {
        assert_eq!(
            mcp_tool_parts("mcp__github__search_repositories"),
            Some(("github", "search_repositories"))
        );
        assert_eq!(
            mcp_tool_parts("mcp__my_server__get_issue"),
            Some(("my_server", "get_issue"))
        );
        // Non-MCP names and malformed shapes never produce a fake split.
        assert_eq!(mcp_tool_parts("bash"), None);
        assert_eq!(mcp_tool_parts("mcp__github"), None);
        assert_eq!(mcp_tool_parts("mcp____tool"), None);
        assert_eq!(mcp_tool_parts("mcp__server__"), None);
    }

    #[test]
    fn truncation_label_reports_the_line_budget() {
        // A capped read: the agent saw 1,172 of 1,303 lines.
        assert_eq!(
            truncation_label(&ToolFacts {
                truncated: true,
                output_lines: Some(1172),
                total_lines: Some(1303),
            })
            .as_deref(),
            Some("truncated · 1172/1303")
        );
        // A cap with no reported budget still reads as truncated.
        assert_eq!(
            truncation_label(&ToolFacts {
                truncated: true,
                ..ToolFacts::default()
            })
            .as_deref(),
            Some("truncated")
        );
        // A complete result never shows the chip.
        assert_eq!(truncation_label(&ToolFacts::default()), None);
    }

    #[test]
    fn command_tools_surface_the_shell_command_not_json() {
        let bash = ToolCall {
            name: "bash".into(),
            summary: r#"{"command":"ls -la"}"#.into(),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({ "command": "ls -la" })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        assert_eq!(tool_command(&bash).as_deref(), Some("ls -la"));
        // The header preview shows the command, not the raw JSON summary.
        assert_eq!(activity_preview(&bash), "ls -la");
        // Non-command tools do not fabricate a command.
        let edit = ToolCall {
            name: "edit".into(),
            ..bash.clone()
        };
        assert_eq!(tool_command(&edit), None);
    }

    #[test]
    fn codemode_previews_the_first_script_line() {
        let codemode = ToolCall {
            name: "codemode".into(),
            summary: r#"{"code":"..."}"#.into(),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({
                "code": "\n\nconst repos = await tools.search_repositories({ q: \"rust gpui\" });\nreturn repos;"
            })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        assert_eq!(
            activity_preview(&codemode),
            "const repos = await tools.search_repositories({ q: \"rust gpui\" });"
        );
    }

    #[test]
    fn fetch_tools_preview_the_url_not_json() {
        let fetch = ToolCall {
            name: "web_fetch".into(),
            summary: r#"{"url":"https://example.com/docs/start"}"#.into(),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({ "url": "https://example.com/docs/start" })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        assert_eq!(
            tool_url(&fetch).as_deref(),
            Some("https://example.com/docs/start")
        );
        // The header drops the scheme so the host leads the line.
        assert_eq!(activity_preview(&fetch), "example.com/docs/start");
        assert_eq!(display_url("http://www.example.com/x"), "example.com/x");
        assert_eq!(display_url("https://example.com"), "example.com");
        assert_eq!(display_url("example.com"), "example.com");

        let search = ToolCall {
            name: "web_search".into(),
            summary: r#"{"query":"rust async book"}"#.into(),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({ "query": "rust async book" })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        assert_eq!(
            tool_search_query(&search).as_deref(),
            Some("rust async book")
        );
        assert_eq!(activity_preview(&search), "rust async book");
        // Other tools never fabricate a link or a query.
        assert_eq!(tool_url(&search), None);
        assert_eq!(tool_search_query(&fetch), None);
    }

    #[test]
    fn only_file_tools_get_the_file_glyph() {
        let read = ToolCall {
            name: "read".into(),
            summary: String::new(),
            path: Some("src/main.rs".into()),
            added: 0,
            removed: 0,
            id: None,
            args: None,
            output: None,
            failed: false,
            facts: Default::default(),
        };
        assert_eq!(file_preview_path(&read), Some("src/main.rs"));
        // The preview still shows the path (unchanged behavior).
        assert_eq!(activity_preview(&read), "src/main.rs");
        // A search tool's `path` is a scope, not the thing it read; it keeps
        // its generic icon instead of a file glyph.
        let grep = ToolCall {
            name: "grep".into(),
            ..read.clone()
        };
        assert_eq!(file_preview_path(&grep), None);
    }

    #[test]
    fn multiline_command_preview_is_single_line_without_changing_the_command() {
        let command = "python3 - <<'PY'\n  print('hello')\nPY";
        let tool = ToolCall {
            name: "bash".into(),
            summary: command.into(),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({ "command": command })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        assert_eq!(
            activity_preview(&tool),
            "python3 - <<'PY' print('hello') PY"
        );
        assert_eq!(tool_command(&tool).as_deref(), Some(command));
    }

    #[test]
    fn line_diff_keeps_context_around_an_insertion() {
        let old =
            "let columns = self.tool_columns();\nlet mut elements = Vec::new();\nfor row in rows {";
        let new = "let columns = self.tool_columns();\nlet total_calls = snapshot.tools.calls.max(1);\nlet mut elements = Vec::new();\nfor row in rows {";
        let rows = line_diff(old, new, Some(highlight::Lang::Rust));
        let kinds: Vec<DiffRowKind> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffRowKind::Context,
                DiffRowKind::Addition,
                DiffRowKind::Context,
                DiffRowKind::Context,
            ]
        );
        assert_eq!(
            rows[1].text,
            "let total_calls = snapshot.tools.calls.max(1);"
        );
        // The line is lexed, so the keyword carries a token.
        assert!(rows[1].tokens.as_ref().is_some_and(|t| !t.is_empty()));
    }

    #[test]
    fn build_edit_diff_walks_pi_edit_arguments() {
        let tool = ToolCall {
            name: "edit".into(),
            summary: String::new(),
            path: Some("src/view.rs".into()),
            added: 1,
            removed: 1,
            id: None,
            args: Some(serde_json::json!({
                "path": "src/view.rs",
                "edits": [{ "oldText": "let a = 1;", "newText": "let a = 2;" }]
            })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        let rows = build_edit_diff(&tool).expect("diff");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, DiffRowKind::Deletion);
        assert_eq!(rows[1].kind, DiffRowKind::Addition);
        assert_eq!(edit_patch_text(&rows), "-let a = 1;\n+let a = 2;\n");
        // A read tool has no edit text to show.
        let read = ToolCall {
            name: "read".into(),
            ..tool
        };
        assert!(build_edit_diff(&read).is_none());
    }

    #[test]
    fn write_diff_marks_every_line_added() {
        let tool = ToolCall {
            name: "write".into(),
            summary: String::new(),
            path: Some("src/new.rs".into()),
            added: 2,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({ "path": "src/new.rs", "content": "a\nb\n" })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        let rows = build_edit_diff(&tool).expect("diff");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.kind == DiffRowKind::Addition));
        assert_eq!(rows[1].text, "b");
    }

    #[test]
    fn section_lang_only_highlights_structured_values() {
        assert_eq!(section_lang(None), None);
        assert_eq!(
            section_lang(Some(&Value::String("raw output".into()))),
            None
        );
        assert_eq!(
            section_lang(Some(&serde_json::json!({ "a": 1 }))),
            Some(highlight::Lang::Json)
        );
    }

    #[test]
    fn first_error_line_picks_first_nonempty_string() {
        assert_eq!(
            first_error_line(&Value::String(
                "error: file not found\n  at main.rs:1".into()
            )),
            Some("error: file not found".to_string())
        );
        // Structured results dig out the first text-bearing value.
        assert_eq!(
            first_error_line(&serde_json::json!({"message": {"text": "boom"}})),
            Some("boom".to_string())
        );
        assert_eq!(first_error_line(&serde_json::json!(null)), None);
    }

    #[test]
    fn active_user_index_tracks_streaming_turn() {
        let messages = vec![
            ChatMessage {
                user: true,
                summary: None,
                steps: vec![Step {
                    text: "a".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: false,
                summary: None,
                steps: vec![Step {
                    text: "b".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: true,
                summary: None,
                steps: vec![Step {
                    text: "c".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: false,
                summary: None,
                steps: vec![Step {
                    text: "d".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
        ];
        assert_eq!(active_user_index(&messages, Some(1), None), Some(0));
        assert_eq!(active_user_index(&messages, Some(3), None), Some(2));
        assert_eq!(active_user_index(&messages, None, None), Some(2));
    }

    #[test]
    fn active_user_index_follows_viewport_when_scrolled() {
        let messages = vec![
            ChatMessage {
                user: true,
                summary: None,
                steps: vec![Step {
                    text: "a".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: false,
                summary: None,
                steps: vec![Step {
                    text: "b".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: true,
                summary: None,
                steps: vec![Step {
                    text: "c".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: false,
                summary: None,
                steps: vec![Step {
                    text: "d".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
        ];
        // Reader scrolled back to the first turn: the tick for turn 0 is
        // active even though the newest turn is turn 2.
        assert_eq!(active_user_index(&messages, None, Some(0)), Some(0));
        // Still reading within the first turn's run (row 1).
        assert_eq!(active_user_index(&messages, None, Some(1)), Some(0));
        // Reached the second user turn: its tick takes over.
        assert_eq!(active_user_index(&messages, None, Some(2)), Some(2));
        assert_eq!(active_user_index(&messages, None, Some(3)), Some(2));
        // Out-of-range hint clamps to the last row.
        assert_eq!(active_user_index(&messages, None, Some(99)), Some(2));
        // The viewport wins while a run streams too: clicking a rail tick to
        // read an earlier turn must leave the streaming turn's tick behind.
        assert_eq!(active_user_index(&messages, Some(3), Some(0)), Some(0));
        assert_eq!(active_user_index(&messages, Some(3), Some(1)), Some(0));
        assert_eq!(active_user_index(&messages, Some(3), Some(2)), Some(2));
    }

    #[test]
    fn parse_inline_extracts_bold_code_and_links() {
        let spans = parse_inline("plain **bold** and `code` and [x](https://a.b)");
        assert_eq!(spans.len(), 6);
        assert_eq!(spans[0].text, "plain ");
        assert!(spans[1].bold);
        assert_eq!(spans[1].text, "bold");
        assert!(spans[3].code);
        assert_eq!(spans[3].text, "code");
        assert_eq!(spans[5].link.as_deref(), Some("https://a.b"));
        assert_eq!(spans[5].text, "x");
    }

    #[test]
    fn parse_inline_keeps_unmatched_markers_literal() {
        let spans = parse_inline("a * b ` c [d");
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "a * b ` c [d");
        assert!(spans.iter().all(|s| !s.bold && !s.code && s.link.is_none()));
    }

    #[test]
    fn parse_inline_escapes_render_literally() {
        let spans = parse_inline(r"\*not bold\*");
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "*not bold*");
        assert!(spans.iter().all(|s| !s.bold));
    }

    #[test]
    fn parse_blocks_splits_paragraphs_headings_and_fences() {
        let blocks = parse_blocks("one\ntwo\n\n## Title\n\n```rs\nlet a = 1;\n```");
        assert_eq!(blocks.len(), 3);
        assert!(matches!(&blocks[0], Block::Paragraph(lines) if lines.join(" ") == "one two"));
        assert!(matches!(&blocks[1], Block::Heading(2, title) if title == "Title"));
        assert!(
            matches!(&blocks[2], Block::Code(Some(lang), lines) if lang == "rs"
            && lines == &["let a = 1;".to_string()])
        );
        // A bare fence carries no language; the chip is simply absent.
        let bare = parse_blocks("```\nx\n```");
        assert!(matches!(&bare[0], Block::Code(None, lines) if lines == &["x".to_string()]));
    }

    #[test]
    fn mermaid_fences_stay_copyable_code_blocks() {
        // D2: diagrams are never faked. A mermaid fence parses as an ordinary
        // code block (so it is copyable) and its label is honest about the
        // missing preview.
        let blocks = parse_blocks("```mermaid\ngraph TD; A-->B;\n```");
        assert!(matches!(&blocks[0], Block::Code(Some(lang), lines)
                if lang == "mermaid" && lines == &["graph TD; A-->B;".to_string()]));
        assert_eq!(
            code_block_label(Some("Mermaid")),
            "Mermaid · source (preview unavailable)"
        );
    }

    #[test]
    fn parse_blocks_collects_loose_and_nested_lists() {
        let blocks = parse_blocks("- a\n- b\n\n  - nested\n\n3. third\n4. fourth");
        assert_eq!(blocks.len(), 2);
        let Block::List(items) = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(items.len(), 3);
        assert!(!items[0].ordered);
        assert_eq!(items[2].depth, 1);
        assert_eq!(items[2].text, "nested");
        let Block::List(ordered) = &blocks[1] else {
            panic!("expected ordered list");
        };
        assert!(ordered[0].ordered);
        assert_eq!(ordered[0].number, 3);
    }

    #[test]
    fn parse_blocks_handles_rules_quotes_and_tables() {
        let blocks =
            parse_blocks("---\n\n> quoted\n> lines\n\n| a | b |\n| --- | --- |\n| 1 | 2 |");
        assert!(matches!(blocks[0], Block::Rule));
        assert!(matches!(&blocks[1], Block::Quote(lines) if lines.len() == 2));
        let Block::Table { header, rows, .. } = &blocks[2] else {
            panic!("expected table");
        };
        assert_eq!(header, &["a", "b"]);
        assert_eq!(rows, &[vec!["1".to_string(), "2".to_string()]]);
    }

    #[test]
    fn parse_blocks_turns_standalone_images_into_image_blocks() {
        // Markdown image on its own line, with a title and an angle-wrapped
        // URL (GitHub's escape for spaces).
        let blocks = parse_blocks(
            "Screenshots or recordings\n\n![shot](<https://example.com/a b.png> \"title\")\n\n<img src=\"https://example.com/c.png\" alt=\"html\" />",
        );
        assert!(
            matches!(&blocks[0], Block::Paragraph(lines) if lines == &["Screenshots or recordings".to_string()])
        );
        assert!(matches!(&blocks[1], Block::Image { alt, url }
                if alt == "shot" && url == "https://example.com/a b.png"));
        assert!(matches!(&blocks[2], Block::Image { alt, url }
                if alt == "html" && url == "https://example.com/c.png"));
    }

    #[test]
    fn parse_blocks_handles_github_pasted_image_tags() {
        // GitHub stores a pasted screenshot as a raw `<img>` whose attributes
        // are ordered `alt width height src`, without a self-closing slash.
        let tag = "<img alt=\"Image\" width=\"1450\" height=\"932\" src=\"https://private-user-images.githubusercontent.com/1/2-a.png?jwt=eyJ0eXAi.eyJpc3Mi.OiJnaXRodWI\">";
        let blocks = parse_blocks(&format!(
            "### What happened?\n\n{tag}\n\n### Steps\n\n1. Open\n"
        ));
        assert!(
            matches!(&blocks[1], Block::Image { alt, url }
                if alt == "Image"
                    && url.starts_with("https://private-user-images.githubusercontent.com/1/2-a.png?jwt=")),
            "expected an image block at index 1"
        );
    }

    #[test]
    fn image_markdown_inside_prose_is_not_split() {
        // A line that only *contains* an image stays prose; only a standalone
        // image line becomes a block, so sentences are never cut in half.
        let blocks = parse_blocks("see ![inline](https://example.com/x.png) here");
        assert!(matches!(&blocks[0], Block::Paragraph(lines)
                if lines.join(" ") == "see ![inline](https://example.com/x.png) here"));
    }

    #[test]
    fn quoted_image_lines_are_recognized() {
        // A quoted bug report keeps its `<img>` on a `> ` line; the block stays
        // a Quote, but the line inside it must still parse as an image.
        let blocks = parse_blocks(
            "> ### What happened?\n> <img alt=\"Image\" width=\"1450\" height=\"932\" src=\"https://private-user-images.githubusercontent.com/1/2-a.png?jwt=eyJ0eXAi.eyJpc3Mi.OiJnaXRodWI\">\n> \n> ### Steps\n> 1. Open\n",
        );
        let Block::Quote(lines) = &blocks[0] else {
            panic!("expected the blockquote to stay a Quote");
        };
        assert!(image_line(&lines[1]).is_some());
        assert_eq!(
            image_line(&lines[1]).unwrap().1,
            "https://private-user-images.githubusercontent.com/1/2-a.png?jwt=eyJ0eXAi.eyJpc3Mi.OiJnaXRodWI"
        );
    }

    #[test]
    fn signed_cdn_urls_rewrite_to_the_durable_asset_url() {
        // A screenshot pasted into a reply is stored as a signed
        // `private-user-images` URL whose JWT expires within minutes; the same
        // asset is served forever from `github.com/user-attachments/assets/…`.
        let signed = "https://private-user-images.githubusercontent.com/74659438/656345150-69f721c8-e025-44cd-8242-4e87eef54f7e.png?jwt=eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.expired";
        assert_eq!(
            normalize_image_url(signed),
            "https://github.com/user-attachments/assets/69f721c8-e025-44cd-8242-4e87eef54f7e"
        );
        // The durable form and other hosts are left exactly as written.
        assert_eq!(
            normalize_image_url(
                "https://github.com/user-attachments/assets/69f721c8-e025-44cd-8242-4e87eef54f7e"
            ),
            "https://github.com/user-attachments/assets/69f721c8-e025-44cd-8242-4e87eef54f7e"
        );
        assert_eq!(
            normalize_image_url("https://user-images.githubusercontent.com/1/2-a.png"),
            "https://user-images.githubusercontent.com/1/2-a.png"
        );
        // A signed path without the asset-id–UUID shape stays untouched rather
        // than guessing at a URL that may not exist.
        assert_eq!(
            normalize_image_url(
                "https://private-user-images.githubusercontent.com/1/2-a.png?jwt=x"
            ),
            "https://private-user-images.githubusercontent.com/1/2-a.png?jwt=x"
        );
    }

    #[test]
    fn parse_blocks_reads_task_list_state() {
        let blocks = parse_blocks("- [ ] open\n- [x] done\n- plain");
        let Block::List(items) = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(items[0].checked, Some(false));
        assert_eq!(items[0].text, "open");
        assert_eq!(items[1].checked, Some(true));
        assert_eq!(items[1].text, "done");
        assert_eq!(items[2].checked, None);
        assert_eq!(items[2].text, "plain");
    }

    #[test]
    fn parse_blocks_reads_table_alignment() {
        let blocks = parse_blocks("| a | b | c |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |");
        let Block::Table { aligns, .. } = &blocks[0] else {
            panic!("expected table");
        };
        assert_eq!(
            aligns,
            &[TableAlign::Left, TableAlign::Center, TableAlign::Right]
        );
    }

    #[test]
    fn parse_blocks_detects_github_alerts() {
        let blocks = parse_blocks("> [!WARNING]\n> This overwrites the file.");
        let Block::Alert(kind, lines) = &blocks[0] else {
            panic!("expected alert");
        };
        assert_eq!(*kind, AlertKind::Warning);
        assert_eq!(lines, &["This overwrites the file.".to_string()]);
        // A plain blockquote stays a quote.
        let quote = parse_blocks("> just a quote");
        assert!(matches!(&quote[0], Block::Quote(_)));
        // An unknown marker is body text, not an alert.
        let unknown = parse_blocks("> [!SOMETHING]\n> x");
        assert!(matches!(&unknown[0], Block::Quote(_)));
    }

    #[test]
    fn code_block_label_prefers_friendly_names() {
        assert_eq!(code_block_label(Some("ts")), "TypeScript");
        assert_eq!(code_block_label(Some("rs")), "Rust");
        // Unknown fence: keep the raw tag rather than dropping context.
        assert_eq!(code_block_label(Some("brainfuck")), "brainfuck");
        // Bare fence: a generic label keeps the header consistent.
        assert_eq!(code_block_label(None), "Plain text");
    }

    #[test]
    fn block_rhythm_separates_headings_more_than_paragraphs() {
        let para = || Block::Paragraph(vec!["x".into()]);
        let heading = || Block::Heading(2, "h".into());
        let list = || Block::List(Vec::new());
        assert!(block_gap(&para(), &heading()) > block_gap(&para(), &para()));
        assert!(block_gap(&heading(), &para()) < block_gap(&para(), &heading()));
        assert!(block_gap(&para(), &list()) < block_gap(&list(), &para()));
    }

    #[test]
    fn inline_runs_pairs_link_ranges_with_urls() {
        let theme = Theme::dark();
        let spans = parse_inline("see [docs](https://pi.dev) now");
        let (body, runs, links, marks) =
            inline_runs(&spans, FontWeight::NORMAL, theme.text_primary, theme);
        assert_eq!(body.as_ref(), "see docs now");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].1, "https://pi.dev");
        assert_eq!(&body[links[0].0.clone()], "docs");
        assert!(marks.is_empty());
        // Underline style rides on the link run.
        assert!(runs.iter().any(|run| run.underline.is_some()));
    }

    #[test]
    fn bare_urls_autolink_and_trim_trailing_punctuation() {
        let spans = parse_inline("see https://pi.dev/docs. and http://a.b/(x) now");
        let links: Vec<&str> = spans.iter().filter_map(|s| s.link.as_deref()).collect();
        assert_eq!(links, vec!["https://pi.dev/docs", "http://a.b/(x)"]);
        let autolinks: Vec<&str> = spans
            .iter()
            .filter(|s| s.autolink)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(autolinks, vec!["https://pi.dev/docs", "http://a.b/(x)"]);
        // A closing bracket with no opener inside the URL is prose.
        let spans = parse_inline("(see https://x.dev/a)");
        assert_eq!(
            spans
                .iter()
                .filter_map(|s| s.link.as_deref())
                .collect::<Vec<_>>(),
            vec!["https://x.dev/a"]
        );
        // Angle brackets stay prose; the URL itself is the link.
        let spans = parse_inline("<https://x.dev>");
        assert_eq!(
            spans
                .iter()
                .filter_map(|s| s.link.as_deref())
                .collect::<Vec<_>>(),
            vec!["https://x.dev"]
        );
        // Markdown links are not autolinks, and code spans stay literal.
        let spans = parse_inline("[docs](https://pi.dev)");
        assert!(spans.iter().all(|s| !s.autolink));
        let spans = parse_inline("`https://pi.dev`");
        assert!(spans.iter().all(|s| s.link.is_none()));
    }

    #[test]
    fn autolink_range_covers_the_rendered_url() {
        let theme = Theme::dark();
        let spans = parse_inline("go https://pi.dev/x now");
        let (body, runs, links, marks) =
            inline_runs(&spans, FontWeight::NORMAL, theme.text_primary, theme);
        assert_eq!(body.as_ref(), "go https://pi.dev/x \u{2197} now");
        assert_eq!(links.len(), 1);
        // The range covers the URL and its external-link mark.
        assert_eq!(&body[links[0].0.clone()], "https://pi.dev/x \u{2197}");
        // The mark is its own run, so the transcript can blank the font
        // glyph and paint the hugeicon arrow over the space.
        assert_eq!(marks.len(), 1);
        assert_eq!(&body[marks[0].clone()], "\u{2197}");
        let hidden = hide_autolink_marks(runs, &marks);
        assert!(hidden.iter().any(|run| run.color.a == 0.0));
    }

    struct AutolinkParagraphView {
        selection: TextSelectionState,
    }

    impl gpui::Render for AutolinkParagraphView {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            self.selection.borrow_mut().begin_frame();
            let _scope = TextScopeGuard::enter(TextScope {
                state: self.selection.clone(),
                message_ix: 0,
            });
            let theme = theme::Theme::for_id(theme::ThemeId::Orbit);
            div().w_full().child(paragraph_text(
                "PR created: https://github.com/imrj05/orbit/pull/36",
                14.,
                26.,
                FontWeight::NORMAL,
                theme.assistant_text,
                ElementId::Name("autolink-paragraph".into()),
                theme,
            ))
        }
    }

    #[gpui::test]
    fn autolink_mark_paints_the_hugeicon_over_its_glyph(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let selection = Rc::new(RefCell::new(TextSelection::new()));
        let view = cx.update(|_, cx| {
            cx.new(|_| AutolinkParagraphView {
                selection: selection.clone(),
            })
        });
        cx.draw(
            point(px(0.), px(0.)),
            gpui::size(px(600.), px(200.)),
            |_, _| view.clone(),
        );
        let state = selection.borrow();
        assert_eq!(state.blocks.len(), 1);
        let block = &state.blocks[0];
        // The mark stays in the copied text and inside the link target...
        let arrow = block.text.find('\u{2197}').expect("mark in the copy text");
        assert!(block.links[0].0.contains(&arrow));
        // ...and keeps a box on the line, which is where the hugeicon draws.
        let start = block.layout.position_for_index(arrow).expect("mark box");
        let end = block
            .layout
            .position_for_index(arrow + '\u{2197}'.len_utf8())
            .expect("mark box end");
        assert!(end.x > start.x, "the mark reserves room for the icon");
    }

    #[test]
    fn followup_gap_is_user_after_assistant() {
        let messages = vec![
            ChatMessage {
                user: true,
                summary: None,
                steps: vec![Step {
                    text: "a".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: false,
                summary: None,
                steps: vec![Step {
                    text: "b".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: true,
                summary: None,
                steps: vec![Step {
                    text: "c".into(),
                    ..Step::default()
                }],
                elapsed: None,
                images: Vec::new(),
                finished_at: None,
                error: None,
                aborted: false,
            },
        ];
        assert!(!starts_followup_turn(&messages, 0));
        assert!(!starts_followup_turn(&messages, 1));
        assert!(starts_followup_turn(&messages, 2));
    }

    #[test]
    fn working_indicator_names_the_live_tool() {
        let bash = ToolCall {
            name: "bash".into(),
            summary: r#"{"command":"cargo test"}"#.into(),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({ "command": "cargo test" })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        let step = Step {
            tools: vec![bash],
            ..Step::default()
        };
        assert_eq!(
            working_activity_label(&step).as_deref(),
            Some("Running cargo test")
        );

        let read = ToolCall {
            name: "read".into(),
            summary: "src/auth.rs".into(),
            path: Some("src/auth.rs".into()),
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({ "path": "src/auth.rs" })),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        let step = Step {
            tools: vec![read],
            ..Step::default()
        };
        assert_eq!(
            working_activity_label(&step).as_deref(),
            Some("Reading src/auth.rs")
        );

        // Thinking-only work reads as thinking; an empty step yields no
        // label so the caller falls back to the generic working form.
        let thinking = Step {
            thinking: "reasoning".into(),
            ..Step::default()
        };
        assert_eq!(
            working_activity_label(&thinking).as_deref(),
            Some("Thinking…")
        );
        // Once the step carries answer text, reasoning is done and the label
        // falls back to the generic working form instead of "Thinking…".
        let answered = Step {
            thinking: "reasoning".into(),
            text: "the answer".into(),
            ..Step::default()
        };
        assert_eq!(working_activity_label(&answered).as_deref(), None);
        assert_eq!(working_activity_label(&Step::default()), None);
    }

    #[test]
    fn thinking_card_settles_when_the_step_moves_on() {
        let thinking = Step {
            thinking: "reasoning".into(),
            ..Step::default()
        };
        // The last step of a live row is the one still reasoning.
        assert!(thinking_is_live(true, true, &thinking));
        // An earlier step is settled even while the row streams.
        assert!(!thinking_is_live(true, false, &thinking));
        // Answer text ends the reasoning phase without waiting for the turn.
        let answered = Step {
            thinking: "reasoning".into(),
            text: "the answer".into(),
            ..Step::default()
        };
        assert!(!thinking_is_live(true, true, &answered));
        // A settled row is never live.
        assert!(!thinking_is_live(false, true, &thinking));
    }

    #[test]
    fn activity_group_live_follows_the_newest_step() {
        let work = |thinking: &str| Step {
            thinking: thinking.into(),
            ..Step::default()
        };
        let answered = Step {
            text: "the answer".into(),
            ..Step::default()
        };
        // A group running to the newest work-only step is live.
        assert!(activity_group_is_live(true, 1, &[work("reasoning")]));
        // Once the newest step holds answer text, the work group settles.
        assert!(!activity_group_is_live(true, 2, &[work("a"), answered]));
        // An earlier group is never live, even while the row streams.
        assert!(!activity_group_is_live(
            true,
            1,
            &[work("first"), work("second")]
        ));
        // A settled row is never live.
        assert!(!activity_group_is_live(false, 1, &[work("reasoning")]));
    }

    #[test]
    fn parse_cache_returns_the_same_blocks_for_the_same_text() {
        let text = "# Title\n\nSome **bold** prose.\n\n- one\n- two\n";
        let first = parse_blocks_cached(text);
        let second = parse_blocks_cached(text);
        // Same content, shared allocation — the streaming hot path must not
        // re-parse an unchanged message every frame.
        assert!(Rc::ptr_eq(&first, &second));
        assert!(matches!(first[0], Block::Heading(1, _)));
        let third = parse_blocks_cached("# Title\n\nchanged\n");
        assert!(!Rc::ptr_eq(&first, &third));
    }

    /// A selection over `texts` with keys `block-0..block-N` — the shape the
    /// panel builds on mouse-down.
    fn selection_over(
        texts: &[&str],
        anchor: (usize, usize),
        focus: (usize, usize),
    ) -> TextSelection {
        let blocks = texts
            .iter()
            .enumerate()
            .map(|(ix, text)| SelectedBlock {
                key: ElementId::Name(format!("block-{ix}").into()),
                text: (*text).to_string().into(),
            })
            .collect();
        let mut state = TextSelection::new();
        state.selection = Some(Selection {
            blocks,
            anchor: SelectPoint {
                block: anchor.0,
                offset: anchor.1,
            },
            focus: SelectPoint {
                block: focus.0,
                offset: focus.1,
            },
        });
        state
    }

    fn test_key(ix: usize) -> ElementId {
        ElementId::Name(format!("block-{ix}").into())
    }

    #[test]
    fn selection_spans_blocks_with_partial_ends() {
        // Cross-paragraph: first block from the anchor offset to its end,
        // middle blocks whole, last block up to the focus offset.
        let state = selection_over(
            &["hello world", "second paragraph", "third"],
            (0, 6),
            (2, 5),
        );
        assert_eq!(
            state.selected_text().as_deref(),
            Some("world\nsecond paragraph\nthird")
        );
        // A backwards drag normalizes to the same text.
        let state = selection_over(
            &["hello world", "second paragraph", "third"],
            (2, 5),
            (0, 6),
        );
        assert_eq!(
            state.selected_text().as_deref(),
            Some("world\nsecond paragraph\nthird")
        );
        // A collapsed selection is nothing to copy.
        let state = selection_over(&["only"], (0, 2), (0, 2));
        assert_eq!(state.selected_text(), None);
    }

    #[test]
    fn selection_ranges_are_per_block() {
        let state = selection_over(
            &["first block", "middle block", "last block"],
            (0, 6),
            (2, 4),
        );
        assert_eq!(state.range_for(&test_key(0), "first block"), Some(6..11));
        assert_eq!(state.range_for(&test_key(1), "middle block"), Some(0..12));
        assert_eq!(state.range_for(&test_key(2), "last block"), Some(0..4));
        // A block outside the selection washes nothing.
        let state = selection_over(&["one", "two"], (1, 1), (1, 2));
        assert_eq!(state.range_for(&test_key(0), "one"), None);
    }

    #[test]
    fn selection_offsets_snap_to_char_boundaries_and_shrinking_text() {
        // Offset 2 sits inside the multi-byte 'é' (bytes 1..3): the start
        // snaps back so the highlighted range is a whole char.
        let state = selection_over(&["héllo"], (0, 2), (0, 3));
        assert_eq!(state.selected_text().as_deref(), Some("é"));
        // Offsets past the current text end (stale mid-stream selection)
        // clamp instead of panicking.
        let state = selection_over(&["short"], (0, 2), (0, 50));
        assert_eq!(state.selected_text().as_deref(), Some("ort"));
    }

    #[test]
    fn highlight_runs_splits_only_the_selected_slice() {
        let font = ui_font();
        let color = Hsla::default();
        let runs = vec![
            code_run(4, Hsla::default(), &font),
            code_run(4, Hsla::default(), &font),
        ];
        let out = highlight_runs(runs, Some(3..6), color);
        let lens: Vec<usize> = out.iter().map(|run| run.len).collect();
        assert_eq!(lens, vec![3, 1, 2, 2]);
        assert_eq!(out[1].background_color, Some(color));
        assert_eq!(out[2].background_color, Some(color));
        assert_eq!(out[3].background_color, None);
        // Without a selection the runs are untouched.
        let out = highlight_runs(vec![code_run(4, Hsla::default(), &font)], None, color);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].background_color, None);
    }

    /// A minimal transcript view: one assistant paragraph and one user
    /// prompt, with fresh shared state for every collaborator.
    fn test_messages() -> Rc<RefCell<Vec<ChatMessage>>> {
        Rc::new(RefCell::new(vec![
            ChatMessage {
                user: false,
                summary: None,
                steps: vec![Step {
                    text: "Alpha bravo charlie delta echo foxtrot golf hotel india juliet \
                           kilo lima mike november oscar papa quebec romeo sierra tango \
                           uniform victor whiskey x-ray yankee zulu."
                        .into(),
                    ..Step::default()
                }],
                images: Vec::new(),
                elapsed: None,
                finished_at: None,
                error: None,
                aborted: false,
            },
            ChatMessage {
                user: true,
                summary: None,
                steps: vec![Step {
                    text: "Second paragraph in a follow-up turn.".into(),
                    ..Step::default()
                }],
                images: Vec::new(),
                elapsed: None,
                finished_at: None,
                error: None,
                aborted: false,
            },
        ]))
    }

    fn test_view(
        state: TextSelectionState,
        messages: Rc<RefCell<Vec<ChatMessage>>>,
        scroller: MessageScrollerState,
    ) -> TranscriptView {
        let user_turns = Rc::new(
            messages
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, message)| message.user)
                .map(|(ix, _)| ix)
                .collect::<Vec<usize>>(),
        );
        TranscriptView {
            scroller,
            messages,
            user_turns,
            text_selection: state,
            streaming: Rc::new(Cell::new(None)),
            stream_started: Rc::new(Cell::new(None)),
            awaiting_reply: Rc::new(Cell::new(false)),
            turn_started: Rc::new(Cell::new(None)),
            expanded_turns: Rc::new(RefCell::new(HashSet::new())),
            expanded_files: Rc::new(RefCell::new(HashSet::new())),
            expanded_activities: Rc::new(RefCell::new(HashMap::new())),
            expanded_tools: Rc::new(RefCell::new(HashSet::new())),
            copied: Rc::new(RefCell::new(HashMap::new())),
            copied_sections: Rc::new(RefCell::new(HashMap::new())),
            expanded_sections: Rc::new(RefCell::new(HashSet::new())),
            expanded_blocks: Rc::new(RefCell::new(HashSet::new())),
            thinking_scrolls: Rc::new(RefCell::new(HashMap::new())),
            collapsed_thoughts: Rc::new(RefCell::new(HashSet::new())),
            thinking_detached: Rc::new(RefCell::new(HashSet::new())),
            hovered_turn: Rc::new(Cell::new(None)),
            hovered_usage: Rc::new(Cell::new(None)),
            rail_hint_dismissed: Rc::new(Cell::new(true)),
            rail_hint_shown_at: Rc::new(Cell::new(None)),
            workspace: None,
            viewport_height: px(600.),
            main_width: px(900.),
            rail_scroll: ScrollHandle::new(),
            rail_autoscroll: Rc::new(Cell::new(None)),
            summary_files: None,
            summary_finished_at: None,
            summary_usage: None,
            review_changes: None,
            image_opener: None,
            search_hits: None,
            search_active: None,
        }
    }

    struct TableTestView {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
        selection: TextSelectionState,
    }

    impl gpui::Render for TableTestView {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            self.selection.borrow_mut().begin_frame();
            let _scope = TextScopeGuard::enter(TextScope {
                state: self.selection.clone(),
                message_ix: 0,
            });
            div().w_full().child(render_table(
                &self.header,
                &self.rows,
                &[],
                0,
                0,
                0,
                theme::Theme::for_id(theme::ThemeId::Orbit),
            ))
        }
    }

    #[gpui::test]
    fn table_cells_keep_readable_wrapping_when_the_pane_narrows(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let selection = Rc::new(RefCell::new(TextSelection::new()));
        let prose =
            "Long prose with **bold** and `inline code` should wrap inside its cell. ".repeat(6);
        let token = "long_unbroken_identifier_".repeat(16);
        let view = cx.update(|_, cx| {
            cx.new(|_| TableTestView {
                header: vec!["Description".into(), "Identifier".into()],
                rows: vec![
                    vec![prose, token.clone()],
                    vec!["Next row".into(), "Short value".into()],
                ],
                selection: selection.clone(),
            })
        });
        let mut narrow_height = None;
        for width in [300., 600.] {
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(width), px(1600.)),
                |_, _| view.clone(),
            );
            let state = selection.borrow();
            assert_eq!(state.blocks.len(), 6);
            let prose = &state.blocks[2];
            let identifier = &state.blocks[3];
            for cell in [prose, identifier] {
                assert!(cell.bounds.left() >= px(0.));
                assert_eq!(
                    cell.bounds.size.width,
                    px(316.),
                    "340 px column minus padding"
                );
                assert!(cell.layout.wrapped_text().lines().count() > 1);
                assert!(cell.bounds.size.height > px(22.));
                assert!(state.blocks[4].bounds.top() >= cell.bounds.bottom());
            }
            assert_eq!(
                identifier.text.as_ref(),
                token.as_str(),
                "wrapping preserves copy text"
            );
            if let Some(height) = narrow_height {
                assert_eq!(
                    prose.bounds.size.height, height,
                    "narrowing the viewport must not squeeze the columns"
                );
            } else {
                narrow_height = Some(prose.bounds.size.height);
            }
        }
    }

    #[gpui::test]
    fn five_prose_columns_overflow_instead_of_squeezing_into_the_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let selection = Rc::new(RefCell::new(TextSelection::new()));
        let view = cx.update(|_, cx| {
            cx.new(|_| TableTestView {
                header: (0..5).map(|ix| format!("Column {ix}")).collect(),
                rows: vec![vec!["A detailed explanation that should remain readable even in a table with many columns. ".repeat(3); 5]],
                selection: selection.clone(),
            })
        });
        for width in [600., 960.] {
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(width), px(800.)),
                |_, _| view.clone(),
            );
            let state = selection.borrow();
            assert_eq!(state.blocks.len(), 10);
            for cell in &state.blocks[5..] {
                assert_eq!(cell.bounds.size.width, px(316.));
            }
            assert!(state.blocks[9].bounds.right() > px(width));
        }
    }

    #[gpui::test]
    fn wide_table_can_scroll_to_its_last_column(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let selection = Rc::new(RefCell::new(TextSelection::new()));
        let view = cx.update(|_, cx| {
            cx.new(|_| TableTestView {
                header: (0..6).map(|ix| format!("Col {ix}")).collect(),
                // Missing cells must not stretch earlier cells out of alignment.
                rows: vec![vec!["Value".into()]],
                selection: selection.clone(),
            })
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(300.), px(300.)),
                |_, _| view.clone(),
            );
        };
        draw(cx);
        let last_before = selection.borrow().blocks[5].bounds;
        assert!(last_before.right() > px(300.));
        assert_eq!(
            selection.borrow().blocks[0].bounds.left(),
            selection.borrow().blocks[6].bounds.left()
        );

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: point(px(150.), px(20.)),
            delta: gpui::ScrollDelta::Pixels(point(px(-1000.), px(0.))),
            ..Default::default()
        });
        draw(cx);
        let state = selection.borrow();
        let last_after = state.blocks[5].bounds;
        assert!(last_after.left() < last_before.left());
        assert!(
            last_after.right() <= px(300.),
            "the last column must be reachable"
        );
    }

    /// The entity wrapper `list()` requires (it reads `window.current_view`).
    struct SelectTestView {
        messages: Rc<RefCell<Vec<ChatMessage>>>,
        state: TextSelectionState,
        scroller: MessageScrollerState,
        main_width: Pixels,
        open_work: bool,
        live: bool,
    }

    impl gpui::Render for SelectTestView {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let mut view = test_view(
                self.state.clone(),
                self.messages.clone(),
                self.scroller.clone(),
            );
            view.main_width = self.main_width;
            if self.open_work {
                view.expanded_turns.borrow_mut().insert(0);
                for tool_ix in 0..self.messages.borrow()[0].tools().count() {
                    view.expanded_tools.borrow_mut().insert((0, tool_ix));
                }
                for step_ix in 0..self.messages.borrow()[0].steps.len() {
                    view.expanded_activities
                        .borrow_mut()
                        .insert((0, step_ix), true);
                }
            }
            if self.live {
                view.streaming.set(Some(0));
            }
            render_transcript(view, cx)
        }
    }

    /// A prompt whose first token has not arrived yet paints the live row at
    /// the tail of the transcript. The regression this guards is the send being
    /// acknowledged by nothing at all until pi produced its first event.
    #[gpui::test]
    fn the_pending_reply_row_paints_before_any_token(cx: &mut gpui::TestAppContext) {
        struct PendingReplyView {
            messages: Rc<RefCell<Vec<ChatMessage>>>,
            state: TextSelectionState,
            scroller: MessageScrollerState,
        }

        impl gpui::Render for PendingReplyView {
            fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
                let view = test_view(
                    self.state.clone(),
                    self.messages.clone(),
                    self.scroller.clone(),
                );
                // The state `submit_as` arms: the prompt is out, the clock runs.
                view.awaiting_reply.set(true);
                view.stream_started.set(Some(Instant::now()));
                render_transcript(view, cx)
            }
        }

        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));
        let state: TextSelectionState = Rc::new(RefCell::new(TextSelection::new()));
        let messages: Rc<RefCell<Vec<ChatMessage>>> = Rc::new(RefCell::new(vec![ChatMessage {
            user: true,
            summary: None,
            steps: vec![Step {
                text: "run the tests".into(),
                ..Step::default()
            }],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        }]));
        // One message row + the pending tail slot, which is what
        // `Transcript::expect_reply` makes the scroller hold.
        let scroller = MessageScrollerState::new(2);
        let view = cx.update(|_, cx| {
            cx.new(|_| PendingReplyView {
                messages,
                state,
                scroller,
            })
        });
        let _ = cx.draw(
            point(px(0.), px(0.)),
            gpui::size(px(900.), px(600.)),
            |_, _| view.clone(),
        );

        let bounds = cx
            .debug_bounds("working-indicator")
            .expect("the live row is painted while the reply is pending");
        assert!(bounds.size.width > px(0.), "the indicator carries a label");
    }

    /// The compaction / branch-summary boundary cards must paint — collapsed
    /// and expanded — with pi's summary content, never a blank row.
    #[gpui::test]
    fn summary_cards_render_collapsed_and_expanded(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));
        let boundary = |branch: bool, summary: &str, tokens: Option<u64>| ChatMessage {
            user: false,
            summary: Some(SummaryRow {
                branch,
                summary: summary.to_string(),
                tokens_before: tokens,
            }),
            steps: vec![Step::default()],
            images: Vec::new(),
            elapsed: None,
            finished_at: None,
            error: None,
            aborted: false,
        };
        let messages: Rc<RefCell<Vec<ChatMessage>>> = Rc::new(RefCell::new(vec![
            boundary(false, "## Goal\n\nShip the fix.", Some(150_700)),
            boundary(true, "Explored approach A.", None),
        ]));
        // Collapsed paints the header alone; `open_work` expands row 0 (the
        // disclosure state summaries share with the turn fold).
        for open in [false, true] {
            let state: TextSelectionState = Rc::new(RefCell::new(TextSelection::new()));
            let scroller = MessageScrollerState::new(messages.borrow().len());
            let view = cx.update(|_, cx| {
                cx.new(|_| SelectTestView {
                    messages: messages.clone(),
                    state,
                    scroller,
                    main_width: px(900.),
                    open_work: open,
                    live: false,
                })
            });
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(900.), px(800.)),
                |_, _| view.clone(),
            );
        }
    }

    #[gpui::test]
    fn tables_fit_their_content_without_filling_the_strip_or_pane(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));
        let state: TextSelectionState = Rc::new(RefCell::new(TextSelection::new()));
        let messages = test_messages();
        let prose = "Normal prose keeps its centered reading width. "
            .repeat(10)
            .trim()
            .to_owned();
        let long_cell =
            "A detailed explanation needs a readable column rather than being squeezed. ".repeat(3);
        messages.borrow_mut()[0].steps[0].text = format!(
            "{prose}\n\n\
             | Repeated group | New component |\n| --- | --- |\n\
             | Gradient, material card, border, window margins | `MiriWindowSurface` |\n\
             | Scroll view and page insets | `MiriPageScrollContainer` |\n\n\
             | One | Two | Three | Four |\n| --- | --- | --- | --- |\n\
             | A | B | C | D |\n\n\
             | Skill | Strengths | Reservations |\n| --- | --- | --- |\n\
             | {long_cell} | {long_cell} | {long_cell} |"
        );
        let scroller = MessageScrollerState::new(messages.borrow().len());
        let view = cx.update(|_, cx| {
            cx.new(|_| SelectTestView {
                messages: messages.clone(),
                state: state.clone(),
                scroller,
                main_width: px(1400.),
                open_work: false,
                live: false,
            })
        });
        for width in [1400., 900., 1800.] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.main_width = px(width);
                    cx.notify();
                })
            });
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(width), px(1600.)),
                |_, _| view.clone(),
            );
            let selection = state.borrow();
            let bounds = |text: &str| {
                selection
                    .blocks
                    .iter()
                    .find(|block| block.text.as_ref() == text)
                    .unwrap()
                    .bounds
            };
            let paragraph = bounds(&prose);
            assert_eq!(paragraph.size.width, px(CONTENT_MAX_WIDTH.min(width - 40.)));
            // Both two-column prose tables and four-column short tables fit.
            // Width depends on content, not merely the number of columns.
            for (first, last) in [("Repeated group", "New component"), ("One", "Four")] {
                assert_eq!(bounds(first).left(), paragraph.left() + px(13.));
                assert!(
                    bounds(last).right() < paragraph.right() - px(100.),
                    "small tables must not stretch"
                );
            }
            // Four short columns need 4 × 96px; three prose columns cap at
            // 3 × 340px. The measured text excludes 12px padding at each end.
            assert_eq!(bounds("Four").right() - bounds("One").left(), px(360.));
            assert_eq!(
                bounds("Reservations").right() - bounds("Skill").left(),
                px(996.)
            );
            if width > CONTENT_MAX_WIDTH + 40. {
                assert!(bounds("Skill").left() < paragraph.left());
                assert!(bounds("Reservations").right() > paragraph.right());
                let table_left = bounds("Skill").left() - px(13.);
                let table_right = bounds("Reservations").right() + px(13.);
                assert_eq!(table_right - table_left, px(1022.));
                assert_eq!(table_left, (px(width) - px(1022.)) / 2.);
            }
        }
    }

    /// Exercise the whole virtualized row, not just an isolated table: the
    /// old row-level width cap hid the breakout even when its cells were wide.
    #[gpui::test]
    fn transcript_table_viewport_breaks_out_of_the_prose_strip(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));
        let state: TextSelectionState = Rc::new(RefCell::new(TextSelection::new()));
        let messages = test_messages();
        let before = "Before the table, prose keeps its reading width. "
            .repeat(8)
            .trim()
            .to_owned();
        let after = "After the table, prose returns to the same reading width. "
            .repeat(8)
            .trim()
            .to_owned();
        let cell =
            "A detailed explanation stays readable instead of being squeezed into a thin column. "
                .repeat(3);
        messages.borrow_mut()[0].steps[0].text = format!(
            "{before}\n\n| Skill | Strengths | Reservations |\n| --- | --- | --- |\n| {cell} | {cell} | {cell} |\n\n{after}"
        );
        let scroller = MessageScrollerState::new(messages.borrow().len());
        let view = cx.update(|_, cx| {
            cx.new(|_| SelectTestView {
                messages: messages.clone(),
                state: state.clone(),
                scroller,
                main_width: px(1400.),
                open_work: false,
                live: false,
            })
        });

        for width in [1400., 800., 1400.] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.main_width = px(width);
                    cx.notify();
                })
            });
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(width), px(1600.)),
                |_, _| view.clone(),
            );
            let selection = state.borrow();
            let before = selection
                .blocks
                .iter()
                .find(|block| block.text.as_ref() == before)
                .unwrap()
                .bounds;
            let after = selection
                .blocks
                .iter()
                .find(|block| block.text.as_ref() == after)
                .unwrap()
                .bounds;
            let first = selection
                .blocks
                .iter()
                .find(|block| block.text.as_ref() == "Skill")
                .unwrap()
                .bounds;
            let last = selection
                .blocks
                .iter()
                .find(|block| block.text.as_ref() == "Reservations")
                .unwrap()
                .bounds;
            assert_eq!(before.size.width, px(CONTENT_MAX_WIDTH.min(width - 40.)));
            assert_eq!(before.left(), after.left());
            assert_eq!(before.size.width, after.size.width);
            assert!(after.top() > last.bottom());
            if width > CONTENT_MAX_WIDTH + 40. {
                assert!(
                    first.left() < before.left(),
                    "table extends into the left margin"
                );
                assert!(
                    last.right() > before.right(),
                    "table extends into the right margin"
                );
                assert!(
                    last.right() < px(width),
                    "all three columns are visible without scrolling"
                );
            } else {
                assert!(
                    first.left() > before.left(),
                    "narrow panes contain the table viewport"
                );
            }
        }
    }

    #[gpui::test]
    fn expanded_activity_stays_inside_the_original_message_column(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));
        let state: TextSelectionState = Rc::new(RefCell::new(TextSelection::new()));
        let messages = test_messages();
        let prose = "A wrapping paragraph before the next activity group. "
            .repeat(10)
            .trim()
            .to_owned();
        let second = "A second paragraph after the first activity group. "
            .repeat(10)
            .trim()
            .to_owned();
        let tool = ToolCall {
            name: "batch_web_fetch".into(),
            summary: "https://example.com/a-long-path/".repeat(12),
            path: None,
            added: 0,
            removed: 0,
            id: None,
            args: Some(serde_json::json!({"url": "https://example.com"})),
            output: None,
            failed: false,
            facts: Default::default(),
        };
        messages.borrow_mut()[0].steps = vec![
            Step { text: prose.clone(), thinking: "First thought\nChecking references".into(), tools: vec![tool.clone()], ..Step::default() },
            Step { text: second.clone(), thinking: "Second thought\nChecking more references".into(), tools: vec![tool], ..Step::default() },
            Step { text: "| A | B | C | D |\n| --- | --- | --- | --- |\n| A wide table | preserves | the normal | activity layout |".into(), ..Step::default() },
        ];
        let scroller = MessageScrollerState::new(messages.borrow().len());
        let view = cx.update(|_, cx| {
            cx.new(|_| SelectTestView {
                messages: messages.clone(),
                state: state.clone(),
                scroller,
                main_width: px(1800.),
                open_work: true,
                live: false,
            })
        });
        for (width, live) in [(1800., false), (1100., false), (800., false), (1800., true)] {
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.main_width = px(width);
                    view.live = live;
                    cx.notify();
                })
            });
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(width), px(1600.)),
                |_, _| view.clone(),
            );
            let column_width = px(CONTENT_MAX_WIDTH.min(width - 40.));
            let column_left = (px(width) - column_width) / 2.;
            for selector in ["activity-group-0-0", "activity-group-0-1"] {
                let bounds = cx
                    .debug_bounds(selector)
                    .expect("expanded activity is visible");
                assert_eq!(
                    bounds.size.width, column_width,
                    "{selector} width at {width}"
                );
                assert_eq!(
                    bounds.left(),
                    column_left,
                    "{selector} alignment at {width}"
                );
            }
            for selector in [
                "thought-card-0-0",
                "thought-card-0-1",
                "tool-card-0-0",
                "tool-card-0-1",
            ] {
                let bounds = cx.debug_bounds(selector).expect("card is visible");
                assert!(bounds.left() >= column_left);
                assert!(
                    bounds.right() <= column_left + column_width + px(6.),
                    "{selector} overflows the message column at {width}: {bounds:?}"
                );
            }
            let selection = state.borrow();
            let first = selection
                .blocks
                .iter()
                .find(|block| block.text.as_ref() == prose)
                .unwrap();
            let next = selection
                .blocks
                .iter()
                .find(|block| block.text.as_ref() == second)
                .unwrap();
            assert_eq!(first.bounds.size.width, column_width);
            assert_eq!(next.bounds.left(), first.bounds.left());
            assert!(
                next.bounds.top() >= first.bounds.bottom() + px(8.),
                "paragraphs must not overlap"
            );
        }
    }

    #[gpui::test]
    fn repeated_tools_with_short_thoughts_do_not_accumulate_bottom_space(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));
        let state: TextSelectionState = Rc::new(RefCell::new(TextSelection::new()));
        let messages = test_messages();
        messages.borrow_mut().truncate(1);
        messages.borrow_mut()[0].steps = vec![Step {
            text: "I will investigate the system appearance behavior.".into(),
            ..Step::default()
        }];
        let scroller = MessageScrollerState::new(1);
        let view = cx.update(|_, cx| {
            cx.new(|_| SelectTestView {
                messages: messages.clone(),
                state: state.clone(),
                scroller: scroller.clone(),
                main_width: px(1400.),
                open_work: false,
                live: true,
            })
        });
        let height = px(1100.);
        for count in 1..=12 {
            if count == 6 {
                // Exercise the same growth with a wide table mixed into the
                // prose. Fixing the gap must not reintroduce zero-width prose
                // during table layout on a narrow pane.
                let cell = "A detailed explanation keeps a readable column. ".repeat(4);
                messages.borrow_mut()[0].steps[0].text.push_str(&format!(
                    "\n\n| Skill | Strengths | Reservations |\n| --- | --- | --- |\n\
                     | {cell} | {cell} | {cell} |\n\nContinuing the investigation."
                ));
            }
            messages.borrow_mut()[0].steps.push(Step {
                thinking: "Adding explicit backdrop imports".into(),
                tools: vec![ToolCall {
                    name: "read".into(),
                    summary: "crates/orbit-pi/src/app/backdrop_layout_tests.rs".into(),
                    path: None,
                    added: 0,
                    removed: 0,
                    id: Some(format!("call-{count}")),
                    args: None,
                    output: None,
                    failed: false,
                    facts: Default::default(),
                }],
                ..Step::default()
            });
            // Match successive tool updates: invalidate the existing row,
            // without manually jumping to the bottom to hide scroll errors.
            scroller.remeasure_items(0..1);
            scroller.note_activity();
            for width in [1400., 800., 1800.] {
                cx.update(|_, cx| {
                    view.update(cx, |view, cx| {
                        view.main_width = px(width);
                        cx.notify();
                    });
                });
                cx.draw(
                    point(px(0.), px(0.)),
                    gpui::size(px(width), height),
                    |_, _| view.clone(),
                );
                let indicator = cx.debug_bounds("working-indicator").unwrap();
                // The row's `pb` is `DynamicSpacing::Base24`; only that final
                // padding belongs below the indicator, never per-row
                // accumulation.
                let row_bottom_pad =
                    DynamicSpacing::Base24.px(&theme::Theme::for_id(theme::ThemeId::Orbit));
                assert_eq!(
                    height - indicator.bottom(),
                    row_bottom_pad,
                    "only the final row padding belongs below the indicator: {count} tools at {width}px"
                );
                if count >= 6 {
                    let selection = state.borrow();
                    let paragraph = selection
                        .blocks
                        .iter()
                        .find(|block| block.text.as_ref() == "Continuing the investigation.")
                        .unwrap();
                    assert_eq!(
                        paragraph.bounds.size.width,
                        px(CONTENT_MAX_WIDTH.min(width - 40.))
                    );
                }
                assert_eq!(scroller.item_count(), 1);
                assert!(scroller.is_following_tail());
            }
        }

        // Settling changes the footer and default activity expansion. Keep
        // the work expanded and ensure its measured height still fits it.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.live = false;
                view.open_work = true;
                cx.notify();
            });
        });
        scroller.remeasure_items(0..1);
        cx.draw(
            point(px(0.), px(0.)),
            gpui::size(px(1800.), height),
            |_, _| view.clone(),
        );
        let last_tool = cx.debug_bounds("tool-card-0-11").unwrap();
        assert!(
            (px(0.)..px(100.)).contains(&(height - last_tool.bottom())),
            "only the footer and row padding should follow the settled work"
        );
    }

    /// The panel's selection plumbing end-to-end: a simulated drag from the
    /// assistant paragraph into the user prompt must produce a cross-block
    /// selection (this caught a panel hitbox that never filled its bounds).
    #[gpui::test]
    fn transcript_drag_selects_across_paragraphs(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit)));
        let state: TextSelectionState = Rc::new(RefCell::new(TextSelection::new()));
        let messages = test_messages();
        let scroller = MessageScrollerState::new(messages.borrow().len());
        let state_in = state.clone();
        let messages_in = messages.clone();
        let scroller_in = scroller.clone();
        let view = cx.update(|_, cx| {
            cx.new(move |_| SelectTestView {
                messages: messages_in,
                state: state_in,
                scroller: scroller_in,
                main_width: px(900.),
                open_work: false,
                live: false,
            })
        });
        // The standalone test window only re-registers the panel's mouse
        // listeners when the entity is painted, so redraw between input
        // events (a real window paints a frame after every event anyway).
        let paint = |cx: &mut gpui::VisualTestContext| {
            let view = view.clone();
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(900.), px(600.)),
                move |_, _| view.clone(),
            );
        };
        paint(cx);

        let blocks = state.borrow();
        assert!(
            blocks.blocks.len() >= 2,
            "expected the rendered paragraphs to register as selectable blocks, got {}",
            blocks.blocks.len()
        );
        let first = blocks.blocks[0].bounds;
        let last = blocks.blocks[blocks.blocks.len() - 1].bounds;
        drop(blocks);

        let start = point(first.left() + px(2.), first.top() + px(8.));
        let end = point(last.left() + px(60.), last.top() + px(8.));
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::none());
        paint(cx);
        cx.simulate_mouse_move(end, Some(MouseButton::Left), gpui::Modifiers::none());
        paint(cx);
        cx.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::none());

        let selected = state.borrow().selected_text();
        assert!(
            selected.is_some(),
            "drag across paragraphs produced no selection"
        );
        let selected = selected.unwrap();
        assert!(
            selected.contains("zulu") && selected.contains("Second"),
            "unexpected selection text: {selected:?}"
        );
    }

    /// Serves a fixed 200×100 PNG for any image request so the markdown
    /// image pipeline (parse → fetch → decode → layout) can run headless,
    /// recording every URI the renderer asked for.
    #[derive(Default)]
    struct PngHttpClient {
        requested: Arc<std::sync::Mutex<Vec<String>>>,
    }

    /// A 200×100 gray PNG, encoded per request so no fixture file is needed.
    fn fixture_png() -> Vec<u8> {
        let pixels = image::RgbaImage::from_pixel(200, 100, image::Rgba([120, 120, 120, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode the fixture png");
        bytes.into_inner()
    }

    impl gpui::http_client::HttpClient for PngHttpClient {
        fn type_name(&self) -> &'static str {
            "PngHttpClient"
        }

        fn user_agent(&self) -> Option<&gpui::http_client::http::HeaderValue> {
            None
        }

        fn proxy(&self) -> Option<&gpui::http_client::Url> {
            None
        }

        fn send(
            &self,
            request: gpui::http_client::Request<gpui::http_client::AsyncBody>,
        ) -> futures::future::BoxFuture<
            'static,
            anyhow::Result<gpui::http_client::Response<gpui::http_client::AsyncBody>>,
        > {
            self.requested
                .lock()
                .expect("request log is not poisoned")
                .push(request.uri().to_string());
            let png = fixture_png();
            Box::pin(async move {
                Ok(gpui::http_client::Response::builder()
                    .status(200)
                    .header("content-type", "image/png")
                    .body(gpui::http_client::AsyncBody::from(png))
                    .expect("build the fixture response"))
            })
        }
    }

    /// A standalone markdown image (the `<img>` tag GitHub stores for a
    /// pasted issue screenshot) must lay out at the reading column's width
    /// with its aspect ratio kept. The element used to force a definite
    /// `height: 0`, from which gpui derives a 0px width — the image decoded
    /// and then painted 0×0, so issue bodies appeared to have no screenshot
    /// at all.
    #[gpui::test]
    fn markdown_image_lays_out_in_the_reading_column(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let requested = Arc::new(std::sync::Mutex::new(Vec::new()));
        cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.set_http_client(Arc::new(PngHttpClient {
                requested: requested.clone(),
            }));
        });

        struct ImageDocView;
        impl gpui::Render for ImageDocView {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                div()
                    .id("image-doc")
                    .w(px(100.))
                    .debug_selector(|| "image-doc".to_string())
                    .child(render_markdown_document(
                        "<img width=\"1450\" height=\"932\" alt=\"Image\" src=\"https://example.com/shot.png\" />",
                        theme::Theme::for_id(theme::ThemeId::Orbit),
                    ))
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| ImageDocView));
        let paint = |cx: &mut gpui::VisualTestContext| {
            let view = view.clone();
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(300.), px(300.)),
                move |_, _| view.clone(),
            );
        };
        paint(cx);
        cx.run_until_parked();
        paint(cx);
        cx.run_until_parked();
        paint(cx);

        assert!(
            requested
                .lock()
                .expect("request log is not poisoned")
                .iter()
                .any(|uri| uri == "https://example.com/shot.png"),
            "the document's image must be fetched"
        );
        let host = cx
            .debug_bounds("image-doc")
            .expect("the document is visible");
        // The 200×100 fixture scales to the 100px column as 100×50; a
        // definite zero height instead collapses the box to 0×0.
        assert_eq!(
            host.size.height,
            px(50.),
            "the image must keep its ratio at column width"
        );
    }

    /// A reply that quotes a pasted screenshot stores a signed
    /// `private-user-images` URL whose JWT expires within minutes, so fetching
    /// it 404s and the reply shows only the alt-text fallback. The renderer
    /// must fetch the durable `user-attachments` URL for the same asset.
    #[gpui::test]
    fn quoted_reply_image_uses_the_durable_asset_url(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        let requested = Arc::new(std::sync::Mutex::new(Vec::new()));
        cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(theme::ThemeId::Orbit));
            cx.set_http_client(Arc::new(PngHttpClient {
                requested: requested.clone(),
            }));
        });

        struct ReplyDocView;
        impl gpui::Render for ReplyDocView {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                div().id("reply-doc").w(px(240.)).child(render_markdown_document(
                    "> ### What happened?\n> <img alt=\"Image\" width=\"1450\" height=\"932\" src=\"https://private-user-images.githubusercontent.com/74659438/656345150-69f721c8-e025-44cd-8242-4e87eef54f7e.png?jwt=eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.expired\">\n> ### Steps to reproduce\n> 1. Open OrbitPI on windows\n",
                    theme::Theme::for_id(theme::ThemeId::Orbit),
                ))
            }
        }

        let view = cx.update(|_, cx| cx.new(|_| ReplyDocView));
        let paint = |cx: &mut gpui::VisualTestContext| {
            let view = view.clone();
            cx.draw(
                point(px(0.), px(0.)),
                gpui::size(px(300.), px(300.)),
                move |_, _| view.clone(),
            );
        };
        paint(cx);
        cx.run_until_parked();
        paint(cx);
        cx.run_until_parked();
        paint(cx);

        let requested = requested.lock().expect("request log is not poisoned");
        assert!(
            requested.iter().any(|uri| {
                uri
                == "https://github.com/user-attachments/assets/69f721c8-e025-44cd-8242-4e87eef54f7e"
            }),
            "the reply must fetch the durable asset URL, got {requested:?}"
        );
        assert!(
            !requested
                .iter()
                .any(|uri| uri.starts_with("https://private-user-images")),
            "the expired signed URL must never be fetched: {requested:?}"
        );
    }
}
