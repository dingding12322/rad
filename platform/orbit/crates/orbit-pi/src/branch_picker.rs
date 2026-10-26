//! Git branch picker — search, checkout, and create-from-current-state.
//!
//! Opened from the status-bar branch chip; follows the same popover conventions
//! as [`crate::command_palette::CommandPalette`].

use gpui::{
    div, point, prelude::*, px, App, Context, ElementId, Entity, FocusHandle, Focusable,
    FontWeight, IntoElement, MouseDownEvent, ParentElement, Render, ScrollHandle, Styled,
    Subscription, Window,
};

use crate::app::{
    button_frame, icon, input_field_frame, menu_header, picker_entry, picker_search_frame,
    picker_surface,
};
use crate::composer::ComposerInput;
use crate::theme::tokens::RaisedExt;
use crate::theme::tokens::{context_menu, input, picker, ButtonSize, DynamicSpacing, TextSize};
use crate::theme::{self, Theme};

const POPOVER_W: f32 = 280.;
/// Branch rows visible before the list scrolls.
const VISIBLE_ROWS: f32 = 7.;

/// A branch row: a one-line picker entry. Layout and the keyboard reveal
/// math both read these, so they can never disagree.
fn row_h(theme: &Theme) -> f32 {
    picker::entry_height(theme).into()
}

/// Air between branch rows.
fn row_gap(theme: &Theme) -> f32 {
    DynamicSpacing::Base01.px(theme).into()
}

fn row_stride(theme: &Theme) -> f32 {
    row_h(theme) + row_gap(theme)
}

/// Largest list height before it scrolls.
fn list_max_h(theme: &Theme) -> f32 {
    VISIBLE_ROWS * row_stride(theme)
}

/// Checkout/create callback: the chosen branch plus the ambient window.
type BranchAction = Box<dyn Fn(String, &mut Window, &mut App)>;
/// Dismiss callback; `bool` is true when an outside mouse-down closed it.
type BranchPickerDismiss = Box<dyn Fn(bool, &mut Window, &mut App)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browse,
    Create,
}

pub struct BranchPicker {
    branches: Vec<String>,
    current: String,
    filter: Entity<ComposerInput>,
    create_input: Entity<ComposerInput>,
    mode: Mode,
    list_scroll: ScrollHandle,
    highlighted: usize,
    last_filter: String,
    _filter_sub: Subscription,
    on_checkout: BranchAction,
    on_create: BranchAction,
    on_dismiss: BranchPickerDismiss,
}

impl BranchPicker {
    pub fn new(
        workspace_label: String,
        branches: Vec<String>,
        current: String,
        on_checkout: BranchAction,
        on_create: BranchAction,
        on_dismiss: BranchPickerDismiss,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("branch_picker.search_branches")
                .with_placeholder_var("workspace", workspace_label)
                .with_key_context("Composer Picker")
        });
        // Keep the derived filter state in sync from the input's notifications
        // so `render` only reads it.
        let filter_sub = cx.observe(&filter, |picker: &mut Self, filter, cx| {
            let needle = filter.read(cx).text().to_lowercase();
            if needle != picker.last_filter {
                picker.last_filter = needle;
                picker.highlighted = 0;
                picker.list_scroll.set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
        });
        let create_input = cx.new(|cx| {
            ComposerInput::new(cx)
                .with_placeholder_key("branch_picker.new_branch_name")
                .with_key_context("Composer Picker")
                .with_max_lines(1)
                .with_wrap(false)
        });
        Self {
            branches,
            current,
            filter,
            create_input,
            mode: Mode::Browse,
            list_scroll: ScrollHandle::new(),
            highlighted: 0,
            last_filter: String::new(),
            _filter_sub: filter_sub,
            on_checkout,
            on_create,
            on_dismiss,
        }
    }

    fn on_cancel(&mut self, _: &crate::PickerCancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Create {
            self.mode = Mode::Browse;
            self.highlighted = 0;
            window.focus(&self.filter.read(cx).focus_handle(cx));
            cx.notify();
            return;
        }
        (self.on_dismiss)(false, window, cx);
    }

    fn on_confirm(
        &mut self,
        _: &crate::PickerConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode == Mode::Create {
            let name = self.create_input.read(cx).text().trim().to_string();
            if !name.is_empty() {
                (self.on_create)(name, window, cx);
            }
            return;
        }
        let rows = self.filtered_branches(&self.last_filter);
        if let Some(branch) = rows.get(self.highlighted).cloned() {
            if branch != self.current {
                (self.on_checkout)(branch, window, cx);
            } else {
                (self.on_dismiss)(false, window, cx);
            }
        }
    }

    fn on_next(&mut self, _: &crate::PickerSelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Browse {
            return;
        }
        self.step(1, cx);
    }

    fn on_prev(&mut self, _: &crate::PickerSelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Browse {
            return;
        }
        self.step(-1, cx);
    }

    fn on_outside_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_dismiss)(true, window, cx);
    }

    fn filtered_branches(&self, needle: &str) -> Vec<String> {
        self.branches
            .iter()
            .filter(|branch| needle.is_empty() || branch.to_lowercase().contains(needle))
            .cloned()
            .collect()
    }

    fn step(&mut self, dir: isize, cx: &mut Context<Self>) {
        let rows = self.filtered_branches(&self.last_filter);
        if rows.is_empty() {
            return;
        }
        let pos = self.highlighted.min(rows.len() - 1);
        let next = if dir > 0 {
            (pos + 1).min(rows.len() - 1)
        } else {
            pos.saturating_sub(1)
        };
        self.highlighted = next;
        let theme = theme::get(cx);
        let (row_h, row_gap) = (row_h(theme), row_gap(theme));
        let row_top = next as f32 * row_stride(theme);
        // `ScrollHandle`'s offset is negative once scrolled down.
        let current = -f32::from(self.list_scroll.offset().y);
        let n = rows.len() as f32;
        let content_h = (n * row_h + (n - 1.).max(0.) * row_gap).max(0.);
        let viewport_h = content_h.min(list_max_h(theme));
        let mut offset = current;
        if row_top < current {
            offset = row_top;
        } else if row_top + row_h > current + viewport_h {
            offset = row_top + row_h - viewport_h;
        }
        let max_offset = (content_h - viewport_h).max(0.);
        self.list_scroll
            .set_offset(point(px(0.), px(-offset.clamp(0., max_offset))));
        cx.notify();
    }

    fn begin_create(
        &mut self,
        _: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mode = Mode::Create;
        self.create_input.update(cx, |input, cx| {
            input.clear(cx);
        });
        window.focus(&self.create_input.read(cx).focus_handle(cx));
        cx.notify();
    }
}

impl Focusable for BranchPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.mode {
            Mode::Browse => self.filter.read(cx).focus_handle(cx),
            Mode::Create => self.create_input.read(cx).focus_handle(cx),
        }
    }
}

impl Render for BranchPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        let this = cx.entity();

        if self.mode == Mode::Create {
            // The same picker surface as Browse, so switching modes never
            // swaps the popover's chrome.
            return picker_surface(div(), &theme)
                .w(px(POPOVER_W))
                .py(DynamicSpacing::Base06.px(&theme))
                .flex()
                .flex_col()
                .overflow_hidden()
                .occlude()
                .on_mouse_down_out(cx.listener(Self::on_outside_down))
                .on_action(cx.listener(Self::on_cancel))
                .on_action(cx.listener(Self::on_confirm))
                .child(
                    div()
                        .px(DynamicSpacing::Base12.px(&theme))
                        .pt(DynamicSpacing::Base04.px(&theme))
                        .pb(DynamicSpacing::Base08.px(&theme))
                        .flex()
                        .flex_col()
                        .gap(DynamicSpacing::Base06.px(&theme))
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(tr!("branch_picker.create_and_checkout_new_branch")),
                        )
                        .child(
                            div()
                                .text_size(TextSize::Small.px(&theme))
                                .text_color(theme.text_muted)
                                .child(tr!("branch_picker.uncommitted_changes_come_with_you")),
                        )
                        .child(
                            input_field_frame(div(), &theme)
                                .w_full()
                                .bg(theme.background)
                                .child(self.create_input.clone()),
                        ),
                )
                .child(
                    button_frame(div(), &theme, ButtonSize::Medium)
                        .mx(DynamicSpacing::Base08.px(&theme))
                        .mb(DynamicSpacing::Base04.px(&theme))
                        .raised(theme.action, &theme)
                        .hover(|s| s.raised(theme.action_hover, &theme))
                        .cursor_pointer()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.action_fg)
                        .on_mouse_up(
                            gpui::MouseButton::Left,
                            cx.listener(|picker, _, window, cx| {
                                let name = picker.create_input.read(cx).text().trim().to_string();
                                if !name.is_empty() {
                                    (picker.on_create)(name, window, cx);
                                }
                            }),
                        )
                        .child(tr!("branch_picker.create_branch")),
                );
        }

        let rows = self.filtered_branches(&self.last_filter);

        let mut list = div()
            .id("branch-picker-list")
            .w_full()
            .max_h(px(list_max_h(&theme)))
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .py(picker::list_padding_y(&theme))
            .flex()
            .flex_col()
            .gap(px(row_gap(&theme)));
        for (ix, branch) in rows.iter().enumerate() {
            let selected = *branch == self.current;
            let highlighted = ix == self.highlighted;
            let branch_label = branch.clone();
            let branch_compare = branch.clone();
            let branch_action = branch.clone();
            let this = this.clone();
            list = list.child(
                picker_entry(
                    div().id(ElementId::NamedInteger("branch-row".into(), ix as u64)),
                    &theme,
                )
                .h(px(row_h(&theme)))
                .flex_none()
                .cursor_pointer()
                .when(selected || highlighted, |row| row.bg(theme.selection))
                .when(!selected && !highlighted, |row| {
                    row.hover(|s| s.bg(theme.surface_hover))
                })
                .on_hover({
                    let this = this.clone();
                    move |hovering, _, cx| {
                        if *hovering {
                            this.update(cx, |picker, cx| {
                                if picker.highlighted != ix {
                                    picker.highlighted = ix;
                                    cx.notify();
                                }
                            });
                        }
                    }
                })
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(move |picker, _, window, cx| {
                        if branch_compare == picker.current {
                            (picker.on_dismiss)(false, window, cx);
                        } else {
                            (picker.on_checkout)(branch_action.clone(), window, cx);
                        }
                    }),
                )
                .child(icon(
                    "icons/branch.svg",
                    context_menu::ICON.px(&theme),
                    theme.text_muted,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if selected {
                            theme.text_primary
                        } else {
                            theme.text_secondary
                        })
                        .child(branch_label),
                )
                .when(selected, |row| {
                    row.child(icon(
                        "icons/check.svg",
                        context_menu::ICON.px(&theme),
                        theme.accent_text,
                    ))
                }),
            );
        }

        picker_surface(div(), &theme)
            .w(px(POPOVER_W))
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .on_mouse_down_out(cx.listener(Self::on_outside_down))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .child(
                picker_search_frame(div(), &theme)
                    .child(icon(
                        "icons/search.svg",
                        input::ICON.px(&theme),
                        theme.text_muted,
                    ))
                    .child(div().flex_1().min_w_0().child(self.filter.clone())),
            )
            .child(
                menu_header(tr!("branch_picker.branches"), &theme)
                    .pt(picker::list_padding_y(&theme)),
            )
            .child(if rows.is_empty() {
                // Zed's no-match state: one muted picker entry.
                div()
                    .py(picker::list_padding_y(&theme))
                    .child(
                        picker_entry(div(), &theme)
                            .text_color(theme.text_muted)
                            .child(tr!("branch_picker.no_branches_match")),
                    )
                    .into_any_element()
            } else {
                list.into_any_element()
            })
            // Footer action, below a full-width hairline like Zed's picker
            // footer.
            .child(
                div()
                    .py(picker::list_padding_y(&theme))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        picker_entry(div(), &theme)
                            .h(picker::entry_height(&theme))
                            .cursor_pointer()
                            .hover(|s| s.bg(theme.surface_hover))
                            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::begin_create))
                            .child(icon(
                                "icons/plus.svg",
                                context_menu::ICON.px(&theme),
                                theme.text_secondary,
                            ))
                            .child(
                                div()
                                    .text_color(theme.text_secondary)
                                    .child(tr!("branch_picker.create_and_checkout_new_branch_2")),
                            ),
                    ),
            )
    }
}
