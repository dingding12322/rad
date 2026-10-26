//! Native data tables.
//!
//! GPUI ships every primitive a data grid needs — flex rows, a scroll
//! container, hit-testing, drag — so the tables are built from them directly
//! rather than borrowed from a component layer. The page keeps ownership of
//! the data: it computes the rows and the column plan, the table draws them and
//! reports a header click or a divider drag back to the page.
//!
//! What the table owns: the header (sort caret, click-to-sort, drag-to-resize),
//! the scrollable body, hover/selection styling, and the empty state. What it
//! does not: any metric. Row content arrives pre-built.
//!
//! Only one row style is interactive beyond selection: session rows carry a
//! right-click menu (§43) whose items the page supplies.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, AnyElement, App, CursorStyle, DragMoveEvent, ElementId, Empty, FontWeight,
    Hsla, IntoElement, MouseButton, Pixels, Render, SharedString, Window,
};

use crate::theme::tokens::{DynamicSpacing, IconSize, Radius, TextSize};
use crate::theme::Theme;

/// A body row's height. Every table height is a whole number of these plus the
/// header, so a table never ends by cutting a row in half.
pub(super) const ROW_H: f32 = 40.;
/// The header row's height.
pub(super) const HEADER_H: f32 = 40.;

/// Horizontal cell padding (each side). Matches the search field's 12px inset
/// so the first column lines up with the toolbar above the table card.
pub(super) const CELL_PADDING: f32 = 16.;

/// Width the resize divider's hit area gets.
const HANDLE_W: f32 = 5.;
/// A column never gets narrower than this, however far it is dragged.
pub(super) const MIN_COL_W: f32 = 48.;

/// Which table a column-width override belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TableKind {
    Sessions,
    Buckets,
    Failures,
    Breakdown,
    Series,
}

/// The sort state drawn on a column header. Three states, like a desktop grid:
/// the third click returns to the page's default ordering.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortState {
    Default,
    Ascending,
    Descending,
}

impl SortState {
    /// The state a header click lands on next.
    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Ascending,
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Default,
        }
    }
}

/// One column's presentation. `id` is stable across renders so a resize can be
/// keyed to it; everything else is what the header and cells need.
#[derive(Clone)]
pub struct Column {
    pub id: &'static str,
    pub label: SharedString,
    pub width: f32,
    pub numeric: bool,
    pub sortable: bool,
    pub resizable: bool,
    pub sort: SortState,
}

impl Column {
    pub fn new(id: &'static str, label: impl Into<SharedString>) -> Self {
        Self {
            id,
            label: label.into(),
            width: 0.,
            numeric: false,
            sortable: false,
            resizable: false,
            sort: SortState::Default,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn numeric(mut self) -> Self {
        self.numeric = true;
        self
    }

    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self.resizable = true;
        self
    }

    pub fn sort(mut self, sort: SortState) -> Self {
        self.sort = sort;
        self
    }

    pub fn resizable(mut self) -> Self {
        self.resizable = true;
        self
    }
}

/// The column dividers' drag payload. Carries the column index so a single
/// handler can serve every divider.
#[derive(Clone)]
struct ResizeDrag(usize);

/// The zero-size view the drag system requires; resizing has no preview.
struct DragGhost;
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// A header click: the column index and the state it should move to.
type SortHandler = dyn Fn(usize, SortState, &mut Window, &mut App);
/// A divider drag step: the column index and the pointer's x in window space.
type ResizeHandler = dyn Fn(usize, f32, &mut Window, &mut App);
/// The end of a divider drag.
type ResizeEndHandler = dyn Fn(&mut Window, &mut App);

/// Callbacks a table routes back to the page. Boxed and reference-counted so a
/// single set can be cloned into every header cell.
#[derive(Clone)]
pub struct TableHandlers {
    /// A header was clicked; `SortState` is the state to move to.
    pub sort: Rc<SortHandler>,
    /// A divider drag began; the `f32` is the pointer's x in window space.
    pub resize_start: Rc<ResizeHandler>,
    /// A divider is being dragged; the `f32` is the pointer's current x.
    pub resize_move: Rc<ResizeHandler>,
    /// The divider drag ended.
    pub resize_end: Rc<ResizeEndHandler>,
}

impl TableHandlers {
    pub fn new(
        sort: impl Fn(usize, SortState, &mut Window, &mut App) + 'static,
        resize_start: impl Fn(usize, f32, &mut Window, &mut App) + 'static,
        resize_move: impl Fn(usize, f32, &mut Window, &mut App) + 'static,
        resize_end: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            sort: Rc::new(sort),
            resize_start: Rc::new(resize_start),
            resize_move: Rc::new(resize_move),
            resize_end: Rc::new(resize_end),
        }
    }
}

/// A data table: a rounded, hairline-framed board with a fixed header over a
/// body. Search, columns, and pagination sit outside this frame.
///
/// The whole table scrolls horizontally once a column has been dragged past
/// the width it was given. The body only scrolls vertically when the frame is
/// shorter than its rows — otherwise the page is the scroller, so a wheel
/// over the table still moves the page.
pub fn data_table(
    id: &'static str,
    columns: &[Column],
    rows: Vec<AnyElement>,
    height: Pixels,
    empty: AnyElement,
    theme: Theme,
    handlers: TableHandlers,
) -> AnyElement {
    let total_w: f32 = columns.iter().map(|column| column.width).sum();
    let content_h = px(HEADER_H + ROW_H * rows.len().max(1) as f32);
    let scroll_y = !rows.is_empty() && height < content_h;
    let frame_h = if rows.is_empty() || scroll_y {
        height
    } else {
        content_h
    };

    let mut header = div()
        .flex_none()
        .h(px(HEADER_H))
        .min_w(px(total_w))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(theme.border);
    for (ix, column) in columns.iter().enumerate() {
        header = header.child(header_cell(ix, column, theme, handlers.clone()));
    }

    let body = if rows.is_empty() {
        div()
            .flex_1()
            .min_h_0()
            .min_w(px(total_w))
            .flex()
            .items_center()
            .justify_center()
            .child(empty)
            .into_any_element()
    } else if scroll_y {
        div()
            .id(ElementId::Name(SharedString::from(format!("{id}-body"))))
            .flex_1()
            .min_h_0()
            .min_w(px(total_w))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    } else {
        div()
            .id(ElementId::Name(SharedString::from(format!("{id}-body"))))
            .flex_none()
            .min_w(px(total_w))
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    };

    div()
        .w_full()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(
            div()
                .id(ElementId::Name(SharedString::from(id)))
                .w_full()
                .h(frame_h)
                .overflow_x_scroll()
                .child(
                    div()
                        .h_full()
                        .min_w(px(total_w))
                        .flex()
                        .flex_col()
                        .child(header)
                        .child(body),
                ),
        )
        .into_any_element()
}

/// One header cell: label, sort caret, and (for a resizable column) the divider.
///
/// The caret lives in the cell's right padding rather than in the flow, so a
/// numeric column's label still right-aligns with the numbers beneath it, and
/// the label can use the full content width before it truncates.
fn header_cell(ix: usize, column: &Column, theme: Theme, handlers: TableHandlers) -> AnyElement {
    let sort_next = column.sort.next();
    let sort = handlers.sort.clone();
    let resize_start = handlers.resize_start.clone();
    let resize_move = handlers.resize_move.clone();
    let resize_end = handlers.resize_end.clone();
    // Sorted columns keep the same ink as the rest of the header; the caret
    // is what marks the active key.
    let mut content = div()
        .h_full()
        .w(px(column.width))
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .px(px(CELL_PADDING))
        .when(column.numeric, |cell| cell.justify_end())
        .when(column.sortable, |cell| {
            cell.cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    sort(ix, sort_next, window, cx);
                })
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(TextSize::Default.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(column.label.clone()),
        );

    if column.sortable {
        content = content.child(sort_caret(column.sort, theme));
    }

    if column.resizable {
        content = content.child(
            div()
                .id(ElementId::Name(SharedString::from(format!(
                    "usage-col-handle-{}",
                    column.id
                ))))
                .occlude()
                .absolute()
                .top_0()
                .right(px(-(HANDLE_W / 2.)))
                .h_full()
                .w(px(HANDLE_W))
                .flex()
                .justify_center()
                .cursor(CursorStyle::ResizeLeftRight)
                .child(
                    div()
                        .h_full()
                        .w(px(1.))
                        .bg(theme.border.opacity(0.))
                        .hover(|style| style.bg(theme.border_strong)),
                )
                .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    // A divider drag is not a sort click.
                    cx.stop_propagation();
                    resize_start(ix, f32::from(event.position.x), window, cx);
                })
                .on_drag(ResizeDrag(ix), |_, _, _, cx| cx.new(|_| DragGhost))
                .on_drag_move(move |event: &DragMoveEvent<ResizeDrag>, window, cx| {
                    let ix = match event.drag(cx) {
                        ResizeDrag(ix) => *ix,
                    };
                    resize_move(ix, f32::from(event.event.position.x), window, cx);
                })
                .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                    resize_end(window, cx);
                }),
        );
    }

    content.into_any_element()
}

/// The sort caret: both directions on the active column (↑↓), nothing on the
/// others — a grid that draws carets everywhere reads as noise.
fn sort_caret(sort: SortState, theme: Theme) -> AnyElement {
    let visible = sort != SortState::Default;
    let color = if visible {
        theme.text_secondary
    } else {
        theme.text_muted
    };
    div()
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .when(!visible, |caret| caret.opacity(0.))
        .child(crate::app::icon(
            "icons/chevron-up.svg",
            IconSize::Indicator.px(&theme),
            color,
        ))
        .child(crate::app::icon(
            "icons/chevron-down.svg",
            IconSize::Indicator.px(&theme),
            color,
        ))
        .into_any_element()
}

/// A body cell's shell: fixed width, padded, numeric columns right-aligned.
/// The caller supplies the content, already clipped to the width.
pub(super) fn cell_shell(column: &Column) -> gpui::Div {
    div()
        .h_full()
        .w(px(column.width))
        .flex_none()
        .flex()
        .items_center()
        .px(px(CELL_PADDING))
        .when(column.numeric, |cell| cell.justify_end())
}

/// A plain text cell: clipped to the column, tabular figures for numeric
/// columns, the column's tertiary/primary register otherwise.
pub(super) fn text_cell(column: &Column, text: String, color: Hsla, theme: Theme) -> AnyElement {
    let text = clip_to(
        &text,
        column.width,
        CELL_PADDING * 2.,
        theme.ui_px(13.).into(),
        0.52,
    );
    if column.numeric {
        return cell_shell(column)
            .child(
                div()
                    .font(super::view::num_font())
                    .whitespace_nowrap()
                    .text_size(TextSize::Default.px(&theme))
                    .text_color(color)
                    .child(text),
            )
            .into_any_element();
    }
    cell_shell(column)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(TextSize::Default.px(&theme))
                .text_color(color)
                .child(text),
        )
        .into_any_element()
}

/// The empty state, centered in the table body.
pub(super) fn empty_cell(text: &str, theme: Theme) -> AnyElement {
    div()
        .py(DynamicSpacing::Base24.px(&theme))
        .flex()
        .justify_center()
        .text_size(TextSize::Default.px(&theme))
        .text_color(theme.text_muted)
        .child(text.to_string())
        .into_any_element()
}

/// Clip a string to what its column can actually show, on a rough
/// average-glyph factor.
///
/// GPUI's own text ellipsis needs a definite width at shaping time; inside a
/// table cell the width arrives from the layout pass, so a cell that overflows
/// is cut mid-glyph instead of ending in "…". Budgeting the characters
/// ourselves keeps every clipped cell legible, and the element's own
/// `truncate()` remains as the backstop.
pub(super) fn clip_to(text: &str, width: f32, chrome: f32, font_size: f32, factor: f32) -> String {
    let usable = (width - chrome).max(24.);
    let budget = (usable / (font_size * factor)).floor().max(3.) as usize;
    if text.chars().count() <= budget {
        return text.to_string();
    }
    let mut out: String = text.chars().take(budget.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Sort key for the failures table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FailureSort {
    When,
    Kind,
    Model,
    Session,
    Message,
}

impl FailureSort {
    pub fn id(self) -> &'static str {
        match self {
            Self::When => "When",
            Self::Kind => "Type",
            Self::Model => "Model",
            Self::Session => "Session",
            Self::Message => "message",
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::When => tr!("usage.col_when"),
            Self::Kind => tr!("usage.col_type"),
            Self::Model => tr!("usage.col_model"),
            Self::Session => tr!("usage.col_session"),
            Self::Message => tr!("usage.col_message"),
        }
    }

    /// Hideable columns; When is always shown.
    pub const HIDEABLE: [Self; 4] = [Self::Kind, Self::Model, Self::Session, Self::Message];
}

/// One row of the failures table, already resolved against the index so the
/// cells do no lookups while painting.
#[derive(Clone, PartialEq, Debug)]
pub struct FailureRow {
    pub ts_ms: i64,
    pub session: u16,
    pub kind: super::model::ErrorKind,
    pub model: String,
    pub session_title: String,
    pub message: String,
}

/// The failure-kind label and its register: provider errors are critical, tool
/// failures a warning.
pub(super) fn failure_kind_register(kind: super::model::ErrorKind, theme: Theme) -> (String, Hsla) {
    match kind {
        super::model::ErrorKind::Provider => (kind.label(), theme.error),
        super::model::ErrorKind::Tool => (kind.label(), theme.warning),
    }
}

/// Format a failure timestamp for its column.
pub(super) fn failure_when(ts_ms: i64) -> String {
    super::model::local_datetime(ts_ms)
        .map(|dt| dt.format("%b %-d, %H:%M").to_string())
        .unwrap_or_default()
}
