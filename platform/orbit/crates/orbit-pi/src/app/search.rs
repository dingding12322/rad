//! In-transcript find (⌘F): a small floating bar that matches message text,
//! counts hits, and jumps between them. Matching is message-level — the
//! transcript's virtualized rows are washed where they match and the selected
//! hit is highlighted, so the reader can see where they are without a
//! document-wide render pass.

use super::*;
use crate::app::{icon_button_frame, picker_search_frame, press, BUTTON_GROUP};
use crate::theme::tokens::{input, ButtonSize, StyledExt, TextSize};

/// State for the open find bar. Dropping this closes the surface.
pub(super) struct TranscriptSearch {
    /// The find field; focus lives here while the bar is open.
    pub input: Entity<ComposerInput>,
    /// Query the current `matches` were computed for (trimmed).
    pub query: String,
    /// Message indices containing `query`, in append order.
    pub matches: Vec<usize>,
    /// Index into `matches` of the selected hit.
    pub index: usize,
    /// Message count when `matches` was computed, so new arrivals refresh it.
    pub matched_count: usize,
}

impl OrbitApp {
    pub(super) fn on_toggle_search(
        &mut self,
        _: &crate::ToggleSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transcript_search.is_some() {
            self.close_search(cx);
        } else {
            self.open_search(window, cx);
        }
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("search.find_in_transcript")
                // `Composer` keeps the editing keys live; `Search` adds the
                // find-specific Enter/Escape bindings (registered after the
                // composer ones, so they win while the bar is focused).
                .with_key_context("Composer Search")
                .with_element_id("transcript-search-input")
                .with_max_lines(1)
                .with_wrap(false)
        });
        let handle = input.read(cx).focus_handle(cx);
        self.transcript_search = Some(TranscriptSearch {
            input,
            query: String::new(),
            matches: Vec::new(),
            index: 0,
            matched_count: self.transcript.message_count(),
        });
        window.focus(&handle);
        cx.notify();
    }

    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) {
        if self.transcript_search.take().is_some() {
            cx.notify();
        }
    }

    /// Recompute hits when the query or the message count changed. Called from
    /// the heartbeat; a no-op while the bar is closed or nothing moved.
    pub(super) fn sync_transcript_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.transcript_search.as_ref() else {
            return;
        };
        let query = search.input.read(cx).text().trim().to_string();
        let count = self.transcript.message_count();
        if query == search.query && count == search.matched_count {
            return;
        }
        let matches = if query.is_empty() {
            Vec::new()
        } else {
            self.transcript.find_messages(&query.to_lowercase())
        };
        if let Some(search) = self.transcript_search.as_mut() {
            search.query = query;
            search.matches = matches;
            search.matched_count = count;
            // Clamp the selection so it always points at a live hit.
            search.index = search.index.min(search.matches.len().saturating_sub(1));
        }
        self.scroll_to_search_hit(cx);
    }

    pub(super) fn on_search_next(
        &mut self,
        _: &crate::SearchNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_search(true, cx);
    }

    pub(super) fn on_search_prev(
        &mut self,
        _: &crate::SearchPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_search(false, cx);
    }

    pub(super) fn on_search_close(
        &mut self,
        _: &crate::SearchClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_search(cx);
        // Hand focus back to the composer so typing resumes immediately.
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle);
    }

    /// Matching message indices for the current query, for the transcript's
    /// row wash. `None` when find is closed or the query is empty.
    pub(super) fn search_hits(&self) -> Option<HashSet<usize>> {
        let search = self.transcript_search.as_ref()?;
        if search.query.is_empty() {
            return None;
        }
        Some(search.matches.iter().copied().collect())
    }

    /// The selected hit, for the stronger active-row wash.
    pub(super) fn search_active(&self) -> Option<usize> {
        let search = self.transcript_search.as_ref()?;
        search.matches.get(search.index).copied()
    }

    fn step_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(search) = self.transcript_search.as_mut() else {
            return;
        };
        let n = search.matches.len();
        if n == 0 {
            return;
        }
        search.index = if forward {
            (search.index + 1) % n
        } else {
            (search.index + n - 1) % n
        };
        self.scroll_to_search_hit(cx);
    }

    /// Scroll the selected hit into view and repaint its wash.
    fn scroll_to_search_hit(&mut self, cx: &mut Context<Self>) {
        let Some(search) = self.transcript_search.as_ref() else {
            return;
        };
        if let Some(&ix) = search.matches.get(search.index) {
            self.transcript.scroll_to_message(ix);
        }
        cx.notify();
    }

    /// The floating find bar, anchored top-right inside the transcript area.
    /// `None` while find is closed.
    pub(super) fn transcript_search_bar(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let search = self.transcript_search.as_ref()?;
        let theme = *theme::get(cx);
        let count = search.matches.len();
        let label = if search.query.is_empty() {
            tr!("search.type_to_find")
        } else if count == 0 {
            tr!("search.no_matches")
        } else {
            format!("{} of {}", search.index + 1, count)
        };
        let nav_enabled = count > 0;
        Some(
            // A text field floating over the transcript, in the shared picker
            // search row; `elevation_2` after it takes over the fill, radius,
            // and hairline.
            picker_search_frame(
                div()
                    .id("transcript-search")
                    .debug_selector(|| "transcript-search".to_string())
                    .absolute()
                    .top(px(10.))
                    .right(px(16.))
                    .w(px(320.))
                    .occlude(),
                &theme,
            )
            .elevation_2(&theme)
            .font_family(theme::ui_font_family())
            .key_context("Search")
            .on_action(cx.listener(Self::on_search_next))
            .on_action(cx.listener(Self::on_search_prev))
            .on_action(cx.listener(Self::on_search_close))
            .child(icon(
                "icons/search.svg",
                input::ICON.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().child(search.input.clone()))
            .child(
                div()
                    .flex_none()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(label),
            )
            .child(search_nav_button(
                "search-prev",
                "icons/chevron-up.svg",
                tr!("search.previous_match"),
                !nav_enabled,
                theme,
                cx.listener(|this, _, _, cx| this.step_search(false, cx)),
            ))
            .child(search_nav_button(
                "search-next",
                "icons/chevron-down.svg",
                tr!("search.next_match"),
                !nav_enabled,
                theme,
                cx.listener(|this, _, _, cx| this.step_search(true, cx)),
            ))
            .child(
                icon_button_frame(div().id("search-close-btn"), &theme, ButtonSize::Compact)
                    .tip(tr!("common.close"))
                    .cursor_pointer()
                    .text_color(theme.text_secondary)
                    .hover(|s| s.bg(theme.surface_hover).text_color(theme.text_primary))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.on_search_close(&crate::SearchClose, window, cx)
                        }),
                    )
                    .child(icon(
                        "icons/x.svg",
                        ButtonSize::Compact.icon_size().px(&theme),
                        theme.text_secondary,
                    )),
            )
            .into_any_element(),
        )
    }
}

fn search_nav_button(
    id: &'static str,
    glyph: &'static str,
    label: String,
    disabled: bool,
    theme: Theme,
    listener: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    icon_button_frame(div().id(id), &theme, ButtonSize::Compact)
        .group(BUTTON_GROUP)
        .tip(label)
        .when(!disabled, |button| {
            press(button)
                .cursor_pointer()
                .hover(|s| s.bg(theme.surface_hover))
                .on_mouse_up(MouseButton::Left, listener)
        })
        .child(icon(
            glyph,
            ButtonSize::Compact.icon_size().px(&theme),
            if disabled {
                theme.text_disabled
            } else {
                theme.text_secondary
            },
        ))
        .into_any_element()
}
