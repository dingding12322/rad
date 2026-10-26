//! Extension UI dialogs — native rendering of pi's `extension_ui_request`
//! sub-protocol (`select` / `confirm` / `input` / `editor`).
//!
//! In RPC mode an extension blocks until the host answers. Tools such as
//! `ask_user_question` walk their questionnaire through these primitives
//! (single-select → `select`, multi-select → an `input` of comma-separated
//! option numbers, custom answers → `input`), so answering them here is what
//! makes a multiple-choice question selectable on the message.
//!
//! The app owns one `Dialog` entity at a time. Its `on_respond` callback
//! carries the answer back to `OrbitApp`, which forwards it over RPC and drops
//! the entity — the same entity-with-callbacks shape as `CommandPalette`.

use gpui::{
    deferred, div, point, prelude::*, px, AnyElement, App, Context, ElementId, Entity, FocusHandle,
    Focusable, FontWeight, IntoElement, MouseButton, MouseDownEvent, Render, ScrollHandle,
    SharedString, Window,
};

use crate::composer::ComposerInput;
use crate::theme::tokens::{
    button, input, list_item, modal, ButtonSize, DynamicSpacing, StyledExt, TextSize,
};
use crate::theme::{self, Theme};

/// Card width — a question plus its options without owning the window.
const CARD_W: f32 = 560.;
/// Option row height (label + inline description).
const OPTION_H: f32 = 44.;
/// Largest option-list height before it scrolls (≈ 7 rows).
const LIST_MAX_H: f32 = 320.;

/// Response callback: the answer plus the ambient window.
type DialogRespond = Box<dyn Fn(DialogResponse, &mut Window, &mut App)>;

/// The answer the host sends back over RPC (`extension_ui_response`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogResponse {
    /// `select` / `input` / `editor` → `{ "value": … }`.
    Value(String),
    /// `confirm` → `{ "confirmed": bool }`.
    Confirmed(bool),
    /// Any dialog → `{ "cancelled": true }` (the extension sees `undefined`).
    Cancelled,
}

/// What the extension is asking for, normalized from the request payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogKind {
    /// Pick one option (the option strings are already display-formatted).
    Select { options: Vec<String> },
    /// Yes / no.
    Confirm,
    /// A single line of free text.
    Input { placeholder: String },
    /// A multi-line editor, optionally prefilled.
    Editor { prefill: String },
}

/// One `extension_ui_request`, normalized for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogRequest {
    /// RPC request id — echoed in the response.
    pub id: String,
    pub title: String,
    /// Supporting copy (`confirm`'s message), when present.
    pub body: Option<String>,
    pub kind: DialogKind,
}

impl DialogRequest {
    /// Build a request from the raw `extension_ui_request` payload. Returns
    /// `None` for a method Orbit does not render as a dialog.
    pub fn from_event(id: String, method: &str, value: &serde_json::Value) -> Option<Self> {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let kind = match method {
            "select" => DialogKind::Select {
                options: value
                    .get("options")
                    .and_then(serde_json::Value::as_array)
                    .map(|options| {
                        options
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "confirm" => DialogKind::Confirm,
            "input" => DialogKind::Input {
                placeholder: text("placeholder"),
            },
            "editor" => DialogKind::Editor {
                prefill: text("prefill"),
            },
            _ => return None,
        };
        Some(Self {
            id,
            title: text("title"),
            body: match method {
                "confirm" => Some(text("message")),
                _ => None,
            },
            kind,
        })
    }
}

/// Marker the access-guard extension puts at the start of a `select` title so
/// Orbit routes it to the inline approval bar instead of the modal dialog.
/// Kept in sync with `GUARD_TITLE_PREFIX` in the extension's `policy.js`.
pub const GUARD_TITLE_PREFIX: &str = "[orbit-guard] ";

/// An inline access-guard approval: the guard extension asks before a mutating
/// tool call runs. Unlike [`DialogRequest`], this renders as a compact bar
/// above the composer — no scrim, no window-sized modal — with one button per
/// option (Allow once / Always allow this tool / Deny).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// RPC request id — echoed in the response.
    pub id: String,
    /// The tool pi wants to run (`bash`, `edit`, …).
    pub tool: String,
    /// The salient argument (command, path, …), already trimmed to one line.
    pub detail: String,
    /// Buttons to offer, in order; the extension maps the chosen string back.
    pub options: Vec<String>,
}

impl ApprovalRequest {
    /// Build an approval from an `extension_ui_request`, or `None` when it is
    /// not a guard request (wrong method, or no marker in the title).
    pub fn from_event(id: String, method: &str, value: &serde_json::Value) -> Option<Self> {
        if method != "select" {
            return None;
        }
        let title = value.get("title").and_then(serde_json::Value::as_str)?;
        let rest = title.strip_prefix(GUARD_TITLE_PREFIX)?;
        // The title is `tool\tdetail`; a missing tab degrades to the whole
        // remainder as the detail so a malformed request still renders.
        let (tool, detail) = rest.split_once('\t').unwrap_or(("", rest));
        let options = value
            .get("options")
            .and_then(serde_json::Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            id,
            tool: tool.to_string(),
            detail: detail.to_string(),
            options,
        })
    }
}

/// A blocking extension dialog, rendered as a modal card.
pub struct Dialog {
    request: DialogRequest,
    focus: FocusHandle,
    /// Highlighted option (select) or the focused button (confirm: 0 = no,
    /// 1 = yes; the affirmative is the default).
    highlighted: usize,
    /// Confirm state: the affirmative is preselected.
    confirmed: bool,
    /// Text field for `input` / `editor`.
    input: Option<Entity<ComposerInput>>,
    scroll: ScrollHandle,
    /// Guards against answering twice (e.g. a click double-firing).
    responded: bool,
    on_respond: DialogRespond,
}

impl Dialog {
    pub fn new(request: DialogRequest, on_respond: DialogRespond, cx: &mut Context<Self>) -> Self {
        let input = match &request.kind {
            DialogKind::Input { placeholder } => Some(cx.new(|cx| {
                ComposerInput::new(cx)
                    .with_element_id("dialog-input")
                    .with_placeholder(placeholder.clone())
                    .with_max_lines(1)
                    .with_wrap(false)
                    .with_key_context("Composer DialogInput")
            })),
            DialogKind::Editor { prefill } => Some(cx.new(|cx| {
                ComposerInput::new(cx)
                    .with_element_id("dialog-editor")
                    .with_text(prefill.clone())
                    .with_max_lines(12)
                    .with_key_context("Composer DialogInput")
            })),
            _ => None,
        };
        Self {
            request,
            focus: cx.focus_handle(),
            highlighted: 0,
            confirmed: true,
            input,
            scroll: ScrollHandle::new(),
            responded: false,
            on_respond,
        }
    }

    /// The RPC id this dialog is answering.
    pub fn request_id(&self) -> &str {
        &self.request.id
    }

    /// The key context that decides which dialog bindings apply: text fields
    /// keep caret keys, the option list claims ↑/↓.
    fn key_context(&self) -> &'static str {
        match self.request.kind {
            DialogKind::Select { .. } | DialogKind::Confirm => "DialogSelect",
            DialogKind::Input { .. } | DialogKind::Editor { .. } => "DialogInput",
        }
    }

    fn respond(&mut self, response: DialogResponse, window: &mut Window, cx: &mut Context<Self>) {
        if self.responded {
            return;
        }
        self.responded = true;
        (self.on_respond)(response, window, cx);
    }

    fn on_cancel(&mut self, _: &crate::DialogCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.respond(DialogResponse::Cancelled, window, cx);
    }

    fn on_confirm(
        &mut self,
        _: &crate::DialogConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let response = match &self.request.kind {
            DialogKind::Select { options } => match options.get(self.highlighted) {
                Some(option) => DialogResponse::Value(option.clone()),
                // Nothing to pick (an empty list) — treat as a dismissal.
                None => DialogResponse::Cancelled,
            },
            DialogKind::Confirm => DialogResponse::Confirmed(self.confirmed),
            DialogKind::Input { .. } | DialogKind::Editor { .. } => {
                let text = self
                    .input
                    .as_ref()
                    .map(|input| input.read(cx).text())
                    .unwrap_or_default();
                DialogResponse::Value(text)
            }
        };
        self.respond(response, window, cx);
    }

    fn on_next(&mut self, _: &crate::DialogNext, _: &mut Window, cx: &mut Context<Self>) {
        if let DialogKind::Select { options } = &self.request.kind {
            if !options.is_empty() {
                self.highlighted = (self.highlighted + 1) % options.len();
                self.reveal_highlighted();
                cx.notify();
            }
        }
    }

    fn on_prev(&mut self, _: &crate::DialogPrev, _: &mut Window, cx: &mut Context<Self>) {
        if let DialogKind::Select { options } = &self.request.kind {
            if !options.is_empty() {
                self.highlighted = (self.highlighted + options.len() - 1) % options.len();
                self.reveal_highlighted();
                cx.notify();
            }
        }
    }

    fn on_scrim_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.respond(DialogResponse::Cancelled, window, cx);
    }

    /// Scroll so the keyboard highlight stays visible (options are few, but a
    /// select may carry more than fit).
    fn reveal_highlighted(&self) {
        let top = self.highlighted as f32 * OPTION_H;
        let bottom = top + OPTION_H;
        let scroll_top = f32::from(self.scroll.offset().y);
        let max_h = LIST_MAX_H;
        if top < scroll_top {
            self.scroll.set_offset(point(px(0.), px(top)));
        } else if bottom > scroll_top + max_h {
            self.scroll.set_offset(point(px(0.), px(bottom - max_h)));
        }
    }
}

impl Focusable for Dialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input
            .as_ref()
            .map(|input| input.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.focus.clone())
    }
}

impl Render for Dialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        let this = cx.entity();

        let mut card = div()
            .id("dialog-card")
            .debug_selector(|| "dialog-card".to_string())
            .w_full()
            .max_w(px(CARD_W))
            .elevation_3(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .font_family(theme::ui_font_family())
            .key_context(self.key_context())
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            // Clicks inside the card must not reach the scrim's dismiss.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        // ── header: origin, title, and (confirm) the message body ──
        // Zed's `ModalHeader` insets, with its Small headline for the title.
        let mut header = div()
            .px(modal::header_padding_x(&theme))
            .pt(modal::header_padding_top(&theme))
            .pb(modal::header_padding_bottom(&theme))
            .flex_none()
            .flex()
            .flex_col()
            .gap(DynamicSpacing::Base04.px(&theme))
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(SharedString::from("Question")),
            )
            .child(
                div()
                    .whitespace_normal()
                    .text_size(modal::HEADLINE.px(&theme))
                    .line_height(modal::HEADLINE.line_height(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_primary)
                    .child(SharedString::from(self.request.title.clone())),
            );
        if let Some(body) = self.request.body.clone() {
            if !body.trim().is_empty() {
                header = header.child(
                    // Zed's header description closes with `mb_2`.
                    div()
                        .mb(theme.rems(0.5))
                        .whitespace_normal()
                        .text_size(TextSize::Default.px(&theme))
                        .text_color(theme.text_secondary)
                        .child(SharedString::from(body)),
                );
            }
        }
        card = card.child(header);

        // ── body: options, buttons, or a text field ──
        match &self.request.kind {
            DialogKind::Select { options } => {
                card = card.child(self.render_options(options, &this, theme));
            }
            DialogKind::Confirm => {
                card = card.child(self.render_confirm(theme, cx));
            }
            DialogKind::Input { .. } | DialogKind::Editor { .. } => {
                if let Some(input) = &self.input {
                    // A modal section holding a Zed `InputField`-sized box.
                    card = card.child(
                        div()
                            .px(modal::section_padding_x(&theme, false))
                            .pb(modal::section_padding_bottom(&theme))
                            .flex_none()
                            .child(
                                div()
                                    .min_h(input::min_height(&theme))
                                    .px(input::padding_x(&theme))
                                    .py(input::padding_y(&theme))
                                    .rounded(input::RADIUS.px(&theme))
                                    .border_1()
                                    .border_color(theme.border_strong)
                                    .bg(theme.surface_elevated)
                                    .child(input.clone()),
                            ),
                    );
                }
            }
        }

        // ── footer: name the keys once ──
        let hint = match self.request.kind {
            DialogKind::Select { .. } => "↑↓ Navigate · ⏎ Select · esc Cancel",
            DialogKind::Confirm => "⏎ Confirm · esc Cancel",
            DialogKind::Input { .. } | DialogKind::Editor { .. } => "⏎ Submit · esc Cancel",
        };
        // Zed's `ModalFooter` pads Base08 all round because its slots hold
        // buttons, whose own padding brings their labels in line with the
        // header; a bare hint takes the header's inset directly.
        card = card.child(
            div()
                .py(modal::footer_padding(&theme))
                .px(modal::header_padding_x(&theme))
                .flex_none()
                .flex()
                .items_center()
                .gap(modal::footer_gap(&theme))
                .border_t_1()
                .border_color(theme.border)
                .text_size(TextSize::Small.px(&theme))
                .text_color(theme.text_muted)
                .child(hint),
        );

        // ── scrim: dimmed backdrop; a click outside cancels the dialog ──
        let scrim = theme.scrim_modal();
        div()
            .id("dialog-layer")
            .debug_selector(|| "dialog-layer".to_string())
            .absolute()
            .inset_0()
            .occlude()
            .bg(scrim)
            .px(DynamicSpacing::Base24.px(&theme))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_scrim_down))
            .child(card)
    }
}

impl Dialog {
    /// The select option list: a scrollable column of full-width rows; the
    /// keyboard highlight doubles as hover.
    fn render_options(
        &self,
        options: &[String],
        this: &Entity<Dialog>,
        theme: Theme,
    ) -> AnyElement {
        if options.is_empty() {
            return div()
                .px(modal::section_padding_x(&theme, false))
                .pb(modal::section_padding_bottom(&theme))
                .text_size(TextSize::Default.px(&theme))
                .text_color(theme.text_muted)
                .child(tr!("dialog.no_options_were_provided"))
                .into_any_element();
        }
        let list_h = (options.len() as f32 * OPTION_H).min(LIST_MAX_H);
        let mut list = div()
            .id("dialog-options")
            .w_full()
            .h(px(list_h))
            .flex_none()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            // Rows are inset like Zed's list items, and pad their labels
            // back out to the header's inset.
            .px(list_item::inset(&theme))
            .pb(modal::section_padding_bottom(&theme))
            .flex()
            .flex_col();
        for (ix, option) in options.iter().enumerate() {
            let highlighted = ix == self.highlighted;
            let this = this.clone();
            list = list.child(
                div()
                    .id(ElementId::NamedInteger("dialog-option".into(), ix as u64))
                    .w_full()
                    .min_h(px(OPTION_H))
                    .px(modal::header_padding_x(&theme) - list_item::inset(&theme))
                    .py(px(9.))
                    .rounded(list_item::RADIUS.px(&theme))
                    .border_1()
                    .border_color(if highlighted {
                        theme.border_strong
                    } else {
                        gpui::transparent_black()
                    })
                    .cursor_pointer()
                    // Pointer movement moves the highlight; a scroll sliding a
                    // row under a stationary pointer must not hijack ↑/↓ nav.
                    .on_mouse_move({
                        let this = this.clone();
                        move |_, _, cx| {
                            this.update(cx, |dialog, cx| {
                                if dialog.highlighted != ix {
                                    dialog.highlighted = ix;
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| {
                            this.update(cx, |dialog, cx| {
                                dialog.highlighted = ix;
                                dialog.on_confirm(&crate::DialogConfirm, window, cx);
                            });
                        }
                    })
                    .when(highlighted, |row| row.bg(theme.selection))
                    .child(
                        div()
                            .whitespace_normal()
                            .text_size(TextSize::Default.px(&theme))
                            .text_color(if highlighted {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .child(SharedString::from(option.clone())),
                    ),
            );
        }
        list.into_any_element()
    }

    /// The confirm dialog's Yes/No buttons; the keyboard highlight and hover
    /// move together, Enter commits the focused one.
    fn render_confirm(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        div()
            .px(modal::section_padding_x(&theme, false))
            .pb(modal::section_padding_bottom(&theme))
            .flex_none()
            .flex()
            .gap(DynamicSpacing::Base08.px(&theme))
            .children([false, true].into_iter().enumerate().map(|(ix, value)| {
                let highlighted = self.confirmed == value;
                let this = this.clone();
                div()
                    .id(ElementId::NamedInteger("dialog-button".into(), ix as u64))
                    .h(ButtonSize::Large.height(&theme))
                    .px(ButtonSize::Large.padding_x(&theme))
                    .flex_1()
                    .rounded(button::RADIUS.px(&theme))
                    .border_1()
                    .border_color(if highlighted {
                        theme.border_strong
                    } else {
                        theme.border
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(TextSize::Default.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .on_hover({
                        let this = this.clone();
                        move |hovering, _, cx| {
                            if *hovering {
                                this.update(cx, |dialog, cx| {
                                    if dialog.confirmed != value {
                                        dialog.confirmed = value;
                                        cx.notify();
                                    }
                                });
                            }
                        }
                    })
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| {
                            this.update(cx, |dialog, cx| {
                                dialog.confirmed = value;
                                dialog.on_confirm(&crate::DialogConfirm, window, cx);
                            });
                        }
                    })
                    .when(highlighted, |button| {
                        button
                            .bg(theme.overlay_strong)
                            .text_color(theme.text_primary)
                    })
                    .when(!highlighted, |button| {
                        button.text_color(theme.text_secondary)
                    })
                    .child(if value { "Yes" } else { "No" })
            }))
            .into_any_element()
    }
}

/// The dialog paints above the command palette and every popover.
pub fn layer(dialog: Entity<Dialog>) -> impl IntoElement {
    deferred(dialog).with_priority(9)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn select_parses_title_and_options() {
        let request = DialogRequest::from_event(
            "id-1".into(),
            "select",
            &json!({
                "title": "Allow dangerous command?",
                "options": ["1. Allow — run it", "2. Block — stop"],
            }),
        )
        .unwrap();
        assert_eq!(request.title, "Allow dangerous command?");
        assert_eq!(
            request.kind,
            DialogKind::Select {
                options: vec!["1. Allow — run it".into(), "2. Block — stop".into()],
            }
        );
    }

    #[test]
    fn select_tolerates_missing_options() {
        let request =
            DialogRequest::from_event("id".into(), "select", &json!({"title": "Pick"})).unwrap();
        assert_eq!(request.kind, DialogKind::Select { options: vec![] });
    }

    #[test]
    fn confirm_carries_message_as_body() {
        let request = DialogRequest::from_event(
            "id-2".into(),
            "confirm",
            &json!({"title": "Clear session?", "message": "All messages will be lost."}),
        )
        .unwrap();
        assert_eq!(request.body.as_deref(), Some("All messages will be lost."));
        assert_eq!(request.kind, DialogKind::Confirm);
    }

    #[test]
    fn input_and_editor_parse_their_fields() {
        let input = DialogRequest::from_event(
            "id-3".into(),
            "input",
            &json!({"title": "Enter a value", "placeholder": "type something…"}),
        )
        .unwrap();
        assert_eq!(
            input.kind,
            DialogKind::Input {
                placeholder: "type something…".into(),
            }
        );
        let editor = DialogRequest::from_event(
            "id-4".into(),
            "editor",
            &json!({"title": "Edit", "prefill": "Line 1\nLine 2"}),
        )
        .unwrap();
        assert_eq!(
            editor.kind,
            DialogKind::Editor {
                prefill: "Line 1\nLine 2".into(),
            }
        );
    }

    #[test]
    fn fire_and_forget_methods_are_not_dialogs() {
        for method in [
            "notify",
            "setStatus",
            "setWidget",
            "setTitle",
            "set_editor_text",
        ] {
            assert!(
                DialogRequest::from_event("id".into(), method, &json!({})).is_none(),
                "{method} must not open a modal"
            );
        }
    }

    #[test]
    fn guard_select_parses_tool_detail_and_options() {
        let request = ApprovalRequest::from_event(
            "id-1".into(),
            "select",
            &json!({
                "title": "[orbit-guard] bash\trm -rf build",
                "options": ["Allow once", "Always allow this tool", "Deny"],
            }),
        )
        .unwrap();
        assert_eq!(request.tool, "bash");
        assert_eq!(request.detail, "rm -rf build");
        assert_eq!(request.options.len(), 3);
    }

    #[test]
    fn a_guard_title_without_a_tab_still_renders() {
        let request = ApprovalRequest::from_event(
            "id".into(),
            "select",
            &json!({"title": "[orbit-guard] edit", "options": ["Deny"]}),
        )
        .unwrap();
        assert_eq!(request.tool, "");
        assert_eq!(request.detail, "edit");
    }

    #[test]
    fn plain_select_and_confirm_are_not_approvals() {
        let plain = ApprovalRequest::from_event(
            "id".into(),
            "select",
            &json!({"title": "Pick", "options": []}),
        );
        assert!(plain.is_none(), "an unmarked select is a normal dialog");
        let confirm =
            ApprovalRequest::from_event("id".into(), "confirm", &json!({"title": "Sure?"}));
        assert!(confirm.is_none(), "confirm is never an approval");
    }

    /// The select dialog lays out a bounded card with its option rows — the
    /// regression guard for a modal that paints nothing (a 0-size flex bug).
    #[gpui::test]
    fn select_dialog_lays_out_a_card(cx: &mut gpui::TestAppContext) {
        use crate::theme::{Theme, ThemeId};
        use gpui::size;

        cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
        let cx = cx.add_empty_window();
        let request = DialogRequest {
            id: "d1".into(),
            title: "Which approach?".into(),
            body: None,
            kind: DialogKind::Select {
                options: vec!["1. Fast — simpler".into(), "2. Safe — slower".into()],
            },
        };
        let _ = cx.draw(point(px(0.), px(0.)), size(px(800.), px(600.)), |_, cx| {
            let on_respond = Box::new(|_: DialogResponse, _: &mut Window, _: &mut App| {});
            cx.new(|cx| Dialog::new(request, on_respond, cx))
        });
        let card = cx
            .debug_bounds("dialog-card")
            .expect("dialog card laid out");
        assert!(card.size.width > px(0.), "card collapsed to zero width");
        assert!(card.size.width <= px(CARD_W));
        assert!(card.size.height > px(0.), "card collapsed to zero height");
    }
}
