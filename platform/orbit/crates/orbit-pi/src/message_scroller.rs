//! Conversation scroller — an original, plain-GPUI implementation built on
//! [`gpui::ListState`] (no component library).
//!
//! Provides tail following, append/prepend/splice, item remeasure,
//! scroll-to-row, and a jump-to-latest control when the reader leaves the
//! live edge.
//!
//! GPUI 0.2.2 has no `FollowMode::Tail`; [`ListAlignment::Bottom`] plus a
//! past-the-end `scroll_to` is the equivalent: the last row's bottom stays
//! against the viewport while it grows.

use std::{cell::Cell, ops::Range, rc::Rc};

use gpui::{
    div, linear_color_stop, linear_gradient, prelude::*, px, rems, svg, ElementId, ListAlignment,
    ListOffset, ListScrollEvent, ListState, Pixels,
};

use crate::app::{button_frame, icon_button_frame, TipExt, BUTTON_GROUP};
use crate::theme::tokens::{ButtonSize, StyledExt};
use crate::theme::Theme;

const LIST_OVERDRAW: f32 = 400.0;

/// Entity-owned scrolling state for the transcript list.
#[derive(Clone)]
pub struct MessageScrollerState {
    list: ListState,
    following_tail: Rc<Cell<bool>>,
    /// First row currently visible in the viewport — the "reader is here"
    /// hint that drives the navigation rail's active tick.
    visible_start: Rc<Cell<usize>>,
    /// New content arrived while the reader was away from the live edge —
    /// the jump-to-latest control says so until they return.
    unread: Rc<Cell<bool>>,
}

impl MessageScrollerState {
    /// Create state for `item_count` rows and enable tail following.
    pub fn new(item_count: usize) -> Self {
        let list = ListState::new(item_count, ListAlignment::Bottom, px(LIST_OVERDRAW));
        let following_tail = Rc::new(Cell::new(true));
        let visible_start = Rc::new(Cell::new(0));
        {
            let following_tail = following_tail.clone();
            let visible_start = visible_start.clone();
            list.set_scroll_handler(move |event: &ListScrollEvent, _, cx| {
                // Bottom alignment: `is_scrolled` means the reader left the tail.
                following_tail.set(!event.is_scrolled);
                visible_start.set(event.visible_range.start);
                cx.refresh_windows();
            });
        }
        Self {
            list,
            following_tail,
            visible_start,
            unread: Rc::new(Cell::new(false)),
        }
    }

    /// Record that content changed. When the reader is away from the live
    /// edge this flags the jump-to-latest control; while following, it's a
    /// no-op (the content is already in view).
    pub fn note_activity(&self) {
        if !self.is_following_tail() {
            self.unread.set(true);
        }
    }

    /// True when content changed while the reader was scrolled up.
    pub fn has_unread(&self) -> bool {
        self.unread.get()
    }

    /// First row currently in view (viewport-top hint for the rail).
    ///
    /// Kept current by the wheel handler and by every programmatic scroll
    /// ([`Self::scroll_to_item`], [`Self::scroll_by`], [`Self::scroll_to_end`],
    /// [`Self::reset`]); gpui only fires the scroll handler for wheel and
    /// scrollbar gestures, so those paths must refresh it by hand or the rail
    /// keeps highlighting the turn the reader left behind.
    pub fn first_visible_index(&self) -> usize {
        self.visible_start.get()
    }

    /// Point the reader hint at `ix` without waiting for the list's scroll
    /// handler — programmatic scrolls never invoke it.
    fn note_visible_start(&self, ix: usize) {
        self.visible_start
            .set(ix.min(self.item_count().saturating_sub(1)));
    }

    pub fn list_state(&self) -> ListState {
        self.list.clone()
    }

    pub fn item_count(&self) -> usize {
        self.list.item_count()
    }

    /// True when the reader has left the live edge and there is room to scroll.
    pub fn is_scrolled_up(&self) -> bool {
        self.list.max_offset_for_scrollbar().height > px(0.) && !self.is_following_tail()
    }

    /// True when the transcript holds more content than the viewport shows
    /// (drives the navigation-rail visibility).
    pub fn is_scrollable(&self) -> bool {
        self.list.max_offset_for_scrollbar().height > px(0.)
    }

    pub fn is_following_tail(&self) -> bool {
        self.following_tail.get()
    }

    /// Reset to `item_count` rows and resume tail following.
    pub fn reset(&self, item_count: usize) {
        self.unread.set(false);
        self.list.reset(item_count);
        self.scroll_to_end();
    }

    /// Replace `old_range` with `count` new rows.
    ///
    /// Returns `false` when the range is outside the current list.
    pub fn splice(&self, old_range: Range<usize>, count: usize) -> bool {
        if !self.valid_range(&old_range) {
            return false;
        }
        self.list.splice(old_range, count);
        self.stick_if_following();
        true
    }

    /// Append `count` rows to the end of the list.
    pub fn append(&self, count: usize) -> bool {
        let item_count = self.item_count();
        self.splice(item_count..item_count, count)
    }

    /// Prepend `count` rows while preserving the current scroll anchor.
    #[allow(dead_code)]
    pub fn prepend(&self, count: usize) -> bool {
        self.splice(0..0, count)
    }

    /// Mark rows in `range` for remeasurement (streaming growth).
    pub fn remeasure_items(&self, range: Range<usize>) -> bool {
        if !self.valid_range(&range) || range.start == range.end {
            return false;
        }
        // These are the same rows, not replacements. GPUI's splice clears
        // the intra-row offset when the anchor lies in the invalidated range;
        // streaming a tall answer must not move the reader on every delta.
        let anchor = (!self.is_following_tail()).then(|| self.list.logical_scroll_top());
        let count = range.end - range.start;
        self.list.splice(range, count);
        if let Some(anchor) = anchor {
            self.list.scroll_to(anchor);
        } else {
            self.stick_if_following();
        }
        true
    }

    /// Remeasure one row whose height changed from an expand/collapse, keeping
    /// the transcript exactly where the reader left it.
    ///
    /// While tail-following, a plain remeasure re-pins the list to the bottom
    /// and yanks the growing row out of view. Freeze the live edge into an
    /// explicit item anchor first (which also leaves tail-following, so the
    /// view stops chasing the bottom).
    ///
    /// Either way the anchor is re-applied after the splice: gpui's `splice`
    /// zeroes the offset when the changed row *is* the anchor, which otherwise
    /// snaps that row's top to the viewport top (the "it jumps when I open a
    /// card in the message I'm reading" case). Restoring the captured pair
    /// leaves the row under the reader fixed and lets the card open in place.
    pub fn remeasure_toggle(&self, ix: usize) -> bool {
        if ix >= self.item_count() {
            return false;
        }
        // Only freeze an anchor when there is content to scroll. A transcript
        // shorter than the viewport is bottom-pinned; materializing an anchor
        // there would snap the whole run to the top.
        let anchor = if self.is_following_tail() {
            if self.is_scrollable() {
                // `logical_scroll_top` reads as past-the-end while following;
                // move back one viewport to turn the visible top item into the
                // anchor.
                let viewport = self.list.viewport_bounds().size.height;
                self.list.scroll_by(-viewport);
                let anchor = self.list.logical_scroll_top();
                if anchor.item_ix < self.item_count() {
                    self.following_tail.set(false);
                    // Programmatic scrolls skip the scroll handler; keep the
                    // rail's "reader is here" hint on the frozen top row.
                    self.visible_start.set(anchor.item_ix);
                    Some(anchor)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            // Already anchored by the reader's own scroll.
            Some(self.list.logical_scroll_top())
        };

        self.list.splice(ix..ix + 1, 1);
        if let Some(anchor) = anchor {
            self.list.scroll_to(anchor);
        } else {
            self.stick_if_following();
        }
        true
    }

    /// Scroll to the row at `index`, if it exists. Leaves tail following.
    pub fn scroll_to_item(&self, index: usize) -> bool {
        if index >= self.item_count() {
            return false;
        }
        self.following_tail.set(false);
        self.list.scroll_to(ListOffset {
            item_ix: index,
            offset_in_item: px(0.),
        });
        // A programmatic scroll skips the list's handler; without this the
        // rail (and repeated ⌘↑/⌘↓ jumps) would read a stale viewport top.
        self.note_visible_start(index);
        true
    }

    /// Resume tail following and scroll to the latest row.
    pub fn scroll_to_end(&self) {
        self.unread.set(false);
        self.following_tail.set(true);
        self.list.scroll_to(ListOffset {
            item_ix: self.item_count(),
            offset_in_item: px(0.),
        });
        // The live edge is the newest content; a follow-up jump uses the
        // reader hint, so keep it pointing at the tail rather than a stale row.
        self.note_visible_start(self.item_count());
    }

    /// Scroll the transcript by `distance` pixels — positive moves toward the
    /// live edge, negative back through history. Used to chain a wheel event
    /// from a nested scroll area (the "Thought" card) once it bottoms out.
    pub fn scroll_by(&self, distance: Pixels) {
        if distance == px(0.) {
            return;
        }
        // At the tail GPUI's logical offset is past-the-end, a full viewport
        // below the actual scroll top. Clamp that sentinel to the real pixel
        // range before applying a chained wheel delta; otherwise small upward
        // gestures over a Thought card get clamped straight back to the tail.
        let max = self.list.max_offset_for_scrollbar().height;
        let current = (-self.list.scroll_px_offset_for_scrollbar().y).clamp(px(0.), max);
        let target = (current + distance).clamp(px(0.), max);
        if target == max {
            self.scroll_to_end();
        } else {
            self.following_tail.set(false);
            self.list
                .set_offset_from_scrollbar(gpui::point(px(0.), -target));
            // Programmatic scrolling does not invoke the list's scroll handler.
            self.note_visible_start(self.list.logical_scroll_top().item_ix);
        }
    }

    fn stick_if_following(&self) {
        if self.is_following_tail() {
            self.scroll_to_end();
        }
    }

    fn valid_range(&self, range: &Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.item_count()
    }
}

/// Viewport + jump-to-latest chrome around a virtualized `list()`.
pub fn render_scroller(
    state: MessageScrollerState,
    theme: Theme,
    list: impl IntoElement,
) -> impl IntoElement {
    let scrolled_up = state.is_scrolled_up();
    div()
        .id(ElementId::Name("message-scroller".into()))
        .relative()
        .w_full()
        .min_w_0()
        .h_full()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .id(ElementId::Name("message-scroller-viewport".into()))
                .w_full()
                .min_w_0()
                .h_full()
                .min_h_0()
                .child(list),
        )
        .when(scrolled_up, |shell| {
            shell
                .child(render_bottom_fade(theme))
                .child(render_jump_button(state, theme))
        })
}

fn render_bottom_fade(theme: Theme) -> impl IntoElement {
    div()
        .id(ElementId::Name("message-scroller-fade".into()))
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(rems(3.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme.background.opacity(0.), 0.),
            linear_color_stop(theme.background, 1.),
        ))
}

fn render_jump_button(state: MessageScrollerState, theme: Theme) -> impl IntoElement {
    // Content arrived while the reader was scrolled up: the round jump
    // affordance grows a "New activity" label so the stream's continued
    // progress is visible from anywhere in the transcript.
    let unread = state.has_unread();
    let button = div()
        .id(ElementId::Name("message-scroller-jump".into()))
        .group(BUTTON_GROUP);
    let button = if unread {
        button_frame(button, &theme, ButtonSize::Large)
    } else {
        icon_button_frame(button, &theme, ButtonSize::Large)
    };
    div()
        .id(ElementId::Name("message-scroller-jump-layer".into()))
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(8.))
        .w_full()
        .flex()
        .justify_center()
        .child(
            // Floating round scroll-to-bottom affordance: the elevated
            // surface, kept round and on the composer's fill.
            button
                .elevation_2(&theme)
                .rounded_full()
                .tip(tr!("message_scroller.jump_to_latest"))
                .bg(theme.surface_secondary)
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_elevated))
                .on_click(move |_, _, cx| {
                    state.scroll_to_end();
                    cx.refresh_windows();
                })
                .child(
                    svg()
                        .path("icons/arrow-down.svg")
                        .flex_none()
                        .size(ButtonSize::Large.icon_size().px(&theme))
                        .text_color(theme.text_primary),
                )
                .when(unread, |button| {
                    button.child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("message_scroller.new_activity")),
                    )
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    type RowHeights = Rc<std::cell::RefCell<std::collections::HashMap<usize, f32>>>;

    /// A bottom-aligned transcript standing in for real message rows: each row
    /// is 40 px unless `heights` gives it a taller one, which is how an opened
    /// thinking/tool card changes a row's size.
    struct TranscriptProbe {
        state: MessageScrollerState,
        heights: RowHeights,
    }

    impl gpui::Render for TranscriptProbe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let heights = self.heights.clone();
            gpui::list(self.state.list_state(), move |ix, _, _| {
                let height = heights.borrow().get(&ix).copied().unwrap_or(40.);
                gpui::div()
                    .id(gpui::ElementId::NamedInteger("probe-row".into(), ix as u64))
                    .debug_selector(move || format!("probe-row-{ix}"))
                    .w_full()
                    .h(px(height))
                    .child(format!("row {ix}"))
                    .into_any_element()
            })
            .size_full()
        }
    }

    /// Open a probe at a 600×400 window and return its first painted frame.
    fn draw_probe(
        cx: &mut gpui::VisualTestContext,
        probe: &gpui::Entity<TranscriptProbe>,
    ) -> gpui::Size<gpui::Pixels> {
        let space = gpui::size(px(600.), px(400.));
        let origin = gpui::point(px(0.), px(0.));
        let _ = cx.draw(origin, space, |_, _| {
            gpui::div().size_full().child(probe.clone())
        });
        space
    }

    /// Expanding a card while tail-following must not re-pin the list to the
    /// bottom: the row under the reader stays put and the card grows below it.
    #[gpui::test]
    fn remeasure_toggle_keeps_the_reader_anchored(cx: &mut gpui::TestAppContext) {
        let state = MessageScrollerState::new(40);
        let heights: RowHeights =
            Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
        let cx = cx.add_empty_window();
        let probe = cx.update(|_, cx| {
            cx.new(|_| TranscriptProbe {
                state: state.clone(),
                heights: heights.clone(),
            })
        });
        draw_probe(cx, &probe);
        let before = cx.debug_bounds("probe-row-30").expect("row 30 visible");
        assert!(state.is_following_tail(), "starts at the live edge");

        heights.borrow_mut().insert(38, 500.);
        state.remeasure_toggle(38);
        draw_probe(cx, &probe);

        assert!(
            !state.is_following_tail(),
            "toggling leaves tail-following so the view holds still"
        );
        let after = cx
            .debug_bounds("probe-row-30")
            .expect("row 30 still visible");
        assert_eq!(
            after.top(),
            before.top(),
            "the row under the reader did not move"
        );
    }

    /// The failing shape: the changed row is itself the row pinned to the top
    /// of the viewport. gpui's splice zeroes that row's scroll offset, so the
    /// anchor has to be re-applied or the row's top snaps to the viewport top.
    #[gpui::test]
    fn remeasure_toggle_holds_a_pinned_row_that_is_itself_toggled(cx: &mut gpui::TestAppContext) {
        let state = MessageScrollerState::new(40);
        let heights: RowHeights =
            Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
        // A tall row 29 sits under the viewport top (offset 420), with rows
        // 30..39 (30 px each) beneath it so the anchor holds while it grows.
        heights.borrow_mut().insert(29, 520.);
        for ix in 30..40 {
            heights.borrow_mut().insert(ix, 30.);
        }
        let cx = cx.add_empty_window();
        let probe = cx.update(|_, cx| {
            cx.new(|_| TranscriptProbe {
                state: state.clone(),
                heights: heights.clone(),
            })
        });
        draw_probe(cx, &probe);
        let before = cx
            .debug_bounds("probe-row-29")
            .expect("row 29 is the pinned row");
        assert!(before.top() < px(0.), "row 29 is scrolled under the top");

        heights.borrow_mut().insert(29, 700.);
        state.remeasure_toggle(29);
        draw_probe(cx, &probe);

        let after = cx.debug_bounds("probe-row-29").expect("row 29 visible");
        assert_eq!(
            after.top(),
            before.top(),
            "toggling the pinned row keeps its offset instead of snapping to the top"
        );
    }

    /// A run shorter than the viewport stays bottom-pinned through a toggle —
    /// freezing an anchor would snap it to the top of the panel.
    #[gpui::test]
    fn remeasure_toggle_keeps_a_short_run_bottom_pinned(cx: &mut gpui::TestAppContext) {
        let state = MessageScrollerState::new(5);
        let heights: RowHeights =
            Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
        let cx = cx.add_empty_window();
        let probe = cx.update(|_, cx| {
            cx.new(|_| TranscriptProbe {
                state: state.clone(),
                heights: heights.clone(),
            })
        });
        draw_probe(cx, &probe);
        let before = cx.debug_bounds("probe-row-4").expect("row 4 visible");

        heights.borrow_mut().insert(4, 200.);
        state.remeasure_toggle(4);
        draw_probe(cx, &probe);

        assert!(
            state.is_following_tail(),
            "a run that still fits stays at the live edge"
        );
        let after = cx.debug_bounds("probe-row-4").expect("row 4 visible");
        assert_eq!(
            after.bottom(),
            before.bottom(),
            "the run stays pinned to the bottom of the panel"
        );
    }

    #[gpui::test]
    fn streaming_preserves_the_readers_offset_inside_the_live_row(cx: &mut gpui::TestAppContext) {
        let state = MessageScrollerState::new(3);
        let heights: RowHeights =
            Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
        heights.borrow_mut().insert(2, 1200.);
        let cx = cx.add_empty_window();
        let probe = cx.update(|_, cx| {
            cx.new(|_| TranscriptProbe {
                state: state.clone(),
                heights: heights.clone(),
            })
        });
        draw_probe(cx, &probe);
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(px(300.), px(200.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(150.))),
            ..Default::default()
        });
        draw_probe(cx, &probe);
        assert!(!state.is_following_tail());
        let before = state.list_state().logical_scroll_top();
        assert_eq!(before.item_ix, 2);
        assert!(before.offset_in_item > px(0.));
        let top = cx.debug_bounds("probe-row-2").unwrap().top();

        for height in [1300., 1450., 1600.] {
            heights.borrow_mut().insert(2, height);
            assert!(state.remeasure_items(2..3));
            state.note_activity();
            draw_probe(cx, &probe);
            assert_eq!(
                state.list_state().logical_scroll_top().offset_in_item,
                before.offset_in_item
            );
            assert_eq!(cx.debug_bounds("probe-row-2").unwrap().top(), top);
            assert!(!state.is_following_tail());
            assert!(state.has_unread());
        }

        state.scroll_to_end();
        heights.borrow_mut().insert(2, 1800.);
        state.remeasure_items(2..3);
        draw_probe(cx, &probe);
        assert!(state.is_following_tail());
        assert!(!state.has_unread());
        assert_eq!(cx.debug_bounds("probe-row-2").unwrap().bottom(), px(400.));
    }

    #[gpui::test]
    fn streaming_and_appending_below_history_keep_the_visible_row_fixed(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = MessageScrollerState::new(40);
        let heights: RowHeights =
            Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
        let cx = cx.add_empty_window();
        let probe = cx.update(|_, cx| {
            cx.new(|_| TranscriptProbe {
                state: state.clone(),
                heights: heights.clone(),
            })
        });
        draw_probe(cx, &probe);
        state.scroll_to_item(20);
        draw_probe(cx, &probe);
        let before = cx.debug_bounds("probe-row-20").unwrap();

        heights.borrow_mut().insert(39, 1200.);
        state.remeasure_items(39..40);
        state.append(1);
        state.note_activity();
        draw_probe(cx, &probe);

        assert_eq!(cx.debug_bounds("probe-row-20").unwrap().top(), before.top());
        assert!(!state.is_following_tail());
        assert!(state.has_unread());
    }

    #[gpui::test]
    fn chained_scroll_leaves_the_tail_and_resumes_following_at_the_bottom(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = MessageScrollerState::new(3);
        let heights: RowHeights =
            Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
        heights.borrow_mut().insert(2, 1200.);
        let cx = cx.add_empty_window();
        let probe = cx.update(|_, cx| {
            cx.new(|_| TranscriptProbe {
                state: state.clone(),
                heights: heights.clone(),
            })
        });
        draw_probe(cx, &probe);
        let bottom = cx.debug_bounds("probe-row-2").unwrap();

        // A small residual from a nested Thought card must move immediately,
        // not require a single gesture larger than the viewport.
        state.scroll_by(px(-20.));
        draw_probe(cx, &probe);
        assert!(!state.is_following_tail());
        assert_eq!(
            cx.debug_bounds("probe-row-2").unwrap().top(),
            bottom.top() + px(20.)
        );
        assert_eq!(state.first_visible_index(), 2);
        state.note_activity();

        state.scroll_by(px(20.));
        draw_probe(cx, &probe);
        assert!(state.is_following_tail());
        assert!(!state.has_unread());
        assert_eq!(cx.debug_bounds("probe-row-2").unwrap().top(), bottom.top());
    }

    #[test]
    fn programmatic_scroll_updates_the_viewport_hint() {
        let state = MessageScrollerState::new(40);
        assert_eq!(state.first_visible_index(), 0);
        // A rail tick / ⌘↑ jump moves the reader hint without a wheel event.
        assert!(state.scroll_to_item(12));
        assert!(!state.is_following_tail());
        assert_eq!(state.first_visible_index(), 12);
        // A second jump from the same state must advance the hint, so
        // repeated keyboard jumps keep walking instead of re-reading row 12.
        assert!(state.scroll_to_item(30));
        assert_eq!(state.first_visible_index(), 30);
        // Resuming the tail repoints the hint at the newest content.
        state.scroll_to_end();
        assert!(state.is_following_tail());
        assert_eq!(state.first_visible_index(), 39);
        // A refused scroll must not move the hint.
        assert!(!state.scroll_to_item(40));
        assert_eq!(state.first_visible_index(), 39);
    }

    #[test]
    fn state_starts_following_tail() {
        let state = MessageScrollerState::new(3);
        assert_eq!(state.item_count(), 3);
        assert!(state.is_following_tail());
        assert!(!state.is_scrolled_up());
    }

    #[test]
    fn append_prepend_splice_and_reset() {
        let state = MessageScrollerState::new(3);
        assert!(state.append(2));
        assert_eq!(state.item_count(), 5);
        assert!(state.prepend(1));
        assert_eq!(state.item_count(), 6);
        assert!(!state.splice(5..7, 0));
        assert!(state.remeasure_items(0..6));
        assert!(!state.remeasure_items(6..7));
        assert!(state.scroll_to_item(2));
        assert!(!state.is_following_tail());
        assert!(!state.scroll_to_item(6));
        state.scroll_to_end();
        assert!(state.is_following_tail());
        state.reset(2);
        assert_eq!(state.item_count(), 2);
        assert!(state.is_following_tail());
    }
}
