//! The daily activity calendar: a contribution-style heatmap of one metric.
//!
//! One cell per local day — seven rows (Monday first, matching the store's own
//! week boundary) and one column per week. The grid spans the filtered range,
//! capped to the most recent year by [`super::aggregate::DailyCalendar`], and a
//! day outside the range keeps its slot without a fill: the calendar never
//! paints a day the filter excluded as though it were a day of zero activity.
//!
//! The measure is the page's active [`ChartMetric`], so the calendar and the
//! timeline always speak in the same register. Cell shades are a single-hue
//! accent ramp, the same restraint as every other chart on the page
//! (§17): empty days are the surface well, active days step up through accent.
//!
//! Everything here is layout and formatting. The totals arrive computed on the
//! snapshot; the page keeps hover and selection.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, AnyElement, App, ElementId, FontWeight, Hsla, IntoElement, MouseButton,
    SharedString, Window,
};

use super::aggregate::{ChartMetric, DailyCalendar};
use super::format;
use super::model::{local_datetime, stamp_label, Granularity};
use crate::theme::tokens::{DynamicSpacing, Radius, StyledExt, TextSize};
use crate::theme::Theme;

/// The smallest cell edge, in points.
const MIN_CELL: f32 = 4.;
/// The tallest a cell grows. The cells always expand sideways to consume the
/// leftover width (the reference chart's `fill` layout), so the calendar spans
/// its container edge to edge; past this height they only get wider, so a
/// short span fills the row instead of growing into a wall.
const MAX_CELL_H: f32 = 22.;
/// The cells sit flush, edge to edge, so the days read as one dense lattice
/// rather than a field of separated tiles.
const GAP: f32 = 0.;
/// The weekday label column to the left of the grid.
const ROW_LABEL_W: f32 = 28.;
/// The month label strip above the grid.
const MONTH_STRIP_H: f32 = 16.;
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const TOOLTIP_W: f32 = 184.;

/// The page's hover callback: the day under the pointer, or nothing.
type HoverFn = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;
/// The page's select callback: the day that was clicked (§9/§73).
type SelectFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A cell's box on screen. Width fills the card; height is capped so the grid
/// stays a calendar band rather than a column of slabs.
struct Geometry {
    w: f32,
    h: f32,
    gap: f32,
}

/// Cell size for the available width. The cells consume the whole row, so the
/// grid always reaches the card's right edge; the height is capped so a wide
/// card grows the cells sideways only.
fn geometry(width: f32, weeks: usize) -> Geometry {
    let weeks = weeks.max(1) as f32;
    let avail = (width - ROW_LABEL_W).max(40.);
    let w = ((avail - GAP * (weeks - 1.0)) / weeks).max(MIN_CELL);
    Geometry {
        w,
        h: w.min(MAX_CELL_H),
        gap: GAP,
    }
}

/// Quantile thresholds over the days that had any activity, so the ramp uses
/// its whole range instead of collapsing under one busy day. Nearest rank, the
/// same rule the latency percentiles use. All-zero data yields zeros.
pub fn thresholds(values: &[f64]) -> [f64; 3] {
    let mut active: Vec<f64> = values.iter().copied().filter(|v| *v > 0.0).collect();
    if active.is_empty() {
        return [0.0; 3];
    }
    active.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pick = |quantile: f64| {
        let rank = (quantile * active.len() as f64).ceil() as usize;
        active[rank.saturating_sub(1).min(active.len() - 1)]
    };
    [pick(0.25), pick(0.50), pick(0.75)]
}

/// A day's shade, 0 (none) to 4 (busiest quartile). The busiest day always
/// reaches the top shade, so a degenerate distribution (one active day, or
/// several tied) still reads at full strength instead of at level 1.
pub fn level(value: f64, thresholds: [f64; 3], max: f64) -> u8 {
    if value <= 0.0 || value.is_nan() {
        0
    } else if value >= max {
        4
    } else if value <= thresholds[0] {
        1
    } else if value <= thresholds[1] {
        2
    } else if value <= thresholds[2] {
        3
    } else {
        4
    }
}

/// The fill for one level: the canvas for none (which reads as a hollow box
/// against the raised card), then a single-hue accent ramp. The well cannot be
/// used for the empty level — in several palettes it equals `surface_elevated`, which
/// is exactly the card the calendar sits on.
pub fn level_color(level: u8, theme: Theme) -> Hsla {
    match level {
        0 => theme.background,
        1 => theme.accent.opacity(0.24),
        2 => theme.accent.opacity(0.46),
        3 => theme.accent.opacity(0.72),
        _ => theme.accent,
    }
}

/// The calendar for one metric. `hover` and `selected` are cell indices into
/// [`DailyCalendar::days`].
#[allow(clippy::too_many_arguments)]
pub fn calendar(
    id: &'static str,
    calendar: &DailyCalendar,
    metric: ChartMetric,
    hover: Option<usize>,
    selected: Option<usize>,
    theme: Theme,
    width: f32,
    on_hover: impl Fn(Option<usize>, &mut Window, &mut App) + 'static,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let weeks = calendar.weeks().max(1);
    let geometry = geometry(width, weeks);
    let values: Vec<f64> = calendar
        .days
        .iter()
        .map(|day| metric.value(&day.totals))
        .collect();
    let thresholds = thresholds(&values);
    let max = values.iter().copied().fold(0.0, f64::max);
    let levels: Vec<u8> = values
        .iter()
        .map(|value| level(*value, thresholds, max))
        .collect();
    let hover = hover.filter(|ix| *ix < calendar.days.len());
    let selected = selected.filter(|ix| *ix < calendar.days.len());
    let on_hover: HoverFn = Rc::new(on_hover);
    let on_select: SelectFn = Rc::new(on_select);

    // Month labels: the first week of the grid, then every week whose Monday
    // opens a new month.
    let mut month_labels: Vec<Option<String>> = vec![None; weeks];
    let mut last_month = None;
    for (week, slot) in month_labels.iter_mut().enumerate() {
        let Some(day) = calendar.days.get(week * 7) else {
            continue;
        };
        let Some(dt) = local_datetime(day.start_ms) else {
            continue;
        };
        use chrono::Datelike;
        let month = (dt.year(), dt.month());
        if last_month != Some(month) {
            *slot = Some(dt.format("%b").to_string());
            last_month = Some(month);
        }
    }

    let strip = month_strip(&month_labels, &geometry, theme);
    let grid = week_grid(
        calendar,
        &levels,
        &geometry,
        hover,
        selected,
        theme,
        on_hover.clone(),
        on_select.clone(),
    );
    let clear_hover = on_hover.clone();
    let tooltip = hover.and_then(|ix| {
        let day = calendar.days.get(ix)?;
        day.in_range
            .then(|| day_readout(day, metric, ix, weeks, &geometry, theme))
    });

    div()
        .id(id)
        .w_full()
        .flex()
        .flex_col()
        // The grid is centered when a capped calendar is narrower than the
        // card, but the *positioned* box is only as wide as the grid, so the
        // tooltip's coordinates stay relative to the first week column.
        .child(
            div().w_full().flex().justify_center().child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(strip)
                    .child(
                        div()
                            .id(ElementId::Name(SharedString::from(format!("{id}-grid"))))
                            .flex()
                            .on_hover(move |entered, window, cx| {
                                if !*entered {
                                    clear_hover(None, window, cx);
                                }
                            })
                            .child(grid),
                    )
                    .children(tooltip),
            ),
        )
        .child(legend(theme))
}

/// The month strip: a spacer for the weekday column, then one slot per week.
/// Month names overflow their narrow slot into the empty ones beside them,
/// which is why only the week a month starts in carries a label.
fn month_strip(labels: &[Option<String>], geometry: &Geometry, theme: Theme) -> AnyElement {
    let mut columns = div().flex().gap(px(geometry.gap));
    for label in labels {
        columns = columns.child(
            div()
                .w(px(geometry.w))
                .flex_none()
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .whitespace_nowrap()
                .child(label.clone().unwrap_or_default()),
        );
    }
    div()
        .h(px(MONTH_STRIP_H))
        .flex()
        .items_center()
        .child(div().w(px(ROW_LABEL_W)).flex_none())
        .child(columns)
        .into_any_element()
}

/// The grid: weekday labels on the left, then a column of seven cells per week.
#[allow(clippy::too_many_arguments)]
fn week_grid(
    calendar: &DailyCalendar,
    levels: &[u8],
    geometry: &Geometry,
    hover: Option<usize>,
    selected: Option<usize>,
    theme: Theme,
    on_hover: HoverFn,
    on_select: SelectFn,
) -> AnyElement {
    // Labels only have room once the cells are big enough to breathe; below
    // that the grid is the message.
    let show_labels = geometry.w >= 8.;
    let mut labels = div()
        .w(px(ROW_LABEL_W))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(geometry.gap));
    for row in 0..WEEKDAYS.len() {
        // Every other row, like a calendar's own axis: Mon / Wed / Fri.
        let text = if show_labels && row % 2 == 0 {
            crate::i18n::translate(&format!("usage.weekday_{row}"))
        } else {
            String::new()
        };
        labels = labels.child(
            div()
                .h(px(geometry.h))
                .flex_none()
                .pr(DynamicSpacing::Base06.px(&theme))
                .flex()
                .items_center()
                .justify_end()
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .child(text),
        );
    }

    let mut weeks = div().flex().gap(px(geometry.gap));
    for week in 0..calendar.weeks().max(1) {
        let mut column = div().flex().flex_col().gap(px(geometry.gap));
        for row in 0..7 {
            let ix = week * 7 + row;
            column = column.child(day_cell(
                calendar,
                levels,
                geometry,
                ix,
                hover,
                selected,
                theme,
                on_hover.clone(),
                on_select.clone(),
            ));
        }
        weeks = weeks.child(column);
    }

    div()
        .flex()
        .items_start()
        .child(labels)
        .child(weeks)
        .into_any_element()
}

/// One day. Days outside the filter keep their slot but paint nothing; an
/// in-range day carries its shade, a hover/selection ring, and the handlers.
#[allow(clippy::too_many_arguments)]
fn day_cell(
    calendar: &DailyCalendar,
    levels: &[u8],
    geometry: &Geometry,
    ix: usize,
    hover: Option<usize>,
    selected: Option<usize>,
    theme: Theme,
    on_hover: HoverFn,
    on_select: SelectFn,
) -> AnyElement {
    let w = px(geometry.w);
    let h = px(geometry.h);
    let Some(day) = calendar.days.get(ix) else {
        return div().w(w).h(h).flex_none().into_any_element();
    };
    if !day.in_range {
        // A slot outside the filter paints nothing. If the pointer crosses it,
        // clear the readout instead of leaving a stale day behind.
        let on_hover = on_hover.clone();
        return div()
            .id(ElementId::NamedInteger(
                SharedString::new_static("usage-heatmap-pad"),
                ix as u64,
            ))
            .w(w)
            .h(h)
            .flex_none()
            .on_hover(move |entered, window, cx| {
                if *entered {
                    on_hover(None, window, cx);
                }
            })
            .into_any_element();
    }
    let shade = level_color(levels.get(ix).copied().unwrap_or(0), theme);
    let is_hover = hover == Some(ix);
    let is_selected = selected == Some(ix);
    // While the pointer is over a day, the rest of the grid recedes so the
    // hovered cell carries the reading (the reference chart's inactive dim).
    let dimmed = hover.is_some() && !is_hover;
    let cell = div()
        .id(ElementId::NamedInteger(
            SharedString::new_static("usage-heatmap-cell"),
            ix as u64,
        ))
        .w(w)
        .h(h)
        .flex_none()
        .rounded(Radius::Small.px(&theme))
        // Every in-range day is an outlined box, empty or not, so the calendar
        // reads as a grid even when a stretch has no activity; the ring
        // brightens for the hovered / selected day.
        .border_1()
        .border_color(if is_hover || is_selected {
            theme.text_primary
        } else {
            theme.border
        })
        .bg(shade)
        .cursor_pointer()
        .when(dimmed, |cell| cell.opacity(0.45))
        .on_hover({
            let on_hover = on_hover.clone();
            move |entered, window, cx| {
                if *entered {
                    on_hover(Some(ix), window, cx);
                }
            }
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_select(ix, window, cx);
        });
    cell.into_any_element()
}

/// The hover readout, anchored to the hovered cell and flipped at the grid's
/// edges so it never escapes the card.
fn day_readout(
    day: &super::aggregate::DayCell,
    metric: ChartMetric,
    ix: usize,
    weeks: usize,
    geometry: &Geometry,
    theme: Theme,
) -> AnyElement {
    let week = ix / 7;
    let row = ix % 7;
    let totals = &day.totals;
    let value = metric.value(totals);
    let primary = match metric {
        ChartMetric::Cost => format::cost(value),
        ChartMetric::Latency => format::duration_ms(value),
        _ => format::compact(value.max(0.0) as u64),
    };
    let mut rows: Vec<(String, String)> = vec![(metric.label(), primary)];
    // The metric's own value is already the first row; the supporting rows
    // below must not repeat its label.
    if metric != ChartMetric::Requests {
        rows.push((tr!("usage.metric_requests"), format::exact(totals.requests)));
    }
    if metric != ChartMetric::Tokens {
        rows.push((
            tr!("usage.metric_tokens"),
            format::compact(totals.tokens.total),
        ));
    }
    if totals.errors > 0 && metric != ChartMetric::Errors {
        rows.push((tr!("usage.metric_errors"), format::exact(totals.errors)));
    }

    let mut body = div()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base02.px(&theme))
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(stamp_label(day.start_ms, Granularity::Day)),
        );
    for (label, value) in rows {
        body = body.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(DynamicSpacing::Base12.px(&theme))
                .whitespace_nowrap()
                .text_size(TextSize::Small.px(&theme))
                .child(div().flex_none().text_color(theme.text_muted).child(label))
                .child(div().min_w_0().text_color(theme.text_primary).child(value)),
        );
    }

    let card = div()
        .absolute()
        .w(px(TOOLTIP_W))
        .p(DynamicSpacing::Base08.px(&theme))
        .elevation_2(&theme)
        .flex()
        .flex_col()
        .child(body)
        .occlude();

    // The card is placed against the grid and clamped to it, so a hovered cell
    // at either edge — or a very wide cell — never pushes the readout off the
    // card. It sits below the cell for the top rows and above it for the bottom.
    let grid_w =
        ROW_LABEL_W + weeks as f32 * geometry.w + (weeks as f32 - 1.).max(0.) * geometry.gap;
    let center_x = ROW_LABEL_W + week as f32 * (geometry.w + geometry.gap) + geometry.w / 2.;
    let left = (center_x - TOOLTIP_W / 2.).clamp(0., (grid_w - TOOLTIP_W).max(0.));
    let box_h = MONTH_STRIP_H + 7. * geometry.h + 6. * geometry.gap;
    let cell_top = MONTH_STRIP_H + row as f32 * (geometry.h + geometry.gap);
    let card = if row >= 4 {
        card.bottom(px(box_h - cell_top + 6.))
    } else {
        card.top(px(cell_top + geometry.h + 6.))
    };
    card.left(px(left)).into_any_element()
}

/// "Less ▢▢▢▢▢ More", the shade key.
fn legend(theme: Theme) -> AnyElement {
    let mut swatches = div()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base02.px(&theme));
    for shade in 0..=4u8 {
        swatches = swatches.child(
            div()
                .size(px(10.))
                .rounded(Radius::Small.px(&theme))
                .border_1()
                .border_color(theme.border)
                .bg(level_color(shade, theme)),
        );
    }
    div()
        .pt(DynamicSpacing::Base08.px(&theme))
        .w_full()
        .flex()
        .items_center()
        .justify_end()
        .gap(DynamicSpacing::Base06.px(&theme))
        .text_size(TextSize::XSmall.px(&theme))
        .text_color(theme.text_muted)
        .child(tr!("heatmap.less"))
        .child(swatches)
        .child(tr!("heatmap.more"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_follow_the_activity_quartiles() {
        let thresholds = thresholds(&[0.0, 1.0, 2.0, 3.0, 4.0, 0.0]);
        // Nearest rank over the four active days: 1, 2, 3.
        assert_eq!(thresholds, [1.0, 2.0, 3.0]);
        assert_eq!(level(0.0, thresholds, 4.0), 0);
        assert_eq!(level(1.0, thresholds, 4.0), 1);
        assert_eq!(level(2.0, thresholds, 4.0), 2);
        assert_eq!(level(3.0, thresholds, 4.0), 3);
        assert_eq!(level(4.0, thresholds, 4.0), 4);
        assert_eq!(level(9.0, thresholds, 9.0), 4);
    }

    #[test]
    fn a_single_active_day_sits_at_the_top() {
        let thresholds = thresholds(&[0.0, 0.0, 7.0]);
        assert_eq!(thresholds, [7.0, 7.0, 7.0]);
        assert_eq!(level(7.0, thresholds, 7.0), 4);
        assert_eq!(level(0.0, thresholds, 7.0), 0);
    }

    #[test]
    fn no_activity_has_no_shade() {
        let thresholds = thresholds(&[0.0, 0.0, 0.0]);
        assert_eq!(thresholds, [0.0, 0.0, 0.0]);
        assert_eq!(level(0.0, thresholds, 0.0), 0);
    }

    #[test]
    fn geometry_always_fills_the_width() {
        // Whatever the span, the grid reaches the card's right edge.
        for (width, weeks) in [
            (1100., 53),
            (1100., 26),
            (1100., 1),
            (1600., 53),
            (360., 53),
        ] {
            let geometry = geometry(width, weeks);
            assert!(geometry.w >= MIN_CELL);
            let weeks = weeks as f32;
            let grid = ROW_LABEL_W + weeks * geometry.w + (weeks - 1.).max(0.) * geometry.gap;
            assert!(
                (grid - width).abs() < 1.0,
                "width {width}, weeks {weeks}: grid was {grid}"
            );
        }
        // The fill is preferred; the height caps so a wide card grows the cells
        // sideways only. The grid always reaches the card's right edge.
        assert_eq!(geometry(1100., 53).h, geometry(1100., 53).w);
        let ultra = geometry(2600., 53);
        assert_eq!(ultra.h, MAX_CELL_H);
        assert!(ultra.w > ultra.h);
    }
}
