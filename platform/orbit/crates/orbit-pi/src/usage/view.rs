//! The Usage page's rendering: header, filters, and every panel.
//!
//! The view is deliberately dumb. It reads a [`UsageSnapshot`] that was
//! computed elsewhere and formats it; the only logic allowed here is layout,
//! interaction, and choosing which register a number is shown in.
//!
//! # Direction contract — Usage, minimal pass
//!
//! **THESIS.** The page is a reading surface, not an instrument rack: the
//! headline metrics are the one board, and every other measure sits on the
//! canvas behind a hairline. Chrome is spent only where it earns its weight.
//!
//! **OWN-WORLD.** Orbit's instrument panel, inherited whole: canvas ground,
//! one raised card per section, one accent on the active series,
//! hairlines inside the cards, tabular mono figures.
//!
//! **STORY.** The operator reads the four headline figures, reads the trend,
//! then opens Breakdown or Records only when the question asks. Every figure
//! is pi's own session measurement; nothing is invented.
//!
//! **FIRST VIEWPORT.** 44px header + filter bar with Simple | Details. Simple
//! is the scan: Daily activity, Summary, Usage over time, Signals, Token health.
//! Details is the audit: Breakdown and Records. The two never share a viewport.
//!
//! **FORM.** Established world. Simple is a stack of 12px raised cards; Details
//! are two cards of tables. Title on a hairline header, content padded inside.
//! Nested cards are forbidden: inner wells recess to the canvas, tables keep
//! only a hairline frame. The trend is an area chart card plus a buckets table
//! card below it — the chart shape, then the same data as Sessions (search,
//! columns, sorting, totals, pagination).
//!
//! Sections: Daily activity · Summary · Usage over time · Buckets · Signals ·
//! Token health · Breakdown · Records.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    anchored, deferred, div, prelude::*, px, relative, AnyElement, App, Context, Corner, Entity,
    Font, FontFeatures, FontWeight, Hsla, IntoElement, MouseButton, Render, SharedString, Window,
};

use super::aggregate::{
    Breakdown, BucketRow, ChartMetric, Direction, GroupRow, Insight, LatencyMetric, LatencyStats,
    SessionRow, TimeSeries, Tone, ToolRow, Totals, UsageSnapshot,
};
use super::chart;
use super::filters::{self, FilterOption};
use super::format;
use super::heatmap;
use super::model::{
    next_bucket, Granularity, RangePreset, TimeFocus, TokenCounts, UsageFilter, UsageIndex,
};
use super::page::{
    BreakdownQueryResult, BreakdownSort, BreakdownTab, BucketQueryResult, BucketSort, DetailTab,
    FailureQueryResult, MenuKind, SeriesQueryResult, SeriesSort, SessionQueryResult, SessionSort,
    ToolQueryResult, UsageMode, UsagePage,
};
use super::table::{
    cell_shell, data_table, empty_cell, text_cell, Column, FailureSort, SortState, TableHandlers,
    TableKind, HEADER_H, ROW_H,
};
use super::tooltip::Tooltip;
use crate::app::{
    button_frame, context_menu_separator, helpers::empty_state_glyph, icon, icon_button_frame,
    picker_entry, picker_search_frame, picker_surface, press, TipExt, BUTTON_GROUP,
};
use crate::composer::ComposerInput;
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{
    button, input, picker, popover, ButtonSize, DynamicSpacing, IconSize, Radius, TextSize,
};
use crate::theme::{self, Theme};

/// Inner column width for a data surface (§58): wide enough for a full table,
/// narrow enough that the eye does not have to travel a metre.
pub(super) const CONTENT_MAX_W: f32 = 1180.;
/// The page column's horizontal padding (both sides), which the tables sit inside.
pub(super) const PAGE_PAD: f32 = 40.;
/// Inner padding of a section card (each side), at the default density —
/// [`DynamicSpacing::Base12`]. Tables budget against this so their last
/// column is not squeezed into a scrollbar.
pub(super) const SECTION_PAD: f32 = 12.;
/// Below this column width the paired panels stack into one column.
const TWO_COLUMN_MIN: f32 = 820.;
const FOUR_KPI_MIN: f32 = 880.;
/// Height of the cache hit-rate trend. Tall enough to read a swing, short
/// enough that the two Token-health panels above it stay the same height.
const HIT_RATE_PLOT_H: f32 = 56.;

/// Ranked bars the Breakdown chart shows; the table below carries the rest.
const CHART_ROWS: usize = 8;

impl Render for UsagePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        // Layout is driven by the main area's width (handed over by the shell)
        // — never by the window, which also holds the sidebar and side pane.
        let available = px(self.main_width());
        let column_w = available.min(px(CONTENT_MAX_W));
        let wide = column_w >= px(TWO_COLUMN_MIN);
        let kpi_cols = if column_w >= px(FOUR_KPI_MIN) {
            4
        } else if column_w >= px(560.) {
            2
        } else {
            1
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text_primary)
            .font_family(theme::ui_font_family())
            .text_size(TextSize::Default.px(&theme))
            .child(self.header(theme, cx))
            .child(self.filter_bar(theme, cx))
            .child(
                div()
                    .id("usage-body")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(self.scroll())
                    .child(
                        div()
                            .flex_none()
                            .w(column_w)
                            .mx_auto()
                            .pb(DynamicSpacing::Base48.px(&theme))
                            .flex()
                            .flex_col()
                            .children(self.active_filter_bar(theme, cx))
                            .child(
                                div()
                                    .w_full()
                                    .px(DynamicSpacing::Base20.px(&theme))
                                    .pt(DynamicSpacing::Base20.px(&theme))
                                    .flex()
                                    .flex_col()
                                    .child(self.body(theme, wide, kpi_cols, window, cx)),
                            ),
                    ),
            )
    }
}

impl UsagePage {
    // ── header ─────────────────────────────────────────────────────────────

    /// The Usage card's title controls, hosted by the shell's top bar: the Back
    /// button and the "Usage" title, replacing the session title while the page
    /// is open. Keeping them in the shared top bar means the card itself starts
    /// at its filter row, mirroring the Git page.
    pub fn top_bar_leading(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                press(
                    button_frame(div().id("usage-top-back"), &theme, ButtonSize::Medium)
                        .group(BUTTON_GROUP)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.surface_hover)),
                )
                .on_mouse_down(MouseButton::Left, {
                    let entity = cx.entity();
                    move |_, window, cx| {
                        entity.update(cx, |page, cx| page.close_page(window, cx));
                    }
                })
                .child(icon(
                    "icons/arrow-left.svg",
                    ButtonSize::Medium.icon_size().px(&theme),
                    theme.text_secondary,
                ))
                .child(
                    div()
                        .text_color(theme.text_secondary)
                        .child(tr!("view.back")),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(icon(
                        "icons/chart-analysis.svg",
                        IconSize::Medium.px(&theme),
                        theme.text_secondary,
                    ))
                    .child(
                        div()
                            .text_size(TextSize::Large.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_primary)
                            .child(tr!("view.usage")),
                    ),
            )
            .into_any_element()
    }

    /// 44px page header: freshness and the page's own actions. The Back
    /// affordance and title now live in the shared top bar (see
    /// [`UsagePage::top_bar_leading`]). Compact by design — the data starts on
    /// the next row.
    fn header(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let refresh_label = tr!("usage.refreshing");
        let refresh_idle_label = tr!("common.refresh");
        let status = if self.is_refreshing() {
            Some(refresh_label.clone())
        } else {
            self.status().map(str::to_string).or_else(|| {
                self.last_updated_ms()
                    .map(|at| format::age_label(super::collect::now_ms() - at))
            })
        };
        let has_data = self.snapshot().is_some_and(|snapshot| !snapshot.is_empty());

        div()
            .h(px(44.))
            .flex_none()
            // The page now sits in a card below the top bar, which owns the
            // caption buttons; the header only needs the normal page inset.
            .pl(px(self.header_leading()))
            .pr(DynamicSpacing::Base12.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .border_b_1()
            .border_color(theme.border)
            .children(status.map(|status| {
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(status)
            }))
            .child(div().flex_1())
            .child(self.export_control(theme, has_data, cx))
            .child(filters::text_button(
                "usage-refresh",
                if self.is_refreshing() {
                    &refresh_label
                } else {
                    &refresh_idle_label
                },
                Some("icons/refresh.svg"),
                self.is_refreshing(),
                true,
                theme,
                {
                    let entity = cx.entity();
                    move |_, _, cx| {
                        entity.update(cx, |page, cx| page.refresh(cx));
                    }
                },
            ))
            .into_any_element()
    }

    fn export_control(&self, theme: Theme, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let open = self.menu() == Some(MenuKind::Export);
        let entity = cx.entity();
        let button = filters::text_button(
            "usage-export",
            &tr!("usage.export"),
            Some("icons/upload.svg"),
            false,
            enabled,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::Export, window, cx)
                });
            },
        );
        if !open {
            return div().child(button).into_any_element();
        }
        let panel = filters::export_menu(cx, theme);
        filters::chip_with_menu(
            "usage-export-anchor",
            button,
            true,
            Corner::TopRight,
            theme,
            move || panel,
        )
    }

    // ── filters ────────────────────────────────────────────────────────────

    /// The filter bar: one row of chips under the header. Every chip writes to
    /// the same filter the whole page reads (§10), so no panel can drift.
    fn filter_bar(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.filter();
        let range = filter.range.clone();
        let index = self.index();

        let mut bar = div()
            .w_full()
            .flex_none()
            .px(DynamicSpacing::Base20.px(&theme))
            .py(DynamicSpacing::Base08.px(&theme))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme));

        let entity = cx.entity();
        let range_open = self.menu() == Some(MenuKind::Range);
        let range_chip = filters::chip(
            "usage-range-chip",
            range.label(),
            Some("icons/clock.svg"),
            range.preset != RangePreset::Last7,
            false,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| page.toggle_menu(MenuKind::Range, window, cx));
            },
        );
        let range_panel = range_open.then(|| filters::range_menu(self, cx, theme));
        bar = bar.child(filters::chip_with_menu(
            "usage-range-anchor",
            range_chip,
            range_open,
            Corner::TopLeft,
            theme,
            move || range_panel.unwrap_or_else(|| div().into_any_element()),
        ));

        if let Some(index) = index {
            let ranked_workspaces = filters::workspace_options(index);
            let ranked_providers = filters::provider_options(index);
            let ranked_models = filters::model_options(index);
            let all_workspaces: Vec<FilterOption> = index
                .workspaces
                .iter()
                .enumerate()
                .map(|(ix, entry)| FilterOption {
                    id: ix as u16,
                    label: entry.label.clone(),
                    sub: None,
                })
                .collect();
            let all_providers: Vec<FilterOption> = index
                .providers
                .iter()
                .enumerate()
                .map(|(ix, entry)| FilterOption {
                    id: ix as u16,
                    label: entry.label.clone(),
                    sub: None,
                })
                .collect();
            let all_models: Vec<FilterOption> = ranked_models
                .iter()
                .map(|option| FilterOption {
                    id: option.id,
                    label: option.label.clone(),
                    sub: None,
                })
                .collect();

            bar = bar
                .child(self.multi_chip(
                    MenuKind::Workspace,
                    "usage-workspace",
                    &tr!("usage.all_workspaces"),
                    &ranked_workspaces,
                    &all_workspaces,
                    &filter.workspaces,
                    theme,
                    cx,
                ))
                .child(self.multi_chip(
                    MenuKind::Provider,
                    "usage-provider",
                    &tr!("usage.all_providers"),
                    &ranked_providers,
                    &all_providers,
                    &filter.providers,
                    theme,
                    cx,
                ))
                .child(self.multi_chip(
                    MenuKind::Model,
                    "usage-model",
                    &tr!("usage.all_models"),
                    &ranked_models,
                    &all_models,
                    &filter.models,
                    theme,
                    cx,
                ));
        }

        // Active narrowings are shown once, as removable chips, under the
        // filter row (§45); the filter row itself stays a set of controls.
        bar.child(div().flex_1())
            .child(self.mode_control(theme, cx))
            .into_any_element()
    }

    /// Simple vs Details. Lives on the filter row so the two readings never
    /// compete for the same viewport.
    fn mode_control(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        segmented(
            "usage-mode",
            &UsageMode::ALL,
            self.usage_mode(),
            |choice| choice.label(),
            |choice| choice.as_str(),
            |_| true,
            false,
            theme,
            {
                let entity = cx.entity();
                move |choice, _, cx| {
                    entity.update(cx, |page, cx| page.set_usage_mode(choice, cx));
                }
            },
        )
    }

    /// One multi-select chip with its popover. `ranked` is what the menu shows
    /// (busiest first); `all` is the full set "Select all" applies.
    #[allow(clippy::too_many_arguments)]
    fn multi_chip(
        &self,
        kind: MenuKind,
        id: &'static str,
        all_label: &str,
        ranked: &[FilterOption],
        all: &[FilterOption],
        selected: &[u16],
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.menu() == Some(kind);
        let label = filters::selection_label(all_label, selected, |value| {
            ranked
                .iter()
                .chain(all.iter())
                .find(|option| option.id == value)
                .map(|option| option.label.clone())
        });
        let entity = cx.entity();
        let chip = filters::chip(
            id,
            label,
            Some("icons/filter.svg"),
            !selected.is_empty(),
            false,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| page.toggle_menu(kind, window, cx));
            },
        );
        if !open {
            return filters::chip_with_menu(id, chip, false, Corner::TopLeft, theme, || {
                div().into_any_element()
            });
        }
        let panel = filters::multi_menu(self, kind, ranked, all, selected, cx, theme);
        filters::chip_with_menu(id, chip, true, Corner::TopLeft, theme, move || panel)
    }

    /// The active-filter bar (§45): every narrowing filter as a removable
    /// chip, so the user always knows why the numbers changed. Only shown when
    /// something beyond the date range is applied.
    fn active_filter_bar(&self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let filter = self.filter();
        let index = self.index()?;
        if !filter.has_narrowing() {
            return None;
        }
        let mut chips: Vec<AnyElement> = Vec::new();

        if let Some(focus) = filter.focus {
            let entity = cx.entity();
            chips.push(filters::toggle_chip(
                "usage-chip-focus".to_string(),
                focus.label(),
                theme,
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_focus(None, cx));
                },
            ));
        }
        if let Some(session) = filter.session {
            if let Some(entry) = index.try_session(session) {
                let label = tr!("usage.chip_session", name = session_title(entry));
                let entity = cx.entity();
                chips.push(filters::toggle_chip(
                    "usage-chip-session".to_string(),
                    label,
                    theme,
                    move |_, _, cx| {
                        entity.update(cx, |page, cx| page.set_session_scope(None, cx));
                    },
                ));
            }
        }
        for id in filter.workspaces.iter().copied() {
            let Some(entry) = index.workspaces.get(id as usize) else {
                continue;
            };
            let entity = cx.entity();
            chips.push(filters::toggle_chip(
                format!("usage-chip-workspace-{id}"),
                tr!("usage.chip_workspace", name = entry.label),
                theme,
                move |_, _, cx| {
                    entity.update(cx, |page, cx| {
                        page.toggle_filter_value(MenuKind::Workspace, id, cx)
                    });
                },
            ));
        }
        for id in filter.providers.iter().copied() {
            let Some(entry) = index.providers.get(id as usize) else {
                continue;
            };
            let entity = cx.entity();
            chips.push(filters::toggle_chip(
                format!("usage-chip-provider-{id}"),
                tr!("usage.chip_provider", name = entry.label),
                theme,
                move |_, _, cx| {
                    entity.update(cx, |page, cx| {
                        page.toggle_filter_value(MenuKind::Provider, id, cx)
                    });
                },
            ));
        }
        for id in filter.models.iter().copied() {
            let Some(entry) = index.models.get(id as usize) else {
                continue;
            };
            let entity = cx.entity();
            chips.push(filters::toggle_chip(
                format!("usage-chip-model-{id}"),
                tr!("usage.chip_model", name = entry.label),
                theme,
                move |_, _, cx| {
                    entity.update(cx, |page, cx| {
                        page.toggle_filter_value(MenuKind::Model, id, cx)
                    });
                },
            ));
        }
        if filter.errors_only {
            let entity = cx.entity();
            chips.push(filters::toggle_chip(
                "usage-chip-errors".to_string(),
                tr!("usage.chip_failed_requests"),
                theme,
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_errors_only(false, cx));
                },
            ));
        }
        if filter.cached_only {
            let entity = cx.entity();
            chips.push(filters::toggle_chip(
                "usage-chip-cached".to_string(),
                tr!("usage.chip_cached_only"),
                theme,
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_cached_only(false, cx));
                },
            ));
        }

        let entity = cx.entity();
        let clear_all = filters::text_button(
            "usage-chips-clear",
            &tr!("usage.clear_all"),
            None,
            false,
            true,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.clear_filters(cx));
            },
        );

        Some(
            div()
                .id("usage-active-filters")
                .w_full()
                .px(DynamicSpacing::Base20.px(&theme))
                .pt(DynamicSpacing::Base12.px(&theme))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .child(
                    div()
                        .pr(DynamicSpacing::Base02.px(&theme))
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(tr!("view.filters")),
                )
                .children(chips)
                .child(clear_all)
                .into_any_element(),
        )
    }

    // ── page states ────────────────────────────────────────────────────────

    fn body(
        &mut self,
        theme: Theme,
        wide: bool,
        kpi_cols: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.is_loading() {
            return self.skeleton(theme);
        }
        if self.index().is_none() {
            // The store could not be read at all: say so, and offer the retry.
            if let Some(error) = self.error().map(str::to_string) {
                let entity = cx.entity();
                let action = press(
                    button_frame(div().id("usage-retry"), &theme, ButtonSize::Medium)
                        .group(BUTTON_GROUP)
                        .mt(DynamicSpacing::Base02.px(&theme))
                        .border_1()
                        .border_color(theme.border)
                        .raised(theme.surface_elevated, &theme)
                        .cursor_pointer()
                        .hover(|style| style.raised(theme.surface_hover, &theme)),
                )
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    entity.update(cx, |page, cx| page.refresh(cx));
                })
                .child(tr!("view.try_again"))
                .into_any_element();
                return self.message_state(
                    theme,
                    "icons/info.svg",
                    &tr!("usage.unable_to_load"),
                    &error,
                    Some(action),
                );
            }
            return self.skeleton(theme);
        }
        let Some(index) = self.index() else {
            return self.skeleton(theme);
        };
        if index.is_empty() && index.unreadable_files > 0 {
            return self.message_state(
                theme,
                "icons/info.svg",
                &tr!("usage.unable_to_read"),
                &tr!(
                    "usage.unreadable_files_hint",
                    count = format::count(index.unreadable_files as u64),
                    store = self.store_hint()
                ),
                None,
            );
        }
        if index.is_empty() {
            return self.empty_store_state(theme, index.unreadable_files);
        }
        let Some(snapshot) = self.snapshot().cloned() else {
            return self.skeleton(theme);
        };
        let snapshot = snapshot.as_ref();
        if snapshot.filtered_out() {
            let action = self.clear_filters_button(theme, cx);
            return self.message_state(
                theme,
                "icons/filter.svg",
                &tr!("usage.no_matches"),
                &tr!("usage.no_matches_hint"),
                action,
            );
        }
        if snapshot.is_empty() {
            return self.empty_store_state(theme, index.unreadable_files);
        }

        match self.usage_mode() {
            UsageMode::Simple => {
                let mut cards = div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base20.px(&theme))
                    .child(self.daily_section(snapshot, theme, cx))
                    .child(self.summary_card(snapshot, theme, kpi_cols, cx))
                    .child(self.activity_section(snapshot, theme, cx))
                    .child(self.series_section(snapshot, theme, cx));
                if !snapshot.insights.is_empty() {
                    cards = cards.child(self.signals_section(&snapshot.insights, theme));
                }
                if snapshot.cache.is_available() || snapshot.summary.totals.tokens.total > 0 {
                    cards = cards.child(self.health_section(snapshot, theme, wide, cx));
                }
                band(
                    "usage-overview",
                    &tr!("usage.overview"),
                    cards.into_any_element(),
                    theme,
                )
            }
            UsageMode::Details => band(
                "usage-details-band",
                &tr!("usage.details"),
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base20.px(&theme))
                    .child(self.breakdown_section(snapshot, theme, cx))
                    .child(self.details_section(snapshot, theme, window, cx))
                    .into_any_element(),
                theme,
            ),
        }
    }

    /// Usage over time as one card: the trend chart and its data table.
    fn activity_section(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let metric = self.metric();
        let latency_metric = self.latency_metric();
        let by = snapshot.series.granularity.label();
        let meta = match metric {
            ChartMetric::Cost => tr!(
                "usage.total_by",
                value = format::cost(metric.total(&snapshot.summary.totals)),
                by = by
            ),
            ChartMetric::Latency => tr!(
                "usage.per_bucket",
                value = latency_metric.label().to_lowercase(),
                by = by
            ),
            _ => tr!(
                "usage.total_by",
                value = format::compact(metric.total(&snapshot.summary.totals) as u64),
                by = by
            ),
        };

        section(
            "usage-activity",
            &tr!("usage.over_time"),
            None,
            Some(meta),
            None,
            self.trend_body(snapshot, theme, cx),
            theme,
        )
    }

    /// The trend's data as its own card: the same framed table as Sessions,
    /// with search, column picker, sortable headers, totals, and pagination.
    /// Split out of the chart card so the plot and the raw buckets each read
    /// as their own surface instead of one tall card.
    fn series_section(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let granularity = snapshot.series.granularity;
        let all = snapshot.series.points.len();
        let result = self.series_result(snapshot, cx);
        let meta = if result.total == all {
            tr!(
                "usage.n_buckets_by",
                buckets = format::count(all as u64),
                by = granularity.label()
            )
        } else {
            tr!(
                "usage.n_of_n_buckets_by",
                buckets = format::count(result.total as u64),
                total = format::count(all as u64),
                by = granularity.label()
            )
        };
        section(
            "usage-series",
            &tr!("usage.buckets"),
            None,
            Some(meta),
            None,
            self.chart_data_table(
                snapshot,
                &result,
                self.metric(),
                self.latency_metric(),
                theme,
                cx,
            ),
            theme,
        )
    }

    /// Derived findings as their own card, so a warning cannot be mistaken for
    /// a metric on the board above.
    fn signals_section(&self, insights: &[Insight], theme: Theme) -> AnyElement {
        let meta = if insights.len() == 1 {
            tr!("usage.one_note")
        } else {
            tr!(
                "usage.n_notes",
                count = format::count(insights.len() as u64)
            )
        };
        section(
            "usage-signals",
            &tr!("usage.signals"),
            None,
            Some(meta),
            None,
            self.signals_body(insights, theme),
            theme,
        )
    }

    /// The contribution-graph calendar as its own card.
    fn daily_section(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let daily_meta = tr!(
            "usage.per_day_last_year",
            metric = self.metric().label().to_lowercase()
        );
        section(
            "usage-daily",
            &tr!("usage.daily_activity"),
            None,
            Some(daily_meta),
            None,
            self.heatmap_body(snapshot, theme, cx),
            theme,
        )
    }

    /// Token composition and cache performance as their own card.
    fn health_section(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        section(
            "usage-token-health",
            &tr!("usage.token_health"),
            None,
            Some(tr!("usage.composition_and_cache")),
            None,
            self.health_body(snapshot, theme, wide, cx),
            theme,
        )
    }

    fn clear_filters_button(&self, theme: Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.filter().has_narrowing() {
            return None;
        }
        let entity = cx.entity();
        Some(
            press(
                button_frame(div().id("usage-state-clear"), &theme, ButtonSize::Medium)
                    .group(BUTTON_GROUP)
                    .mt(DynamicSpacing::Base02.px(&theme))
                    .border_1()
                    .border_color(theme.border)
                    .raised(theme.surface_elevated, &theme)
                    .cursor_pointer()
                    .hover(|style| style.raised(theme.surface_hover, &theme)),
            )
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                entity.update(cx, |page, cx| page.clear_filters(cx));
            })
            .child(tr!("view.clear_filters"))
            .into_any_element(),
        )
    }

    /// Lightweight skeleton: the page's shape, without a spinner (§45).
    fn skeleton(&self, theme: Theme) -> AnyElement {
        // A placeholder bar, drawn in the trough so it reads against a card.
        let bar = |width: f32, height: f32| {
            div()
                .w(px(width))
                .h(px(height))
                .rounded(Radius::Small.px(&theme))
                .bg(theme.surface_secondary)
                .into_any_element()
        };
        // One card shell, so the load→loaded swap keeps the same shape: a
        // header strip over a body.
        let shell = |body: AnyElement| {
            div()
                .w_full()
                .flex_none()
                .rounded(Radius::XLarge.px(&theme))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_elevated)
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .h(px(42.))
                        .px(DynamicSpacing::Base12.px(&theme))
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(bar(96., 10.)),
                )
                .child(body)
                .into_any_element()
        };

        // Summary: a metric grid divided by hairlines.
        let mut grid = div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap(DynamicSpacing::Base01.px(&theme))
            .bg(theme.border);
        for _ in 0..4 {
            grid = grid.child(
                div()
                    .flex_1()
                    .min_w(px(150.))
                    .p(DynamicSpacing::Base12.px(&theme))
                    .bg(theme.surface_elevated)
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(bar(56., 10.))
                    .child(bar(88., 18.))
                    .child(bar(112., 10.)),
            );
        }

        // Activity: the switcher row, the plot, then the always-open bands.
        let activity = div()
            .p(DynamicSpacing::Base12.px(&theme))
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(bar(320., 26.))
            .child(
                div()
                    .h(px(168.))
                    .w_full()
                    .rounded(Radius::XLarge.px(&theme))
                    .bg(theme.surface_secondary),
            )
            .child(
                div()
                    .h(px(96.))
                    .w_full()
                    .rounded(Radius::XLarge.px(&theme))
                    .bg(theme.surface_secondary),
            )
            .child(
                div()
                    .h(px(72.))
                    .w_full()
                    .rounded(Radius::XLarge.px(&theme))
                    .bg(theme.surface_secondary),
            );

        // Records: a header rule and a handful of rows.
        let mut records = div()
            .w_full()
            .pt(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col();
        for ix in 0..6 {
            records = records.child(
                div()
                    .h(px(26.))
                    .w_full()
                    .px(DynamicSpacing::Base12.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base12.px(&theme))
                    .when(ix > 0, |row| row.border_t_1().border_color(theme.border))
                    .child(bar(140., 10.))
                    .child(div().flex_1())
                    .child(bar(72., 10.)),
            );
        }

        // Secondary bands are sections, not cards: a quiet rule over the
        // placeholder body.
        let section_shell = |body: AnyElement| {
            div()
                .w_full()
                .flex_none()
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .pb(DynamicSpacing::Base12.px(&theme))
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base04.px(&theme))
                        .border_b_1()
                        .border_color(theme.border)
                        .child(bar(120., 12.))
                        .child(bar(220., 10.)),
                )
                .child(div().pt(DynamicSpacing::Base12.px(&theme)).child(body))
                .into_any_element()
        };

        let heading = |title_w: f32, desc_w: f32| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base04.px(&theme))
                .child(bar(title_w, 10.))
                .child(bar(desc_w, 10.))
                .into_any_element()
        };

        let body = match self.usage_mode() {
            UsageMode::Simple => div()
                .w_full()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base20.px(&theme))
                .child(heading(72., 280.))
                .child(shell(grid.into_any_element()))
                .child(section_shell(activity.into_any_element())),
            UsageMode::Details => div()
                .w_full()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base20.px(&theme))
                .child(heading(64., 320.))
                .child(section_shell(
                    div()
                        .p(DynamicSpacing::Base12.px(&theme))
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base12.px(&theme))
                        .child(bar(220., 26.))
                        .child(
                            div()
                                .h(px(96.))
                                .w_full()
                                .rounded(Radius::XLarge.px(&theme))
                                .bg(theme.surface_secondary),
                        )
                        .into_any_element(),
                ))
                .child(section_shell(records.into_any_element())),
        };

        body.into_any_element()
    }

    /// The store has no usage at all (§43).
    fn empty_store_state(&self, theme: Theme, unreadable: usize) -> AnyElement {
        let note = (unreadable > 0).then(|| {
            tr!(
                "usage.unreadable_files_period",
                count = format::count(unreadable as u64)
            )
        });
        let mut state = self.message_state(
            theme,
            "icons/usage-total.svg",
            &tr!("usage.no_data_yet"),
            &tr!("usage.no_data_hint", store = self.store_hint()),
            None,
        );
        if let Some(note) = note {
            state = div()
                .flex()
                .flex_col()
                .child(state)
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(note),
                )
                .into_any_element();
        }
        state
    }

    /// The store the page reads, for the states that have to name it.
    fn store_hint(&self) -> String {
        self.store_path()
            .map(|path| format::short_path(&path))
            .unwrap_or_else(|| tr!("usage.the_session_store"))
    }

    /// A centered message with an optional action.
    fn message_state(
        &self,
        theme: Theme,
        icon_path: &'static str,
        title: &str,
        body: &str,
        action: Option<AnyElement>,
    ) -> AnyElement {
        div()
            .w_full()
            .h(px(400.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(empty_state_glyph(theme, icon_path))
            .child(
                div()
                    .text_size(TextSize::Large.px(&theme))
                    .line_height(theme.ui_px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .max_w(px(440.))
                    .text_align(gpui::TextAlign::Center)
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(body.to_string()),
            )
            .children(action)
            .into_any_element()
    }

    // ── summary card ───────────────────────────────────────────────────────

    /// The Summary card: the metric grid first, as full-bleed cells divided by
    /// hairlines (a measurement board, not a grid of floating cards), then the
    /// quieter secondary figures on the strip beneath.
    ///
    /// The grid is built as explicit rows of `flex_1` cells rather than one
    /// wrapping container of percentage-width cells: percentage widths inside a
    /// wrapping flex row are resolved against the row's own (content-derived)
    /// width, which let the board grow past the window.
    fn summary_card(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cols: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cells = self.kpi_cells(snapshot);
        let mut grid = div().w_full().flex().flex_col();

        for (row_ix, chunk) in cells.chunks(cols).enumerate() {
            let mut row = div().w_full().flex();
            for (col_ix, cell) in chunk.iter().enumerate() {
                let entity = cx.entity();
                let tint = match cell.tone {
                    CellTone::Normal => theme.text_primary,
                    CellTone::Muted => theme.text_muted,
                };
                let mut element = div()
                    .id(SharedString::from(format!("usage-kpi-{row_ix}-{col_ix}")))
                    .flex_1()
                    .min_w(px(150.))
                    .px(DynamicSpacing::Base12.px(&theme))
                    .py(DynamicSpacing::Base12.px(&theme))
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .when(row_ix > 0, |cell| {
                        cell.border_t_1().border_color(theme.border)
                    })
                    .when(col_ix + 1 < chunk.len(), |cell| {
                        cell.border_r_1().border_color(theme.border)
                    })
                    .when(cell.click.is_some(), |cell| {
                        cell.cursor_pointer()
                            .hover(|style| style.bg(theme.surface_hover))
                    })
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text_muted)
                            .child(cell.label.clone()),
                    )
                    .child(
                        div()
                            .font(num_font())
                            .text_size(theme.ui_px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tint)
                            .child(cell.value.clone()),
                    )
                    .child(
                        div()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .child(cell.sub.clone()),
                    );
                if let Some(metric) = cell.click {
                    element = element.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        // Point the main chart at the metric the card names (§79).
                        entity.update(cx, |page, cx| page.set_metric(metric, cx));
                    });
                }
                row = row.child(element);
            }
            // Pad a short final row so its cells keep the grid's column widths.
            for _ in chunk.len()..cols {
                row = row.child(div().flex_1().min_w(px(150.)).when(row_ix > 0, |cell| {
                    cell.border_t_1().border_color(theme.border)
                }));
            }
            grid = grid.child(row);
        }

        let body = div().w_full().flex().flex_col().child(grid).child(
            div()
                .w_full()
                .px(DynamicSpacing::Base12.px(&theme))
                .py(DynamicSpacing::Base08.px(&theme))
                .border_t_1()
                .border_color(theme.border)
                .child(self.summary_strip(snapshot, theme, cx)),
        );

        card(
            "usage-summary",
            &tr!("usage.summary"),
            None,
            Some(summary_meta(snapshot)),
            None,
            body.into_any_element(),
            theme,
            true,
        )
    }

    /// One cell per headline metric. A metric whose data does not exist says
    /// "Unavailable" and why — never a fabricated zero (§6/§44).
    fn kpi_cells(&self, snapshot: &UsageSnapshot) -> Vec<KpiCell> {
        let totals = &snapshot.summary.totals;
        let mut cells = Vec::new();
        cells.push(KpiCell {
            label: tr!("usage.metric_requests"),
            value: format::count(totals.requests),
            sub: delta_sub(
                snapshot,
                ChartMetric::Requests,
                &tr!("usage.sub_requests_in_range"),
            ),
            tone: CellTone::Normal,
            click: Some(ChartMetric::Requests),
        });
        cells.push(KpiCell {
            label: tr!("usage.metric_total_tokens"),
            value: format::compact(totals.tokens.total),
            sub: match totals.tokens_per_request() {
                Some(avg) => tr!(
                    "usage.sub_avg_per_request",
                    value = format::compact(avg as u64)
                ),
                None => tr!("usage.sub_no_requests"),
            },
            tone: CellTone::Normal,
            click: Some(ChartMetric::Tokens),
        });
        cells.push(match snapshot.cache.hit_rate {
            Some(rate) => KpiCell {
                label: tr!("usage.metric_cache_hit_rate"),
                value: format::percent(rate),
                sub: tr!(
                    "usage.sub_read_written",
                    read = format::compact(snapshot.cache.cache_read),
                    written = format::compact(snapshot.cache.cache_write)
                ),
                tone: CellTone::Normal,
                // Focus the cache analytics: plot cache volume over time.
                click: Some(ChartMetric::Cache),
            },
            None => KpiCell {
                label: tr!("usage.metric_cache_hit_rate"),
                value: tr!("usage.unavailable"),
                sub: tr!("usage.sub_no_cache_tokens"),
                tone: CellTone::Muted,
                click: None,
            },
        });
        cells.push(match totals.avg_duration_ms() {
            Some(avg) => KpiCell {
                label: tr!("usage.metric_avg_response"),
                value: format::duration_ms(avg),
                sub: if snapshot.latency.has_percentiles() {
                    tr!(
                        "usage.sub_p95_measured",
                        p95 = format::duration_ms(snapshot.latency.p95_ms as f64),
                        count = format::count(snapshot.latency.samples)
                    )
                } else {
                    tr!(
                        "usage.sub_measured",
                        count = format::count(snapshot.latency.samples)
                    )
                },
                tone: CellTone::Normal,
                click: Some(ChartMetric::Latency),
            },
            None => KpiCell {
                label: tr!("usage.metric_avg_response"),
                value: tr!("usage.unavailable"),
                sub: tr!("usage.sub_no_request_gaps"),
                tone: CellTone::Muted,
                click: None,
            },
        });
        cells
    }

    /// Secondary readout under the board: the metrics that inform the headline
    /// numbers but should not compete with them (§88/§101). Every figure stays
    /// on the strip — nothing is behind a disclosure.
    fn summary_strip(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let totals = &snapshot.summary.totals;
        let headline: Vec<(String, String)> = vec![
            (
                tr!("usage.stat_input_tokens"),
                format::compact(totals.tokens.input),
            ),
            (
                tr!("usage.stat_output_tokens"),
                format::compact(totals.tokens.output),
            ),
            (
                tr!("usage.stat_cost"),
                match (totals.cost_coverage(), totals.cost_per_request()) {
                    (0.0, _) => tr!("usage.unavailable"),
                    (_, Some(per)) => tr!(
                        "usage.stat_cost_per_req",
                        cost = format::cost(totals.cost_usd),
                        per = format::cost(per)
                    ),
                    (_, None) => format::cost(totals.cost_usd),
                },
            ),
            (
                tr!("usage.stat_failed"),
                if totals.errors > 0 {
                    format::exact(totals.errors)
                } else {
                    tr!("usage.none")
                },
            ),
            (
                tr!("usage.stat_sessions"),
                format::count(snapshot.summary.sessions),
            ),
            (
                tr!("usage.stat_turns"),
                format::count(snapshot.summary.turns),
            ),
            (
                tr!("usage.stat_tool_calls"),
                format::count(snapshot.summary.tool_runs),
            ),
        ];
        let mut extra: Vec<(String, String)> = vec![(
            tr!("usage.stat_bash_commands"),
            format::count(snapshot.summary.bash_runs),
        )];
        if snapshot.summary.tool_errors > 0 {
            extra.push((
                tr!("usage.stat_tool_failures"),
                format::count(snapshot.summary.tool_errors),
            ));
        }
        if totals.duration_samples > 0 {
            extra.push((
                tr!("usage.stat_generation_time"),
                format::span_ms(totals.duration_ms as i64),
            ));
        }
        if let Some(avg) = totals.avg_prompt() {
            extra.push((tr!("usage.stat_avg_prompt"), format::compact(avg as u64)));
        }
        if let Some(ratio) = totals.output_input_ratio() {
            // Three decimals: output is routinely a fraction of a percent of
            // the prompt, and "0.00×" would read as zero.
            let text = if ratio > 0.0 && ratio < 0.001 {
                "<0.001×".to_string()
            } else {
                format!("{ratio:.3}×")
            };
            extra.push((tr!("usage.stat_output_input"), text));
        }
        if let Some(per_mtok) = totals.cost_per_mtok() {
            extra.push((tr!("usage.stat_cost_per_mtok"), format::cost(per_mtok)));
        }
        if totals.peak_prompt > 0 {
            extra.push((
                tr!("usage.stat_peak_prompt"),
                format::compact(totals.peak_prompt),
            ));
        }
        if totals.reasoning_reported > 0 {
            extra.push((
                tr!("usage.stat_reasoning"),
                tr!(
                    "usage.stat_reasoning_of_output",
                    value = format::compact(totals.reasoning)
                ),
            ));
        }

        let mut values: Vec<AnyElement> = headline
            .into_iter()
            .chain(extra)
            .map(|(label, value)| summary_stat(label, value, theme))
            .collect();

        // Keep the failed-request drill-down available even though Errors is no
        // longer a headline cell.
        if totals.errors > 0 {
            let fail_entity = cx.entity();
            values.push(
                press(
                    button_frame(
                        div().id("usage-summary-failures"),
                        &theme,
                        ButtonSize::Compact,
                    )
                    .text_color(theme.error)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.text_primary)),
                )
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    fail_entity.update(cx, |page, cx| page.set_errors_only(true, cx));
                })
                .child(tr!("view.view_failures"))
                .into_any_element(),
            );
        }

        div()
            .w_full()
            .flex()
            .items_start()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_wrap()
                    .gap_x(DynamicSpacing::Base20.px(&theme))
                    .gap_y(DynamicSpacing::Base06.px(&theme))
                    .children(values),
            )
            .into_any_element()
    }

    /// Derived findings, one line each. The card title carries the heading, so
    /// this is only the list.
    fn signals_body(&self, insights: &[Insight], theme: Theme) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .children(insights.iter().map(|insight| {
                let (path, color) = match insight.tone {
                    Tone::Positive => ("icons/check.svg", theme.success),
                    Tone::Warning => ("icons/info.svg", theme.warning),
                    Tone::Neutral => ("icons/spark.svg", theme.text_muted),
                };
                div()
                    .flex()
                    .items_start()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .text_size(TextSize::Small.px(&theme))
                    .child(div().pt(DynamicSpacing::Base01.px(&theme)).child(icon(
                        path,
                        IconSize::XSmall.px(&theme),
                        color,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.text_secondary)
                            .child(insight.text.clone()),
                    )
                    .into_any_element()
            }))
            .into_any_element()
    }

    // ── trend ──────────────────────────────────────────────────────────────

    /// The trend: one area chart across several measures, with the metric
    /// switcher and the chart's data table in its own body. The Usage over
    /// time card supplies the frame.
    fn trend_body(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let metric = self.metric();
        let latency_metric = self.latency_metric();

        // Metric switcher: one visualization, several measures (§19). A metric
        // with no source data stays visible but disabled, and the line under
        // the chart says why.
        let unavailable: Vec<&str> = ChartMetric::ALL
            .iter()
            .filter(|option| !option.available(&snapshot.summary))
            .map(|option| option.as_str())
            .collect();
        let tabs = segmented(
            "usage-metric",
            &ChartMetric::ALL,
            metric,
            |option| option.label(),
            |option| option.as_str(),
            |option| option.available(&snapshot.summary),
            true,
            theme,
            {
                let entity = cx.entity();
                move |option, _, cx| {
                    entity.update(cx, |page, cx| page.set_metric(option, cx));
                }
            },
        );

        // Latency register selector (§20): only offered when the window has
        // enough observations for percentiles to be meaningful.
        let latency_selector = (metric == ChartMetric::Latency).then(|| {
            segmented(
                "usage-latency",
                &LatencyMetric::ALL,
                latency_metric,
                |choice| choice.label(),
                |choice| choice.as_str(),
                |choice| choice.available(&snapshot.latency),
                true,
                theme,
                {
                    let entity = cx.entity();
                    move |choice, _, cx| {
                        entity.update(cx, |page, cx| page.set_latency_metric(choice, cx));
                    }
                },
            )
        });

        // The metric switcher sits in the body, above the plot: a row of
        // selectors under a heading reads as controls, and no eight-way
        // switcher can overflow a 42px header on a narrow column.
        let selectors = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .flex_wrap()
            .child(tabs)
            .children(latency_selector);

        let hover = self.hover_bucket();
        let entity = cx.entity();
        let on_hover = move |bucket: Option<usize>, _: &mut Window, cx: &mut App| {
            entity.update(cx, |page, cx| page.set_hover_bucket(bucket, cx));
        };

        // Pre-compute each bucket's width so the click handler (which must be
        // `'static`) can scope the page without touching the snapshot.
        let granularity = snapshot.series.granularity;
        let focus_points: Vec<TimeFocus> = snapshot
            .series
            .points
            .iter()
            .map(|point| TimeFocus {
                start_ms: point.start_ms,
                end_ms: next_bucket(point.start_ms, granularity),
                granularity,
            })
            .collect();
        let selected = self.filter().focus.and_then(|focus| {
            snapshot
                .series
                .points
                .iter()
                .position(|p| p.start_ms == focus.start_ms)
        });
        let select_entity = cx.entity();
        let on_select = move |ix: usize, _: &mut Window, cx: &mut App| {
            let Some(focus) = focus_points.get(ix).copied() else {
                return;
            };
            select_entity.update(cx, |page, cx| {
                // Clicking the selected bucket again clears the scope (§73).
                let next = if page.filter().focus == Some(focus) {
                    None
                } else {
                    Some(focus)
                };
                page.set_focus(next, cx);
            });
        };

        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(selectors)
            .child(chart::timeline(
                "usage-timeline",
                &snapshot.series,
                metric,
                latency_metric,
                hover,
                selected,
                theme,
                on_hover,
                on_select,
            ));
        if snapshot.latency.samples > 0 {
            content = content.child(self.latency_line(snapshot, theme));
        }
        let reason = unavailable_reason(&unavailable);
        if !reason.is_empty() {
            content = content.child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .text_color(theme.text_muted)
                    .child(reason),
            );
        }

        content.into_any_element()
    }

    // ── daily activity (calendar heatmap) ──────────────────────────────────

    /// The daily activity calendar: one cell per day, shaded by the active
    /// metric. A second reading of the same activity — the trend shows the shape
    /// across adaptive buckets, the calendar shows the day-of-week rhythm, the
    /// streaks, and the quiet stretches at a glance. It is always a trailing
    /// year (like a contribution graph), so it stays readable whatever the date
    /// range is; the workspace / model / provider scope still applies to every
    /// cell. Clicking a day scopes the whole page to it.
    fn heatmap_body(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let metric = self.metric();
        let calendar = &snapshot.calendar;

        let hover = self.hover_day();
        // The day the page is scoped to, when the range sits inside one day (a
        // calendar click, or the Today preset). The calendar does not read the
        // range for its window, so this is only the selection ring.
        let range = &self.filter().range;
        let selected = calendar.days.iter().position(|day| {
            day.in_range
                && range.start_ms >= day.start_ms
                && range.end_ms > range.start_ms
                && range.end_ms <= next_bucket(day.start_ms, Granularity::Day)
        });

        // Pre-compute each in-range day's start so the click handler (which
        // must be `'static`) can scope the page without touching the snapshot.
        let day_starts: Vec<Option<i64>> = calendar
            .days
            .iter()
            .map(|day| day.in_range.then_some(day.start_ms))
            .collect();

        let entity = cx.entity();
        let on_hover = move |day: Option<usize>, _: &mut Window, cx: &mut App| {
            entity.update(cx, |page, cx| page.set_hover_day(day, cx));
        };
        let select_entity = cx.entity();
        let on_select = move |ix: usize, _: &mut Window, cx: &mut App| {
            let Some(Some(day_ms)) = day_starts.get(ix).copied() else {
                return;
            };
            select_entity.update(cx, |page, cx| page.scope_to_day(day_ms, cx));
        };

        // Empty-day cells paint `background`; on the raised section card they
        // recess without a second frame. Nested cards are forbidden.
        let width = self.table_width().max(240.);
        heatmap::calendar(
            "usage-calendar",
            calendar,
            metric,
            hover,
            selected,
            theme,
            width,
            on_hover,
            on_select,
        )
        .into_any_element()
    }

    /// The chart's data as the same framed table as Sessions: search, column
    /// picker, sortable headers, totals, and pagination. Click a bucket to
    /// focus the page.
    fn chart_data_table(
        &self,
        snapshot: &UsageSnapshot,
        result: &SeriesQueryResult,
        metric: ChartMetric,
        latency_metric: LatencyMetric,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (columns, keys) = self.series_columns(metric);
        let total_w: f32 = columns.iter().map(|column| column.width).sum();
        let granularity = snapshot.series.granularity;
        let selected = self.filter().focus.map(|focus| focus.start_ms);
        let entity = cx.entity();
        let mut rows = Vec::with_capacity(result.rows.len());
        for (ix, point) in result.rows.iter().enumerate() {
            let last = ix + 1 == result.rows.len();
            let start_ms = point.start_ms;
            let focus = TimeFocus {
                start_ms,
                end_ms: next_bucket(start_ms, granularity),
                granularity,
            };
            let mut line = div()
                .id(SharedString::from(format!("usage-series-row-{start_ms}")))
                .h(px(ROW_H))
                .w_full()
                .min_w(px(total_w))
                .flex()
                .items_center()
                .when(!last, |row| row.border_b_1().border_color(theme.border))
                .when(selected == Some(start_ms), |row| row.bg(theme.selection))
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover))
                .on_mouse_down(MouseButton::Left, {
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |page, cx| {
                            let next = if page.filter().focus == Some(focus) {
                                None
                            } else {
                                Some(focus)
                            };
                            page.set_focus(next, cx);
                        });
                    }
                });
            for (col_ix, column) in columns.iter().enumerate() {
                let key = keys.get(col_ix).copied().unwrap_or(SeriesSort::Time);
                let (text, color) = match key {
                    SeriesSort::Time => (point.stamp.clone(), theme.text_secondary),
                    SeriesSort::Value => (
                        chart::format_point(metric, latency_metric, point),
                        theme.text_primary,
                    ),
                    SeriesSort::Requests => {
                        (format::exact(point.totals.requests), theme.text_muted)
                    }
                    SeriesSort::Tokens => {
                        (format::compact(point.totals.tokens.total), theme.text_muted)
                    }
                };
                line = line.child(text_cell(column, text, color, theme));
            }
            rows.push(line.into_any_element());
        }

        let visible = if result.rows.is_empty() {
            3
        } else {
            result.rows.len()
        } as f32;
        let height = HEADER_H + ROW_H * visible;
        let table = self.table_element(
            TableKind::Series,
            "usage-series-table",
            columns,
            rows,
            height,
            empty_cell(&tr!("usage.no_buckets_match_search"), theme),
            theme,
            Rc::new(
                move |page, ix, sort, cx| match (keys.get(ix).copied(), sort) {
                    (Some(key), SortState::Ascending) => page.set_series_sort(key, false, cx),
                    (Some(key), SortState::Descending) => page.set_series_sort(key, true, cx),
                    _ => page.set_series_sort(SeriesSort::Time, false, cx),
                },
            ),
            cx,
        );
        let toolbar = table_toolbar(
            search_box(self.series_search(), theme),
            self.series_columns_control(theme, cx),
            theme,
        );

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(toolbar)
            .child(table)
            .child(self.series_totals(snapshot, result, theme))
            .child(self.series_pagination_footer(result, theme, cx))
            .into_any_element()
    }

    /// Column-visibility control for the usage-over-time table. Time is fixed.
    fn series_columns_control(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.menu() == Some(MenuKind::SeriesColumns);
        let entity = cx.entity();
        let chip = filters::chip(
            "usage-series-columns-chip",
            tr!("usage.columns"),
            None,
            !self.series_hidden_columns().is_empty(),
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::SeriesColumns, window, cx)
                });
            },
        );
        if !open {
            return filters::chip_with_menu(
                "usage-series-columns-anchor",
                chip,
                false,
                Corner::TopRight,
                theme,
                || div().into_any_element(),
            );
        }
        let panel = filters::series_columns_menu(self, cx, theme);
        filters::chip_with_menu(
            "usage-series-columns-anchor",
            chip,
            true,
            Corner::TopRight,
            theme,
            move || panel,
        )
    }

    /// Totals under the usage-over-time table: the whole filtered set on the
    /// left, the visible page on the right.
    fn series_totals(
        &self,
        snapshot: &UsageSnapshot,
        result: &SeriesQueryResult,
        theme: Theme,
    ) -> AnyElement {
        let totals = &snapshot.summary.totals;
        let page_requests: u64 = result.rows.iter().map(|row| row.totals.requests).sum();
        let page_tokens: u64 = result.rows.iter().map(|row| row.totals.tokens.total).sum();
        let mut filtered = tr!(
            "usage.filtered_totals_requests",
            requests = format::count(totals.requests),
            tokens = format::compact(totals.tokens.total)
        );
        if totals.cost_coverage() > 0.0 {
            filtered.push_str(&format!(" · {}", format::cost(totals.cost_usd)));
        }
        let page = tr!(
            "usage.page_requests",
            requests = format::count(page_requests),
            tokens = format::compact(page_tokens)
        );
        totals_row(&filtered, &page, theme)
    }

    /// Range, rows-per-page, and Previous / Next — the same footer as Sessions.
    fn series_pagination_footer(
        &self,
        result: &SeriesQueryResult,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pages = result.page_count();
        let current = result.page;
        let summary = if result.total == 0 {
            tr!("usage.no_buckets_match").to_string()
        } else {
            tr!(
                "usage.showing_range",
                first = format::count(result.first_row() as u64),
                last = format::count(result.last_row() as u64),
                total = format::count(result.total as u64)
            )
        };

        let size_open = self.menu() == Some(MenuKind::SeriesPageSize);
        let entity = cx.entity();
        let size_chip = filters::chip(
            "usage-series-page-size-chip",
            result.page_size.to_string(),
            None,
            false,
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::SeriesPageSize, window, cx)
                });
            },
        );
        let size_panel = size_open.then(|| filters::series_page_size_menu(self, cx, theme));
        let size_control = filters::chip_with_menu(
            "usage-series-page-size-anchor",
            size_chip,
            size_open,
            Corner::TopRight,
            theme,
            move || size_panel.unwrap_or_else(|| div().into_any_element()),
        );

        let prev =
            filters::outline_button("usage-series-page-prev", "Previous", current > 1, theme, {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| {
                        page.set_series_page(current.saturating_sub(1), cx)
                    });
                }
            });
        let next =
            filters::outline_button("usage-series-page-next", "Next", current < pages, theme, {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_series_page(current + 1, cx));
                }
            });

        table_pager(summary, size_control, prev, next, current, pages, theme)
    }

    fn bucket_columns_control(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.menu() == Some(MenuKind::BucketColumns);
        let entity = cx.entity();
        let chip = filters::chip(
            "usage-bucket-columns-chip",
            tr!("usage.columns"),
            None,
            !self.bucket_hidden_columns().is_empty(),
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::BucketColumns, window, cx)
                });
            },
        );
        if !open {
            return filters::chip_with_menu(
                "usage-bucket-columns-anchor",
                chip,
                false,
                Corner::TopRight,
                theme,
                || div().into_any_element(),
            );
        }
        let panel = filters::bucket_columns_menu(self, cx, theme);
        filters::chip_with_menu(
            "usage-bucket-columns-anchor",
            chip,
            true,
            Corner::TopRight,
            theme,
            move || panel,
        )
    }

    fn bucket_totals(&self, result: &BucketQueryResult, theme: Theme) -> AnyElement {
        let page_requests: u64 = result.rows.iter().map(|row| row.totals.requests).sum();
        let page_tokens: u64 = result.rows.iter().map(|row| row.totals.tokens.total).sum();
        let mut filtered = tr!(
            "usage.filtered_totals_requests",
            requests = format::count(result.totals.requests),
            tokens = format::compact(result.totals.tokens.total)
        );
        if result.totals.cost_coverage() > 0.0 {
            filtered.push_str(&format!(" · {}", format::cost(result.totals.cost_usd)));
        }
        let page = tr!(
            "usage.page_requests",
            requests = format::count(page_requests),
            tokens = format::compact(page_tokens)
        );
        totals_row(&filtered, &page, theme)
    }

    fn bucket_pagination_footer(
        &self,
        result: &BucketQueryResult,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pages = result.page_count();
        let current = result.page;
        let summary = if result.total == 0 {
            tr!("usage.no_buckets_match").to_string()
        } else {
            tr!(
                "usage.showing_range",
                first = format::count(result.first_row() as u64),
                last = format::count(result.last_row() as u64),
                total = format::count(result.total as u64)
            )
        };
        let size_open = self.menu() == Some(MenuKind::BucketPageSize);
        let entity = cx.entity();
        let size_chip = filters::chip(
            "usage-bucket-page-size-chip",
            result.page_size.to_string(),
            None,
            false,
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::BucketPageSize, window, cx)
                });
            },
        );
        let size_panel = size_open.then(|| filters::bucket_page_size_menu(self, cx, theme));
        let size_control = filters::chip_with_menu(
            "usage-bucket-page-size-anchor",
            size_chip,
            size_open,
            Corner::TopRight,
            theme,
            move || size_panel.unwrap_or_else(|| div().into_any_element()),
        );
        let prev = filters::outline_button(
            "usage-bucket-page-prev",
            &tr!("usage.previous"),
            current > 1,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| {
                        page.set_bucket_page(current.saturating_sub(1), cx)
                    });
                }
            },
        );
        let next = filters::outline_button(
            "usage-bucket-page-next",
            &tr!("usage.next"),
            current < pages,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_bucket_page(current + 1, cx));
                }
            },
        );
        table_pager(summary, size_control, prev, next, current, pages, theme)
    }

    fn failure_columns_control(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.menu() == Some(MenuKind::FailureColumns);
        let entity = cx.entity();
        let chip = filters::chip(
            "usage-failure-columns-chip",
            tr!("usage.columns"),
            None,
            !self.failure_hidden_columns().is_empty(),
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::FailureColumns, window, cx)
                });
            },
        );
        if !open {
            return filters::chip_with_menu(
                "usage-failure-columns-anchor",
                chip,
                false,
                Corner::TopRight,
                theme,
                || div().into_any_element(),
            );
        }
        let panel = filters::failure_columns_menu(self, cx, theme);
        filters::chip_with_menu(
            "usage-failure-columns-anchor",
            chip,
            true,
            Corner::TopRight,
            theme,
            move || panel,
        )
    }

    fn failure_totals(&self, result: &FailureQueryResult, theme: Theme) -> AnyElement {
        let page_n = result.rows.len();
        let filtered = tr!(
            "usage.filtered_totals_events",
            events = format::count(result.total as u64)
        );
        let page = tr!("usage.page_events", events = format::count(page_n as u64));
        totals_row(&filtered, &page, theme)
    }

    fn failure_pagination_footer(
        &self,
        result: &FailureQueryResult,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pages = result.page_count();
        let current = result.page;
        let summary = if result.total == 0 {
            tr!("usage.no_failures_match").to_string()
        } else {
            tr!(
                "usage.showing_range",
                first = format::count(result.first_row() as u64),
                last = format::count(result.last_row() as u64),
                total = format::count(result.total as u64)
            )
        };
        let size_open = self.menu() == Some(MenuKind::FailurePageSize);
        let entity = cx.entity();
        let size_chip = filters::chip(
            "usage-failure-page-size-chip",
            result.page_size.to_string(),
            None,
            false,
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::FailurePageSize, window, cx)
                });
            },
        );
        let size_panel = size_open.then(|| filters::failure_page_size_menu(self, cx, theme));
        let size_control = filters::chip_with_menu(
            "usage-failure-page-size-anchor",
            size_chip,
            size_open,
            Corner::TopRight,
            theme,
            move || size_panel.unwrap_or_else(|| div().into_any_element()),
        );
        let prev = filters::outline_button(
            "usage-failure-page-prev",
            &tr!("usage.previous"),
            current > 1,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| {
                        page.set_failure_page(current.saturating_sub(1), cx)
                    });
                }
            },
        );
        let next = filters::outline_button(
            "usage-failure-page-next",
            &tr!("usage.next"),
            current < pages,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_failure_page(current + 1, cx));
                }
            },
        );
        table_pager(summary, size_control, prev, next, current, pages, theme)
    }

    /// Response-time readout (§36): average, then percentiles once there are
    /// enough observations for them to mean something.
    fn latency_line(&self, snapshot: &UsageSnapshot, theme: Theme) -> AnyElement {
        let latency = &snapshot.latency;
        let mut items: Vec<(&str, String)> = vec![
            ("avg", format::duration_ms(latency.avg_ms)),
            ("max", format::duration_ms(latency.max_ms as f64)),
        ];
        if latency.has_percentiles() {
            items.insert(1, ("p50", format::duration_ms(latency.p50_ms as f64)));
            items.insert(2, ("p95", format::duration_ms(latency.p95_ms as f64)));
            items.insert(3, ("p99", format::duration_ms(latency.p99_ms as f64)));
        }
        div()
            .pt(DynamicSpacing::Base08.px(&theme))
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base12.px(&theme))
            .flex_wrap()
            .text_size(TextSize::Small.px(&theme))
            .child(
                div()
                    .text_color(theme.text_muted)
                    .font_weight(FontWeight::MEDIUM)
                    .child(tr!("view.response_time")),
            )
            .children(items.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base04.px(&theme))
                    .child(div().text_color(theme.text_muted).child(label))
                    .child(
                        div()
                            .font(num_font())
                            .text_color(theme.text_secondary)
                            .child(value),
                    )
                    .into_any_element()
            }))
            .child(
                div()
                    .text_size(TextSize::XSmall.px(&theme))
                    .text_color(theme.text_muted)
                    .child(if latency.has_percentiles() {
                        tr!("usage.sub_measured", count = format::count(latency.samples))
                    } else {
                        tr!(
                            "usage.sub_measured_needs_min",
                            count = format::count(latency.samples),
                            min = LatencyStats::MIN_SAMPLES
                        )
                    }),
            )
            .into_any_element()
    }

    // ── breakdown panels ───────────────────────────────────────────────────

    /// The Breakdown card: one dimension at a time, each shown as a ranked bar
    /// chart over a sortable data table. The chart reads the aggregate's own
    /// token ranking; the table sorts independently, so an operator can rank by
    /// requests or cache without disturbing the overview.
    fn breakdown_section(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.breakdown_tab();
        let tabs = segmented(
            "usage-breakdown-tab",
            &BreakdownTab::ALL,
            tab,
            |choice| choice.label(),
            |choice| choice.as_str(),
            |_| true,
            true,
            theme,
            {
                let entity = cx.entity();
                move |choice, _, cx| {
                    entity.update(cx, |page, cx| page.set_breakdown_tab(choice, cx));
                }
            },
        );

        // The shape first (a ranked bar chart), then the same treatment as the
        // Sessions table: search, a column picker, the table, totals, pagination.
        let (meta, chart, table, totals, footer) = match tab {
            BreakdownTab::Models => {
                let chart_rows = self.breakdown_top(&snapshot.models, CHART_ROWS, cx);
                let result = self.breakdown_result(&snapshot.models, cx);
                (
                    model_meta(&snapshot.models),
                    self.breakdown_chart(
                        &chart_rows,
                        result.totals.tokens.total,
                        "models",
                        theme,
                        cx,
                    ),
                    self.breakdown_table(&result, "models", theme, cx),
                    self.breakdown_totals(&result, theme),
                    self.breakdown_footer(result.page, result.page_size, result.total, theme, cx),
                )
            }
            BreakdownTab::Workspaces => {
                let chart_rows = self.breakdown_top(&snapshot.workspaces, CHART_ROWS, cx);
                let result = self.breakdown_result(&snapshot.workspaces, cx);
                (
                    workspace_meta(&snapshot.workspaces),
                    self.breakdown_chart(
                        &chart_rows,
                        result.totals.tokens.total,
                        "workspaces",
                        theme,
                        cx,
                    ),
                    self.breakdown_table(&result, "workspaces", theme, cx),
                    self.breakdown_totals(&result, theme),
                    self.breakdown_footer(result.page, result.page_size, result.total, theme, cx),
                )
            }
            BreakdownTab::Providers => {
                let chart_rows = self.breakdown_top(&snapshot.providers, CHART_ROWS, cx);
                let result = self.breakdown_result(&snapshot.providers, cx);
                (
                    provider_meta(&snapshot.providers),
                    self.breakdown_chart(
                        &chart_rows,
                        result.totals.tokens.total,
                        "providers",
                        theme,
                        cx,
                    ),
                    self.breakdown_table(&result, "providers", theme, cx),
                    self.breakdown_totals(&result, theme),
                    self.breakdown_footer(result.page, result.page_size, result.total, theme, cx),
                )
            }
            BreakdownTab::Tools => {
                let chart_rows = self.tools_top(snapshot, CHART_ROWS, cx);
                let result = self.tools_result(snapshot, cx);
                (
                    tools_meta(snapshot),
                    self.tools_chart(&chart_rows, result.calls, theme),
                    self.tools_table(&result, theme, cx),
                    self.tools_totals(&result, theme),
                    self.breakdown_footer(result.page, result.page_size, result.total, theme, cx),
                )
            }
        };

        let search = search_box(self.breakdown_search(), theme);
        let toolbar = table_toolbar(search, self.breakdown_columns_control(theme, cx), theme);

        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base12.px(&theme))
            .child(tabs)
            .child(chart)
            .child(toolbar)
            .child(table)
            .child(totals)
            .child(footer);

        section(
            "usage-breakdowns",
            &tr!("usage.breakdown"),
            None,
            Some(meta),
            None,
            body.into_any_element(),
            theme,
        )
    }

    /// The Breakdown column-visibility control, mirroring the Sessions one.
    fn breakdown_columns_control(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.menu() == Some(MenuKind::BreakdownColumns);
        let entity = cx.entity();
        let chip = filters::chip(
            "usage-breakdown-columns-chip",
            tr!("usage.columns"),
            None,
            !self.breakdown_hidden_columns().is_empty(),
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::BreakdownColumns, window, cx)
                });
            },
        );
        if !open {
            return filters::chip_with_menu(
                "usage-breakdown-columns-anchor",
                chip,
                false,
                Corner::TopRight,
                theme,
                || div().into_any_element(),
            );
        }
        let panel = filters::breakdown_columns_menu(self, cx, theme);
        filters::chip_with_menu(
            "usage-breakdown-columns-anchor",
            chip,
            true,
            Corner::TopRight,
            theme,
            move || panel,
        )
    }

    /// The totals line under a token-dimension table.
    fn breakdown_totals(&self, result: &BreakdownQueryResult, theme: Theme) -> AnyElement {
        let page_requests: u64 = result.rows.iter().map(|row| row.totals.requests).sum();
        let page_tokens: u64 = result.rows.iter().map(|row| row.totals.tokens.total).sum();
        let mut filtered = tr!(
            "usage.filtered_totals_requests",
            requests = format::count(result.totals.requests),
            tokens = format::compact(result.totals.tokens.total)
        );
        if result.totals.cost_coverage() > 0.0 {
            filtered.push_str(&format!(" · {}", format::cost(result.totals.cost_usd)));
        }
        let page = tr!(
            "usage.page_requests",
            requests = format::count(page_requests),
            tokens = format::compact(page_tokens)
        );
        totals_row(&filtered, &page, theme)
    }

    /// The totals line under the Tools table.
    fn tools_totals(&self, result: &ToolQueryResult, theme: Theme) -> AnyElement {
        let page_calls: u64 = result.rows.iter().map(|row| row.calls).sum();
        let page_errors: u64 = result.rows.iter().map(|row| row.errors).sum();
        let filtered = tr!(
            "usage.filtered_totals_calls",
            calls = format::count(result.calls),
            failed = format::count(result.errors)
        );
        let page = tr!(
            "usage.page_calls",
            calls = format::count(page_calls),
            failed = format::count(page_errors)
        );
        totals_row(&filtered, &page, theme)
    }

    /// The breakdown footer: range, rows-per-page, and page stepping — the same
    /// shape as the Sessions footer, driven by the open dimension's state.
    fn breakdown_footer(
        &self,
        page: usize,
        page_size: usize,
        total: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pages = total.div_ceil(page_size.max(1)).max(1);
        let current = page.clamp(1, pages);
        let start = (current - 1) * page_size;
        let shown = page_size.min(total.saturating_sub(start));
        let summary = if total == 0 {
            tr!("usage.no_rows").to_string()
        } else {
            tr!(
                "usage.showing_range",
                first = format::count((start + 1) as u64),
                last = format::count((start + shown) as u64),
                total = format::count(total as u64)
            )
        };

        let size_open = self.menu() == Some(MenuKind::BreakdownPageSize);
        let entity = cx.entity();
        let size_chip = filters::chip(
            "usage-breakdown-page-size-chip",
            page_size.to_string(),
            None,
            false,
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::BreakdownPageSize, window, cx)
                });
            },
        );
        let size_panel = size_open.then(|| filters::breakdown_page_size_menu(self, cx, theme));
        let size_control = filters::chip_with_menu(
            "usage-breakdown-page-size-anchor",
            size_chip,
            size_open,
            Corner::TopRight,
            theme,
            move || size_panel.unwrap_or_else(|| div().into_any_element()),
        );

        let prev = filters::outline_button(
            "usage-breakdown-page-prev",
            &tr!("usage.previous"),
            current > 1,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| {
                        page.set_breakdown_page(current.saturating_sub(1), cx)
                    });
                }
            },
        );
        let next = filters::outline_button(
            "usage-breakdown-page-next",
            &tr!("usage.next"),
            current < pages,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_breakdown_page(current + 1, cx));
                }
            },
        );

        table_pager(summary, size_control, prev, next, current, pages, theme)
    }

    /// One page of a token dimension as a standard data table — the same
    /// component and column shape as the Sessions table. Rows are clickable to
    /// scope the page to that model / workspace / provider.
    fn breakdown_table(
        &self,
        result: &BreakdownQueryResult,
        kind: &'static str,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = &result.rows;
        let (columns, keys) = self.breakdown_columns();
        let total_w: f32 = columns.iter().map(|column| column.width).sum();
        let total_tokens = result.totals.tokens.total;
        let entity = cx.entity();
        let mut elements = Vec::with_capacity(rows.len());
        for (ix, row) in rows.iter().enumerate() {
            let last = ix + 1 == rows.len();
            let target = row.id;
            let mut line = div()
                .id(SharedString::from(format!(
                    "usage-breakdown-{kind}-{}",
                    row.label
                )))
                .h(px(ROW_H))
                .w_full()
                .min_w(px(total_w))
                .flex()
                .items_center()
                .when(!last, |row| row.border_b_1().border_color(theme.border))
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover));
            for column in &columns {
                let cell = match column.id {
                    "name" => breakdown_name_cell(column, row, theme),
                    "requests" => text_cell(
                        column,
                        format::count(row.totals.requests),
                        theme.text_secondary,
                        theme,
                    ),
                    "input" => text_cell(
                        column,
                        format::compact(row.totals.tokens.input),
                        theme.text_muted,
                        theme,
                    ),
                    "output" => text_cell(
                        column,
                        format::compact(row.totals.tokens.output),
                        theme.text_muted,
                        theme,
                    ),
                    "cache" => text_cell(
                        column,
                        format::compact(row.totals.tokens.cache_read),
                        theme.text_muted,
                        theme,
                    ),
                    "share" => {
                        let share = if total_tokens > 0 {
                            row.totals.tokens.total as f64 / total_tokens as f64
                        } else {
                            row.share
                        };
                        text_cell(column, format::share(share), theme.text_muted, theme)
                    }
                    _ => text_cell(
                        column,
                        format::compact(row.totals.tokens.total),
                        theme.text_primary,
                        theme,
                    ),
                };
                line = line.child(cell);
            }
            let click_entity = entity.clone();
            line = line.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                click_entity.update(cx, |page, cx| {
                    let mut filter = page.filter().clone();
                    match kind {
                        "workspaces" => toggle(&mut filter.workspaces, target),
                        "providers" => toggle(&mut filter.providers, target),
                        _ => toggle(&mut filter.models, target),
                    }
                    page.set_filter(filter, cx);
                });
            });
            elements.push(line.into_any_element());
        }
        // The table grows with its page: choosing 100 rows shows 100, not a
        // ten-row viewport. The page scrolls; the table never does.
        let visible = if result.rows.is_empty() {
            3
        } else {
            result.rows.len()
        } as f32;
        let height = HEADER_H + ROW_H * visible;
        self.table_element(
            TableKind::Breakdown,
            "usage-breakdown-table",
            columns,
            elements,
            height,
            empty_cell(&tr!("usage.no_rows_match_search"), theme),
            theme,
            Rc::new(move |page, ix, sort, cx| {
                let Some(Some(key)) = keys.get(ix).copied() else {
                    return;
                };
                match sort {
                    SortState::Ascending => page.set_breakdown_sort(key, false, cx),
                    SortState::Descending => page.set_breakdown_sort(key, true, cx),
                    SortState::Default => page.set_breakdown_sort(BreakdownSort::Tokens, true, cx),
                }
            }),
            cx,
        )
    }

    /// The ranked bar chart above a token dimension's table: the top rows as
    /// magnitude bars, so the shape reads before the figures. Rows are clickable
    /// to scope the page exactly like the table's rows.
    fn breakdown_chart(
        &self,
        rows: &[GroupRow],
        total: u64,
        kind: &'static str,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut chart = div()
            .w_full()
            .pb(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base02.px(&theme));
        if rows.is_empty() {
            chart = chart.child(empty_line(&tr!("usage.no_usage_in_range"), theme));
        }
        for row in rows {
            let fraction = if total > 0 {
                row.totals.tokens.total as f64 / total as f64
            } else {
                row.share
            };
            let target = row.id;
            let entity = cx.entity();
            chart = chart.child(
                div()
                    .id(SharedString::from(format!(
                        "usage-breakdown-chart-{kind}-{}",
                        row.label
                    )))
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base04.px(&theme))
                    .rounded(Radius::Medium.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_hover))
                    .child(chart_label(&row.label, row.sub.as_deref(), theme))
                    .child(bar_track(fraction, theme))
                    .child(chart_value(
                        &format::compact(row.totals.tokens.total),
                        theme,
                    ))
                    .child(chart_share(&format::share(fraction), theme))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        entity.update(cx, |page, cx| {
                            let mut filter = page.filter().clone();
                            match kind {
                                "workspaces" => toggle(&mut filter.workspaces, target),
                                "providers" => toggle(&mut filter.providers, target),
                                _ => toggle(&mut filter.models, target),
                            }
                            page.set_filter(filter, cx);
                        });
                    }),
            );
        }
        chart.into_any_element()
    }

    /// The ranked bar chart above the Tools table: the busiest tools as bars.
    fn tools_chart(&self, rows: &[ToolRow], total: u64, theme: Theme) -> AnyElement {
        let mut chart = div()
            .w_full()
            .pb(DynamicSpacing::Base12.px(&theme))
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base02.px(&theme));
        if rows.is_empty() {
            chart = chart.child(empty_line(&tr!("usage.no_tool_calls_in_range"), theme));
        }
        for row in rows {
            let fraction = if total > 0 {
                row.calls as f64 / total as f64
            } else {
                0.0
            };
            let tooltip = tr!(
                "usage.tool_chart_tooltip",
                name = row.label,
                calls = format::count(row.calls),
                errors = format::count(row.errors),
                avg = row
                    .avg_duration_ms()
                    .map(format::duration_ms)
                    .unwrap_or_else(|| "—".into())
            );
            chart = chart.child(
                div()
                    .id(SharedString::from(format!(
                        "usage-tools-chart-{}",
                        row.label
                    )))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base04.px(&theme))
                    .rounded(Radius::Medium.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .child(chart_label(
                        &row.label,
                        Some(row.class.label().as_str()),
                        theme,
                    ))
                    .child(bar_track(fraction, theme))
                    .child(chart_value(&format::count(row.calls), theme))
                    .child(chart_share(&format::share(fraction), theme)),
            );
        }
        chart.into_any_element()
    }

    /// The tools data table: every tool with its family, calls, failures, and
    /// duration. Ordered busiest first, the aggregate's own ranking.
    fn tools_table(
        &self,
        result: &ToolQueryResult,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = self.tool_columns();
        let total_w: f32 = columns.iter().map(|column| column.width).sum();
        let mut elements = Vec::with_capacity(result.rows.len());
        for (ix, row) in result.rows.iter().enumerate() {
            let last = ix + 1 == result.rows.len();
            let mut line = div()
                .h(px(ROW_H))
                .w_full()
                .min_w(px(total_w))
                .flex()
                .items_center()
                .when(!last, |row| row.border_b_1().border_color(theme.border))
                .hover(|style| style.bg(theme.surface_hover));
            for column in &columns {
                let cell = match column.id {
                    "tool" => cell_shell(column)
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(column.width - 96.))
                                .truncate()
                                .text_size(TextSize::Default.px(&theme))
                                .text_color(theme.text_primary)
                                .child(row.label.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(TextSize::XSmall.px(&theme))
                                .text_color(theme.text_muted)
                                .child(row.class.label()),
                        )
                        .into_any_element(),
                    "calls" => text_cell(
                        column,
                        format::count(row.calls),
                        theme.text_secondary,
                        theme,
                    ),
                    "errors" => text_cell(
                        column,
                        if row.errors > 0 {
                            format::count(row.errors)
                        } else {
                            "—".into()
                        },
                        if row.errors > 0 {
                            theme.error
                        } else {
                            theme.text_muted
                        },
                        theme,
                    ),
                    "avg" => text_cell(
                        column,
                        row.avg_duration_ms()
                            .map(format::duration_ms)
                            .unwrap_or_else(|| "—".into()),
                        theme.text_muted,
                        theme,
                    ),
                    _ => text_cell(
                        column,
                        if row.max_ms > 0 {
                            format::duration_ms(row.max_ms as f64)
                        } else {
                            "—".into()
                        },
                        theme.text_muted,
                        theme,
                    ),
                };
                line = line.child(cell);
            }
            elements.push(line.into_any_element());
        }
        let visible = if result.rows.is_empty() {
            3
        } else {
            result.rows.len()
        } as f32;
        let height = HEADER_H + ROW_H * visible;
        self.table_element(
            TableKind::Breakdown,
            "usage-tools-table",
            columns,
            elements,
            height,
            empty_cell(&tr!("usage.no_tools_match_search"), theme),
            theme,
            Rc::new(|_: &mut UsagePage, _: usize, _: SortState, _: &mut Context<UsagePage>| {}),
            cx,
        )
    }

    /// Token composition: input / output / cache read / cache write, as one
    /// stacked bar plus its four rows. The four categories sum to the total,
    /// so nothing is double-counted (§23).
    fn composition_body(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = snapshot.summary.totals.tokens;
        let total = tokens.total;
        let slices: [(String, u64, Hsla, ChartMetric); 4] = [
            (
                tr!("usage.slice_input"),
                tokens.input,
                theme.accent,
                ChartMetric::Input,
            ),
            (
                tr!("usage.slice_output"),
                tokens.output,
                theme.accent.opacity(0.62),
                ChartMetric::Output,
            ),
            (
                tr!("usage.slice_cache_read"),
                tokens.cache_read,
                theme.accent.opacity(0.40),
                ChartMetric::Cache,
            ),
            (
                tr!("usage.slice_cache_write"),
                tokens.cache_write,
                theme.accent.opacity(0.22),
                ChartMetric::Cache,
            ),
        ];

        let mut stack = div()
            .w_full()
            .h(px(8.))
            .rounded(Radius::Full.px(&theme))
            .overflow_hidden()
            .flex()
            .bg(theme.surface_secondary);
        for &(_, value, color, _) in slices.iter() {
            if value == 0 || total == 0 {
                continue;
            }
            stack = stack.child(
                div()
                    .h_full()
                    .w(relative(value as f32 / total as f32))
                    .bg(color),
            );
        }

        let mut content = div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(stack);
        for (label, value, color, metric) in slices.iter() {
            // Clicking a component points the main chart at it (§19); hover
            // surfaces the exact figure (§42).
            let entity = cx.entity();
            let metric = *metric;
            let share = if total == 0 {
                "—".to_string()
            } else {
                format::share(*value as f64 / total as f64)
            };
            let tooltip = tr!(
                "usage.slice_tooltip",
                label = label,
                tokens = format::exact(*value),
                share = share
            );
            content = content.child(
                div()
                    .id(SharedString::from(format!("usage-composition-{label}")))
                    .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
                    .px(DynamicSpacing::Base08.px(&theme))
                    .py(DynamicSpacing::Base04.px(&theme))
                    .rounded(Radius::Medium.px(&theme))
                    .flex()
                    .items_center()
                    .gap(DynamicSpacing::Base08.px(&theme))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_hover))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        entity.update(cx, |page, cx| page.set_metric(metric, cx));
                    })
                    .child(
                        div()
                            .size(px(8.))
                            .rounded(Radius::Full.px(&theme))
                            .flex_none()
                            .bg(*color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_secondary)
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .whitespace_nowrap()
                            .font(num_font())
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_primary)
                            .child(format::compact(*value)),
                    )
                    .child(
                        div()
                            .w(px(52.))
                            .flex_none()
                            .whitespace_nowrap()
                            .font(num_font())
                            .text_size(TextSize::Small.px(&theme))
                            .text_color(theme.text_muted)
                            .text_align(gpui::TextAlign::Right)
                            .child(share),
                    )
                    .into_any_element(),
            );
        }
        content = content.child(
            div()
                .px(DynamicSpacing::Base08.px(&theme))
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .child(if total == 0 {
                    tr!("usage.no_tokens_in_range")
                } else if tokens.cache_read == 0 && tokens.cache_write == 0 {
                    tr!("usage.cache_unavailable_providers")
                } else {
                    tr!("usage.composition_formula")
                }),
        );

        div().w_full().child(content).into_any_element()
    }

    // ── health panels ──────────────────────────────────────────────────────

    /// Cache performance, with the formula stated in the panel (§24/§25).
    fn cache_body(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cache = &snapshot.cache;
        let available = cache.is_available();
        let hit = cache.hit_rate;
        let mut content = div()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base08.px(&theme));
        if !available {
            content = content.child(empty_line(&tr!("usage.cache_unavailable_range"), theme));
            return div().w_full().child(content).into_any_element();
        }

        if let Some(rate) = hit {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(DynamicSpacing::Base06.px(&theme))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(DynamicSpacing::Base08.px(&theme))
                            .child(
                                div()
                                    .font(num_font())
                                    .text_size(theme.ui_px(20.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format::percent(rate)),
                            )
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_muted)
                                    .child(tr!("view.of_prompt_tokens_served_from_cache")),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(6.))
                            .rounded(Radius::Full.px(&theme))
                            .bg(theme.surface_secondary)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((rate / 100.0).clamp(0.0, 1.0) as f32))
                                    .rounded(Radius::Full.px(&theme))
                                    .bg(theme.accent),
                            ),
                    ),
            );
        }

        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base02.px(&theme))
                .children(
                    [
                        (tr!("usage.cache_reads"), format::compact(cache.cache_read)),
                        (
                            tr!("usage.cache_writes"),
                            format::compact(cache.cache_write),
                        ),
                        (
                            tr!("usage.uncached_input"),
                            format::compact(cache.uncached_input),
                        ),
                        (
                            tr!("usage.requests_served_from_cache"),
                            tr!(
                                "usage.of_total",
                                count = format::count(cache.cached_requests),
                                total = format::count(snapshot.summary.totals.requests)
                            ),
                        ),
                    ]
                    .into_iter()
                    .map(|(label, value)| stat_row(&label, &value, theme)),
                ),
        );

        // Hit rate across the window lives in its own full-width strip below
        // the two panels (see `hit_rate_trend`), so each well stays the same
        // height and the trend gets the width it needs.
        let entity = cx.entity();
        let cached_only = self.filter().cached_only;
        content = content.child(
            div()
                .px(DynamicSpacing::Base08.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base06.px(&theme))
                .text_size(TextSize::XSmall.px(&theme))
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .child(tr!("usage.hit_rate_formula")),
                )
                .child(
                    press(
                        button_frame(div().id("usage-cache-toggle"), &theme, ButtonSize::Compact)
                            .text_color(theme.text_secondary)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.accent_text)),
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        entity.update(cx, |page, cx| page.set_cached_only(!cached_only, cx));
                    })
                    .child(if cached_only {
                        tr!("usage.show_all_requests")
                    } else {
                        tr!("usage.show_cached_only")
                    }),
                ),
        );

        div().w_full().child(content).into_any_element()
    }

    /// The cache hit rate across the window, full width under the two health
    /// panels: one bar per bucket, so a drop is visible. Each bar carries its
    /// own hover readout — the strip is a measurement, not decoration — and a
    /// bucket with no cache traffic draws as a stub. `None` when there is too
    /// little cache traffic to read a trend.
    fn hit_rate_trend(&self, snapshot: &UsageSnapshot, theme: Theme) -> Option<AnyElement> {
        let readable = snapshot
            .series
            .points
            .iter()
            .filter(|point| point.totals.tokens.input + point.totals.tokens.cache_read > 0)
            .count();
        (readable >= 2).then(|| {
            subpanel(
                &tr!("view.hit_rate_over_time"),
                None,
                hit_rate_bars(&snapshot.series, theme),
                theme,
            )
        })
    }

    // ── health ─────────────────────────────────────────────────────────────

    /// Token composition beside cache performance, with the cache hit-rate
    /// trend as a full-width strip beneath so the two wells stay the same
    /// height. Each well recesses to the canvas so the troughs keep contrast
    /// on the raised section card — not a second card nested inside it.
    fn health_body(
        &self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let totals = &snapshot.summary.totals;
        let tokens = totals.tokens;
        let composition_meta = if tokens.total == 0 {
            tr!("usage.no_tokens_in_range_short")
        } else {
            tr!("usage.n_tokens", count = format::compact(tokens.total))
        };
        let cache_meta = match snapshot.cache.hit_rate {
            Some(rate) if snapshot.cache.is_available() => {
                tr!("usage.n_hit_rate", rate = format::percent(rate))
            }
            _ => tr!("usage.unavailable_short"),
        };
        let composition = subpanel(
            &tr!("usage.token_composition"),
            Some(composition_meta),
            self.composition_body(snapshot, theme, cx),
            theme,
        );
        let cache = subpanel(
            &tr!("usage.cache_performance"),
            Some(cache_meta),
            self.cache_body(snapshot, theme, cx),
            theme,
        );
        let trend = self.hit_rate_trend(snapshot, theme);
        let panels: AnyElement = if wide {
            div()
                .w_full()
                .flex()
                .items_start()
                .gap(DynamicSpacing::Base16.px(&theme))
                .child(div().flex_1().min_w_0().child(composition))
                .child(div().flex_1().min_w_0().child(cache))
                .into_any_element()
        } else {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base16.px(&theme))
                .child(composition)
                .child(cache)
                .into_any_element()
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base16.px(&theme))
            .child(panels)
            .children(trend)
            .into_any_element()
    }

    // ── records ────────────────────────────────────────────────────────────

    /// One records section, three tables behind a selector: the searchable
    /// session list, the day/week/month rollup, and the failure log. Keeps the
    /// page from ending in three stacked, mostly-empty table sections.
    fn details_section(
        &mut self,
        snapshot: &UsageSnapshot,
        theme: Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.detail_tab();
        let tabs = segmented(
            "usage-detail-tab",
            &DetailTab::ALL,
            tab,
            |choice| choice.label(),
            |choice| choice.as_str(),
            |_| true,
            true,
            theme,
            {
                let entity = cx.entity();
                move |choice, _, cx| {
                    entity.update(cx, |page, cx| page.set_detail_tab(choice, cx));
                }
            },
        );
        let tabs_row = div().pb(DynamicSpacing::Base12.px(&theme)).child(tabs);

        let (meta, content) = match tab {
            DetailTab::Sessions => {
                let result = self.session_page(cx);
                let all = snapshot.sessions.len();
                let meta = if result.total == all {
                    tr!("usage.n_sessions", count = format::count(all as u64))
                } else {
                    tr!(
                        "usage.n_of_n_sessions",
                        count = format::count(result.total as u64),
                        total = format::count(all as u64)
                    )
                };
                let table = self.sessions_content(&result, theme, cx);
                let toolbar = table_toolbar(
                    self.session_search_box(theme),
                    self.columns_control(theme, cx),
                    theme,
                );
                (
                    meta,
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base12.px(&theme))
                        .child(tabs_row)
                        .child(toolbar)
                        .child(table)
                        .into_any_element(),
                )
            }
            DetailTab::Daily => {
                let granularity = snapshot.buckets.granularity;
                let result = self.bucket_result(snapshot, cx);
                let all = snapshot.buckets.rows.len();
                let meta = if result.total == all {
                    tr!(
                        "usage.n_buckets_by",
                        buckets = format::count(all as u64),
                        by = granularity.label()
                    )
                } else {
                    tr!(
                        "usage.n_of_n_buckets_by",
                        buckets = format::count(result.total as u64),
                        total = format::count(all as u64),
                        by = granularity.label()
                    )
                };
                let table = self.buckets_content(&result, granularity, theme, cx);
                let toolbar = table_toolbar(
                    search_box(self.bucket_search(), theme),
                    self.bucket_columns_control(theme, cx),
                    theme,
                );
                (
                    meta,
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base12.px(&theme))
                        .child(tabs_row)
                        .child(toolbar)
                        .child(table)
                        .into_any_element(),
                )
            }
            DetailTab::Failures => {
                let errors = &snapshot.errors;
                let result = self.failure_result(cx);
                let all = errors.rows.len();
                let meta = if result.total == all {
                    tr!(
                        "usage.failures_meta",
                        provider = format::count(errors.provider),
                        tool = format::count(errors.tool),
                        stopped = format::count(errors.aborted)
                    )
                } else {
                    tr!(
                        "usage.failures_meta_filtered",
                        events = format::count(result.total as u64),
                        total = format::count(all as u64),
                        provider = format::count(errors.provider),
                        tool = format::count(errors.tool),
                        stopped = format::count(errors.aborted)
                    )
                };
                let table = self.failures_content(&result, theme, cx);
                let toolbar = table_toolbar(
                    search_box(self.failure_search(), theme),
                    self.failure_columns_control(theme, cx),
                    theme,
                );
                (
                    meta,
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base12.px(&theme))
                        .child(tabs_row)
                        .child(toolbar)
                        .child(table)
                        .into_any_element(),
                )
            }
        };

        section(
            "usage-details",
            &tr!("usage.records"),
            None,
            Some(meta),
            None,
            content,
            theme,
        )
    }

    /// The sessions search field, as it appears in the details header.
    fn session_search_box(&self, theme: Theme) -> AnyElement {
        search_box(self.search(), theme)
    }

    /// The session table: searchable, sortable, pageable (§48/§49/§50).
    ///
    /// The page owns the ordering, so a header click comes back here before
    /// anything re-sorts; the table just draws the page it is handed.
    fn sessions_content(
        &self,
        result: &SessionQueryResult,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The viewport matches the current page so the page — not a nested
        // table — is the scroller. Pagination already caps how many rows land
        // here.
        let visible_rows = result.rows.len().max(3) as f32;
        let table_h = ROW_H * visible_rows + HEADER_H;
        let footer = self.pagination_footer(result, theme, cx);

        let (columns, keys) = self.session_columns();
        let rows = self.session_row_elements(&result.rows, &columns, &keys, theme, cx);
        let table = self.table_element(
            TableKind::Sessions,
            "usage-sessions-table",
            columns,
            rows,
            table_h,
            empty_cell(&tr!("usage.no_sessions_match_search"), theme),
            theme,
            Rc::new(
                move |page, ix, sort, cx| match (keys.get(ix).copied().flatten(), sort) {
                    (Some(key), SortState::Ascending) => page.set_session_sort(key, false, cx),
                    (Some(key), SortState::Descending) => page.set_session_sort(key, true, cx),
                    _ => page.set_session_sort(SessionSort::Tokens, true, cx),
                },
            ),
            cx,
        );

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(table)
            .children(self.totals_line(result, theme))
            .child(footer)
            .into_any_element()
    }

    /// Day/week/month table. The granularity adapts with the range, so the
    /// same table is the weekly and monthly trend view for long windows (§42).
    fn buckets_content(
        &self,
        result: &BucketQueryResult,
        granularity: Granularity,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible_rows = result.rows.len().max(3) as f32;
        let table_h = ROW_H * visible_rows + HEADER_H;
        let (columns, keys) = self.bucket_columns();
        let rows = self.bucket_row_elements(&result.rows, &columns, &keys, granularity, theme);
        let table = self.table_element(
            TableKind::Buckets,
            "usage-buckets-table",
            columns,
            rows,
            table_h,
            empty_cell(&tr!("usage.no_buckets_match_search"), theme),
            theme,
            Rc::new(
                move |page, ix, sort, cx| match (keys.get(ix).copied(), sort) {
                    (Some(key), SortState::Ascending) => page.set_bucket_sort(key, false, cx),
                    (Some(key), SortState::Descending) => page.set_bucket_sort(key, true, cx),
                    _ => page.set_bucket_sort(BucketSort::Date, true, cx),
                },
            ),
            cx,
        );

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(table)
            .child(self.bucket_totals(result, theme))
            .child(self.bucket_pagination_footer(result, theme, cx))
            .into_any_element()
    }

    /// Failures (§34): every provider error and tool failure in the range, in
    /// the same table as the rest of the page.
    fn failures_content(
        &self,
        result: &FailureQueryResult,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible_rows = result.rows.len().max(3) as f32;
        let table_h = ROW_H * visible_rows + HEADER_H;
        let (columns, keys) = self.failure_columns();
        let rows = self.failure_row_elements(&result.rows, &columns, &keys, theme);
        let table = self.table_element(
            TableKind::Failures,
            "usage-failures-table",
            columns,
            rows,
            table_h,
            empty_cell(&tr!("usage.no_failures_match_search"), theme),
            theme,
            Rc::new(move |page, ix, sort, cx| {
                let key = keys.get(ix).copied().unwrap_or(FailureSort::When);
                match sort {
                    SortState::Ascending => page.set_failure_sort(key, false, cx),
                    SortState::Descending => page.set_failure_sort(key, true, cx),
                    SortState::Default => page.set_failure_sort(FailureSort::When, true, cx),
                }
            }),
            cx,
        );

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(table)
            .child(self.failure_totals(result, theme))
            .child(self.failure_pagination_footer(result, theme, cx))
            .child(retry_note(theme))
            .into_any_element()
    }

    // ── column plans ───────────────────────────────────────────────────────

    /// Session columns plus their sort keys (the trailing affordance is not
    /// sortable). Widths come from the plan, overridden by any divider drag.
    fn session_columns(&self) -> (Vec<Column>, Vec<Option<SessionSort>>) {
        // Room the table steals for its scrollbar, hairlines and cell padding.
        const CHROME: f32 = 48.;
        const GAPS: f32 = 12.;
        const TITLE_MIN: f32 = 180.;
        const OPEN_W: f32 = 38.;
        let budget = (self.table_width() - CHROME).max(360.);
        let hidden = self.hidden_columns();
        let (sort, desc) = self.session_sort_state();

        // `true` marks a column dropped when the window is too narrow (and
        // hidden by the user on request).
        let plan: [(SessionSort, f32, bool); 12] = [
            (SessionSort::Title, 0., false),
            (SessionSort::Workspace, 110., false),
            (SessionSort::Provider, 110., false),
            (SessionSort::Model, 156., false),
            (SessionSort::Started, 88., true),
            (SessionSort::Duration, 92., false),
            (SessionSort::Requests, 92., false),
            (SessionSort::Input, 70., true),
            (SessionSort::Output, 76., true),
            (SessionSort::Cache, 72., true),
            (SessionSort::Tokens, 82., false),
            (SessionSort::Errors, 82., false),
        ];
        let keep: Vec<(SessionSort, f32, bool)> = plan
            .into_iter()
            .filter(|(key, _, _)| *key == SessionSort::Title || !hidden.contains(key))
            .collect();

        // Optional columns are added in priority order while the title still
        // breathes: the per-bucket token columns are the first to go.
        let mut running: f32 = keep
            .iter()
            .filter(|(key, _, optional)| !*optional && *key != SessionSort::Title)
            .map(|(key, default, _)| self.col_width(TableKind::Sessions, key.as_str(), *default))
            .sum();
        let mut keep_optional: Vec<SessionSort> = Vec::new();
        for (key, default, optional) in &keep {
            if !*optional {
                continue;
            }
            let width = self.col_width(TableKind::Sessions, key.as_str(), *default);
            if running + width + OPEN_W + GAPS + TITLE_MIN <= budget {
                running += width;
                keep_optional.push(*key);
            }
        }
        let title_default = (budget - running - OPEN_W - GAPS).clamp(TITLE_MIN, 460.);

        let mut columns = Vec::with_capacity(keep.len() + 1);
        let mut keys = Vec::with_capacity(keep.len() + 1);
        for (key, default, optional) in keep {
            if optional && !keep_optional.contains(&key) {
                continue;
            }
            let default = if key == SessionSort::Title {
                title_default
            } else {
                default
            };
            let width = self.col_width(TableKind::Sessions, key.as_str(), default);
            let numeric = !matches!(
                key,
                SessionSort::Title
                    | SessionSort::Workspace
                    | SessionSort::Provider
                    | SessionSort::Model
            );
            let mut column = Column::new(key.as_str(), key.label())
                .width(width)
                .sortable()
                .sort(sort_state(key == sort, desc));
            if numeric {
                column = column.numeric();
            }
            columns.push(column);
            keys.push(Some(key));
        }
        // The trailing affordance: no header label, no sorting, fixed width.
        columns.push(Column::new("open", "").width(OPEN_W));
        keys.push(None);
        (columns, keys)
    }

    /// The token breakdown table's columns plus their sort keys — the same
    /// shape as the Sessions table: a name column that takes the remainder,
    /// then fixed numeric columns, with Share as the trailing display column.
    fn breakdown_columns(&self) -> (Vec<Column>, Vec<Option<BreakdownSort>>) {
        let hidden = self.breakdown_hidden_columns();
        let is_hidden = |id: &'static str| hidden.contains(&id);
        let measures: [(BreakdownSort, f32); 5] = [
            (BreakdownSort::Requests, 92.),
            (BreakdownSort::Input, 80.),
            (BreakdownSort::Output, 80.),
            (BreakdownSort::Cache, 80.),
            (BreakdownSort::Tokens, 88.),
        ];
        let mut fixed = 0.;
        for (key, default) in measures {
            if !is_hidden(key.as_str()) {
                fixed += default;
            }
        }
        if !is_hidden("share") {
            fixed += 72.;
        }
        let budget = self.table_width();
        let name_w = (budget - fixed).clamp(160., 460.);
        let (sort, desc) = self.breakdown_sort_state();
        let mut columns = Vec::with_capacity(7);
        let mut keys = Vec::with_capacity(7);
        columns.push(
            Column::new(BreakdownSort::Name.as_str(), BreakdownSort::Name.label())
                .width(self.col_width(TableKind::Breakdown, "name", name_w))
                .sortable()
                .sort(sort_state(sort == BreakdownSort::Name, desc)),
        );
        keys.push(Some(BreakdownSort::Name));
        for (key, default) in measures {
            if is_hidden(key.as_str()) {
                continue;
            }
            columns.push(
                Column::new(key.as_str(), key.label())
                    .width(self.col_width(TableKind::Breakdown, key.as_str(), default))
                    .numeric()
                    .sortable()
                    .sort(sort_state(key == sort, desc)),
            );
            keys.push(Some(key));
        }
        if !is_hidden("share") {
            columns.push(
                Column::new("share", "Share")
                    .width(self.col_width(TableKind::Breakdown, "share", 72.))
                    .numeric(),
            );
            keys.push(None);
        }
        (columns, keys)
    }

    /// The tools table's columns, with any hidden ones dropped. The tool name
    /// takes the room left over; every measure is its own narrow column.
    fn tool_columns(&self) -> Vec<Column> {
        let hidden = self.breakdown_hidden_columns();
        let is_hidden = |id: &'static str| hidden.contains(&id);
        let measures: [(&'static str, &'static str, f32); 4] = [
            ("calls", "Calls", 76.),
            ("errors", "Errors", 80.),
            ("avg", "Avg", 76.),
            ("max", "Max", 76.),
        ];
        let mut fixed = 0.;
        for (id, _, default) in measures {
            if !is_hidden(id) {
                fixed += default;
            }
        }
        let budget = self.table_width();
        let name_w = (budget - fixed).clamp(160., 460.);
        let mut columns = vec![Column::new("tool", "Tool").width(name_w)];
        for (id, label, default) in measures {
            if !is_hidden(id) {
                columns.push(Column::new(id, label).width(default).numeric());
            }
        }
        columns
    }

    /// Breakdown columns plus their sort keys. Hidden columns are dropped;
    /// Date always stays.
    fn bucket_columns(&self) -> (Vec<Column>, Vec<BucketSort>) {
        let (sort, desc) = self.bucket_sort_state();
        let plan: [(BucketSort, f32, bool); 5] = [
            (BucketSort::Date, 0., false),
            (BucketSort::Requests, 104., true),
            (BucketSort::Tokens, 96., true),
            (BucketSort::Cache, 104., true),
            (BucketSort::Errors, 84., true),
        ];
        let keep: Vec<(BucketSort, f32, bool)> = plan
            .into_iter()
            .filter(|(key, _, _)| self.bucket_column_visible(*key))
            .collect();
        let fixed: f32 = keep
            .iter()
            .filter(|(key, _, _)| *key != BucketSort::Date)
            .map(|(key, default, _)| self.col_width(TableKind::Buckets, key.id(), *default))
            .sum();
        let date_default = (self.table_width() - fixed).clamp(160., 480.);
        let mut columns = Vec::with_capacity(keep.len());
        let mut keys = Vec::with_capacity(keep.len());
        for (key, default, numeric) in keep {
            let default = if key == BucketSort::Date {
                date_default
            } else {
                default
            };
            let width = self.col_width(TableKind::Buckets, key.id(), default);
            let mut column = Column::new(key.id(), key.label())
                .width(width)
                .sortable()
                .sort(sort_state(key == sort, desc));
            if numeric {
                column = column.numeric();
            }
            columns.push(column);
            keys.push(key);
        }
        (columns, keys)
    }

    /// Usage-over-time columns: Time plus the active metric, then requests and
    /// tokens so the table agrees with the plot above it. Hidden columns are
    /// dropped here; Time always stays.
    fn series_columns(&self, metric: ChartMetric) -> (Vec<Column>, Vec<SeriesSort>) {
        let (sort, desc) = self.series_sort_state();
        let plan: [(SeriesSort, String, f32, bool); 4] = [
            (SeriesSort::Time, tr!("usage.col_time"), 0., false),
            (SeriesSort::Value, metric.label(), 112., true),
            (
                SeriesSort::Requests,
                tr!("usage.metric_requests"),
                104.,
                true,
            ),
            (SeriesSort::Tokens, tr!("usage.metric_tokens"), 96., true),
        ];
        let keep: Vec<(SeriesSort, String, f32, bool)> = plan
            .into_iter()
            .filter(|(key, _, _, _)| self.series_column_visible(*key))
            .collect();
        let fixed: f32 = keep
            .iter()
            .filter(|(key, _, _, _)| *key != SeriesSort::Time)
            .map(|(key, _, default, _)| self.col_width(TableKind::Series, key.id(), *default))
            .sum();
        let time_default = (self.table_width() - fixed).clamp(160., 480.);
        let mut columns = Vec::with_capacity(keep.len());
        let mut keys = Vec::with_capacity(keep.len());
        for (key, label, default, numeric) in keep {
            let default = if key == SeriesSort::Time {
                time_default
            } else {
                default
            };
            let width = self.col_width(TableKind::Series, key.id(), default);
            let mut column = Column::new(key.id(), label)
                .width(width)
                .sortable()
                .sort(sort_state(key == sort, desc));
            if numeric {
                column = column.numeric();
            }
            columns.push(column);
            keys.push(key);
        }
        (columns, keys)
    }

    /// Failure columns plus their sort keys. The message takes the remainder.
    /// Hidden columns are dropped; When always stays.
    fn failure_columns(&self) -> (Vec<Column>, Vec<FailureSort>) {
        let (sort, desc) = self.failure_sort_state();
        let plan: [(FailureSort, f32, bool); 5] = [
            (FailureSort::When, 0., false),
            (FailureSort::Kind, 96., false),
            (FailureSort::Model, 150., false),
            (FailureSort::Session, 200., false),
            (FailureSort::Message, 180., false),
        ];
        let keep: Vec<(FailureSort, f32, bool)> = plan
            .into_iter()
            .filter(|(key, _, _)| self.failure_column_visible(*key))
            .collect();
        let message_visible = keep.iter().any(|(key, _, _)| *key == FailureSort::Message);
        let fixed: f32 = keep
            .iter()
            .filter(|(key, _, _)| *key != FailureSort::When && *key != FailureSort::Message)
            .map(|(key, default, _)| self.col_width(TableKind::Failures, key.id(), *default))
            .sum();
        let flex_default = if message_visible {
            (self.table_width() - fixed - 108.).clamp(180., 560.)
        } else {
            (self.table_width() - fixed).clamp(160., 480.)
        };
        let mut columns = Vec::with_capacity(keep.len());
        let mut keys = Vec::with_capacity(keep.len());
        for (key, default, _) in keep {
            let default = match key {
                FailureSort::When if !message_visible => flex_default,
                FailureSort::When => 108.,
                FailureSort::Message => flex_default,
                _ => default,
            };
            let width = self.col_width(TableKind::Failures, key.id(), default);
            let mut column = Column::new(key.id(), key.label()).width(width).resizable();
            if key != FailureSort::Message {
                column = column.sortable().sort(sort_state(key == sort, desc));
            }
            columns.push(column);
            keys.push(key);
        }
        (columns, keys)
    }

    // ── row plans ──────────────────────────────────────────────────────────

    /// One session per row, cell content in the register its column deserves
    /// (§54). The selected row is the page's scoped session.
    fn session_row_elements(
        &self,
        rows: &[SessionRow],
        columns: &[Column],
        keys: &[Option<SessionSort>],
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let selected = self.filter().session;
        let entity = cx.entity();
        let context_row = self.context_row();
        let mut out = Vec::with_capacity(rows.len());
        for (ix, row) in rows.iter().enumerate() {
            let last = ix + 1 == rows.len();
            let mut line = div()
                .id(("usage-session-row", ix))
                .group("usage-row")
                .h(px(ROW_H))
                .w_full()
                .min_w(px(columns.iter().map(|column| column.width).sum::<f32>()))
                .flex()
                .items_center()
                .when(!last, |row| row.border_b_1().border_color(theme.border))
                .hover(|style| style.bg(theme.surface_hover))
                .when(selected == Some(row.session), |line| {
                    line.bg(theme.selection)
                })
                .on_mouse_down(MouseButton::Left, {
                    let entity = entity.clone();
                    let session = row.session;
                    move |event, window, cx| {
                        if event.click_count >= 2 {
                            entity.update(cx, |page, cx| page.open_session(window, cx, session));
                        } else {
                            entity.update(cx, |page, cx| page.set_session_scope(Some(session), cx));
                        }
                    }
                })
                .on_mouse_down(MouseButton::Right, {
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |page, cx| page.open_context_menu(ix, cx));
                    }
                });
            for (col_ix, column) in columns.iter().enumerate() {
                let cell = match keys.get(col_ix).copied().flatten() {
                    Some(key) => session_cell(row, key, theme, column),
                    None => open_session_cell(row.session, entity.clone(), theme, column),
                };
                line = line.child(cell);
            }
            if context_row == Some(ix) {
                line = line.child(session_context_menu(row, theme, entity.clone()));
            }
            out.push(line.into_any_element());
        }
        out
    }

    /// One bucket per row. The date column carries the granularity as a quieter
    /// suffix, so an all-time monthly table still says what each row is.
    fn bucket_row_elements(
        &self,
        rows: &[BucketRow],
        columns: &[Column],
        keys: &[BucketSort],
        granularity: Granularity,
        theme: Theme,
    ) -> Vec<AnyElement> {
        let total_w: f32 = columns.iter().map(|column| column.width).sum();
        let mut out = Vec::with_capacity(rows.len());
        for (ix, row) in rows.iter().enumerate() {
            let last = ix + 1 == rows.len();
            let mut line = div()
                .h(px(ROW_H))
                .w_full()
                .min_w(px(total_w))
                .flex()
                .items_center()
                .when(!last, |row| row.border_b_1().border_color(theme.border))
                .hover(|style| style.bg(theme.surface_hover));
            for (ix, column) in columns.iter().enumerate() {
                let key = keys.get(ix).copied().unwrap_or(BucketSort::Date);
                if key == BucketSort::Date {
                    line = line.child(
                        cell_shell(column)
                            .gap(DynamicSpacing::Base08.px(&theme))
                            .child(
                                div()
                                    .text_size(TextSize::Small.px(&theme))
                                    .text_color(theme.text_primary)
                                    .child(row.label.clone()),
                            )
                            .child(
                                div()
                                    .text_size(TextSize::XSmall.px(&theme))
                                    .text_color(theme.text_muted)
                                    .child(granularity.label()),
                            ),
                    );
                } else {
                    let (text, color) = match key {
                        BucketSort::Requests => {
                            (format::count(row.totals.requests), theme.text_secondary)
                        }
                        BucketSort::Tokens => {
                            (format::compact(row.totals.tokens.total), theme.text_primary)
                        }
                        BucketSort::Cache => (
                            match row.totals.cache_hit_rate() {
                                Some(rate) => format::percent(rate),
                                None => "—".to_string(),
                            },
                            theme.text_muted,
                        ),
                        BucketSort::Errors => (
                            format::count(row.totals.errors),
                            if row.totals.errors > 0 {
                                theme.error
                            } else {
                                theme.text_muted
                            },
                        ),
                        BucketSort::Date => unreachable!("handled above"),
                    };
                    line = line.child(text_cell(column, text, color, theme));
                }
            }
            out.push(line.into_any_element());
        }
        out
    }

    /// One failure per row, newest first unless the user re-sorted.
    fn failure_row_elements(
        &self,
        rows: &[super::table::FailureRow],
        columns: &[Column],
        keys: &[FailureSort],
        theme: Theme,
    ) -> Vec<AnyElement> {
        let total_w: f32 = columns.iter().map(|column| column.width).sum();
        let mut elements = Vec::with_capacity(rows.len());
        for (ix, row) in rows.iter().enumerate() {
            let last = ix + 1 == rows.len();
            let mut line = div()
                .h(px(ROW_H))
                .w_full()
                .min_w(px(total_w))
                .flex()
                .items_center()
                .when(!last, |row| row.border_b_1().border_color(theme.border))
                .hover(|style| style.bg(theme.surface_hover));
            for (ix, column) in columns.iter().enumerate() {
                let key = keys.get(ix).copied().unwrap_or(FailureSort::When);
                let (label, color) = match key {
                    FailureSort::When => (super::table::failure_when(row.ts_ms), theme.text_muted),
                    FailureSort::Kind => {
                        let (label, color) = super::table::failure_kind_register(row.kind, theme);
                        (label, color)
                    }
                    FailureSort::Model => (row.model.clone(), theme.text_secondary),
                    FailureSort::Session => (row.session_title.clone(), theme.text_secondary),
                    FailureSort::Message => (row.message.clone(), theme.text_muted),
                };
                line = line.child(text_cell(column, label, color, theme));
            }
            elements.push(line.into_any_element());
        }
        elements
    }

    /// A table with its handlers wired to this page. The column ids and widths
    /// are captured so a divider drag knows what it grabbed; `on_sort` maps the
    /// clicked column to a key.
    #[allow(clippy::too_many_arguments)]
    fn table_element(
        &self,
        table: TableKind,
        id: &'static str,
        columns: Vec<Column>,
        rows: Vec<AnyElement>,
        height: f32,
        empty: AnyElement,
        theme: Theme,
        on_sort: Rc<SetSort>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids = Rc::new(columns.iter().map(|column| column.id).collect::<Vec<_>>());
        let widths = Rc::new(
            columns
                .iter()
                .map(|column| column.width)
                .collect::<Vec<_>>(),
        );
        let handlers = table_handlers(cx.entity(), table, ids, widths, on_sort);
        data_table(id, &columns, rows, px(height), empty, theme, handlers)
    }

    /// The column-visibility control for the sessions table (§41).
    fn columns_control(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let open = self.menu() == Some(MenuKind::Columns);
        let entity = cx.entity();
        let chip = filters::chip(
            "usage-columns-chip",
            tr!("usage.columns"),
            None,
            !self.hidden_columns().is_empty(),
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::Columns, window, cx)
                });
            },
        );
        if !open {
            return filters::chip_with_menu(
                "usage-columns-anchor",
                chip,
                false,
                Corner::TopRight,
                theme,
                || div().into_any_element(),
            );
        }
        let panel = filters::columns_menu(self, cx, theme);
        filters::chip_with_menu(
            "usage-columns-anchor",
            chip,
            true,
            Corner::TopRight,
            theme,
            move || panel,
        )
    }

    /// The footer summary (§53): totals over the whole filtered set, clearly
    /// distinguished from the totals on the visible page.
    fn totals_line(&self, result: &SessionQueryResult, theme: Theme) -> Option<AnyElement> {
        let snapshot = self.snapshot()?;
        let totals = &snapshot.summary.totals;
        let page_requests: u64 = result.rows.iter().map(|row| row.totals.requests).sum();
        let page_tokens: u64 = result.rows.iter().map(|row| row.totals.tokens.total).sum();

        let mut filtered = tr!(
            "usage.filtered_totals_requests",
            requests = format::count(totals.requests),
            tokens = format::compact(totals.tokens.total)
        );
        if totals.cost_coverage() > 0.0 {
            filtered.push_str(&format!(" · {}", format::cost(totals.cost_usd)));
        }
        let page = tr!(
            "usage.page_requests",
            requests = format::count(page_requests),
            tokens = format::compact(page_tokens)
        );

        Some(
            div()
                .w_full()
                .pt(DynamicSpacing::Base08.px(&theme))
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base12.px(&theme))
                .text_size(TextSize::Small.px(&theme))
                .child(div().text_color(theme.text_muted).child(filtered))
                .child(div().flex_1())
                .child(div().text_color(theme.text_muted).child(page))
                .into_any_element(),
        )
    }

    /// The sessions footer (§27/§75): range, rows-per-page, and page stepping.
    fn pagination_footer(
        &self,
        result: &SessionQueryResult,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pages = result.page_count();
        let current = result.page;
        let summary = if result.total == 0 {
            tr!("usage.no_sessions_match").to_string()
        } else {
            tr!(
                "usage.showing_range",
                first = format::count(result.first_row() as u64),
                last = format::count(result.last_row() as u64),
                total = format::count(result.total as u64)
            )
        };

        let size_open = self.menu() == Some(MenuKind::PageSize);
        let entity = cx.entity();
        let size_chip = filters::chip(
            "usage-page-size-chip",
            result.page_size.to_string(),
            None,
            false,
            true,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    page.toggle_menu(MenuKind::PageSize, window, cx)
                });
            },
        );
        let size_panel = size_open.then(|| filters::page_size_menu(self, cx, theme));
        let size_control = filters::chip_with_menu(
            "usage-page-size-anchor",
            size_chip,
            size_open,
            Corner::TopRight,
            theme,
            move || size_panel.unwrap_or_else(|| div().into_any_element()),
        );

        let prev = filters::outline_button(
            "usage-page-prev",
            &tr!("usage.previous"),
            current > 1,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_page(current.saturating_sub(1), cx));
                }
            },
        );
        let next = filters::outline_button(
            "usage-page-next",
            &tr!("usage.next"),
            current < pages,
            theme,
            {
                let entity = cx.entity();
                move |_, _, cx| {
                    entity.update(cx, |page, cx| page.set_page(current + 1, cx));
                }
            },
        );

        table_pager(summary, size_control, prev, next, current, pages, theme)
    }
}

/// The sort state for a column: only the active key shows a direction.
fn sort_state(active: bool, desc: bool) -> SortState {
    if !active {
        SortState::Default
    } else if desc {
        SortState::Descending
    } else {
        SortState::Ascending
    }
}

/// A header click routed to the page: which column and the state to move to.
type SetSort = dyn Fn(&mut UsagePage, usize, SortState, &mut Context<UsagePage>);

/// Wire a table's handlers to the page: header clicks sort, divider drags
/// resize the column they grabbed.
fn table_handlers(
    entity: Entity<UsagePage>,
    table: TableKind,
    ids: Rc<Vec<&'static str>>,
    widths: Rc<Vec<f32>>,
    on_sort: Rc<SetSort>,
) -> TableHandlers {
    let sort_entity = entity.clone();
    let start_entity = entity.clone();
    let move_entity = entity.clone();
    let end_entity = entity;
    TableHandlers::new(
        move |ix, sort, _window, cx| {
            sort_entity.update(cx, |page, cx| on_sort(page, ix, sort, cx));
        },
        move |ix, x, _window, cx| {
            if let (Some(id), Some(width)) = (ids.get(ix).copied(), widths.get(ix).copied()) {
                start_entity.update(cx, |page, _| page.begin_resize(table, id, x, width));
            }
        },
        move |_ix, x, _window, cx| {
            move_entity.update(cx, |page, cx| page.drag_resize(x, cx));
        },
        move |_window, cx| {
            end_entity.update(cx, |page, _| page.end_resize());
        },
    )
}

/// One session cell, in the register its column deserves (§54).
fn session_cell(row: &SessionRow, key: SessionSort, theme: Theme, column: &Column) -> AnyElement {
    let totals = row.totals;
    let (text, color) = match key {
        SessionSort::Title => (row.title.clone(), theme.text_primary),
        SessionSort::Workspace => (row.workspace.clone(), theme.text_secondary),
        SessionSort::Provider => (row.provider.clone(), theme.text_secondary),
        SessionSort::Model => (row.top_model.clone(), theme.text_secondary),
        SessionSort::Started => (
            super::model::bucket_label(row.ended_ms, Granularity::Day),
            theme.text_muted,
        ),
        SessionSort::Duration => (format::span_ms(row.duration_ms()), theme.text_muted),
        SessionSort::Requests => (format::count(totals.requests), theme.text_secondary),
        SessionSort::Input => (format::compact(totals.tokens.input), theme.text_muted),
        SessionSort::Output => (format::compact(totals.tokens.output), theme.text_muted),
        SessionSort::Cache => (
            // "0" would claim the provider reported no cache reuse; "—" says
            // nothing was reported at all.
            if totals.tokens.cache_read == 0 && !row.cache_capable {
                "—".to_string()
            } else {
                format::compact(totals.tokens.cache_read)
            },
            theme.text_muted,
        ),
        SessionSort::Tokens => (format::compact(totals.tokens.total), theme.text_primary),
        SessionSort::Errors => {
            let errors = totals.errors + row.tool_errors;
            (
                format::count(errors),
                if errors > 0 {
                    theme.error
                } else {
                    theme.text_muted
                },
            )
        }
        // Tool calls are shown in the tool activity panel, which ranks them
        // properly; the table's column plan does not include one.
        SessionSort::Tools => (format::count(row.tool_runs), theme.text_muted),
    };
    text_cell(column, text, color, theme)
}

/// The trailing affordance: open this session in the chat surface. Hidden
/// until its row is hovered, so thirteen rows do not ship thirteen arrows.
fn open_session_cell(
    session: u16,
    page: Entity<UsagePage>,
    theme: Theme,
    column: &Column,
) -> AnyElement {
    div()
        .w(px(column.width))
        .h_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            icon_button_frame(div(), &theme, ButtonSize::Default)
                .id(SharedString::from(format!("usage-open-{session}")))
                .tip(tr!("usage.ctx_open_session"))
                .cursor_pointer()
                .opacity(0.)
                .group_hover("usage-row", |style| style.opacity(1.))
                .hover(|style| style.bg(theme.overlay_strong))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    // Do not let the row's own click handler also fire: this
                    // is a jump to another surface, not a scope change.
                    cx.stop_propagation();
                    page.update(cx, |page, cx| page.open_session(window, cx, session));
                })
                .child(icon(
                    "icons/arrow-up-right.svg",
                    ButtonSize::Default.icon_size().px(&theme),
                    theme.text_muted,
                )),
        )
        .into_any_element()
}

/// The session row's right-click menu (§43): only actions that are wired.
fn session_context_menu(row: &SessionRow, theme: Theme, page: Entity<UsagePage>) -> AnyElement {
    let session = row.session;
    let session_id = row.id.clone();
    let provider_id = row.provider_id;
    let model_id = row.top_model_id;
    let provider_label = row.provider.clone();
    let model_label = row.top_model.clone();

    let mut items: Vec<AnyElement> = Vec::new();
    items.push(context_item(tr!("usage.ctx_open_session"), theme, {
        let page = page.clone();
        move |window, cx| {
            page.update(cx, |page, cx| page.open_session(window, cx, session));
        }
    }));
    items.push(context_item(tr!("usage.ctx_scope_to_session"), theme, {
        let page = page.clone();
        move |_, cx| {
            page.update(cx, |page, cx| page.set_session_scope(Some(session), cx));
        }
    }));
    items.push(context_separator(theme));
    items.push(context_item(
        tr!("usage.ctx_copy_session_id"),
        theme,
        move |_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(session_id.clone()));
        },
    ));
    items.push(context_separator(theme));
    items.push(context_item(
        tr!("usage.ctx_filter_by_provider", name = provider_label),
        theme,
        {
            let page = page.clone();
            move |_, cx| {
                page.update(cx, |page, cx| {
                    page.toggle_filter_value(MenuKind::Provider, provider_id, cx)
                });
            }
        },
    ));
    if let Some(model_id) = model_id {
        items.push(context_item(
            tr!("usage.ctx_filter_by_model", name = model_label),
            theme,
            {
                let page = page.clone();
                move |_, cx| {
                    page.update(cx, |page, cx| {
                        page.toggle_filter_value(MenuKind::Model, model_id, cx)
                    });
                }
            },
        ));
    }
    items.push(context_item(tr!("usage.ctx_filter_by_workspace"), theme, {
        let page = page.clone();
        move |_, cx| {
            page.update(cx, |page, cx| {
                let workspace = page
                    .index()
                    .and_then(|index| index.try_session(session).map(|entry| entry.workspace));
                if let Some(workspace) = workspace {
                    page.toggle_filter_value(MenuKind::Workspace, workspace, cx);
                }
            });
        }
    }));

    anchored()
        .position_mode(gpui::AnchoredPositionMode::Local)
        .anchor(Corner::TopLeft)
        .snap_to_window_with_margin(popover::WINDOW_MARGIN)
        .child(deferred(context_menu_shell(theme, page, items)))
        .into_any_element()
}

/// The menu shell, on the branch selector's picker metrics
/// ([`picker_surface`]), dismissed by an outside click.
fn context_menu_shell(theme: Theme, page: Entity<UsagePage>, items: Vec<AnyElement>) -> AnyElement {
    picker_surface(div().id("usage-row-menu"), &theme)
        .py(picker::list_padding_y(&theme))
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base01.px(&theme))
        .overflow_hidden()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(move |_, _, cx| {
            page.update(cx, |page, cx| page.close_context_menu(cx));
        })
        .children(items)
        .into_any_element()
}

/// One row of a context menu ([`picker_entry`]).
fn context_item(
    label: String,
    theme: Theme,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    picker_entry(div(), &theme)
        .h(picker::entry_height(&theme))
        .flex_none()
        .text_color(theme.text_secondary)
        .cursor_pointer()
        .hover(|style| style.bg(theme.surface_hover).text_color(theme.text_primary))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
        .child(label)
        .into_any_element()
}

fn context_separator(theme: Theme) -> AnyElement {
    context_menu_separator(&theme).into_any_element()
}

// ── shared pieces ─────────────────────────────────────────────────────────

/// A top-level band on the canvas: a semantic title, a one-line description,
/// optional meta, a hairline, then the content. Used for every secondary
/// section so the page reads as hairlines and whitespace rather than a stack
/// of boxes (DESIGN.md: hairlines over boxes). The summary metric board is
/// the one place that keeps a card.
fn section(
    id: &'static str,
    title: &str,
    description: Option<&str>,
    meta: Option<String>,
    right: Option<AnyElement>,
    content: AnyElement,
    theme: Theme,
) -> AnyElement {
    card(
        id,
        title,
        description,
        meta,
        right,
        div()
            .w_full()
            .px(DynamicSpacing::Base12.px(&theme))
            .pt(DynamicSpacing::Base12.px(&theme))
            .pb(DynamicSpacing::Base12.px(&theme))
            .child(content)
            .into_any_element(),
        theme,
        false,
    )
}

/// A reading band: the Simple / Details group heading, then its sections.
/// Title is the 11px label register; the 15px titles live on the sections
/// inside, so the two levels never compete.
fn band(id: &'static str, title: &str, content: AnyElement, theme: Theme) -> AnyElement {
    div()
        .id(SharedString::from(id))
        .w_full()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base20.px(&theme))
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .line_height(theme.ui_px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(title.to_uppercase()),
        )
        .child(content)
        .into_any_element()
}

/// A recessed well inside a section card: canvas fill and a hairline, never a
/// second raised card. Used for the token-health panels, whose troughs need
/// a surface distinct from the raised section.
fn subpanel(label: &str, meta: Option<String>, content: AnyElement, theme: Theme) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(DynamicSpacing::Base08.px(&theme))
        .p(DynamicSpacing::Base12.px(&theme))
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            div()
                .flex()
                .items_center()
                .gap(DynamicSpacing::Base08.px(&theme))
                .child(
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(label.to_uppercase()),
                )
                .children(meta.map(|meta| {
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .text_color(theme.text_muted)
                        .child(meta)
                })),
        )
        .child(content)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn card(
    id: &'static str,
    title: &str,
    description: Option<&str>,
    meta: Option<String>,
    right: Option<AnyElement>,
    content: AnyElement,
    theme: Theme,
    clip: bool,
) -> AnyElement {
    let mut root = div()
        .id(SharedString::from(id))
        .flex_none()
        .w_full()
        .rounded(Radius::XLarge.px(&theme))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface_elevated)
        .shadow(theme.composer_shadow())
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .px(DynamicSpacing::Base12.px(&theme))
                .py(DynamicSpacing::Base12.px(&theme))
                .flex()
                .flex_col()
                .gap(DynamicSpacing::Base04.px(&theme))
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .child(
                            div()
                                .text_size(TextSize::Large.px(&theme))
                                .line_height(theme.ui_px(20.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(title.to_string()),
                        )
                        .children(meta.map(|meta| {
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .child(meta)
                        }))
                        .child(div().flex_1())
                        .children(right),
                )
                .children(description.map(|description| {
                    div()
                        .text_size(TextSize::Small.px(&theme))
                        .line_height(theme.ui_px(16.))
                        .text_color(theme.text_muted)
                        .child(description.to_string())
                })),
        )
        .child(content);
    if clip {
        root = root.overflow_hidden();
    }
    root.into_any_element()
}

/// A segmented control's selection callback, boxed so one segment's handler
/// can be cloned into every option.
type SegmentPick<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// A segmented control: one active segment, the rest quiet. The group is
/// joined the way the settings card's "One at a time / All" control is — no
/// gap, a shared hairline between neighbours, and only the group's outer
/// corners round. Height, horizontal padding, label size, and radius all come
/// from the button tokens ([`button_frame`] at [`ButtonSize::Medium`]), so the
/// group lines up with the page's chips and buttons.
///
/// `inset` picks the resting fill for the surface the group sits on, exactly as
/// [`filters::chip`] does: `surface_elevated` on the canvas, `background` inside a raised
/// card so the inactive segments read as recessed wells instead of vanishing
/// into the card. `enabled` keeps a segment that has no data yet visible but
/// inert — the chart's metric switcher dims a metric it cannot plot rather
/// than dropping it and reflowing the group.
#[allow(clippy::too_many_arguments)]
fn segmented<T>(
    prefix: &'static str,
    options: &[T],
    active: T,
    label: impl Fn(T) -> String,
    key: impl Fn(T) -> &'static str,
    enabled: impl Fn(T) -> bool,
    inset: bool,
    theme: Theme,
    on_pick: impl Fn(T, &mut Window, &mut App) + 'static,
) -> AnyElement
where
    T: Copy + PartialEq + 'static,
{
    let on_pick: SegmentPick<T> = Rc::new(on_pick);
    let resting = if inset {
        theme.background
    } else {
        theme.surface_elevated
    };
    let radius = button::RADIUS.px(&theme);
    let many = options.len() > 1;
    let last = options.len().saturating_sub(1);
    let mut row = div().flex().items_center();
    for (ix, option) in options.iter().copied().enumerate() {
        let is_active = option == active;
        let is_enabled = enabled(option);
        let handler = on_pick.clone();
        row = row.child(
            button_frame(div(), &theme, ButtonSize::Medium)
                .id(SharedString::from(format!("{prefix}-{}", key(option))))
                .rounded_none()
                .border_1()
                .border_color(theme.border)
                // One control: only the outer corners round, and the segments
                // share the hairline between them.
                .when(!many, |tab| tab.rounded(radius))
                .when(many && ix == 0, |tab| tab.rounded_l(radius))
                .when(many && ix == last, |tab| tab.rounded_r(radius).border_l_0())
                .when(many && ix > 0 && ix < last, |tab| tab.border_l_0())
                .when(is_active, |tab| {
                    tab.raised_flat(theme.surface_active)
                        .text_color(theme.text_primary)
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                })
                .when(!is_active && is_enabled, |tab| {
                    tab.raised_flat(resting)
                        .text_color(theme.text_secondary)
                        .cursor_pointer()
                        .hover(|style| style.raised_flat(theme.surface_hover))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            handler(option, window, cx)
                        })
                })
                // A metric with no source data stays visible but disabled, so
                // the group keeps its shape as availability changes.
                .when(!is_active && !is_enabled, |tab| {
                    tab.bg(resting)
                        .text_color(theme.text_disabled)
                        .cursor_default()
                })
                .child(label(option)),
        );
    }
    row.into_any_element()
}

/// A search field, shared by the Sessions table and the Breakdown table so both
/// read the same.
fn search_box(input: &Entity<ComposerInput>, theme: Theme) -> AnyElement {
    picker_search_frame(div(), &theme)
        .flex_1()
        .min_w(px(180.))
        .child(icon(
            "icons/search.svg",
            input::ICON.px(&theme),
            theme.text_muted,
        ))
        .child(div().flex_1().min_w_0().child(input.clone()))
        .into_any_element()
}

/// Search on the left, Columns on the right — the controls sit above the
/// table card, never inside it.
fn table_toolbar(search: AnyElement, columns: AnyElement, theme: Theme) -> AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base08.px(&theme))
        .child(search)
        .child(columns)
        .into_any_element()
}

/// Footer under a table card: the count on the left, outlined Previous / Next
/// on the right. Rows-per-page stays as a quiet chip beside the pager.
fn table_pager(
    summary: String,
    size_control: AnyElement,
    prev: AnyElement,
    next: AnyElement,
    current: usize,
    pages: usize,
    theme: Theme,
) -> AnyElement {
    div()
        .w_full()
        .pt(DynamicSpacing::Base12.px(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base08.px(&theme))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(TextSize::Default.px(&theme))
                .text_color(theme.text_muted)
                .child(summary),
        )
        .child(
            div()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("view.rows")),
        )
        .child(size_control)
        .child(
            div()
                .px(DynamicSpacing::Base02.px(&theme))
                .font(num_font())
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(format!("{current}/{pages}")),
        )
        .child(prev)
        .child(next)
        .into_any_element()
}

/// A chart row's label column (name plus its secondary line).
fn chart_label(label: &str, sub: Option<&str>, theme: Theme) -> AnyElement {
    div()
        .w(px(180.))
        .flex_none()
        .flex()
        .flex_col()
        .child(
            div()
                .truncate()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_primary)
                .child(label.to_string()),
        )
        .children(sub.map(|sub| {
            div()
                .truncate()
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .child(sub.to_string())
        }))
        .into_any_element()
}

/// A chart row's magnitude track and its accent fill.
fn bar_track(fraction: f64, theme: Theme) -> AnyElement {
    div()
        .flex_1()
        .min_w(px(48.))
        .h(px(8.))
        .rounded(Radius::Full.px(&theme))
        .bg(theme.surface_secondary)
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0.0, 1.0) as f32))
                .rounded(Radius::Full.px(&theme))
                .bg(theme.accent),
        )
        .into_any_element()
}

/// A chart row's value column.
fn chart_value(value: &str, theme: Theme) -> AnyElement {
    div()
        .w(px(72.))
        .flex_none()
        .font(num_font())
        .text_size(TextSize::Small.px(&theme))
        .text_color(theme.text_primary)
        .text_align(gpui::TextAlign::Right)
        .child(value.to_string())
        .into_any_element()
}

/// A chart row's share column.
fn chart_share(share: &str, theme: Theme) -> AnyElement {
    div()
        .w(px(56.))
        .flex_none()
        .whitespace_nowrap()
        .font(num_font())
        .text_size(TextSize::Small.px(&theme))
        .text_color(theme.text_muted)
        .text_align(gpui::TextAlign::Right)
        .child(share.to_string())
        .into_any_element()
}

/// The totals line under a data table: filtered totals left, this page right.
fn totals_row(filtered: &str, page: &str, theme: Theme) -> AnyElement {
    div()
        .w_full()
        .pt(DynamicSpacing::Base08.px(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base12.px(&theme))
        .text_size(TextSize::Small.px(&theme))
        .child(
            div()
                .text_color(theme.text_muted)
                .child(filtered.to_string()),
        )
        .child(div().flex_1())
        .child(div().text_color(theme.text_muted).child(page.to_string()))
        .into_any_element()
}

fn empty_line(text: &str, theme: Theme) -> AnyElement {
    div()
        .px(DynamicSpacing::Base08.px(&theme))
        .py(DynamicSpacing::Base06.px(&theme))
        .text_size(TextSize::Small.px(&theme))
        .text_color(theme.text_muted)
        .child(text.to_string())
        .into_any_element()
}

fn retry_note(theme: Theme) -> AnyElement {
    div()
        .text_size(TextSize::Small.px(&theme))
        .text_color(theme.text_muted)
        .child(tr!("view.retries_are_unavailable_pi_records_automatic_ret"))
        .into_any_element()
}

fn stat_row(label: &str, value: &str, theme: Theme) -> AnyElement {
    div()
        .px(DynamicSpacing::Base08.px(&theme))
        .py(DynamicSpacing::Base03.px(&theme))
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base12.px(&theme))
        .text_size(TextSize::Small.px(&theme))
        .child(
            div()
                .flex_1()
                .text_color(theme.text_muted)
                .child(label.to_string()),
        )
        .child(
            div()
                .font(num_font())
                .text_color(theme.text_secondary)
                .child(value.to_string()),
        )
        .into_any_element()
}

/// The cache hit rate across the window: one bar per bucket, drawn bottom-up,
/// each a real measurement with its own hover readout (§24). A bucket that
/// reported no cache traffic draws as a faint stub rather than a 0% bar —
/// "no data" and "a 0% hit rate" are different facts.
fn hit_rate_bars(series: &TimeSeries, theme: Theme) -> AnyElement {
    div()
        .w_full()
        .h(px(HIT_RATE_PLOT_H))
        .flex()
        .items_end()
        .gap(DynamicSpacing::Base02.px(&theme))
        .children(series.points.iter().enumerate().map(|(ix, point)| {
            let rate = Totals {
                tokens: TokenCounts {
                    input: point.totals.tokens.input,
                    cache_read: point.totals.tokens.cache_read,
                    ..Default::default()
                },
                ..Totals::default()
            }
            .cache_hit_rate();
            let (height, color, tooltip) = match rate {
                Some(rate) => (
                    (rate / 100.0).clamp(0.03, 1.0) as f32,
                    theme.accent.opacity(0.55),
                    tr!(
                        "usage.cache_bar_tooltip",
                        stamp = point.stamp,
                        rate = format::percent(rate)
                    ),
                ),
                None => (
                    0.03,
                    theme.surface_secondary,
                    tr!("usage.cache_bar_no_traffic", stamp = point.stamp),
                ),
            };
            div()
                .id(SharedString::from(format!("usage-cache-bar-{ix}")))
                .group("usage-cache-bar")
                .tooltip(move |_, cx| cx.new(|_| Tooltip::new(tooltip.clone())).into())
                .flex_1()
                .min_w(px(2.))
                .h_full()
                .flex()
                .items_end()
                .child(
                    div()
                        .debug_selector(move || format!("usage-cache-bar-fill-{ix}"))
                        .w_full()
                        .rounded_t(Radius::Small.px(&theme))
                        .bg(color)
                        .h(relative(height))
                        .group_hover("usage-cache-bar", |style| style.bg(theme.accent)),
                )
                .into_any_element()
        }))
        .into_any_element()
}

/// Tabular figures for numeric columns (§54). Both bundled faces carry `tnum`,
/// and requesting it is what stops columns from jittering as digits change.
pub(crate) fn num_font() -> Font {
    let mut font = gpui::font(theme::ui_font_family());
    font.features = FontFeatures(Arc::new(vec![
        ("tnum".to_string(), 1),
        ("lnum".to_string(), 1),
    ]));
    font
}

/// The name cell of a breakdown table: the row's label, with its secondary
/// line (provider, parent folder) trailing in the quiet register.
fn breakdown_name_cell(column: &Column, row: &GroupRow, theme: Theme) -> AnyElement {
    cell_shell(column)
        .gap(DynamicSpacing::Base08.px(&theme))
        .child(
            div()
                .flex_none()
                .max_w(px(column.width - 24.))
                .truncate()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_primary)
                .child(row.label.clone()),
        )
        .children(row.sub.clone().map(|sub| {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .child(sub)
        }))
        .into_any_element()
}

/// The Summary card's meta line: the range's headline counts.
fn summary_meta(snapshot: &UsageSnapshot) -> String {
    tr!(
        "usage.summary_meta",
        requests = format::count(snapshot.summary.totals.requests),
        tokens = format::compact(snapshot.summary.totals.tokens.total),
        sessions = format::count(snapshot.summary.sessions)
    )
}

fn toggle(list: &mut Vec<u16>, value: u16) {
    match list.iter().position(|entry| *entry == value) {
        Some(ix) => {
            list.remove(ix);
        }
        None => list.push(value),
    }
}

fn session_title(entry: &super::model::SessionEntry) -> String {
    if entry.title.is_empty() {
        tr!(
            "usage.session_fallback",
            id = entry.id.chars().take(8).collect::<String>()
        )
    } else {
        entry.title.clone()
    }
}

fn model_meta(breakdown: &Breakdown) -> String {
    tr!(
        "usage.meta_models_requests",
        models = format::count(breakdown.rows.len() as u64),
        requests = format::count(breakdown.totals.requests)
    )
}

fn workspace_meta(breakdown: &Breakdown) -> String {
    tr!(
        "usage.meta_workspaces_tokens",
        workspaces = format::count(breakdown.rows.len() as u64),
        tokens = format::compact(breakdown.totals.tokens.total)
    )
}

fn provider_meta(breakdown: &Breakdown) -> String {
    tr!(
        "usage.meta_providers_requests",
        providers = format::count(breakdown.rows.len() as u64),
        requests = format::count(breakdown.totals.requests)
    )
}

fn tools_meta(snapshot: &UsageSnapshot) -> String {
    let tools = &snapshot.tools;
    match snapshot.summary.tool_error_rate() {
        Some(rate) if tools.errors > 0 => tr!(
            "usage.meta_calls_failed_pct",
            calls = format::count(tools.calls),
            failed = format::count(tools.errors),
            rate = format::percent(rate)
        ),
        Some(_) => tr!(
            "usage.meta_calls_none_failed",
            calls = format::count(tools.calls)
        ),
        None => tr!("usage.no_tool_calls"),
    }
}

/// One label/value pair in the secondary readout under the board.
fn summary_stat(label: String, value: String, theme: Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base06.px(&theme))
        .text_size(TextSize::Small.px(&theme))
        .child(div().text_color(theme.text_muted).child(label))
        .child(
            div()
                .font(num_font())
                .text_color(theme.text_secondary)
                .child(value),
        )
        .into_any_element()
}

/// Secondary line for a KPI cell: the comparison when one exists, otherwise a
/// plain fact about the range. Direction is carried by an arrow as well as the
/// sign, so it never depends on color (§62).
fn delta_sub(snapshot: &UsageSnapshot, metric: ChartMetric, fallback: &str) -> String {
    let delta = snapshot.delta(metric);
    if delta.unavailable {
        return fallback.to_string();
    }
    // The sign already carries direction; the arrow only earns its place when
    // there is no percentage to read.
    match delta.pct {
        Some(_) => tr!("usage.delta_vs_previous", delta = format::delta(delta.pct)),
        None => match delta.direction {
            Direction::Up => tr!("usage.delta_up_from_nothing"),
            Direction::Down => tr!("usage.delta_down_from_last"),
            Direction::Flat => tr!("usage.delta_no_change"),
        },
    }
}

/// Why a metric is not offered, phrased for the data that is missing.
fn unavailable_reason(unavailable: &[&str]) -> String {
    let mut reasons: Vec<String> = Vec::new();
    if unavailable.contains(&"cost") {
        reasons.push(tr!("usage.reason_needs_price_table"));
    }
    if unavailable.contains(&"latency") {
        reasons.push(tr!("usage.reason_needs_request_gaps"));
    }
    if reasons.is_empty() {
        String::new()
    } else {
        tr!(
            "usage.not_shown",
            metrics = unavailable.join(", "),
            reasons = reasons.join("; ")
        )
    }
}

/// The metric cell model: label, value, secondary line, and interaction.
struct KpiCell {
    label: String,
    value: String,
    sub: String,
    tone: CellTone,
    click: Option<ChartMetric>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellTone {
    Normal,
    Muted,
}

// ── export ─────────────────────────────────────────────────────────────────

/// Every request matching the filter, as CSV. The columns are the normalized
/// record; nothing derived is exported as if it were measured.
pub fn export_csv(index: &UsageIndex, filter: &UsageFilter) -> String {
    let mut out = String::from(
        "timestamp,session_id,session,workspace,provider,model,input,output,cache_read,cache_write,total,reasoning,cost_usd,duration_ms,outcome\n",
    );
    for record in &index.requests {
        if !filter.matches_request(index, record) {
            continue;
        }
        let session = index.session(record.session);
        let (provider, model) = index.model_pair(record.model);
        let stamp = super::model::local_datetime(record.ts_ms)
            .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
            .unwrap_or_default();
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            csv_field(&stamp),
            csv_field(&session.id),
            csv_field(&session.title),
            csv_field(&index.workspace_of_session(record.session).path),
            csv_field(&provider),
            csv_field(&model),
            record.tokens.input,
            record.tokens.output,
            record.tokens.cache_read,
            record.tokens.cache_write,
            record.tokens.total,
            record.reasoning.unwrap_or(0),
            record
                .cost_usd
                .map(|cost| format!("{cost:.6}"))
                .unwrap_or_default(),
            record.duration_ms.unwrap_or(0),
            match record.outcome {
                super::model::Outcome::Stop => "stop",
                super::model::Outcome::ToolUse => "tool_use",
                super::model::Outcome::Length => "length",
                super::model::Outcome::Error => "error",
                super::model::Outcome::Aborted => "aborted",
            },
        ));
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// The current view as JSON: the aggregates on screen, respecting the filter
/// and the session search.
pub fn export_json(index: &UsageIndex, snapshot: &UsageSnapshot, search: &str) -> String {
    let summary = &snapshot.summary;
    let totals = &summary.totals;
    // Rows carry the interned id, not the raw one: resolve it so an export
    // can be joined against pi's own session and model identifiers.
    let breakdown = |rows: &[GroupRow], kind: &str| -> Vec<serde_json::Value> {
        rows.iter()
            .map(|row| {
                let raw_id = match kind {
                    "models" => index.model(row.id).id.clone(),
                    "providers" => index.providers[row.id as usize].id.clone(),
                    _ => index.workspaces[row.id as usize].path.clone(),
                };
                serde_json::json!({
                    "id": raw_id,
                    "label": row.label,
                    "sub": row.sub,
                    "requests": row.totals.requests,
                    "tokens": row.totals.tokens.total,
                    "input": row.totals.tokens.input,
                    "output": row.totals.tokens.output,
                    "cache_read": row.totals.tokens.cache_read,
                    "cache_write": row.totals.tokens.cache_write,
                    "cost_usd": row.totals.cost_usd,
                    "share": row.share,
                })
            })
            .collect()
    };
    let needle = search.trim().to_lowercase();
    let sessions: Vec<serde_json::Value> = snapshot
        .sessions
        .iter()
        .filter(|row| {
            needle.is_empty()
                || row.title.to_lowercase().contains(&needle)
                || row.workspace.to_lowercase().contains(&needle)
                || row.top_model.to_lowercase().contains(&needle)
        })
        .map(|row| {
            serde_json::json!({
                "session_id": index.session(row.session).id,
                "title": row.title,
                "workspace": row.workspace,
                "top_model": row.top_model,
                "models": row.models,
                "started_ms": row.started_ms,
                "ended_ms": row.ended_ms,
                "requests": row.totals.requests,
                "input": row.totals.tokens.input,
                "output": row.totals.tokens.output,
                "cache_read": row.totals.tokens.cache_read,
                "cache_write": row.totals.tokens.cache_write,
                "total": row.totals.tokens.total,
                "cost_usd": row.totals.cost_usd,
                "errors": row.totals.errors,
                "tool_calls": row.tool_runs,
                "tool_errors": row.tool_errors,
            })
        })
        .collect();
    let buckets: Vec<serde_json::Value> = snapshot
        .buckets
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "start_ms": row.start_ms,
                "label": row.label,
                "requests": row.totals.requests,
                "tokens": row.totals.tokens.total,
                "input": row.totals.tokens.input,
                "output": row.totals.tokens.output,
                "cache_hit_rate": row.totals.cache_hit_rate(),
                "errors": row.totals.errors,
            })
        })
        .collect();
    let errors: Vec<serde_json::Value> = snapshot
        .errors
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "timestamp_ms": row.ts_ms,
                "session_id": index.session(row.session).id,
                "kind": row.kind.as_str(),
                "model": index.model(row.model).label,
                "message": row.message,
            })
        })
        .collect();
    let tools: Vec<serde_json::Value> = snapshot
        .tools
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "tool": row.label,
                "class": row.class.as_str(),
                "calls": row.calls,
                "errors": row.errors,
                "avg_duration_ms": row.avg_duration_ms(),
                "max_duration_ms": row.max_ms,
            })
        })
        .collect();

    let payload = serde_json::json!({
        "generated_at_ms": super::collect::now_ms(),
        "range": {
            "preset": snapshot.filter.range.preset.as_str(),
            "label": snapshot.filter.range.preset.as_str(),
            "start_ms": snapshot.filter.range.start_ms,
            "end_ms": snapshot.filter.range.end_ms,
            "granularity": snapshot.series.granularity.as_str(),
        },
        "summary": {
            "requests": totals.requests,
            "turns": summary.turns,
            "sessions": summary.sessions,
            "models": summary.models,
            "providers": summary.providers,
            "workspaces": summary.workspaces,
            "input": totals.tokens.input,
            "output": totals.tokens.output,
            "cache_read": totals.tokens.cache_read,
            "cache_write": totals.tokens.cache_write,
            "total": totals.tokens.total,
            "reasoning": totals.reasoning,
            "cache_hit_rate": totals.cache_hit_rate(),
            "errors": totals.errors,
            "aborted": totals.aborted,
            "avg_duration_ms": totals.avg_duration_ms(),
            "duration_samples": totals.duration_samples,
            "tool_calls": summary.tool_runs,
            "bash_calls": summary.bash_runs,
            "tool_errors": summary.tool_errors,
            "cost_usd": totals.cost_usd,
            "priced_requests": totals.priced_requests,
        },
        "previous": snapshot.previous.as_ref().map(|previous| {
            serde_json::json!({
                "requests": previous.totals.requests,
                "total": previous.totals.tokens.total,
                "input": previous.totals.tokens.input,
                "output": previous.totals.tokens.output,
                "cache_read": previous.totals.tokens.cache_read,
                "cache_write": previous.totals.tokens.cache_write,
                "cost_usd": previous.totals.cost_usd,
                "errors": previous.totals.errors,
                "avg_duration_ms": previous.totals.avg_duration_ms(),
            })
        }),
        "series": snapshot.series.points.iter().map(|point| {
            serde_json::json!({
                "start_ms": point.start_ms,
                "label": point.label,
                "requests": point.totals.requests,
                "input": point.totals.tokens.input,
                "output": point.totals.tokens.output,
                "cache_read": point.totals.tokens.cache_read,
                "cache_write": point.totals.tokens.cache_write,
                "total": point.totals.tokens.total,
                "errors": point.totals.errors,
                "cost_usd": point.totals.cost_usd,
                "avg_duration_ms": point.totals.avg_duration_ms(),
            })
        }).collect::<Vec<_>>(),
        "models": breakdown(&snapshot.models.rows, "models"),
        "providers": breakdown(&snapshot.providers.rows, "providers"),
        "workspaces": breakdown(&snapshot.workspaces.rows, "workspaces"),
        "sessions": sessions,
        "buckets": buckets,
        // The calendar's days, in the same shape as the buckets/series so an
        // export can be joined day by day. The calendar is a fixed trailing
        // year, independent of the date range, and only the days it actually
        // covers are exported.
        "calendar": snapshot
            .calendar
            .in_range()
            .map(|day| {
                serde_json::json!({
                    "start_ms": day.start_ms,
                    "requests": day.totals.requests,
                    "input": day.totals.tokens.input,
                    "output": day.totals.tokens.output,
                    "cache_read": day.totals.tokens.cache_read,
                    "cache_write": day.totals.tokens.cache_write,
                    "total": day.totals.tokens.total,
                    "errors": day.totals.errors,
                    "cost_usd": day.totals.cost_usd,
                })
            })
            .collect::<Vec<_>>(),
        "tools": tools,
        "errors_detail": errors,
        "insights": snapshot.insights.iter().map(|insight| insight.text.clone()).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod hit_rate_layout_tests {
    use gpui::prelude::*;
    use gpui::{point, px, AvailableSpace, Context, IntoElement, Render, TestAppContext, Window};

    use super::{hit_rate_bars, HIT_RATE_PLOT_H};
    use crate::theme::{Theme, ThemeId};
    use crate::usage::aggregate::{SeriesPoint, TimeSeries, Totals};
    use crate::usage::model::{Granularity, TokenCounts};

    fn bar_point(input: u64, cache_read: u64) -> SeriesPoint {
        SeriesPoint {
            start_ms: 0,
            label: String::new(),
            stamp: "stamp".into(),
            totals: Totals {
                tokens: TokenCounts {
                    input,
                    cache_read,
                    ..Default::default()
                },
                ..Default::default()
            },
            latency: Default::default(),
        }
    }

    /// Draws the graph as a real view so `debug_bounds` can see the bars.
    struct BarsProbe {
        series: TimeSeries,
        theme: Theme,
    }

    impl Render for BarsProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
                .w(px(300.))
                .child(hit_rate_bars(&self.series, self.theme))
        }
    }

    /// The hit-rate graph's bars must share one baseline and their heights must
    /// track the rate: a no-traffic bucket is the 3% stub, 100% fills the plot,
    /// and 50% is half of it. This guards the `relative` height against a
    /// parent-height regression.
    #[gpui::test]
    fn cache_bars_are_bottom_aligned_and_proportional(cx: &mut TestAppContext) {
        let theme = Theme::for_id(ThemeId::Orbit);
        let series = TimeSeries {
            granularity: Granularity::Day,
            points: vec![
                bar_point(0, 0),     // no cache traffic -> 3% stub
                bar_point(0, 100),   // 100% hit rate
                bar_point(100, 100), // 50% hit rate
            ],
        };
        let cx = cx.add_empty_window();
        let _ = cx.draw(
            point(px(0.), px(0.)),
            AvailableSpace::min_size(),
            |_, cx| cx.new(|_| BarsProbe { series, theme }),
        );
        let stub = cx
            .debug_bounds("usage-cache-bar-fill-0")
            .expect("stub bar laid out");
        let full = cx
            .debug_bounds("usage-cache-bar-fill-1")
            .expect("full bar laid out");
        let half = cx
            .debug_bounds("usage-cache-bar-fill-2")
            .expect("half bar laid out");

        let plot = HIT_RATE_PLOT_H;
        assert_eq!(stub.bottom(), full.bottom(), "bars must share a baseline");
        assert_eq!(half.bottom(), full.bottom(), "bars must share a baseline");
        assert!(
            (f32::from(full.size.height) - plot).abs() < 0.5,
            "100% must fill the plot: {full:?}"
        );
        assert!(
            (f32::from(half.size.height) - plot / 2.).abs() < 0.5,
            "50% must be half the plot: {half:?}"
        );
        assert!(
            f32::from(stub.size.height) < plot * 0.1,
            "a no-traffic bucket must be a stub: {stub:?}"
        );
    }
}
