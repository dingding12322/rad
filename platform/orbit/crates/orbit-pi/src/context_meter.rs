//! Context-window indicator: a circular used-ring, compact hover copy, and
//! a click-to-open breakdown carrying the manual-compaction action
//! (`compact`). Numbers come from pi `get_session_stats`; conversation vs
//! "other" is a chars/4 estimate over the loaded transcript (the same
//! heuristic pi's own /context view uses). Categories the protocol does not
//! expose (rules, MCP, skills, …) are not invented.

use std::rc::Rc;

use gpui::{
    canvas, deferred, div, point, prelude::*, px, relative, svg, AnyElement, App, Background,
    Bounds, ClickEvent, Context, Entity, FontWeight, Hsla, IntoElement, MouseDownEvent,
    PathBuilder, Pixels, SharedString, Window,
};
use orbit_rpc::{ContextUsage, SessionUsage};

use crate::app::{button_frame, icon_button_frame, TipExt};
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{ButtonSize, DynamicSpacing, IconSize, Radius, StyledExt, TextSize};
use crate::theme::Theme;

const RING: f32 = 16.;
const RING_BOX: f32 = 22.;
const STROKE: f32 = 2.;
/// Gap between the ring and the bottom edge of the hover/details card.
const POPUP_GAP: f32 = 8.;
/// Hover card width. The overlay lives in the 22px ring box; without an
/// explicit width Taffy clamps it to that hit target and the copy wraps
/// one character per line.
const HOVER_W: f32 = 200.;
const PANEL_W: f32 = 280.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContextPopup {
    None,
    Hover,
    Details,
}

/// A click handler for the details panel's action button.
type ActionClick = dyn Fn(&ClickEvent, &mut Window, &mut App);

/// The entity-bound form [`context_control`] relays the action through.
type EntityAction<V> = dyn Fn(&mut V, &mut Window, &mut Context<V>);

pub struct ContextSlice {
    pub label: SharedString,
    pub tokens: u64,
    pub color: Hsla,
}

/// Fill color for the ring / "In use" slice, keyed off the used fraction
/// (0..=1) so the color always matches the drawn ring even when pi reports
/// `tokens` without an explicit `percent`.
pub fn fill_color(fraction: Option<f32>, theme: &Theme) -> Hsla {
    match fraction {
        Some(f) if f >= 0.90 => theme.error,
        Some(f) if f >= 0.70 => theme.warning,
        _ => theme.accent,
    }
}

pub fn format_percent(usage: &ContextUsage) -> Option<String> {
    let percent = usage.percent?;
    let rounded = percent.round().clamp(0.0, 100.0);
    if usage.tokens.unwrap_or(0) > 0 && rounded < 1.0 {
        Some("<1%".into())
    } else {
        Some(format!("{rounded:.0}%"))
    }
}

/// One-decimal label for a cache hit rate (`99.9`, `68.4`, `100`). A rate
/// that merely *rounds* up to 100 (e.g. 99.96%) is withheld at `99.9`, since
/// claiming a perfect hit rate would be false — the model still processed
/// uncached prompt tokens. Only an exact 100 prints as `100`.
pub fn hit_percent_label(percent: f32) -> String {
    if !percent.is_finite() {
        return "0".into();
    }
    let mut value = (percent.clamp(0.0, 100.0) * 10.0).round() / 10.0;
    if percent < 100.0 && value >= 100.0 {
        value = 99.9;
    }
    let text = format!("{value:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_string()
}

/// Compact token label (`151K`, `1.0K`, `688`) matching the reference UI.
pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let whole = n / 1_000_000;
        let tenths = ((n % 1_000_000) + 50_000) / 100_000;
        if tenths == 0 {
            format!("{whole}M")
        } else if tenths == 10 {
            format!("{}M", whole + 1)
        } else {
            format!("{whole}.{tenths}M")
        }
    } else if n >= 1_000 {
        let whole = n / 1_000;
        let tenths = ((n % 1_000) + 50) / 100;
        if tenths == 0 {
            format!("{whole}K")
        } else if tenths == 10 {
            format!("{}K", whole + 1)
        } else {
            format!("{whole}.{tenths}K")
        }
    } else {
        n.to_string()
    }
}

/// Split current window usage into conversation vs everything else.
///
/// `conversation_est` is the chars/4 heuristic over loaded messages. It is
/// clamped to the pi-reported used total so the slices never exceed 100%.
pub fn usage_slices(
    usage: &ContextUsage,
    conversation_est: u64,
    theme: &Theme,
) -> Vec<ContextSlice> {
    let Some(used) = usage.tokens else {
        return Vec::new();
    };
    if used == 0 {
        return Vec::new();
    }
    let conversation = conversation_est.min(used);
    let other = used - conversation;
    let mut out = Vec::new();
    if other > 0 {
        let label = if conversation == 0 {
            tr!("context_meter.in_use")
        } else {
            tr!("context_meter.other_context")
        };
        out.push(ContextSlice {
            label: label.into(),
            tokens: other,
            color: if conversation == 0 {
                fill_color(usage.fraction(), theme)
            } else {
                theme.accent.opacity(0.55)
            },
        });
    }
    if conversation > 0 {
        out.push(ContextSlice {
            label: tr!("context_meter.conversation").into(),
            tokens: conversation,
            color: theme.accent,
        });
    }
    out
}

/// Circular used-ring. `fraction` is 0..=1 of the context window.
pub fn context_ring(fraction: f32, fill: Hsla, track: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            paint_ring(window, bounds, fraction, fill.to_rgb(), track.to_rgb());
        },
    )
    .size(px(RING))
}

fn paint_ring(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    fraction: f32,
    fill: gpui::Rgba,
    track: gpui::Rgba,
) {
    let cx = bounds.origin.x + bounds.size.width / 2.;
    let cy = bounds.origin.y + bounds.size.height / 2.;
    let stroke = px(STROKE);
    let radius = (bounds.size.width.min(bounds.size.height) / 2.) - stroke / 2.;
    if radius <= px(0.) {
        return;
    }

    if let Ok(path) = circle_stroke(cx, cy, radius, stroke) {
        window.paint_path(path, Background::from(track));
    }

    let fraction = fraction.clamp(0.0, 1.0);
    if fraction < 0.008 {
        return;
    }
    let path = if fraction >= 0.997 {
        circle_stroke(cx, cy, radius, stroke)
    } else {
        arc_stroke(cx, cy, radius, fraction, stroke)
    };
    if let Ok(path) = path {
        window.paint_path(path, Background::from(fill));
    }
}

fn circle_stroke(
    cx: Pixels,
    cy: Pixels,
    radius: Pixels,
    stroke: Pixels,
) -> anyhow::Result<gpui::Path<Pixels>> {
    // Two semicircles: a single 360° SVG arc is degenerate (start == end).
    let mut builder = PathBuilder::stroke(stroke);
    builder.move_to(point(cx, cy - radius));
    builder.arc_to(
        point(radius, radius),
        px(0.),
        false,
        true,
        point(cx, cy + radius),
    );
    builder.arc_to(
        point(radius, radius),
        px(0.),
        false,
        true,
        point(cx, cy - radius),
    );
    builder.build()
}

fn arc_stroke(
    cx: Pixels,
    cy: Pixels,
    radius: Pixels,
    fraction: f32,
    stroke: Pixels,
) -> anyhow::Result<gpui::Path<Pixels>> {
    // 12 o'clock, clockwise. `sweep = true` is SVG clockwise.
    let start = point(cx, cy - radius);
    let angle = fraction * std::f32::consts::TAU;
    let end = point(cx + radius * angle.sin(), cy - radius * angle.cos());
    let mut builder = PathBuilder::stroke(stroke);
    builder.move_to(start);
    builder.arc_to(point(radius, radius), px(0.), fraction > 0.5, true, end);
    builder.build()
}

/// Compact hover card: "59% context used" / "151K / 256K tokens" and, when
/// pi reports a cost, the running session spend.
pub fn compact_card(
    usage: Option<&ContextUsage>,
    session: Option<&SessionUsage>,
    theme: Theme,
) -> impl IntoElement {
    let (title, subtitle) = match usage {
        Some(usage) => {
            let title = format_percent(usage)
                .map(|p| tr!("context_meter.context_used", percent = p))
                .unwrap_or_else(|| tr!("context_meter.context_usage"));
            let subtitle = match usage.tokens {
                Some(used) => tr!(
                    "context_meter.tokens_of",
                    used = format_tokens(used),
                    total = format_tokens(usage.context_window)
                ),
                None => tr!(
                    "context_meter.of_tokens",
                    total = format_tokens(usage.context_window)
                ),
            };
            (title, subtitle)
        }
        None => (
            tr!("context_meter.context_usage"),
            tr!("context_meter.waiting_for_agent"),
        ),
    };
    let cost = session.and_then(|s| s.cost).map(format_cost);
    div()
        .w_full()
        .px(DynamicSpacing::Base12.px(&theme))
        .py(DynamicSpacing::Base08.px(&theme))
        .elevation_2(&theme)
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(TextSize::Default.px(&theme))
                .text_color(theme.text_primary)
                .whitespace_nowrap()
                .child(title),
        )
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .whitespace_nowrap()
                .child(subtitle),
        )
        .children(cost.map(|cost| {
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .whitespace_nowrap()
                .child(tr!("context_meter.spent", cost = cost))
        }))
}

/// Money label for a session cost in USD. Sub-cent amounts read `<$0.01`
/// rather than rounding to a misleading `$0.00`.
fn format_cost(cost: f64) -> String {
    if !cost.is_finite() || cost <= 0.0 {
        "$0.00".into()
    } else if cost < 0.01 {
        "<$0.01".into()
    } else if cost < 100.0 {
        format!("${cost:.2}")
    } else {
        format!("${cost:.0}")
    }
}

/// Full click panel: percent, token total, segmented bar, per-slice legend,
/// and — when an active session can compact — the manual-compaction action.
#[allow(clippy::too_many_arguments)]
pub fn details_card(
    usage: Option<&ContextUsage>,
    session: Option<&SessionUsage>,
    slices: &[ContextSlice],
    is_compacting: bool,
    on_compact: Option<Rc<ActionClick>>,
    theme: Theme,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_outside: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let percent = usage.and_then(format_percent);
    let totals = usage.map(|u| match u.tokens {
        Some(used) => tr!(
            "context_meter.tokens_of",
            used = format_tokens(used),
            total = format_tokens(u.context_window)
        ),
        None => tr!(
            "context_meter.of_tokens",
            total = format_tokens(u.context_window)
        ),
    });
    let window_tokens = usage.map(|u| u.context_window).unwrap_or(0);

    div()
        .w(px(PANEL_W))
        .elevation_2(&theme)
        .occlude()
        .on_mouse_down_out(on_outside)
        .flex()
        .flex_col()
        .px(px(14.))
        .pt(DynamicSpacing::Base12.px(&theme))
        .pb(DynamicSpacing::Base12.px(&theme))
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_size(TextSize::Default.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(tr!("context_meter.context_usage")),
                )
                .child(
                    icon_button_frame(div().id("context-close"), &theme, ButtonSize::Compact)
                        .tip(tr!("common.close"))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.surface_hover))
                        .on_click(on_close)
                        .child(glyph(
                            "icons/x.svg",
                            ButtonSize::Compact.icon_size().px(&theme),
                            theme.text_muted,
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_size(TextSize::Default.px(&theme))
                        .text_color(theme.text_primary)
                        .child(
                            percent
                                .map(|p| tr!("context_meter.percent_full", percent = p))
                                .unwrap_or_else(|| tr!("context_meter.unknown")),
                        ),
                )
                .children(totals.map(|label| {
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(label)
                })),
        )
        .child(segmented_bar(slices, window_tokens, theme))
        .child(legend(slices, theme))
        .children(session.map(|session| session_usage_section(session, theme)))
        .children(on_compact.map(|on_compact| compact_action(is_compacting, on_compact, theme)))
}

/// The details panel's one action: compact this session's context now. The
/// label names the in-flight state while pi works, so the control is never
/// a second click on a run that is already compacting.
fn compact_action(is_compacting: bool, on_compact: Rc<ActionClick>, theme: Theme) -> AnyElement {
    let button = button_frame(div().id("context-compact-now"), &theme, ButtonSize::Medium)
        .w_full()
        .border_1()
        .border_color(theme.border)
        .raised(theme.surface_elevated, &theme)
        .font_weight(FontWeight::MEDIUM);
    if is_compacting {
        button
            .text_color(theme.text_muted)
            .cursor_default()
            .child(tr!("context_meter.compacting"))
            .into_any_element()
    } else {
        button
            .text_color(theme.text_secondary)
            .cursor_pointer()
            .hover(|s| s.raised(theme.surface_hover, &theme))
            .on_click(move |event, window, cx| on_compact(event, window, cx))
            .child(tr!("context_meter.compact_now"))
            .into_any_element()
    }
}

fn segmented_bar(slices: &[ContextSlice], window: u64, theme: Theme) -> impl IntoElement + use<> {
    let mut bar = div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .overflow_hidden()
        .bg(theme.surface_secondary)
        .flex();
    if window > 0 {
        for slice in slices {
            let fraction = (slice.tokens as f32 / window as f32).clamp(0.0, 1.0);
            if fraction <= 0.0 {
                continue;
            }
            let color = slice.color;
            bar = bar.child(div().h_full().w(relative(fraction)).bg(color));
        }
    }
    bar
}

fn legend(slices: &[ContextSlice], theme: Theme) -> impl IntoElement + use<> {
    if slices.is_empty() {
        return div()
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(tr!("context_meter.no_usage_reported_for_this_session_yet"))
            .into_any_element();
    }
    div()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base06.px(&theme))
        .children(slices.iter().map(|slice| {
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .size(px(8.))
                        .rounded(Radius::Full.px(&theme))
                        .bg(slice.color),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(slice.label.clone()),
                )
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_primary)
                        .child(format_tokens(slice.tokens)),
                )
        }))
        .into_any_element()
}

/// Full-session token/cost totals: a hairline divider, a section label, then
/// one full-width label/value row per metric. A single column (rather than a
/// two-up grid) keeps every value right-aligned and lets the cache-read
/// hit-rate suffix (`66.7M · 89%`) render without colliding with the next
/// column.
pub fn session_usage_section(usage: &SessionUsage, theme: Theme) -> AnyElement {
    let cost = usage.cost.map(format_cost).unwrap_or_else(|| "—".into());
    div()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base06.px(&theme))
        .child(div().w_full().h(px(1.)).bg(theme.border))
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("context_meter.session_usage")),
        )
        .child(session_row(
            "icons/usage-input.svg",
            tr!("context_meter.input"),
            format_tokens(usage.input),
            theme,
        ))
        .child(session_row(
            "icons/usage-output.svg",
            tr!("context_meter.output"),
            format_tokens(usage.output),
            theme,
        ))
        .child(session_row(
            "icons/cache-read.svg",
            tr!("context_meter.cache_read"),
            cache_read_label(usage),
            theme,
        ))
        .child(session_row(
            "icons/cache-write.svg",
            tr!("context_meter.cache_write"),
            format_tokens(usage.cache_write),
            theme,
        ))
        .child(session_row(
            "icons/usage-total.svg",
            tr!("context_meter.total"),
            format_tokens(usage.total),
            theme,
        ))
        .child(session_row(
            "icons/usage-cost.svg",
            tr!("context_meter.cost"),
            cost,
            theme,
        ))
        .into_any_element()
}

/// `40.0K · 89%` for the session cache-read row: token count plus the
/// provider cache hit rate over prompt tokens (`None` keeps just the count).
fn cache_read_label(usage: &SessionUsage) -> String {
    let tokens = format_tokens(usage.cache_read);
    match usage.cache_read_percent() {
        Some(percent) => tr!(
            "context_meter.cache_read_share",
            tokens = tokens,
            percent = hit_percent_label(percent)
        ),
        None => tokens,
    }
}

/// One full-width label/value row: a muted metric icon, the label, then the
/// value pinned right. The label flexes so the value edge stays aligned
/// regardless of label or value length.
fn session_row(
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
        .gap(px(7.))
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

/// Inline stroke icon at a fixed size, tinted with `currentColor`.
fn glyph(path: &'static str, size: Pixels, color: Hsla) -> impl IntoElement {
    svg().path(path).flex_none().size(size).text_color(color)
}

/// Sit the card fully above the ring, right-aligned with it.
///
/// `bottom: 100%` pins the card's bottom edge to the ring's top. `right: 0`
/// keeps it on the meter instead of drifting left. Width is required: this
/// overlay's containing block is the 22px ring, so auto-width collapses.
fn popup_above_ring(width: f32, child: impl IntoElement) -> AnyElement {
    div()
        .absolute()
        .bottom_full()
        .right_0()
        .mb(px(POPUP_GAP))
        .w(px(width))
        .child(deferred(child))
        .into_any_element()
}

/// Data the context meter renders, all sourced from one `get_session_stats`
/// response plus the local transcript estimate.
pub struct ContextMeterData<'a> {
    /// Point-in-time context-window snapshot.
    pub usage: Option<&'a ContextUsage>,
    /// Cumulative session token/cost totals.
    pub session: Option<&'a SessionUsage>,
    /// chars/4 estimate over the loaded transcript.
    pub conversation_est: u64,
}

/// Ring chip + optional hover/details popover, anchored above the control.
#[allow(clippy::too_many_arguments)]
pub fn context_control<V: 'static>(
    data: ContextMeterData<'_>,
    popup: ContextPopup,
    entity: &Entity<V>,
    theme: Theme,
    on_hover: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
    on_click: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    is_compacting: bool,
    on_compact: Option<Rc<EntityAction<V>>>,
    on_close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> impl IntoElement {
    let usage = data.usage;
    let session = data.session;
    let fraction = usage.and_then(ContextUsage::fraction).unwrap_or(0.0);
    let fill = fill_color(Some(fraction), &theme);
    let slices = usage
        .map(|u| usage_slices(u, data.conversation_est, &theme))
        .unwrap_or_default();
    let this = entity.clone();
    let this_click = entity.clone();
    let this_close = entity.clone();
    let this_outside = entity.clone();
    let on_close = Rc::new(on_close);

    let popup_el: Option<AnyElement> = match popup {
        ContextPopup::None => None,
        ContextPopup::Hover => Some(popup_above_ring(
            HOVER_W,
            compact_card(usage, session, theme),
        )),
        ContextPopup::Details => {
            let close = this_close.clone();
            let outside = this_outside;
            let on_close_btn = on_close.clone();
            let on_close_out = on_close.clone();
            let on_compact = on_compact.map(|compact| {
                let this = entity.clone();
                Rc::new(
                    move |_event: &ClickEvent, window: &mut Window, cx: &mut App| {
                        this.update(cx, |app, cx| {
                            (compact.as_ref())(app, window, cx);
                        });
                    },
                ) as Rc<ActionClick>
            });
            Some(popup_above_ring(
                PANEL_W,
                details_card(
                    usage,
                    session,
                    &slices,
                    is_compacting,
                    on_compact,
                    theme,
                    move |_, window, cx| {
                        close.update(cx, |app, cx| {
                            (on_close_btn.as_ref())(app, window, cx);
                        });
                    },
                    move |_, window, cx| {
                        outside.update(cx, |app, cx| {
                            (on_close_out.as_ref())(app, window, cx);
                        });
                    },
                ),
            ))
        }
    };

    let percent = usage.and_then(format_percent);

    // Percent + ring share one hit target. The overlay stays on the 22px
    // ring box so it still opens above the meter, not the label.
    div()
        .id("context-meter")
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .cursor_pointer()
        .on_hover(move |hovered, _, cx| {
            this.update(cx, |app, cx| on_hover(app, *hovered, cx));
        })
        .on_mouse_up(gpui::MouseButton::Left, move |_, window, cx| {
            this_click.update(cx, |app, cx| on_click(app, window, cx));
        })
        .children(percent.map(|label| {
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(fill)
                .whitespace_nowrap()
                .child(label)
        }))
        .child(
            div()
                .relative()
                .size(px(RING_BOX))
                .flex()
                .items_center()
                .justify_center()
                .child(context_ring(fraction, fill, theme.surface_elevated))
                .children(popup_el),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(tokens: Option<u64>, window: u64, percent: Option<f64>) -> ContextUsage {
        ContextUsage {
            tokens,
            context_window: window,
            percent,
        }
    }

    #[test]
    fn tokens_match_reference_style() {
        assert_eq!(format_tokens(688), "688");
        assert_eq!(format_tokens(1_000), "1K");
        assert_eq!(format_tokens(1_900), "1.9K");
        assert_eq!(format_tokens(9_600), "9.6K");
        assert_eq!(format_tokens(60_000), "60K");
        assert_eq!(format_tokens(150_700), "150.7K");
        assert_eq!(format_tokens(256_000), "256K");
    }

    #[test]
    fn tokens_round_boundaries_have_no_trailing_zero() {
        assert_eq!(format_tokens(100_050), "100.1K");
        assert_eq!(format_tokens(100_000), "100K");
        assert_eq!(format_tokens(1_000_001), "1M");
        assert_eq!(format_tokens(1_050_000), "1.1M");
        assert_eq!(format_tokens(1_000_000), "1M");
    }

    #[test]
    fn fill_color_tracks_fraction() {
        let theme = Theme::dark();
        assert_eq!(fill_color(Some(0.5), &theme), theme.accent);
        assert_eq!(fill_color(Some(0.75), &theme), theme.warning);
        assert_eq!(fill_color(Some(0.95), &theme), theme.error);
        assert_eq!(fill_color(None, &theme), theme.accent);
    }

    #[test]
    fn percent_sub_one() {
        let u = usage(Some(400), 200_000, Some(0.2));
        assert_eq!(format_percent(&u), Some("<1%".into()));
    }

    #[test]
    fn cost_labels_sub_cent_and_scales() {
        assert_eq!(format_cost(0.0), "$0.00");
        assert_eq!(format_cost(0.004), "<$0.01");
        assert_eq!(format_cost(0.45), "$0.45");
        assert_eq!(format_cost(12.345), "$12.35");
        assert_eq!(format_cost(250.0), "$250");
        assert_eq!(format_cost(f64::NAN), "$0.00");
    }

    #[test]
    fn session_cache_read_label_appends_hit_rate() {
        let mut usage = SessionUsage {
            input: 50_000,
            output: 0,
            cache_read: 40_000,
            cache_write: 0,
            total: 90_000,
            cost: None,
        };
        assert_eq!(cache_read_label(&usage), "40K · 44.4%");

        // A real cache miss reads `0%`, not `None` — the prompt had tokens.
        usage.cache_read = 0;
        assert_eq!(cache_read_label(&usage), "0 · 0%");

        // No prompt tokens at all: just the count, no invented percentage.
        usage.input = 0;
        assert_eq!(cache_read_label(&usage), "0");
    }

    #[test]
    fn slices_split_conversation_and_other() {
        let u = usage(Some(60_000), 200_000, Some(30.0));
        let slices = usage_slices(&u, 50_000, &Theme::dark());
        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].label.as_ref(), "Other context");
        assert_eq!(slices[0].tokens, 10_000);
        assert_eq!(slices[1].label.as_ref(), "Conversation");
        assert_eq!(slices[1].tokens, 50_000);
    }

    #[test]
    fn slices_clamp_conversation_to_used() {
        let u = usage(Some(1_000), 200_000, Some(1.0));
        let slices = usage_slices(&u, 9_999, &Theme::dark());
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].label.as_ref(), "Conversation");
        assert_eq!(slices[0].tokens, 1_000);
    }

    #[test]
    fn slices_single_in_use_without_conversation() {
        let u = usage(Some(8_000), 200_000, Some(4.0));
        let slices = usage_slices(&u, 0, &Theme::dark());
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].label.as_ref(), "In use");
        assert_eq!(slices[0].tokens, 8_000);
    }
}
