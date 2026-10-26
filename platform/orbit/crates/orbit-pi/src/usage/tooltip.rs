//! A small native tooltip.
//!
//! GPUI owns the hover plumbing (`InteractiveElement::tooltip`); this is just
//! the view it builds — one line of text in the app's own popover register, so
//! a tooltip never arrives wearing another product's chrome. Its metrics are
//! Zed's `tooltip_container` (see [`crate::theme::tokens::tooltip`]).

use gpui::{div, prelude::*, Context, IntoElement, Render, SharedString, Window};

use crate::theme;
use crate::theme::tokens::{tooltip, StyledExt};

/// A single-line tooltip.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        // The outer padding keeps the card off the cursor, as Zed's does.
        div()
            .pl(tooltip::offset_x(&theme))
            .pt(tooltip::offset_y(&theme))
            .child(
                div()
                    .debug_selector(|| "tooltip-card".into())
                    .elevation_2(&theme)
                    .font_family(theme::ui_font_family())
                    .text_size(tooltip::TEXT.px(&theme))
                    .text_color(theme.text_primary)
                    .py(tooltip::padding_y(&theme))
                    .px(tooltip::padding_x(&theme))
                    .child(
                        div()
                            .max_w(tooltip::max_width(&theme))
                            .child(self.text.clone()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, AvailableSpace, TestAppContext};

    use super::*;
    use crate::theme::{Theme, ThemeId};

    /// Lay a tooltip out the way GPUI does (`prepaint_tooltip` uses
    /// `AvailableSpace::min_size()`) and return its card bounds.
    fn card_for(text: &'static str, cx: &mut TestAppContext) -> gpui::Bounds<gpui::Pixels> {
        cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
        let cx = cx.add_empty_window();
        let _ = cx.draw(
            point(px(0.), px(0.)),
            AvailableSpace::min_size(),
            |_, cx| cx.new(|_| Tooltip::new(text)),
        );
        cx.debug_bounds("tooltip-card")
            .expect("tooltip card laid out")
    }

    /// A short label stays on one line at its own width — min-content layout
    /// must not wrap it word by word.
    #[gpui::test]
    fn a_short_tooltip_is_one_line(cx: &mut TestAppContext) {
        let card = card_for("Toggle sidebar (⌘B)", cx);
        let one_line = px(40.);
        assert!(card.size.height < one_line, "wrapped: {card:?}");
        assert!(card.size.width < px(288.), "not content-sized: {card:?}");
        assert!(card.size.width > px(80.), "collapsed: {card:?}");
    }

    /// A long label wraps at Zed's 288px title width (plus the card's padding
    /// and hairline) instead of running off the screen on one line.
    #[gpui::test]
    fn a_long_tooltip_wraps_at_zeds_width(cx: &mut TestAppContext) {
        let card = card_for(
            "Removes the session file from disk after asking once, and cannot be undone \
             from inside Orbit because pi keeps no trash for its own session store",
            cx,
        );
        let widest = px(288. + 2. * 8. + 2.);
        assert!(card.size.width <= widest, "overflowed: {card:?}");
        assert!(card.size.width > px(200.), "wrapped word by word: {card:?}");
        assert!(card.size.height > px(40.), "never wrapped: {card:?}");
    }
}
