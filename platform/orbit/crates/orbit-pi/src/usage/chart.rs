//! The usage timeline.
//!
//! The plot is drawn with GPUI's own `canvas`: a dashed grid, a filled area and
//! a straight stroke, in the app's palette. Orbit keeps the parts a general
//! chart library cannot know: the compact y-axis figures, the hover marker and
//! the readout card — the page's own interaction, in the page's own type
//! (§18/§56).
//!
//! Restraint is the point (§17): one accent, straight segments, no rainbow.
//! The chart is a measurement, not a poster.

use std::rc::Rc;

use gpui::{
    canvas, div, fill, point, prelude::*, px, relative, size, AnyElement, App, Bounds, ElementId,
    FontWeight, Hsla, IntoElement, MouseButton, PathBuilder, SharedString, Window,
};

use super::aggregate::{ChartMetric, LatencyMetric, TimeSeries};
use super::format;
use crate::theme::tokens::{DynamicSpacing, StyledExt, TextSize};
use crate::theme::Theme;

const PLOT_H: f32 = 168.;
const Y_AXIS_W: f32 = 56.;
const TOOLTIP_W: f32 = 208.;
/// The strip below the plot reserved for the x-axis labels.
const AXIS_GAP: f32 = 18.;
/// Most x-axis labels before they start colliding.
const MAX_X_LABELS: usize = 8;
/// Approximate width of one x-axis label, used to center it on its bucket.
const X_LABEL_W: f32 = 64.;

/// The page's hover callback: the bucket under the pointer, or nothing.
type HoverFn = Rc<dyn Fn(Option<usize>, &mut Window, &mut App)>;
/// The page's select callback: the bucket that was clicked (§9).
type SelectFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The plotted value of one bucket. Latency is the only metric whose register
/// is user-switchable (§20); everything else reads straight off the totals.
pub fn point_value(
    metric: ChartMetric,
    latency_metric: LatencyMetric,
    point: &super::aggregate::SeriesPoint,
) -> f64 {
    if metric == ChartMetric::Latency {
        latency_metric
            .value(&point.latency)
            .or_else(|| point.totals.avg_duration_ms())
            .unwrap_or(0.0)
    } else {
        metric.value(&point.totals)
    }
}

/// The formatted figure for a bucket, in the metric's own register — shared by
/// the chart axis and the "View data" table so they always agree (§52).
pub fn format_point(
    metric: ChartMetric,
    latency_metric: LatencyMetric,
    point: &super::aggregate::SeriesPoint,
) -> String {
    axis_label(point_value(metric, latency_metric, point), metric)
}

/// Round a maximum up to a friendly axis top (1/2/2.5/5 × 10ⁿ).
fn nice_max(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let exponent = max.log10().floor();
    let base = 10f64.powf(exponent);
    let normalized = max / base;
    let step = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 2.5 {
        2.5
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * base
}

/// Axis label for a value in this metric's units.
fn axis_label(value: f64, metric: ChartMetric) -> String {
    match metric {
        ChartMetric::Cost => format::cost(value),
        ChartMetric::Latency => format::duration_ms(value),
        _ => format::compact(value.max(0.0) as u64),
    }
}

/// The timeline chart for one metric.
///
/// `on_hover` reports the bucket under the pointer (or nothing when the
/// pointer leaves the plot), so the page keeps the readout in its own state
/// and the marker and card always agree.
#[allow(clippy::too_many_arguments)]
pub fn timeline(
    id: &'static str,
    series: &TimeSeries,
    metric: ChartMetric,
    latency_metric: LatencyMetric,
    hover: Option<usize>,
    selected: Option<usize>,
    theme: Theme,
    on_hover: impl Fn(Option<usize>, &mut Window, &mut App) + 'static,
    on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let values: Vec<f64> = series
        .points
        .iter()
        .map(|point| point_value(metric, latency_metric, point))
        .collect();
    let max = nice_max(values.iter().copied().fold(0.0, f64::max));
    let hover = hover.filter(|ix| *ix < values.len());
    let selected = selected.filter(|ix| *ix < values.len());
    let count = values.len();
    let accent = theme.accent;

    // X labels: one every `tick_margin` buckets, so names never collide.
    let tick_margin = if count <= MAX_X_LABELS {
        1
    } else {
        count.div_ceil(MAX_X_LABELS)
    };

    // Y axis: five figures, each centered on the grid line it names, in the
    // same 10px register as the x labels.
    let plot_h = PLOT_H - AXIS_GAP;
    let line_h = theme.ui_px(10.) * 1.4;
    let axis = div()
        .relative()
        .w(px(Y_AXIS_W))
        .h(px(PLOT_H))
        .flex_none()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(px(plot_h))
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .children((0..=4).map(|step| {
                    // step 4 = the axis maximum (top line), 0 = the baseline.
                    // The plot's origin is its top edge, so the position is the
                    // complement of the value's fraction.
                    let fraction = 1.0 - step as f32 / 4.0;
                    div()
                        .absolute()
                        .top(relative(fraction))
                        .right(px(8.))
                        .mt(-(line_h / 2.0))
                        .child(axis_label(max * (step as f64 / 4.0), metric))
                        .into_any_element()
                })),
        );

    // Hover + click targets: one flexible cell per bucket. Exact hit-testing
    // with no pointer-to-data coordinate math, and cheap at ≤ 70 cells.
    let on_hover: HoverFn = Rc::new(on_hover);
    let on_select: SelectFn = Rc::new(on_select);
    let cells: Vec<AnyElement> = (0..count)
        .map(|ix| {
            let on_hover = on_hover.clone();
            let on_select = on_select.clone();
            div()
                .id(ElementId::NamedInteger(
                    "usage-chart-cell".into(),
                    ix as u64,
                ))
                .flex_1()
                .h_full()
                .on_hover(move |entered, window, cx| {
                    if *entered {
                        on_hover(Some(ix), window, cx);
                    }
                })
                // A click selects this bucket as the page's time scope (§9).
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    on_select(ix, window, cx);
                })
                .into_any_element()
        })
        .collect();
    let clear_hover = on_hover.clone();

    // The readout sits inside the plot so it can never escape the page, and
    // flips side with the bucket so it never covers the marker.
    let tooltip = hover.map(|ix| readout(series, metric, latency_metric, ix, theme, count));

    let plot = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(PLOT_H))
        // The plot itself: grid, area and stroke, painted by the canvas.
        .child(div().absolute().inset_0().child(plot_canvas(
            values.clone(),
            max,
            accent,
            theme.border,
        )))
        // The x labels, in the axis strip below the plot.
        .child(x_axis_labels(series, count, tick_margin, theme))
        // A selected bucket keeps a visible band so the active time scope is
        // obvious even after the pointer leaves (§73).
        .children(selected.map(|ix| selection_band(ix, count, theme)))
        // The page owns the pointer: one invisible cell per bucket, plus a
        // guide line and a marker drawn over the plot.
        .children(hover.map(|ix| marker(ix, count, values[ix], max, theme)))
        .child(
            div()
                .id(SharedString::new_static("usage-chart-overlay"))
                .absolute()
                .inset_0()
                .flex()
                .on_hover(move |entered, window, cx| {
                    if !*entered {
                        clear_hover(None, window, cx);
                    }
                })
                .children(cells),
        )
        .children(tooltip);

    div()
        .id(id)
        .w_full()
        .flex()
        .flex_col()
        .child(div().w_full().flex().items_start().child(axis).child(plot))
}

/// The plot's geometry, painted directly: a dashed grid, the baseline, the
/// filled area under the series, then the series line on top.
///
/// Points sit at their bucket's center, so the marker and the selection band —
/// both positioned from the same fractions — land on the line (§73).
fn plot_canvas(values: Vec<f64>, max: f64, accent: Hsla, grid: Hsla) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
            let width = f32::from(bounds.size.width);
            let plot_h = f32::from(bounds.size.height) - AXIS_GAP;
            if width <= 0. || plot_h <= 0. {
                return;
            }

            let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));

            // Dashed grid at the quarter lines, solid baseline at the foot.
            let dash: f32 = 4.;
            let gap: f32 = 2.;
            for step in 0..4 {
                let y = plot_h * step as f32 / 4.0;
                let mut x = 0.;
                while x < width {
                    let w = dash.min(width - x);
                    window.paint_quad(fill(Bounds::new(at(x, y), size(px(w), px(1.))), grid));
                    x += dash + gap;
                }
            }
            window.paint_quad(fill(
                Bounds::new(at(0., plot_h), size(px(width), px(1.))),
                grid,
            ));

            if values.is_empty() {
                return;
            }
            let n = values.len();
            let x_at = |i: usize| (i as f32 + 0.5) / n as f32 * width;
            let y_at = |v: f64| plot_h * (1.0 - (v / max.max(f64::EPSILON)).clamp(0.0, 1.0) as f32);

            // Area: the line, down to the baseline and closed.
            let mut area = PathBuilder::fill();
            area.move_to(at(x_at(0), y_at(values[0])));
            for (i, v) in values.iter().enumerate().skip(1) {
                area.line_to(at(x_at(i), y_at(*v)));
            }
            area.line_to(at(x_at(n - 1), plot_h));
            area.line_to(at(x_at(0), plot_h));
            area.close();
            if let Ok(path) = area.build() {
                window.paint_path(path, accent.opacity(0.14));
            }

            // The line itself.
            let mut line = PathBuilder::stroke(px(1.));
            line.move_to(at(x_at(0), y_at(values[0])));
            for (i, v) in values.iter().enumerate().skip(1) {
                line.line_to(at(x_at(i), y_at(*v)));
            }
            if let Ok(path) = line.build() {
                window.paint_path(path, accent);
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// The x-axis labels, centered under their buckets. The first and last hug the
/// plot edges so they are never clipped by the column.
fn x_axis_labels(
    series: &TimeSeries,
    count: usize,
    tick_margin: usize,
    theme: Theme,
) -> AnyElement {
    let mut strip = div()
        .absolute()
        .bottom_0()
        .left_0()
        .w_full()
        .h(px(AXIS_GAP))
        .text_size(TextSize::XSmall.px(&theme))
        .text_color(theme.text_muted);
    for (i, point) in series.points.iter().enumerate() {
        if (i + 1) % tick_margin != 0 {
            continue;
        }
        let fraction = (i as f32 + 0.5) / count.max(1) as f32;
        let label = div()
            .absolute()
            .top(px(4.))
            .whitespace_nowrap()
            .child(point.label.clone());
        let label = if i == 0 {
            label.left_0()
        } else if i + 1 == count {
            label.right_0()
        } else {
            label
                .left(relative(fraction))
                .w(px(X_LABEL_W))
                .ml(px(-(X_LABEL_W / 2.)))
                .text_align(gpui::TextAlign::Center)
        };
        strip = strip.child(label);
    }
    strip.into_any_element()
}

/// The hover guide and marker: pure layout, so no canvas is needed.
///
/// The dot is placed by `bottom`, which resolves against the plot's height — a
/// percentage margin would resolve against the *width* and park the dot on the
/// baseline. Its ring is the page background, so the marker reads as a point on
/// the line rather than a blob over it.
fn marker(ix: usize, count: usize, value: f64, max: f64, theme: Theme) -> AnyElement {
    let fraction = (ix as f32 + 0.5) / count.max(1) as f32;
    let value_fraction = (value / max.max(f64::EPSILON)).clamp(0.0, 1.0) as f32;
    div()
        .absolute()
        .top_0()
        .bottom(px(AXIS_GAP))
        .left(relative(fraction))
        .w(px(1.))
        .bg(theme.accent.opacity(0.35))
        .child(
            div()
                .absolute()
                .bottom(relative(value_fraction))
                .left(px(-3.5))
                .mt(DynamicSpacing::Base04.px(&theme))
                .size(px(7.))
                .rounded_full()
                .border_2()
                .border_color(theme.background)
                .bg(theme.accent),
        )
        .into_any_element()
}

/// The selected bucket's band: a restrained accent wash behind the point.
fn selection_band(ix: usize, count: usize, theme: Theme) -> AnyElement {
    let fraction = (ix as f32 + 0.5) / count.max(1) as f32;
    div()
        .absolute()
        .top_0()
        .bottom(px(AXIS_GAP))
        .left(relative(fraction))
        .w(relative(1.0 / count.max(1) as f32))
        .ml(relative(-0.5 / count.max(1) as f32))
        .bg(theme.accent.opacity(0.10))
        .into_any_element()
}

/// The hover readout. Every row is a real measurement from the bucket; the
/// metric's own register decides which rows are worth showing.
fn readout(
    series: &TimeSeries,
    metric: ChartMetric,
    latency_metric: LatencyMetric,
    ix: usize,
    theme: Theme,
    count: usize,
) -> AnyElement {
    let Some(bucket) = series.points.get(ix) else {
        return div().into_any_element();
    };
    let totals = &bucket.totals;
    let mut rows: Vec<(String, String)> = Vec::new();
    match metric {
        ChartMetric::Tokens | ChartMetric::Input | ChartMetric::Output | ChartMetric::Cache => {
            rows.push((tr!("usage.metric_requests"), format::exact(totals.requests)));
            rows.push((
                tr!("usage.slice_input"),
                format::compact(totals.tokens.input),
            ));
            rows.push((
                tr!("usage.slice_output"),
                format::compact(totals.tokens.output),
            ));
            rows.push((
                tr!("usage.slice_cache_read"),
                format::compact(totals.tokens.cache_read),
            ));
            rows.push((
                tr!("usage.slice_cache_write"),
                format::compact(totals.tokens.cache_write),
            ));
        }
        ChartMetric::Requests => {
            rows.push((tr!("usage.metric_requests"), format::exact(totals.requests)));
            rows.push((tr!("usage.metric_errors"), format::exact(totals.errors)));
            rows.push((
                tr!("usage.metric_tokens"),
                format::compact(totals.tokens.total),
            ));
        }
        ChartMetric::Cost => {
            rows.push((tr!("usage.stat_cost"), format::cost(totals.cost_usd)));
            rows.push((tr!("usage.metric_requests"), format::exact(totals.requests)));
            rows.push((tr!("usage.priced"), format::exact(totals.priced_requests)));
        }
        ChartMetric::Latency => {
            // The active register first, then the others when the bucket has
            // enough observations for them to mean anything (§20).
            let latency = &bucket.latency;
            for choice in LatencyMetric::ALL {
                if let Some(value) = choice.value(latency) {
                    rows.push((
                        latency_row_label(choice, choice == latency_metric),
                        format::duration_ms(value),
                    ));
                } else if choice == LatencyMetric::Average {
                    rows.push((tr!("usage.latency_average"), "—".into()));
                }
            }
            rows.push((
                tr!("usage.measured"),
                format::exact(latency.samples.max(totals.duration_samples)),
            ));
            rows.push((tr!("usage.metric_requests"), format::exact(totals.requests)));
        }
        ChartMetric::Errors => {
            rows.push((tr!("usage.metric_errors"), format::exact(totals.errors)));
            rows.push((tr!("usage.stopped"), format::exact(totals.aborted)));
            rows.push((tr!("usage.metric_requests"), format::exact(totals.requests)));
        }
    }
    let footer = match metric {
        ChartMetric::Tokens | ChartMetric::Input | ChartMetric::Output | ChartMetric::Cache => {
            Some((
                tr!("usage.total"),
                format!(
                    "{} ({})",
                    format::compact(totals.tokens.total),
                    format::exact(totals.tokens.total)
                ),
            ))
        }
        ChartMetric::Errors => totals
            .error_rate()
            .map(|rate| (tr!("usage.failure_rate"), format::percent(rate))),
        _ => None,
    };

    let mut body = div()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base02.px(&theme))
        .line_height(theme.ui_px(11.5) * 1.25)
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(bucket.stamp.clone()),
        );
    for (label, value) in rows {
        body = body.child(readout_row(&label, &value, theme));
    }
    if let Some((label, value)) = footer {
        body = body
            .child(div().h(px(1.)).w_full().bg(theme.border))
            .child(readout_row(&label, &value, theme));
    }

    let fraction = (ix as f32 + 0.5) / count.max(1) as f32;
    let on_left_half = fraction <= 0.5;
    let card = div()
        .absolute()
        .top(px(4.))
        .w(px(TOOLTIP_W))
        .p(DynamicSpacing::Base08.px(&theme))
        .elevation_2(&theme)
        .flex()
        .flex_col()
        .child(body)
        .occlude();
    if on_left_half {
        card.ml(DynamicSpacing::Base12.px(&theme))
            .left(relative(fraction))
            .into_any_element()
    } else {
        card.mr(DynamicSpacing::Base12.px(&theme))
            .right(relative(1.0 - fraction))
            .into_any_element()
    }
}

/// Static labels for the latency readout, with the active register marked.
fn latency_row_label(choice: LatencyMetric, shown: bool) -> String {
    match (choice, shown) {
        (LatencyMetric::Average, false) => tr!("usage.latency_average"),
        (LatencyMetric::Average, true) => tr!("usage.latency_average_shown"),
        (LatencyMetric::P50, false) => tr!("usage.latency_p50"),
        (LatencyMetric::P50, true) => tr!("usage.latency_p50_shown"),
        (LatencyMetric::P95, false) => tr!("usage.latency_p95"),
        (LatencyMetric::P95, true) => tr!("usage.latency_p95_shown"),
        (LatencyMetric::P99, false) => tr!("usage.latency_p99"),
        (LatencyMetric::P99, true) => tr!("usage.latency_p99_shown"),
    }
}

fn readout_row(label: &str, value: &str, theme: Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(DynamicSpacing::Base12.px(&theme))
        .whitespace_nowrap()
        .text_size(TextSize::Small.px(&theme))
        .child(
            div()
                .flex_none()
                .text_color(theme.text_muted)
                .child(label.to_string()),
        )
        .child(
            div()
                .min_w_0()
                .text_color(theme.text_primary)
                .child(value.to_string()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_tops_are_friendly_numbers() {
        assert_eq!(nice_max(18_400_000.0), 20_000_000.0);
        assert_eq!(nice_max(842.0), 1_000.0);
        assert_eq!(nice_max(1.0), 1.0);
        assert_eq!(nice_max(2_500.0), 2_500.0);
        assert_eq!(nice_max(0.0), 1.0);
        assert!(nice_max(f64::NAN) > 0.0);
    }

    #[test]
    fn axis_labels_use_the_metric_register() {
        assert_eq!(axis_label(18_400_000.0, ChartMetric::Tokens), "18.4M");
        assert_eq!(axis_label(1_800.0, ChartMetric::Latency), "1.8s");
        assert_eq!(axis_label(12.4, ChartMetric::Cost), "$12.40");
        assert_eq!(axis_label(482.0, ChartMetric::Requests), "482");
    }
}
