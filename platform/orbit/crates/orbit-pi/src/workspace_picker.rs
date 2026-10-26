//! Workspace picker — the new-task page's folder selector.
//!
//! Replaces the bare OS dialog with a popover that answers the common case
//! first: the folders you already run sessions in, newest activity first,
//! filterable, keyboard-driven. The native dialog is still one row away
//! ("Choose folder…") for anywhere else. Follows the same popover conventions
//! as [`crate::branch_picker::BranchPicker`].

use std::path::PathBuf;

use gpui::{
    div, point, prelude::*, px, App, Context, ElementId, Entity, FocusHandle, Focusable,
    FontWeight, IntoElement, MouseDownEvent, ParentElement, Render, ScrollHandle, Styled,
    Subscription, Window,
};

use crate::app::{icon, menu_header, picker_entry, picker_search_frame, picker_surface};
use crate::composer::ComposerInput;
use crate::theme::tokens::{context_menu, input, list, picker, DynamicSpacing};
use crate::theme::{self, Theme};

/// The field caps at this width; the popover matches it exactly. Feeds
/// [`width_for_window`], the single source of truth both the new-task field
/// (`render_empty_state`) and this popover use, so the two can never drift
/// apart.
pub const FIELD_MAX_W: f32 = 400.;
/// Page side padding around the field column (`render_empty_state`).
pub const PAGE_PAD: f32 = 24.;
/// Smallest the popover ever gets (very narrow windows).
const POPOVER_MIN_W: f32 = 240.;
/// Folder rows visible before the recents list scrolls.
const VISIBLE_ROWS: f32 = 4.;

/// A folder row: a one-line picker entry. Layout and the keyboard reveal
/// math both read it, so they can never disagree.
fn row_h(theme: &Theme) -> f32 {
    picker::entry_height(theme).into()
}

/// Tallest the recents list grows before scrolling (≈ 4 rows). Sized so the
/// whole popover still fits below the centered field at the 960×640 minimum
/// window, instead of `snap_to_window` shoving it up over the card.
fn list_max_h(theme: &Theme) -> f32 {
    VISIBLE_ROWS * row_h(theme)
}
/// Recents the app hands us at most.
pub const MAX_RECENTS: usize = 8;

/// Pick callback: the chosen folder plus the ambient window.
type WorkspacePick = Box<dyn Fn(PathBuf, &mut Window, &mut App)>;
/// Browse callback: open the native folder dialog.
type WorkspaceBrowse = Box<dyn Fn(&mut Window, &mut App)>;
/// Dismiss callback; `bool` is true when an outside mouse-down closed it.
type WorkspacePickerDismiss = Box<dyn Fn(bool, &mut Window, &mut App)>;

/// Field/popover width for a given main-area width. The new-task field takes
/// this as a definite `w()` (not `w_full().max_w(FIELD_MAX_W)`): percent
/// widths nested under the centered max-width column resolve against the
/// unclamped page width in this gpui/taffy stack, which let the card paint
/// past the column to the window's right edge. Definite width here means the
/// card is clamped up front and this popover always lands flush under it.
pub fn width_for_window(viewport_w: f32) -> f32 {
    (viewport_w - 2. * PAGE_PAD).clamp(POPOVER_MIN_W, FIELD_MAX_W)
}

/// One selectable folder: a recent workspace or the current one.
#[derive(Debug, Clone)]
pub struct WorkspaceEntry {
    /// Folder display name (`workspace_label`).
    pub name: String,
    pub path: PathBuf,
    /// Age of the newest session here (`2h`); `None` for a folder with no
    /// sessions yet.
    pub last_active: Option<String>,
}

pub struct WorkspacePicker {
    entries: Vec<WorkspaceEntry>,
    current: Option<PathBuf>,
    /// Matches the field's rendered width — the app measures the window at
    /// open time so the popover never overflows or juts past the card.
    width: f32,
    filter: Entity<ComposerInput>,
    scroll: ScrollHandle,
    /// `0..rows.len()` — one past the last folder row is "Choose folder…".
    highlighted: usize,
    last_filter: String,
    _filter_sub: Subscription,
    on_pick: WorkspacePick,
    on_browse: WorkspaceBrowse,
    /// `bool` = dismissed by an outside mouse-down (vs. escape).
    on_dismiss: WorkspacePickerDismiss,
}

impl WorkspacePicker {
    pub fn new(
        entries: Vec<WorkspaceEntry>,
        current: Option<PathBuf>,
        width: f32,
        on_pick: WorkspacePick,
        on_browse: WorkspaceBrowse,
        on_dismiss: WorkspacePickerDismiss,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("workspace_picker.filter_folders")
                .with_key_context("Composer Picker")
        });
        // Keep the derived filter state in sync from the input's notifications
        // so `render` only reads it.
        let filter_sub = cx.observe(&filter, |picker: &mut Self, filter, cx| {
            let needle = filter.read(cx).text().to_lowercase();
            if needle != picker.last_filter {
                picker.last_filter = needle;
                picker.highlighted = 0;
                picker.scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
        });
        Self {
            entries,
            current,
            width,
            filter,
            scroll: ScrollHandle::new(),
            highlighted: 0,
            last_filter: String::new(),
            _filter_sub: filter_sub,
            on_pick,
            on_browse,
            on_dismiss,
        }
    }

    // ── actions (Picker key context) ─────────────────────────────────────

    fn on_cancel(&mut self, _: &crate::PickerCancel, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_dismiss)(false, window, cx);
    }

    fn on_confirm(
        &mut self,
        _: &crate::PickerConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.filtered(&self.last_filter);
        self.activate(self.highlighted, &rows, window, cx);
    }

    fn on_next(&mut self, _: &crate::PickerSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn on_prev(&mut self, _: &crate::PickerSelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn on_outside_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_dismiss)(true, window, cx);
    }

    // ── internals ────────────────────────────────────────────────────────

    /// Folder rows matching the filter (name or path substring).
    fn filtered(&self, needle: &str) -> Vec<WorkspaceEntry> {
        let needle = needle.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                needle.is_empty()
                    || entry.name.to_lowercase().contains(&needle)
                    || entry
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&needle)
            })
            .cloned()
            .collect()
    }

    /// Selectable row count: folder rows plus the "Choose folder…" row.
    fn row_count(&self, rows: &[WorkspaceEntry]) -> usize {
        rows.len() + 1
    }

    fn step(&mut self, dir: isize, cx: &mut Context<Self>) {
        let rows = self.filtered(&self.last_filter);
        let count = self.row_count(&rows);
        if count == 0 {
            return;
        }
        let pos = self.highlighted.min(count - 1) as isize;
        let next = (pos + dir).rem_euclid(count as isize) as usize;
        self.highlighted = next;
        self.ensure_visible(rows.len(), theme::get(cx));
        cx.notify();
    }

    fn ensure_visible(&mut self, folder_rows: usize, theme: &Theme) {
        let row_h = row_h(theme);
        // The browse row sits below the list: scroll to the very bottom.
        let row_top = if self.highlighted >= folder_rows {
            folder_rows as f32 * row_h + row_h
        } else {
            self.highlighted as f32 * row_h
        };
        let content_h = folder_rows as f32 * row_h;
        let viewport_h = content_h.min(list_max_h(theme));
        // `ScrollHandle`'s offset is negative once scrolled down.
        let current = -f32::from(self.scroll.offset().y);
        let mut offset = current;
        if row_top < current {
            offset = row_top;
        } else if row_top + row_h > current + viewport_h {
            offset = row_top + row_h - viewport_h;
        }
        let max_offset = (content_h - viewport_h).max(0.);
        self.scroll
            .set_offset(point(px(0.), px(-offset.clamp(0., max_offset))));
    }

    fn activate(
        &mut self,
        ix: usize,
        rows: &[WorkspaceEntry],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match rows.get(ix) {
            Some(entry) if self.current.as_ref() == Some(&entry.path) => {
                (self.on_dismiss)(false, window, cx)
            }
            Some(entry) => (self.on_pick)(entry.path.clone(), window, cx),
            None => (self.on_browse)(window, cx),
        }
    }
}

impl Focusable for WorkspacePicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.read(cx).focus_handle(cx)
    }
}

impl Render for WorkspacePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        let this = cx.entity();

        let rows = self.filtered(&self.last_filter);

        // ── folder rows ──
        let mut list = div()
            .id("workspace-picker-list")
            .w_full()
            .max_h(px(list_max_h(&theme)))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col();
        for (ix, entry) in rows.iter().enumerate() {
            let is_current = self.current.as_ref() == Some(&entry.path);
            let highlighted = ix == self.highlighted;
            let this = this.clone();
            list = list.child(
                picker_entry(
                    div().id(ElementId::NamedInteger("workspace-row".into(), ix as u64)),
                    &theme,
                )
                .h(px(row_h(&theme)))
                .flex_none()
                .cursor_pointer()
                .when(highlighted, |row| row.bg(theme.selection))
                // The current folder keeps the `active` fill at rest (the
                // same "chosen" register the model picker uses); the accent
                // check alone is never the only signal.
                .when(!highlighted && is_current, |row| row.bg(theme.selection))
                .when(!highlighted && !is_current, |row| {
                    row.hover(|s| s.bg(theme.surface_hover))
                })
                // Pointer movement moves the highlight; a scroll sliding rows
                // under a stationary pointer must not hijack ↑/↓ navigation.
                .on_mouse_move({
                    let this = this.clone();
                    move |_, _, cx| {
                        this.update(cx, |picker, cx| {
                            if picker.highlighted != ix {
                                picker.highlighted = ix;
                                cx.notify();
                            }
                        });
                    }
                })
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(move |picker, _, window, cx| {
                        let rows = picker.filtered(&picker.last_filter.clone());
                        picker.activate(ix, &rows, window, cx);
                    }),
                )
                // The check carries "current" — the icon stays quiet, one
                // accent signal per fact.
                .child(icon(
                    "icons/folder.svg",
                    context_menu::ICON.px(&theme),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_baseline()
                        .gap(DynamicSpacing::Base08.px(&theme))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(170.))
                                .truncate()
                                .font_weight(if highlighted || is_current {
                                    FontWeight::MEDIUM
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(if highlighted {
                                    theme.text_primary
                                } else if is_current {
                                    theme.selection_fg
                                } else {
                                    theme.text_secondary
                                })
                                .child(entry.name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                // Paths are machine text — mono, like the
                                // model picker's id line.
                                .font_family(theme::code_font_family())
                                .text_size(picker::SECONDARY_TEXT.px(&theme))
                                .text_color(if highlighted || is_current {
                                    theme.text_secondary
                                } else {
                                    theme.text_muted
                                })
                                .child(entry.path.to_string_lossy().into_owned()),
                        ),
                )
                .when_some(entry.last_active.clone(), |row, ago| {
                    row.child(
                        div()
                            .flex_none()
                            .text_size(picker::SECONDARY_TEXT.px(&theme))
                            .text_color(if highlighted || is_current {
                                theme.text_secondary
                            } else {
                                theme.text_muted
                            })
                            .child(ago),
                    )
                })
                .when(is_current, |row| {
                    row.child(icon(
                        "icons/check.svg",
                        context_menu::ICON.px(&theme),
                        theme.accent_text,
                    ))
                }),
            );
        }

        let browse_highlighted = self.highlighted == rows.len();

        picker_surface(div(), &theme)
            .w(px(self.width))
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .on_mouse_down_out(cx.listener(Self::on_outside_down))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            // search row — Zed's picker head.
            .child(
                picker_search_frame(div(), &theme)
                    .text_color(theme.text_primary)
                    .child(icon("icons/search.svg", input::ICON.px(&theme), theme.text_muted))
                    .child(div().flex_1().min_w_0().child(self.filter.clone())),
            )
            // section label — a count on the right, like every other list
            // header, in a trailing slot on the sub-header's own metrics.
            .child(
                menu_header(tr!("workspace_picker.recent_folders"), &theme)
                    .pt(picker::list_padding_y(&theme))
                    .when(!rows.is_empty(), |header| {
                        header.child(
                            div()
                                .flex_none()
                                .h(list::sub_header_height(&theme))
                                .px(list::sub_header_inset_x(&theme))
                                .flex()
                                .items_center()
                                .text_size(list::SUB_HEADER_TEXT.px(&theme))
                                .text_color(theme.text_muted)
                                .child(rows.len().to_string()),
                        )
                    }),
            )
            .child(if rows.is_empty() {
                // Zed's no-match state: one muted picker entry.
                div()
                    .py(picker::list_padding_y(&theme))
                    .child(
                        picker_entry(div(), &theme)
                            .text_color(theme.text_muted)
                            .child(icon(
                                "icons/search.svg",
                                context_menu::ICON.px(&theme),
                                theme.text_muted,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(tr!("workspace_picker.no_folders_match")),
                                    )
                                    .child(
                                        div()
                                            .text_size(picker::SECONDARY_TEXT.px(&theme))
                                            .child(tr!(
                                                "workspace_picker.try_another_name_or_choose_a_folder_below"
                                            )),
                                    ),
                            ),
                    )
                    .into_any_element()
            } else {
                list.into_any_element()
            })
            // browse row — the OS dialog, one step away, below a full-width
            // hairline like Zed's picker footer.
            .child(
                div()
                    .py(picker::list_padding_y(&theme))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        picker_entry(div().id("workspace-browse-row"), &theme)
                            .h(px(row_h(&theme)))
                            .cursor_pointer()
                            .when(browse_highlighted, |row| row.bg(theme.selection))
                            .when(!browse_highlighted, |row| {
                                row.hover(|s| s.bg(theme.surface_hover))
                            })
                            .on_mouse_move({
                                let this = this.clone();
                                move |_, _, cx| {
                                    this.update(cx, |picker, cx| {
                                        let browse_ix =
                                            picker.filtered(&picker.last_filter).len();
                                        if picker.highlighted != browse_ix {
                                            picker.highlighted = browse_ix;
                                            cx.notify();
                                        }
                                    });
                                }
                            })
                            .on_mouse_up(
                                gpui::MouseButton::Left,
                                cx.listener(|picker, _, window, cx| {
                                    (picker.on_browse)(window, cx);
                                }),
                            )
                            // The browse action opens the OS dialog, so it
                            // leads with a launch glyph, not a second folder.
                            .child(icon(
                                "icons/arrow-up-right.svg",
                                context_menu::ICON.px(&theme),
                                theme.text_secondary,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(theme.text_secondary)
                                    .child(tr!("workspace_picker.choose_folder")),
                            ),
                    ),
            )
    }
}
