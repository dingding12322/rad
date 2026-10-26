//! Extension custom UI — pi's `ctx.ui.custom()` rendered over RPC.
//!
//! pi's RPC mode can render an extension's `Component` to terminal lines and
//! stream them as `extension_ui_request` frames (`method: "custom"`, phases
//! `open`/`render`/`close`). This is the host side: a modal that paints those
//! lines in a monospace grid, translates keystrokes back into the terminal
//! bytes the component's `handleInput` expects, and answers `close`/cancel.
//!
//! The server owns layout — it composites components and overlays into final
//! lines — so this module never needs to know a pi-tui type. It is gated on
//! the `custom` capability from `get_state`; a pi that lacks it never sends a
//! frame, and the dormant code costs nothing.
//!
//! Wire contract: `crates/orbit-rpc/docs/pi-custom-ui-proposal.md`.

use alacritty_terminal::term::TermMode;
use gpui::{
    deferred, div, prelude::*, px, App, ClickEvent, Context, Entity, FocusHandle, Focusable, Font,
    FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Render, ScrollHandle,
    SharedString, StyledText, Window,
};
use serde_json::Value;

use crate::app::{icon, icon_button_frame, TipExt};
use crate::terminal::encode_key;
use crate::theme;
use crate::theme::tokens::{ButtonSize, DynamicSpacing, IconSize, Radius, StyledExt, TextSize};
use crate::widgets;

/// Column budget the host offers the component: the card is exactly this many
/// monospace cells wide at the active code font size.
pub const CUSTOM_UI_COLUMNS: usize = 96;
/// Row budget reported to the server (a hint; the card scrolls past it).
pub const CUSTOM_UI_ROWS: usize = 20;
/// Widest the card may grow before columns are clamped by the window.
const CARD_MAX_W: f32 = 960.;

/// The server-reported cursor cell (0-based), for the drawn caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomCursor {
    pub row: usize,
    pub col: usize,
}

/// Keyboard protocol the component expects. Everything the host emits today is
/// the legacy subset; `kitty` is recorded so a future encoder can honor it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardMode {
    Legacy,
    Kitty,
}

/// An `open`/`render` frame: the component's rendered lines plus context.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomSurface {
    pub id: String,
    pub title: Option<String>,
    pub lines: Vec<String>,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub cursor: Option<CustomCursor>,
    pub keyboard: KeyboardMode,
    pub mouse: bool,
    pub overlay: bool,
}

/// A decoded `method:"custom"` frame.
#[derive(Debug, Clone, PartialEq)]
pub enum CustomFrame {
    Open(CustomSurface),
    Render(CustomSurface),
    Close { id: String, result: Value },
}

impl CustomFrame {
    /// Decode a `custom` request from its raw `extension_ui_request` payload.
    /// Returns `None` for any other method or an unknown phase.
    pub fn from_event(value: &Value) -> Option<Self> {
        if value.get("method").and_then(Value::as_str) != Some("custom") {
            return None;
        }
        let id = value.get("id").and_then(Value::as_str)?.to_string();
        match value
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or("render")
        {
            phase @ ("open" | "render") => {
                let surface = CustomSurface {
                    id,
                    title: value
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    lines: parse_lines(value.get("lines")),
                    width: value
                        .get("width")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize),
                    height: value
                        .get("height")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize),
                    cursor: parse_cursor(value.get("cursor")),
                    keyboard: if value.get("keyboard").and_then(Value::as_str) == Some("kitty") {
                        KeyboardMode::Kitty
                    } else {
                        KeyboardMode::Legacy
                    },
                    mouse: value.get("mouse").and_then(Value::as_bool).unwrap_or(false),
                    overlay: value
                        .get("overlay")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                };
                Some(if phase == "open" {
                    CustomFrame::Open(surface)
                } else {
                    CustomFrame::Render(surface)
                })
            }
            "close" => Some(CustomFrame::Close {
                id,
                result: value.get("result").cloned().unwrap_or(Value::Null),
            }),
            _ => None,
        }
    }
}

fn parse_lines(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|lines| {
            lines
                .iter()
                .map(|line| line.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_cursor(value: Option<&Value>) -> Option<CustomCursor> {
    let value = value?;
    Some(CustomCursor {
        row: value.get("row").and_then(Value::as_u64)? as usize,
        col: value.get("col").and_then(Value::as_u64)? as usize,
    })
}

/// Ask the app to forward one encoded keystroke to the component.
pub type CustomInput = Box<dyn Fn(String, &mut Window, &mut App)>;
/// Ask the app to cancel the surface (host-side dismissal).
pub type CustomCancel = Box<dyn Fn(&mut Window, &mut App)>;

/// The stack of open custom surfaces, newest last.
///
/// Each entry caches its RPC id beside the entity. The app must locate and
/// drop a surface from inside that surface's own scrim/close handler, where
/// gpui has leased the entity for the duration of the handler — reading it
/// back out would panic with "already being updated". Every lookup here goes
/// through the cached id and never reads an entity.
#[derive(Default)]
pub struct CustomUiSurfaces {
    entries: Vec<(String, Entity<CustomUi>)>,
}

impl CustomUiSurfaces {
    /// Add a surface under its RPC id.
    pub fn push(&mut self, id: String, ui: Entity<CustomUi>) {
        self.entries.push((id, ui));
    }

    /// The live surface for `id`, if any.
    pub fn find(&self, id: &str) -> Option<Entity<CustomUi>> {
        self.entries
            .iter()
            .find(|(entry_id, _)| entry_id == id)
            .map(|(_, ui)| ui.clone())
    }

    /// Whether a surface for `id` is open.
    pub fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|(entry_id, _)| entry_id == id)
    }

    /// Drop the surface for `id`.
    pub fn remove(&mut self, id: &str) {
        self.entries.retain(|(entry_id, _)| entry_id != id);
    }

    /// The newest surface (top of the stack), which owns focus.
    pub fn last(&self) -> Option<Entity<CustomUi>> {
        self.entries.last().map(|(_, ui)| ui.clone())
    }

    /// The RPC ids of every open surface.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(id, _)| id.as_str())
    }

    /// Every open surface, bottom to top.
    pub fn iter(&self) -> impl Iterator<Item = &Entity<CustomUi>> {
        self.entries.iter().map(|(_, ui)| ui)
    }
}

/// One live custom surface: the latest frame plus the app callbacks that
/// answer it.
pub struct CustomUi {
    surface: CustomSurface,
    focus: FocusHandle,
    scroll: ScrollHandle,
    on_input: CustomInput,
    on_cancel: CustomCancel,
}

impl CustomUi {
    pub fn new(
        surface: CustomSurface,
        on_input: CustomInput,
        on_cancel: CustomCancel,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            surface,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            on_input,
            on_cancel,
        }
    }

    /// Apply a `render` frame in place.
    pub fn apply(&mut self, surface: CustomSurface) {
        self.surface = surface;
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bytes) = encode_key(&event.keystroke, TermMode::empty()) {
            (self.on_input)(String::from_utf8_lossy(&bytes).into_owned(), window, cx);
        }
    }

    /// Escape is bound to this action (so it does not reach the global
    /// `AbortRun`) and forwarded as `ESC` — components cancel themselves via
    /// `SelectList.onCancel`/`onKey`, exactly as in the TUI.
    fn on_escape(
        &mut self,
        _: &crate::CustomUiEscape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        (self.on_input)("\x1b".to_string(), window, cx);
    }

    fn on_scrim_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_cancel)(window, cx);
    }

    /// The header's close control is the explicit, always-available dismissal
    /// (Escape is forwarded to the component, which may or may not back out).
    fn on_close_click(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_cancel)(window, cx);
    }
}

impl Focusable for CustomUi {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CustomUi {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        let font_size = theme.code_px(12.);
        let line_height = theme.code_px(18.);
        let font = widgets::widget_font();
        let cell = measure_cell(window, &font, font_size);
        let columns = self
            .surface
            .width
            .unwrap_or(CUSTOM_UI_COLUMNS)
            .clamp(20, 240);
        let viewport = window.viewport_size();
        let viewport_w = f32::from(viewport.width);
        let viewport_h = f32::from(viewport.height);
        // Exactly `columns` monospace cells, but never wider than the shared
        // modal width or the window, and never a sliver.
        let max_w = CARD_MAX_W.min((viewport_w - 48.).max(280.));
        let desired = f32::from(cell) * columns as f32;
        let card_w = px(desired.min(max_w).max(300_f32.min(max_w)));

        // ── the component's lines, in the grid the server laid out ──
        let mut grid = div().relative().flex().flex_col();
        for line in &self.surface.lines {
            let (text, runs) = widgets::styled_line(line, &theme, &font);
            grid = grid.child(
                div()
                    .h(line_height)
                    .whitespace_nowrap()
                    .child(StyledText::new(text).with_runs(runs)),
            );
        }
        if let Some(cursor) = self.surface.cursor {
            grid = grid.child(
                div()
                    .absolute()
                    .left(cell * cursor.col as f32)
                    .top(line_height * cursor.row as f32)
                    .w(px(2.))
                    .h(line_height)
                    .rounded_full()
                    .bg(theme.accent),
            );
        }

        // ── header: the surface's origin, its title, and a close control ──
        let mut meta = div()
            .flex()
            .items_center()
            .gap(DynamicSpacing::Base08.px(&theme))
            .child(
                div()
                    .text_size(TextSize::Small.px(&theme))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(SharedString::from("Extension UI")),
            );
        if let Some(title) = self
            .surface
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            meta = meta
                .child(
                    div()
                        .text_color(theme.border_strong)
                        .child(SharedString::from("·")),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(TextSize::Small.px(&theme))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_primary)
                        .child(SharedString::from(title.to_string())),
                );
        }
        let header = div()
            .flex_none()
            .h(px(42.))
            .debug_selector(|| "custom-ui-header".to_string())
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(10.))
            .border_b_1()
            .border_color(theme.border)
            .child(icon(
                "icons/extensions.svg",
                IconSize::Small.px(&theme),
                theme.text_muted,
            ))
            .child(div().flex_1().min_w_0().flex().items_center().child(meta))
            .child(
                icon_button_frame(div().id("custom-ui-close"), &theme, ButtonSize::Default)
                    .debug_selector(|| "custom-ui-close".to_string())
                    .tip(tr!("common.close"))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_hover))
                    .on_click(cx.listener(Self::on_close_click))
                    .child(icon(
                        "icons/x.svg",
                        ButtonSize::Default.icon_size().px(&theme),
                        theme.text_muted,
                    )),
            );

        // ── body: an inset code surface holding the server's grid ──
        let body = div()
            .id("custom-ui-body")
            .debug_selector(|| "custom-ui-body".to_string())
            .mx(px(10.))
            .my(px(10.))
            .px(px(10.))
            .py(DynamicSpacing::Base08.px(&theme))
            .rounded(Radius::XLarge.px(&theme))
            .bg(theme.code_bg)
            .border_1()
            .border_color(theme.border)
            .min_h(px(f32::from(line_height)))
            .max_h(px((viewport_h * 0.68).max(f32::from(line_height) * 3.)))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .font_family(theme::code_font_family())
            .text_size(font_size)
            .text_color(theme.code_text)
            .child(grid);

        // ── footer: name the keys once ──
        let footer = div()
            .flex_none()
            .h(px(34.))
            .debug_selector(|| "custom-ui-footer".to_string())
            .px(px(14.))
            .flex()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .text_size(TextSize::Small.px(&theme))
            .text_color(theme.text_muted)
            .child(SharedString::from("↑↓ Navigate · ⏎ Select · esc Back"));

        let card = div()
            .id("custom-ui-card")
            .debug_selector(|| "custom-ui-card".to_string())
            .w(card_w)
            .elevation_3(&theme)
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .font_family(theme::ui_font_family())
            // The component gets first crack at every key; only `CustomUiEscape`
            // is intercepted, so the global Escape → AbortRun never fires here.
            .key_context("CustomUi")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::on_escape))
            // Clicks inside must not reach the scrim's cancel.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(footer);

        let scrim = theme.scrim_modal();
        div()
            .id("custom-ui-layer")
            .debug_selector(|| "custom-ui-layer".to_string())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(scrim)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_scrim_down))
            .child(card)
    }
}

/// Deferred overlay layer, above the transcript like the extension dialog.
pub fn layer(ui: Entity<CustomUi>) -> impl IntoElement {
    deferred(ui).with_priority(9)
}

/// Measure one monospace cell so the card's pixel width matches the column
/// count the server rendered at.
fn measure_cell(window: &Window, font: &Font, size: Pixels) -> Pixels {
    let font_id = window.text_system().resolve_font(font);
    window
        .text_system()
        .advance(font_id, size, 'M')
        .map(|advance| advance.width)
        .unwrap_or(px(8.))
        .max(px(1.))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn open_frame_carries_lines_width_and_cursor() {
        let frame = CustomFrame::from_event(&json!({
            "type": "extension_ui_request",
            "id": "u1",
            "method": "custom",
            "phase": "open",
            "title": "Select a review preset",
            "lines": ["┌─", "│ Review uncommitted changes"],
            "width": 33,
            "height": 10,
            "cursor": { "row": 1, "col": 2 },
            "keyboard": "kitty",
            "mouse": true,
            "overlay": false,
        }))
        .unwrap();
        let CustomFrame::Open(surface) = frame else {
            panic!("expected open");
        };
        assert_eq!(surface.id, "u1");
        assert_eq!(surface.title.as_deref(), Some("Select a review preset"));
        assert_eq!(surface.lines.len(), 2);
        assert_eq!(surface.width, Some(33));
        assert_eq!(surface.height, Some(10));
        assert_eq!(surface.cursor, Some(CustomCursor { row: 1, col: 2 }));
        assert_eq!(surface.keyboard, KeyboardMode::Kitty);
        assert!(surface.mouse);
        assert!(!surface.overlay);
    }

    #[test]
    fn render_frame_defaults_to_legacy_and_no_cursor() {
        let frame = CustomFrame::from_event(&json!({
            "id": "u2",
            "method": "custom",
            "phase": "render",
            "lines": ["updated"],
        }))
        .unwrap();
        let CustomFrame::Render(surface) = frame else {
            panic!("expected render");
        };
        assert_eq!(surface.keyboard, KeyboardMode::Legacy);
        assert_eq!(surface.cursor, None);
        assert!(!surface.mouse);
        assert_eq!(surface.width, None);
    }

    #[test]
    fn a_phase_less_frame_is_treated_as_render() {
        let frame = CustomFrame::from_event(&json!({
            "id": "u3",
            "method": "custom",
            "lines": ["x"],
        }))
        .unwrap();
        assert!(matches!(frame, CustomFrame::Render(_)));
    }

    #[test]
    fn close_frame_carries_the_result() {
        let frame = CustomFrame::from_event(&json!({
            "id": "u4",
            "method": "custom",
            "phase": "close",
            "result": "uncommitted",
        }))
        .unwrap();
        assert_eq!(
            frame,
            CustomFrame::Close {
                id: "u4".into(),
                result: json!("uncommitted"),
            }
        );
    }

    #[test]
    fn close_without_result_is_null() {
        let frame =
            CustomFrame::from_event(&json!({"id": "u5", "method": "custom", "phase": "close"}))
                .unwrap();
        assert_eq!(
            frame,
            CustomFrame::Close {
                id: "u5".into(),
                result: Value::Null,
            }
        );
    }

    #[test]
    fn other_methods_and_unknown_phases_are_rejected() {
        assert!(CustomFrame::from_event(&json!({"id": "x", "method": "select"})).is_none());
        assert!(
            CustomFrame::from_event(&json!({"id": "x", "method": "custom", "phase": "zzz"}))
                .is_none()
        );
    }

    fn stroke(key: &str, modifiers: gpui::Modifiers) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: None,
        }
    }

    /// The legacy byte subset the proposal pins (§7) — locked here so a change
    /// to `encode_key` cannot silently break a component's `handleInput`.
    #[test]
    fn legacy_input_contract_matches_the_proposal() {
        let plain = gpui::Modifiers::default();
        let ctrl = gpui::Modifiers {
            control: true,
            ..Default::default()
        };
        let encode = |key: &str, modifiers| {
            encode_key(&stroke(key, modifiers), TermMode::empty())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        assert_eq!(encode("enter", plain).as_deref(), Some("\r"));
        assert_eq!(encode("escape", plain).as_deref(), Some("\x1b"));
        assert_eq!(encode("up", plain).as_deref(), Some("\x1b[A"));
        assert_eq!(encode("down", plain).as_deref(), Some("\x1b[B"));
        assert_eq!(encode("tab", plain).as_deref(), Some("\t"));
        assert_eq!(encode("backspace", plain).as_deref(), Some("\x7f"));
        assert_eq!(encode("delete", plain).as_deref(), Some("\x1b[3~"));
        assert_eq!(encode("a", plain).as_deref(), Some("a"));
        assert_eq!(encode("c", ctrl).as_deref(), Some("\x03"));
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::theme::ThemeId;
    use gpui::{point, size, Modifiers, TestAppContext};

    struct Harness {
        ui: Entity<CustomUi>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.ui.clone())
        }
    }

    fn surface() -> CustomSurface {
        CustomSurface {
            id: "surface-1".into(),
            title: Some("Select a review preset".into()),
            lines: (0..6).map(|row| format!("row {row}")).collect(),
            width: Some(40),
            height: None,
            cursor: Some(CustomCursor { row: 2, col: 3 }),
            keyboard: KeyboardMode::Legacy,
            mouse: false,
            overlay: false,
        }
    }

    /// The frame paints the chrome it promises: a card with a header, the
    /// server's grid body, a close control, and the key-hint footer.
    #[gpui::test]
    fn custom_frame_lays_out_chrome_and_grid(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let ui = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(ThemeId::Orbit));
            cx.new(|cx| {
                CustomUi::new(
                    surface(),
                    Box::new(|_: String, _: &mut Window, _: &mut App| {}),
                    Box::new(|_: &mut Window, _: &mut App| {}),
                    cx,
                )
            })
        });
        let harness = cx.update(|_, cx| cx.new(|_| Harness { ui }));

        let _ = cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            harness.clone()
        });

        let card = cx.debug_bounds("custom-ui-card").expect("card painted");
        let header = cx.debug_bounds("custom-ui-header").expect("header painted");
        let close = cx.debug_bounds("custom-ui-close").expect("close painted");
        let body = cx
            .debug_bounds("custom-ui-body")
            .expect("grid body painted");
        let footer = cx.debug_bounds("custom-ui-footer").expect("footer painted");

        assert!(card.size.width > px(280.), "card is a usable width");
        assert!(
            card.size.height > px(140.),
            "card has header + body + footer"
        );

        // Centered in the 900×700 window.
        let center = f32::from(card.origin.x) + f32::from(card.size.width) / 2.;
        assert!(
            (center - 450.).abs() < 1.,
            "card is horizontally centered: {center}"
        );

        // Chrome hugs the card's top and bottom; the grid sits inset between.
        assert!(
            header.origin.y <= card.origin.y + px(1.),
            "header is at the top"
        );
        assert!(
            footer.origin.y + footer.size.height >= card.origin.y + card.size.height - px(1.),
            "footer is at the bottom"
        );
        assert!(
            body.origin.y >= header.origin.y + header.size.height - px(1.),
            "body starts below the header"
        );
        assert!(
            body.size.width < card.size.width,
            "body is inset from the card edges"
        );
        assert!(close.origin.y < body.origin.y, "close lives in the header");
    }

    /// A blank line (or a line of only SGR codes) in the component's output
    /// must not abort the render. Regression: `styled_line` returned a fallback
    /// space with no runs, and `StyledText::with_runs` panicked with "invalid
    /// text run" because the runs did not cover the text.
    #[gpui::test]
    fn blank_surface_lines_render(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let ui = cx.update(|_, cx| {
            cx.set_global(theme::Theme::for_id(ThemeId::Orbit));
            let mut surface = surface();
            surface.lines = vec!["header".into(), "".into(), "\x1b[0m".into()];
            cx.new(|cx| {
                CustomUi::new(
                    surface,
                    Box::new(|_: String, _: &mut Window, _: &mut App| {}),
                    Box::new(|_: &mut Window, _: &mut App| {}),
                    cx,
                )
            })
        });
        let harness = cx.update(|_, cx| cx.new(|_| Harness { ui }));

        let _ = cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
            harness.clone()
        });

        assert!(
            cx.debug_bounds("custom-ui-card").is_some(),
            "the card still paints"
        );
    }

    /// The surface stack the app owns. Removal goes through the cached RPC id,
    /// exactly like `OrbitApp::close_custom_ui`.
    struct Host {
        surfaces: CustomUiSurfaces,
        cancels: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().children(self.surfaces.iter().cloned())
        }
    }

    /// Clicking the scrim dismisses the surface from inside that surface's own
    /// mouse-down handler. Regression: the dismiss path used to read the
    /// surface entity back out to match its id, but gpui leases the entity for
    /// the duration of the handler, so the read double-leased it — a panic that
    /// aborted inside AppKit's `mouseDown:` (`panic_cannot_unwind`).
    #[gpui::test]
    fn clicking_the_scrim_dismisses_without_a_double_lease(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(theme::Theme::for_id(ThemeId::Orbit)));

        let cancels = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let host = cx.update(|_, cx| {
            cx.new(|_| Host {
                surfaces: CustomUiSurfaces::default(),
                cancels: cancels.clone(),
            })
        });

        let weak = host.downgrade();
        let ui = cx.update(|_, cx| {
            let on_cancel: CustomCancel = Box::new(move |_window, cx| {
                let _ = weak.update(cx, |host, cx| {
                    host.surfaces.remove("surface-1");
                    host.cancels.set(host.cancels.get() + 1);
                    cx.notify();
                });
            });
            cx.new(|cx| CustomUi::new(surface(), Box::new(|_, _, _| {}), on_cancel, cx))
        });
        cx.update(|_, cx| {
            host.update(cx, |host, cx| {
                host.surfaces.push("surface-1".into(), ui.clone());
                cx.notify();
            })
        });

        let draw = |cx: &mut gpui::VisualTestContext| {
            let _ = cx.draw(point(px(0.), px(0.)), size(px(900.), px(700.)), |_, _| {
                host.clone()
            });
        };
        draw(cx);
        let card = cx.debug_bounds("custom-ui-card").expect("card painted");

        // A point on the scrim, well clear of the centered card.
        cx.simulate_click(point(px(8.), card.origin.y), Modifiers::none());
        draw(cx);

        assert_eq!(cancels.get(), 1, "the scrim click cancels the surface");
        assert!(
            !cx.update(|_, cx| host.read(cx).surfaces.contains("surface-1")),
            "the surface is dismissed"
        );
    }
}
