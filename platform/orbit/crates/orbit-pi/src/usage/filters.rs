//! The filter bar: the one place the page's scope is set.
//!
//! Four chips (date / workspace / provider / model) open anchored popovers —
//! the same `anchored` + `deferred` pattern as the model picker, so they float
//! above the page and stay inside the window. Multi-selects get a search field
//! and select-all/clear; the range chip carries a compact month calendar for
//! custom windows.
//!
//! Every control writes through `UsagePage`'s filter setters, so one state
//! object drives every panel on the page (§10).

use crate::app::{
    button_frame, context_menu_separator, icon_button_frame, menu_header, picker_entry,
    picker_search_frame, picker_surface, press, refresh_glyph, TipExt, BUTTON_GROUP,
};
use chrono::Datelike;
use gpui::{
    anchored, deferred, div, point, prelude::*, px, AnyElement, App, Corner, ElementId, Entity,
    FontWeight, IntoElement, MouseButton, MouseDownEvent, SharedString, Window,
};

use super::format;
use super::model::{
    local_day_start, local_month_start, next_bucket, Granularity, RangePreset, UsageIndex,
};
use super::page::{
    BucketSort, ExportFormat, MenuKind, SeriesSort, SessionSort, UsagePage, PAGE_SIZES,
};
use super::table::FailureSort;
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{
    context_menu, input, list, picker, popover, ButtonSize, DynamicSpacing, IconSize, TextSize,
};
use crate::theme::Theme;
use crate::{app::icon, composer::ComposerInput};

/// Every trigger on the filter bar and under the tables — filter chips, the
/// removable narrowing chips, text and pager buttons — shares one button
/// size, so a row of them lines up and a menu knows how far to drop.
const TRIGGER_SIZE: ButtonSize = ButtonSize::Medium;

/// One selectable value in a multi-select menu.
pub struct FilterOption {
    pub id: u16,
    pub label: String,
    pub sub: Option<String>,
}

/// A filter chip plus its popover. The panel hangs below (or below-right of)
/// the chip, floating above the page like every other menu in the app.
pub fn chip_with_menu(
    id: &'static str,
    chip: impl IntoElement,
    open: bool,
    corner: Corner,
    theme: Theme,
    panel: impl FnOnce() -> AnyElement,
) -> AnyElement {
    div()
        .id(ElementId::Name(SharedString::from(id)))
        .relative()
        .flex()
        .flex_col()
        .items_start()
        .child(chip)
        .children(open.then(|| {
            // The anchor corner sits at the chip's own top edge, so the offset
            // must clear the full trigger height before the menu gap —
            // otherwise the panel hangs over the chip that opened it (and a
            // second click lands inside the popup). See the same fix in
            // `settings.rs`.
            anchored()
                .position_mode(gpui::AnchoredPositionMode::Local)
                .anchor(corner)
                .offset(point(
                    px(-1.),
                    TRIGGER_SIZE.height(&theme) + popover::MENU_OFFSET,
                ))
                .snap_to_window_with_margin(popover::WINDOW_MARGIN)
                .child(deferred(panel()))
        }))
        .into_any_element()
}

/// A filter chip: a [`TRIGGER_SIZE`] button, hairline, and unmistakably "on"
/// when it narrows the view (active fill, `selection_fg` text — never accent
/// alone).
///
/// `inset` picks the resting fill for the surface the chip sits on: a chip on
/// the canvas takes `surface_elevated`, while a chip inside a raised card takes
/// `background` so it reads as a recessed well instead of vanishing into the card.
///
/// `icon_path` is the optional leading icon. A plain picker (rows-per-page)
/// passes `None`: the trailing chevron already marks it as a dropdown, and a
/// leading chevron there would read as a second one.
pub fn chip(
    id: &'static str,
    label: String,
    icon_path: Option<&'static str>,
    active: bool,
    inset: bool,
    theme: Theme,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let foreground = if active {
        theme.text_primary
    } else {
        theme.text_secondary
    };
    let resting = if inset {
        theme.background
    } else {
        theme.surface_elevated
    };
    let leading_color = if active {
        theme.text_primary
    } else {
        theme.text_muted
    };
    button_frame(
        div().id(ElementId::Name(SharedString::from(id))),
        &theme,
        TRIGGER_SIZE,
    )
    .border_1()
    .border_color(if active {
        theme.border_strong
    } else {
        theme.border
    })
    .raised(
        if active {
            theme.surface_active
        } else {
            resting
        },
        &theme,
    )
    .cursor_pointer()
    .hover(|style| {
        style.raised(
            if active {
                theme.surface_active
            } else {
                theme.surface_hover
            },
            &theme,
        )
    })
    .on_mouse_down(MouseButton::Left, on_click)
    .children(icon_path.map(|path| {
        icon(
            path,
            ButtonSize::Medium.icon_size().px(&theme),
            leading_color,
        )
    }))
    .child(
        div()
            .max_w(px(180.))
            .truncate()
            .text_color(foreground)
            .child(label),
    )
    .child(icon(
        "icons/chevron-down.svg",
        IconSize::XSmall.px(&theme),
        if active {
            theme.text_primary
        } else {
            theme.text_muted
        },
    ))
}

/// The popover shell shared by every filter menu, on the branch selector's
/// picker metrics ([`picker_surface`]). `width` pins menus whose content needs
/// a definite width (a truncating list, the calendar grid); `None` sizes the
/// menu to its entries from the shell's minimum. A click outside closes it,
/// and so does Escape — while a filter menu is open it owns that key, so it can
/// never reach the global abort binding.
fn panel(
    id: &'static str,
    width: Option<f32>,
    theme: Theme,
    children: Vec<AnyElement>,
    page: Entity<UsagePage>,
) -> AnyElement {
    let dismiss_click = page.clone();
    picker_surface(div().id(ElementId::Name(SharedString::from(id))), &theme)
        .when_some(width, |menu, width| menu.w(px(width)))
        .flex()
        .flex_col()
        .overflow_hidden()
        .occlude()
        .key_context("Picker")
        .on_mouse_down_out(move |_, _, cx| {
            dismiss_click.update(cx, |page, cx| page.dismiss_menus(cx));
        })
        .on_action({
            let cancel_page = page.clone();
            move |_: &crate::PickerCancel, _, cx| {
                cancel_page.update(cx, |page, cx| page.dismiss_menus(cx));
            }
        })
        // Enter belongs to the menu while it is open: without this it would
        // reach the app's Submit and send the composer's pending text.
        .on_action(move |_: &crate::PickerConfirm, _, cx| {
            page.update(cx, |page, cx| page.dismiss_menus(cx));
        })
        .children(children)
        .into_any_element()
}

/// The search field row at the top of a multi-select menu.
fn search_row(query: &Entity<ComposerInput>, theme: Theme) -> AnyElement {
    picker_search_frame(div(), &theme)
        .child(icon(
            "icons/search.svg",
            input::ICON.px(&theme),
            theme.text_muted,
        ))
        .child(div().flex_1().min_w_0().child(query.clone()))
        .into_any_element()
}

/// One row in a menu.
#[allow(clippy::too_many_arguments)]
fn row(
    id: ElementId,
    label: &str,
    sub: Option<&str>,
    selected: bool,
    highlighted: bool,
    theme: Theme,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    picker_entry(div().id(id), &theme)
        .h(picker::entry_height(&theme))
        .flex_none()
        .cursor_pointer()
        .when(highlighted || selected, |row| row.bg(theme.selection))
        .when(!highlighted && !selected, |row| {
            row.hover(|style| style.bg(theme.surface_hover))
        })
        .on_hover(on_hover)
        .on_mouse_down(MouseButton::Left, on_click)
        .child(
            // A check mark column keeps labels aligned whether or not a row
            // is selected; selection is also carried by the row's text weight.
            div()
                .w(context_menu::ICON.px(&theme))
                .flex_none()
                .flex()
                .items_center()
                .child(if selected {
                    icon(
                        "icons/check.svg",
                        context_menu::ICON.px(&theme),
                        theme.accent_text,
                    )
                    .into_any_element()
                } else {
                    div().into_any_element()
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .when(selected, |text| text.font_weight(FontWeight::MEDIUM))
                .text_color(if highlighted || selected {
                    theme.text_primary
                } else {
                    theme.text_secondary
                })
                .child(label.to_string()),
        )
        .children(sub.map(|sub| {
            div()
                .flex_none()
                .max_w(px(120.))
                .ml(context_menu::keybinding_gap(&theme) - context_menu::icon_gap(&theme))
                .truncate()
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(sub.to_string())
        }))
        .into_any_element()
}

fn footer_row(
    id: &'static str,
    label: &str,
    theme: Theme,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    button_frame(div().id(id), &theme, ButtonSize::Medium)
        .flex_1()
        .cursor_pointer()
        .text_color(theme.text_muted)
        .hover(|style| {
            style
                .bg(theme.surface_hover)
                .text_color(theme.text_secondary)
        })
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label.to_string())
        .into_any_element()
}

fn menu_footer(theme: Theme, left: AnyElement, right: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(DynamicSpacing::Base04.px(&theme))
        .px(DynamicSpacing::Base04.px(&theme))
        .py(DynamicSpacing::Base04.px(&theme))
        .border_t_1()
        .border_color(theme.border)
        .child(left)
        .child(right)
        .into_any_element()
}

/// The search-filtered view of a menu's options.
fn visible_options<'a>(
    options: &'a [FilterOption],
    query: &str,
    selected: &[u16],
) -> Vec<&'a FilterOption> {
    let needle = query.trim().to_lowercase();
    let mut rows: Vec<&FilterOption> = options
        .iter()
        .filter(|option| {
            needle.is_empty()
                || option.label.to_lowercase().contains(&needle)
                || option
                    .sub
                    .as_deref()
                    .is_some_and(|sub| sub.to_lowercase().contains(&needle))
        })
        .collect();
    // Selected values first, then the index's own order (already ranked by
    // the caller).
    rows.sort_by_key(|option| !selected.contains(&option.id));
    rows
}

/// A multi-select menu: search, the ranked list, and select-all/clear.
pub fn multi_menu(
    page: &UsagePage,
    kind: MenuKind,
    options: &[FilterOption],
    all: &[FilterOption],
    selected: &[u16],
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let query = page.menu_query().read(cx).text();
    let rows = visible_options(options, &query, selected);
    let highlight = page.menu_highlight();

    let mut children: Vec<AnyElement> = vec![search_row(page.menu_query(), theme)];
    let list: Vec<AnyElement> =
        std::iter::once((None, tr!("usage.all"), None::<String>, selected.is_empty()))
            .chain(rows.iter().map(|option| {
                (
                    Some(option.id),
                    option.label.clone(),
                    option.sub.clone(),
                    selected.contains(&option.id),
                )
            }))
            .enumerate()
            .map(|(ix, (id, label, sub, is_selected))| {
                let entity = cx.entity();
                let target = id;
                row(
                    ElementId::NamedInteger("usage-menu-row".into(), ix as u64),
                    &label,
                    sub.as_deref(),
                    is_selected,
                    ix == highlight,
                    theme,
                    move |_, _, cx| {
                        entity.update(cx, |page, cx| match target {
                            Some(id) => page.toggle_filter_value(kind, id, cx),
                            None => page.clear_dimension(kind, cx),
                        });
                    },
                    {
                        let entity = cx.entity();
                        move |entered, _, cx| {
                            if *entered {
                                entity.update(cx, |page, cx| page.set_menu_highlight(ix, cx));
                            }
                        }
                    },
                )
            })
            .collect();
    children.push(
        div()
            .id("usage-menu-list")
            .max_h(px(260.))
            .overflow_y_scroll()
            .py(list::padding_y(&theme))
            .flex()
            .flex_col()
            .children(list)
            .into_any_element(),
    );
    if rows.is_empty() && !query.trim().is_empty() {
        children.push(
            picker_entry(div(), &theme)
                .h(picker::entry_height(&theme))
                .flex_none()
                .text_color(theme.text_muted)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(tr!("usage.no_query_match", query = query.trim())),
                )
                .into_any_element(),
        );
    }
    let ids: Vec<u16> = all.iter().map(|option| option.id).collect();
    let entity = cx.entity();
    let select_all = footer_row("usage-menu-all", &tr!("usage.select_all"), theme, {
        let ids = ids.clone();
        move |_, _, cx| {
            entity.update(cx, |page, cx| page.select_all(kind, &ids, cx));
        }
    });
    let entity = cx.entity();
    let clear = footer_row(
        "usage-menu-clear",
        &tr!("usage.clear"),
        theme,
        move |_, _, cx| {
            entity.update(cx, |page, cx| page.clear_dimension(kind, cx));
        },
    );
    children.push(menu_footer(theme, select_all, clear));
    let page = cx.entity();
    panel("usage-filter-menu", Some(268.), theme, children, page)
}

/// The export popover: the two things the page can write, and what each
/// contains. Both respect the current filters.
pub fn export_menu(cx: &mut gpui::Context<UsagePage>, theme: Theme) -> AnyElement {
    let mut children: Vec<AnyElement> = Vec::new();
    for (format, label, hint) in [
        (
            ExportFormat::Csv,
            tr!("usage.export_csv"),
            tr!("usage.export_csv_hint"),
        ),
        (
            ExportFormat::Json,
            tr!("usage.export_json"),
            tr!("usage.export_json_hint"),
        ),
    ] {
        let entity = cx.entity();
        children.push(row(
            ElementId::Name(SharedString::from(format!(
                "usage-export-{}",
                format.as_str()
            ))),
            &label,
            Some(&hint),
            false,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| {
                    page.export(format, cx);
                    page.close_menu(cx);
                });
            },
            |_, _, _| {},
        ));
    }
    let page = cx.entity();
    panel("usage-export-menu", Some(268.), theme, children, page)
}

/// The rows-per-page picker (§26): one choice, applied immediately.
pub fn page_size_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let current = page.page_size();
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.rows_per_page"), theme)];
    for size in PAGE_SIZES {
        let selected = size == current;
        let entity = cx.entity();
        children.push(row(
            ElementId::NamedInteger("usage-page-size".into(), size as u64),
            &size.to_string(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.set_page_size(size, cx));
            },
            |_, _, _| {},
        ));
    }
    let page = cx.entity();
    panel("usage-page-size-menu", None, theme, children, page)
}

/// The column-visibility picker (§41). The title column is fixed; every other
/// column can be turned off, and the choice is persisted.
pub fn columns_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.columns"), theme)];
    // The display order of the table's own plan, minus the fixed title/open.
    let columns: [SessionSort; 11] = [
        SessionSort::Workspace,
        SessionSort::Provider,
        SessionSort::Model,
        SessionSort::Started,
        SessionSort::Duration,
        SessionSort::Requests,
        SessionSort::Input,
        SessionSort::Output,
        SessionSort::Cache,
        SessionSort::Tokens,
        SessionSort::Errors,
    ];
    for column in columns {
        let selected = page.column_visible(column);
        let entity = cx.entity();
        children.push(row(
            ElementId::Name(SharedString::from(format!(
                "usage-column-{}",
                column.as_str()
            ))),
            &column.label(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.toggle_column(column, cx));
            },
            |_, _, _| {},
        ));
    }
    let entity = cx.entity();
    let show_all = footer_row(
        "usage-columns-all",
        &tr!("usage.show_all"),
        theme,
        move |_, _, cx| {
            entity.update(cx, |page, cx| page.show_all_columns(cx));
        },
    );
    children.push(menu_footer(theme, show_all, div().into_any_element()));
    let page = cx.entity();
    panel("usage-columns-menu", None, theme, children, page)
}

/// The breakdown table's column-visibility picker. The name column is fixed;
/// every other column of the open dimension can be turned off.
pub fn breakdown_columns_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.columns"), theme)];
    for (id, label) in page.breakdown_column_options() {
        let selected = page.breakdown_column_visible(id);
        let entity = cx.entity();
        children.push(row(
            ElementId::Name(SharedString::from(format!("usage-breakdown-col-{id}"))),
            &label,
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.toggle_breakdown_column(id, cx));
            },
            |_, _, _| {},
        ));
    }
    let entity = cx.entity();
    let show_all = footer_row(
        "usage-breakdown-columns-all",
        &tr!("usage.show_all"),
        theme,
        move |_, _, cx| {
            entity.update(cx, |page, cx| page.show_all_breakdown_columns(cx));
        },
    );
    children.push(menu_footer(theme, show_all, div().into_any_element()));
    let page = cx.entity();
    panel("usage-breakdown-columns-menu", None, theme, children, page)
}

/// The breakdown table's rows-per-page picker.
pub fn breakdown_page_size_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let current = page.breakdown_view(page.breakdown_tab()).page_size;
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.rows_per_page"), theme)];
    for size in PAGE_SIZES {
        let selected = size == current;
        let entity = cx.entity();
        children.push(row(
            ElementId::NamedInteger("usage-breakdown-page-size".into(), size as u64),
            &size.to_string(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.set_breakdown_page_size(size, cx));
            },
            |_, _, _| {},
        ));
    }
    let page = cx.entity();
    panel(
        "usage-breakdown-page-size-menu",
        None,
        theme,
        children,
        page,
    )
}

/// The usage-over-time table's column-visibility picker. Time is fixed.
pub fn series_columns_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.columns"), theme)];
    for column in SeriesSort::HIDEABLE {
        let selected = page.series_column_visible(column);
        let label = if column == SeriesSort::Value {
            page.metric().label()
        } else {
            column.label()
        };
        let entity = cx.entity();
        children.push(row(
            ElementId::Name(SharedString::from(format!(
                "usage-series-col-{}",
                column.id()
            ))),
            &label,
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.toggle_series_column(column, cx));
            },
            |_, _, _| {},
        ));
    }
    let entity = cx.entity();
    let show_all = footer_row(
        "usage-series-columns-all",
        &tr!("usage.show_all"),
        theme,
        move |_, _, cx| {
            entity.update(cx, |page, cx| page.show_all_series_columns(cx));
        },
    );
    children.push(menu_footer(theme, show_all, div().into_any_element()));
    let page = cx.entity();
    panel("usage-series-columns-menu", None, theme, children, page)
}

/// The usage-over-time table's rows-per-page picker.
pub fn series_page_size_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let current = page.series_page_size();
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.rows_per_page"), theme)];
    for size in PAGE_SIZES {
        let selected = size == current;
        let entity = cx.entity();
        children.push(row(
            ElementId::NamedInteger("usage-series-page-size".into(), size as u64),
            &size.to_string(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.set_series_page_size(size, cx));
            },
            |_, _, _| {},
        ));
    }
    let page = cx.entity();
    panel("usage-series-page-size-menu", None, theme, children, page)
}

/// The Daily records table's column-visibility picker. Date is fixed.
pub fn bucket_columns_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.columns"), theme)];
    for column in BucketSort::HIDEABLE {
        let selected = page.bucket_column_visible(column);
        let entity = cx.entity();
        children.push(row(
            ElementId::Name(SharedString::from(format!(
                "usage-bucket-col-{}",
                column.id()
            ))),
            &column.label(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.toggle_bucket_column(column, cx));
            },
            |_, _, _| {},
        ));
    }
    let entity = cx.entity();
    let show_all = footer_row(
        "usage-bucket-columns-all",
        &tr!("usage.show_all"),
        theme,
        move |_, _, cx| {
            entity.update(cx, |page, cx| page.show_all_bucket_columns(cx));
        },
    );
    children.push(menu_footer(theme, show_all, div().into_any_element()));
    let page = cx.entity();
    panel("usage-bucket-columns-menu", None, theme, children, page)
}

/// The Daily records table's rows-per-page picker.
pub fn bucket_page_size_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let current = page.bucket_page_size();
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.rows_per_page"), theme)];
    for size in PAGE_SIZES {
        let selected = size == current;
        let entity = cx.entity();
        children.push(row(
            ElementId::NamedInteger("usage-bucket-page-size".into(), size as u64),
            &size.to_string(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.set_bucket_page_size(size, cx));
            },
            |_, _, _| {},
        ));
    }
    let page = cx.entity();
    panel("usage-bucket-page-size-menu", None, theme, children, page)
}

/// The Failures records table's column-visibility picker. When is fixed.
pub fn failure_columns_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.columns"), theme)];
    for column in FailureSort::HIDEABLE {
        let selected = page.failure_column_visible(column);
        let entity = cx.entity();
        children.push(row(
            ElementId::Name(SharedString::from(format!(
                "usage-failure-col-{}",
                column.id()
            ))),
            &column.label(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.toggle_failure_column(column, cx));
            },
            |_, _, _| {},
        ));
    }
    let entity = cx.entity();
    let show_all = footer_row(
        "usage-failure-columns-all",
        &tr!("usage.show_all"),
        theme,
        move |_, _, cx| {
            entity.update(cx, |page, cx| page.show_all_failure_columns(cx));
        },
    );
    children.push(menu_footer(theme, show_all, div().into_any_element()));
    let page = cx.entity();
    panel("usage-failure-columns-menu", None, theme, children, page)
}

/// The Failures records table's rows-per-page picker.
pub fn failure_page_size_menu(
    page: &UsagePage,
    cx: &mut gpui::Context<UsagePage>,
    theme: Theme,
) -> AnyElement {
    let current = page.failure_page_size();
    let mut children: Vec<AnyElement> = vec![menu_title(&tr!("usage.rows_per_page"), theme)];
    for size in PAGE_SIZES {
        let selected = size == current;
        let entity = cx.entity();
        children.push(row(
            ElementId::NamedInteger("usage-failure-page-size".into(), size as u64),
            &size.to_string(),
            None,
            selected,
            false,
            theme,
            move |_, _, cx| {
                entity.update(cx, |page, cx| page.set_failure_page_size(size, cx));
            },
            |_, _, _| {},
        ));
    }
    let page = cx.entity();
    panel("usage-failure-page-size-menu", None, theme, children, page)
}

/// A small heading at the top of a menu: Zed's list sub-header.
fn menu_title(label: &str, theme: Theme) -> AnyElement {
    menu_header(label.to_string(), &theme).into_any_element()
}

/// The date-range menu: presets, then a compact calendar for custom windows.
pub fn range_menu(page: &UsagePage, cx: &mut gpui::Context<UsagePage>, theme: Theme) -> AnyElement {
    let current = page.filter().range.clone();
    let mut children: Vec<AnyElement> = Vec::new();
    let mut rows: Vec<AnyElement> = Vec::new();
    for preset in RangePreset::ALL {
        let selected = current.preset == preset;
        let entity = cx.entity();
        // The custom row names the action, not the range it would replace.
        let label = preset.label().to_string();
        rows.push(row(
            ElementId::Name(SharedString::from(format!(
                "usage-range-{}",
                preset.as_str()
            ))),
            &label,
            preset_summary(preset).as_deref(),
            selected,
            false,
            theme,
            move |_, window, cx| {
                entity.update(cx, |page, cx| {
                    if preset == RangePreset::Custom {
                        page.open_calendar(window, cx);
                    } else {
                        page.set_preset(preset, cx);
                        page.open_menu(None, window, cx);
                    }
                });
            },
            |_, _, _| {},
        ));
    }
    // The calendar appears in place once "Custom…" is chosen.
    if page.calendar_open() {
        rows.push(context_menu_separator(&theme).into_any_element());
        rows.push(calendar(page, cx, theme));
    }
    // The shell already pads the list top and bottom.
    children.push(div().flex().flex_col().children(rows).into_any_element());
    let page = cx.entity();
    panel("usage-range-menu", Some(244.), theme, children, page)
}

/// The hint shown on the right of each preset row.
fn preset_summary(preset: RangePreset) -> Option<String> {
    match preset {
        RangePreset::Today => Some(tr!("usage.compare_with_yesterday")),
        RangePreset::Last7 => Some(tr!("usage.vs_previous_7_days")),
        RangePreset::Last14 => Some(tr!("usage.vs_previous_14_days")),
        RangePreset::Last30 => Some(tr!("usage.vs_previous_30_days")),
        RangePreset::PreviousMonth => Some(tr!("usage.complete_month")),
        RangePreset::All => Some(tr!("usage.no_comparison")),
        _ => None,
    }
}

/// A compact month calendar for the custom range. Two clicks set the window:
/// the first is the start, the second the end (a reversed pair is normalised).
fn calendar(page: &UsagePage, cx: &mut gpui::Context<UsagePage>, theme: Theme) -> AnyElement {
    let month_ms = page
        .calendar_month()
        .unwrap_or_else(|| page.filter().range.start_ms);
    let first = local_month_start(month_ms);
    let (start, end) = page.custom_bounds();
    let today = local_day_start(super::collect::now_ms());

    let Some(dt) = super::model::local_datetime(first) else {
        return div().into_any_element();
    };
    let days_in_month = {
        let (year, month) = (dt.year(), dt.month());
        let next = if month == 12 {
            chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)
        } else {
            chrono::NaiveDate::from_ymd_opt(year, month + 1, 1)
        };
        match next {
            Some(next) => (next - dt.date_naive()).num_days().max(28),
            None => 30,
        }
    };
    let offset = dt.weekday().num_days_from_monday() as i64;

    // Month header with the two navigation affordances. One button tall, so
    // it lines up with the icon buttons it holds.
    let header = div()
        .h(TRIGGER_SIZE.height(&theme))
        .px(DynamicSpacing::Base08.px(&theme))
        .flex()
        .items_center()
        .child(
            icon_button_frame(div().id("usage-cal-prev"), &theme, ButtonSize::Default)
                .tip(tr!("usage.previous_month"))
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover))
                .on_mouse_down(MouseButton::Left, {
                    let entity = cx.entity();
                    let prev = local_month_start(first - 86_400_000);
                    move |_, _, cx| {
                        entity.update(cx, |page, cx| page.set_calendar_month(prev, cx));
                    }
                })
                .child(icon(
                    "icons/chevron-left.svg",
                    ButtonSize::Default.icon_size().px(&theme),
                    theme.text_muted,
                )),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .justify_center()
                .text_size(TextSize::Small.px(&theme))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_primary)
                .child(dt.format("%B %Y").to_string()),
        )
        .child(
            icon_button_frame(div().id("usage-cal-next"), &theme, ButtonSize::Default)
                .tip(tr!("usage.next_month"))
                .cursor_pointer()
                .hover(|style| style.bg(theme.surface_hover))
                .on_mouse_down(MouseButton::Left, {
                    let entity = cx.entity();
                    let next = next_bucket(first, Granularity::Month);
                    move |_, _, cx| {
                        entity.update(cx, |page, cx| page.set_calendar_month(next, cx));
                    }
                })
                .child(icon(
                    "icons/chevron-right.svg",
                    ButtonSize::Default.icon_size().px(&theme),
                    theme.text_muted,
                )),
        );

    let weekdays = div()
        .flex()
        .px(DynamicSpacing::Base08.px(&theme))
        .pb(DynamicSpacing::Base02.px(&theme))
        .children(["M", "T", "W", "T", "F", "S", "S"].map(|day| {
            div()
                .flex_1()
                .flex()
                .justify_center()
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .child(day)
                .into_any_element()
        }));

    // Each day is a Default-size button; an empty slot keeps the same height
    // so every week row lines up.
    let day_h = ButtonSize::Default.height(&theme);
    let mut grid = div()
        .flex()
        .flex_col()
        .px(DynamicSpacing::Base08.px(&theme))
        .pb(DynamicSpacing::Base06.px(&theme));
    let day_ix = 1i64;
    let mut cell_ix = 0i64;
    let total_cells = ((offset + days_in_month + 6) / 7) * 7;
    while cell_ix < total_cells {
        let mut week = div().flex();
        for _ in 0..7 {
            let day_number = day_ix + cell_ix - offset;
            if day_ix + cell_ix <= offset || day_number > days_in_month {
                week = week.child(div().flex_1().h(day_h).into_any_element());
            } else {
                let day_ms = local_day_start(first + (day_number - 1) * 86_400_000);
                let selected = Some(day_ms) == start || Some(day_ms) == end;
                let in_range = match (start, end) {
                    (Some(from), Some(to)) => day_ms > from.min(to) && day_ms < from.max(to),
                    _ => false,
                };
                let entity = cx.entity();
                week = week.child(
                    button_frame(
                        div().id(ElementId::NamedInteger(
                            "usage-cal-day".into(),
                            day_number as u64,
                        )),
                        &theme,
                        ButtonSize::Default,
                    )
                    .flex_1()
                    .cursor_pointer()
                    .when(selected, |cell| {
                        cell.bg(theme.surface_active).text_color(theme.text_primary)
                    })
                    .when(in_range, |cell| {
                        cell.bg(theme.overlay).text_color(theme.text_primary)
                    })
                    .when(!selected && !in_range, |cell| {
                        cell.text_color(if day_ms == today {
                            theme.accent_text
                        } else {
                            theme.text_secondary
                        })
                        .hover(|style| style.bg(theme.surface_hover))
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        entity.update(cx, |page, cx| page.pick_day(day_ms, cx));
                    })
                    .child(day_number.to_string())
                    .into_any_element(),
                );
            }
            cell_ix += 1;
        }
        grid = grid.child(week.into_any_element());
    }

    let hint = match (start, end) {
        (Some(from), Some(to)) => super::model::DateRange::custom(from, to).label(),
        (Some(_), None) => tr!("usage.pick_end_date"),
        _ => tr!("usage.pick_start_date"),
    };

    div()
        .flex()
        .flex_col()
        .child(header)
        .child(weekdays)
        .child(grid)
        .child(
            div()
                .px(DynamicSpacing::Base12.px(&theme))
                .pb(DynamicSpacing::Base06.px(&theme))
                .text_size(TextSize::XSmall.px(&theme))
                .text_color(theme.text_muted)
                .child(hint),
        )
        .into_any_element()
}

/// The chip label for a multi-select dimension.
pub fn selection_label(
    all: &str,
    selected: &[u16],
    label_of: impl Fn(u16) -> Option<String>,
) -> String {
    match selected.len() {
        0 => all.to_string(),
        1 => label_of(selected[0]).unwrap_or_else(|| all.to_string()),
        count => tr!("usage.n_selected", count = count),
    }
}

/// A small "narrowed" indicator chip used for the boolean filters that are
/// switched on (errors only / cached only), with an inline clear ×.
pub fn toggle_chip(
    id: String,
    label: impl Into<String>,
    theme: Theme,
    on_clear: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let clear_id = format!("{id}-clear");
    // Sized like the filter chips it sits beside; the right edge is tighter
    // so the clear button reads as part of the chip.
    button_frame(
        div().id(ElementId::Name(SharedString::from(id))),
        &theme,
        TRIGGER_SIZE,
    )
    .pr(DynamicSpacing::Base06.px(&theme))
    .border_1()
    .border_color(theme.border_strong)
    .raised(theme.surface_active, &theme)
    .text_color(theme.text_primary)
    .child(label.into())
    .child(
        icon_button_frame(
            div().id(ElementId::Name(SharedString::from(clear_id))),
            &theme,
            ButtonSize::None,
        )
        .tip(tr!("usage.clear_filter"))
        .cursor_pointer()
        .hover(|style| style.bg(theme.overlay_strong))
        .on_mouse_down(MouseButton::Left, on_clear)
        .child(icon(
            "icons/x.svg",
            ButtonSize::None.icon_size().px(&theme),
            theme.text_primary,
        )),
    )
    .into_any_element()
}

/// A flat text button (Clear filters, Export…).
pub fn text_button(
    id: &'static str,
    label: &str,
    icon_path: Option<&'static str>,
    icon_active: bool,
    enabled: bool,
    theme: Theme,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let color = if enabled {
        theme.text_secondary
    } else {
        theme.text_muted
    };
    button_frame(
        div().id(ElementId::Name(SharedString::from(id))),
        &theme,
        TRIGGER_SIZE,
    )
    .group(BUTTON_GROUP)
    .text_color(color)
    .when(enabled, |button| {
        press(button)
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_hover).text_color(theme.text_primary))
            .on_mouse_down(MouseButton::Left, on_click)
    })
    .children(icon_path.map(|path| {
        if icon_active {
            // The leading glyph is the refresh affordance: turn it in
            // place instead of swapping in a loader, matching the popover.
            refresh_glyph(
                ElementId::Name(SharedString::from(format!("{id}-spin"))),
                ButtonSize::Medium.icon_size().px(&theme),
                true,
                color,
                theme,
            )
        } else {
            icon(path, ButtonSize::Medium.icon_size().px(&theme), color).into_any_element()
        }
    }))
    .child(label.to_string())
    .into_any_element()
}

/// Outlined pager control: Previous / Next sit under the table card.
pub fn outline_button(
    id: &'static str,
    label: &str,
    enabled: bool,
    theme: Theme,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    button_frame(
        div().id(ElementId::Name(SharedString::from(id))),
        &theme,
        TRIGGER_SIZE,
    )
    .group(BUTTON_GROUP)
    .border_1()
    .border_color(theme.border)
    .text_color(if enabled {
        theme.text_primary
    } else {
        theme.text_disabled
    })
    .when(!enabled, |button| button.bg(theme.overlay))
    .when(enabled, |button| {
        press(button)
            .raised(theme.background, &theme)
            .cursor_pointer()
            .hover(|style| {
                style
                    .raised(theme.surface_hover, &theme)
                    .border_color(theme.border_strong)
            })
            .on_mouse_down(MouseButton::Left, on_click)
    })
    .child(label.to_string())
    .into_any_element()
}

/// Options for the workspace dimension, ranked by request count.
pub fn workspace_options(index: &UsageIndex) -> Vec<FilterOption> {
    let mut counts = vec![0u64; index.workspaces.len()];
    for record in &index.requests {
        counts[index.session(record.session).workspace as usize] += 1;
    }
    let mut options: Vec<(u64, FilterOption)> = (0..index.workspaces.len())
        .map(|ix| {
            let entry = &index.workspaces[ix];
            (
                counts[ix],
                FilterOption {
                    id: ix as u16,
                    label: entry.label.clone(),
                    sub: Some(entry.path.clone()),
                },
            )
        })
        .collect();
    options.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    options.into_iter().map(|(_, option)| option).collect()
}

/// Options for the provider dimension, ranked by request count.
pub fn provider_options(index: &UsageIndex) -> Vec<FilterOption> {
    let mut counts = vec![0u64; index.providers.len()];
    for record in &index.requests {
        counts[index.model(record.model).provider as usize] += 1;
    }
    let mut options: Vec<(u64, FilterOption)> = (0..index.providers.len())
        .map(|ix| {
            let entry = &index.providers[ix];
            (
                counts[ix],
                FilterOption {
                    id: ix as u16,
                    label: entry.label.clone(),
                    sub: Some(format::count(counts[ix])),
                },
            )
        })
        .collect();
    options.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    options.into_iter().map(|(_, option)| option).collect()
}

/// Options for the model dimension, ranked by request count.
pub fn model_options(index: &UsageIndex) -> Vec<FilterOption> {
    let mut counts = vec![0u64; index.models.len()];
    for record in &index.requests {
        counts[record.model as usize] += 1;
    }
    let mut options: Vec<(u64, FilterOption)> = (0..index.models.len())
        .map(|ix| {
            let entry = &index.models[ix];
            (
                counts[ix],
                FilterOption {
                    id: ix as u16,
                    label: entry.label.clone(),
                    sub: Some(index.provider_of(ix as u16).label.clone()),
                },
            )
        })
        .collect();
    options.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(&b.1.label)));
    options.into_iter().map(|(_, option)| option).collect()
}
