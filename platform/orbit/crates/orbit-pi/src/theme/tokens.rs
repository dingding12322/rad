//! Zed's design tokens, ported onto Orbit's [`Theme`].
//!
//! A one-to-one port of the sizing vocabulary in Zed's `ui` crate: the
//! `styles/{units,spacing,typography,elevation,animation}.rs` modules plus
//! the metrics its `Icon`, `Button`, `ListItem`, `List`, `Popover`,
//! `ContextMenu`, `Modal`, `Tooltip`, `InputField`, and `Scrollbar`
//! components are built from (zed-industries/zed `main`, read 2026-09-25).
//! Names and numbers follow Zed's source so any value can be checked against
//! it line for line. Colors stay Orbit's semantic palette: Zed's token set is
//! sizes, spacing, elevation, and motion only.
//!
//! # Scaling
//!
//! Zed authors sizes in rems, where one rem is the user's UI font size
//! (16px by default). Orbit authors its chrome against a 14px UI font
//! ([`DEFAULT_UI_FONT_SIZE`](super::DEFAULT_UI_FONT_SIZE)), so a token
//! resolves as `zed_px × ui_font_size / DEFAULT_UI_FONT_SIZE`, the same rule
//! as [`Theme::ui_px`]. At Orbit's default settings every token is exactly
//! Zed's default pixel value, and it grows and shrinks with the UI font size
//! the way a Zed rem does. `theme.ui_px(v)` is Zed's `rems_from_px(v)`;
//! [`Theme::rems`] is Zed's `rems(v)`. Values Zed pins in `px(…)` (hairlines,
//! the context menu's 200px minimum width, scrollbar metrics) stay fixed.
//!
//! # Density
//!
//! Zed has three UI densities. Orbit's Spacing Density setting is a
//! percentage, so the three are anchored at 80 % (Compact), 100 % (Default),
//! and 120 % (Comfortable): at those settings [`DynamicSpacing`] returns
//! Zed's values exactly, and between anchors it interpolates linearly.
//!
//! Resolve a token through the theme — `DynamicSpacing::Base08.px(&theme)`,
//! `TextSize::Small.px(&theme)`, `div().elevation_2(&theme)`.

// This is the whole Zed vocabulary; surfaces adopt it one at a time, so a
// token no surface reads yet is expected rather than dead.
#![allow(dead_code)]

use std::time::Duration;

use gpui::{
    hsla, linear_color_stop, linear_gradient, point, px, relative, Background, BoxShadow,
    DefiniteLength, Hsla, Pixels, Styled,
};

use super::{Theme, ThemeMode};

// ── Units (`ui/src/styles/units.rs`) ────────────────────────────────────

/// Zed's rem, in px, at the default UI font size.
pub const BASE_REM_PX: f32 = 16.;

impl Theme {
    /// Zed's `rems(value)`: `value` rems of [`BASE_REM_PX`], tracking the UI
    /// font size setting like every other token.
    pub fn rems(&self, value: f32) -> Pixels {
        self.ui_px(value * BASE_REM_PX)
    }
}

// ── Corner radius + borders (`gpui_macros/src/styles.rs`) ───────────────

/// GPUI's `rounded_*` scale. Every step but `None` and `Full` is in rems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Radius {
    /// 0px.
    None,
    /// 2px (0.125rem).
    XSmall,
    /// 4px (0.25rem) — buttons, list items, menu entries.
    Small,
    /// 6px (0.375rem) — input fields.
    Medium,
    /// 8px (0.5rem) — every elevated surface.
    Large,
    /// 12px (0.75rem).
    XLarge,
    /// 16px (1rem).
    XXLarge,
    /// 24px (1.5rem).
    XXXLarge,
    /// 9999px — pills and discs.
    Full,
}

impl Radius {
    pub fn px(self, theme: &Theme) -> Pixels {
        match self {
            Self::None => px(0.),
            Self::XSmall => theme.rems(0.125),
            Self::Small => theme.rems(0.25),
            Self::Medium => theme.rems(0.375),
            Self::Large => theme.rems(0.5),
            Self::XLarge => theme.rems(0.75),
            Self::XXLarge => theme.rems(1.),
            Self::XXXLarge => theme.rems(1.5),
            Self::Full => px(9999.),
        }
    }
}

/// GPUI's `border_1()`: a 1px hairline that never scales.
pub const BORDER_WIDTH: Pixels = px(1.);

// ── Density + dynamic spacing (`ui/src/styles/spacing.rs`) ──────────────

/// Zed's UI density.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiDensity {
    Compact,
    #[default]
    Default,
    Comfortable,
}

impl UiDensity {
    /// The Spacing Density percentage this density is anchored to.
    pub const fn percent(self) -> u32 {
        match self {
            Self::Compact => 80,
            Self::Default => 100,
            Self::Comfortable => 120,
        }
    }

    /// The density nearest a Spacing Density percentage.
    pub fn nearest(percent: u32) -> Self {
        if percent < 90 {
            Self::Compact
        } else if percent > 110 {
            Self::Comfortable
        } else {
            Self::Default
        }
    }

    /// The active density — Zed's `ui_density(cx)`. For UI that has to branch
    /// on density; never compute a spacing from it, use [`DynamicSpacing`].
    pub fn of(theme: &Theme) -> Self {
        Self::nearest(theme.ui.spacing_density)
    }
}

/// Zed's dynamic spacing scale. The number after `Base` is the pixel value
/// at the default UI font size and density.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DynamicSpacing {
    Base00,
    Base01,
    Base02,
    Base03,
    Base04,
    Base06,
    Base08,
    Base12,
    Base16,
    Base20,
    Base24,
    Base32,
    Base40,
    Base48,
}

impl DynamicSpacing {
    pub const ALL: [Self; 14] = [
        Self::Base00,
        Self::Base01,
        Self::Base02,
        Self::Base03,
        Self::Base04,
        Self::Base06,
        Self::Base08,
        Self::Base12,
        Self::Base16,
        Self::Base20,
        Self::Base24,
        Self::Base32,
        Self::Base40,
        Self::Base48,
    ];

    /// `(compact, default, comfortable)` px @16px/rem — the table Zed feeds
    /// `derive_dynamic_spacing!`. Base24 and up use its `(n - 4, n, n + 4)`
    /// formula.
    pub const fn values(self) -> (f32, f32, f32) {
        match self {
            Self::Base00 => (0., 0., 0.),
            Self::Base01 => (1., 1., 2.),
            Self::Base02 => (1., 2., 4.),
            Self::Base03 => (2., 3., 4.),
            Self::Base04 => (2., 4., 6.),
            Self::Base06 => (3., 6., 8.),
            Self::Base08 => (4., 8., 10.),
            Self::Base12 => (10., 12., 14.),
            Self::Base16 => (14., 16., 18.),
            Self::Base20 => (18., 20., 22.),
            Self::Base24 => (20., 24., 28.),
            Self::Base32 => (28., 32., 36.),
            Self::Base40 => (36., 40., 44.),
            Self::Base48 => (44., 48., 52.),
        }
    }

    /// Unscaled px at one of Zed's densities.
    pub const fn at(self, density: UiDensity) -> f32 {
        let (compact, default, comfortable) = self.values();
        match density {
            UiDensity::Compact => compact,
            UiDensity::Default => default,
            UiDensity::Comfortable => comfortable,
        }
    }

    /// Unscaled px at a Spacing Density percentage: Zed's value at each
    /// anchor, linear between anchors, and held past the outer two.
    pub fn at_percent(self, percent: u32) -> f32 {
        let (compact, default, comfortable) = self.values();
        let t = ((percent as f32 - 100.) / 20.).clamp(-1., 1.);
        if t < 0. {
            default + (default - compact) * t
        } else {
            default + (comfortable - default) * t
        }
    }

    /// The resolved spacing: density first, then the UI font scale — Zed's
    /// `DynamicSpacing::px(cx)`.
    pub fn px(self, theme: &Theme) -> Pixels {
        theme.ui_px(self.at_percent(theme.ui.spacing_density))
    }
}

// ── Typography (`ui/src/styles/typography.rs`, `theme/src/buffer_line_height.rs`)

/// Semantic UI text sizes — Zed's `TextSize` (its `LabelSize` shares the
/// scale).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum TextSize {
    /// 14px — regular UI text.
    #[default]
    Default,
    /// 16px.
    Large,
    /// 12px.
    Small,
    /// 10px.
    XSmall,
    /// The UI font size setting itself (Orbit's default is 14px; Zed's, 16px).
    Ui,
    /// The Editor font size setting.
    Editor,
}

impl TextSize {
    pub fn px(self, theme: &Theme) -> Pixels {
        match self {
            Self::Large => theme.ui_px(16.),
            Self::Default => theme.ui_px(14.),
            Self::Small => theme.ui_px(12.),
            Self::XSmall => theme.ui_px(10.),
            Self::Ui => px(theme.ui.ui_font_size),
            Self::Editor => px(theme.ui.editor_font_size),
        }
    }
}

/// Headline sizes on a Major Second scale — Zed's `HeadlineSize`.
/// Reserved by the modal metrics ([`modal::HEADLINE`]); no surface paints a
/// page headline through it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadlineSize {
    /// ~14px.
    XSmall,
    /// 16px.
    Small,
    /// ~18px.
    #[default]
    Medium,
    /// ~20px.
    Large,
    /// ~23px.
    XLarge,
}

impl HeadlineSize {
    pub const fn rems(self) -> f32 {
        match self {
            Self::XSmall => 0.88,
            Self::Small => 1.0,
            Self::Medium => 1.125,
            Self::Large => 1.27,
            Self::XLarge => 1.43,
        }
    }

    pub fn px(self, theme: &Theme) -> Pixels {
        theme.rems(self.rems())
    }

    /// Zed gives every headline `rems(1.6)` — an absolute 25.6px at the
    /// default size, not a 1.6× multiplier.
    pub fn line_height(self, theme: &Theme) -> Pixels {
        theme.rems(1.6)
    }
}

/// Line-height multipliers — Zed's `BufferLineHeight`. Context menus force
/// `Comfortable` whatever the surrounding text style says.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum BufferLineHeight {
    /// 1.618× — the golden ratio, GPUI's own default.
    #[default]
    Comfortable,
    /// 1.3×.
    Standard,
    Custom(f32),
}

impl BufferLineHeight {
    pub const fn value(self) -> f32 {
        match self {
            Self::Comfortable => 1.618,
            Self::Standard => 1.3,
            Self::Custom(value) => value,
        }
    }

    /// The multiplier as a relative length, for `.line_height(…)`.
    pub fn relative(self) -> DefiniteLength {
        relative(self.value())
    }

    /// The laid-out line height at `font_size`. GPUI rounds line heights to
    /// whole pixels, so this is the height a line of text actually occupies.
    pub fn resolve(self, font_size: Pixels) -> Pixels {
        (font_size * self.value()).round()
    }
}

// ── Icons (`ui/src/components/icon.rs`) ─────────────────────────────────

/// Zed's `IconSize`. There is no `Large`: the scale jumps from Medium (16px)
/// to XLarge (48px).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum IconSize {
    /// 10px.
    Indicator,
    /// 12px.
    XSmall,
    /// 14px — context menu entries.
    Small,
    /// 16px.
    #[default]
    Medium,
    /// 48px.
    XLarge,
    /// A size in rems. Zed pads a custom size by the size itself (a TODO in
    /// its source); here a custom size has no padding.
    Custom(f32),
}

impl IconSize {
    pub fn px(self, theme: &Theme) -> Pixels {
        match self {
            Self::Indicator => theme.ui_px(10.),
            Self::XSmall => theme.ui_px(12.),
            Self::Small => theme.ui_px(14.),
            Self::Medium => theme.ui_px(16.),
            Self::XLarge => theme.ui_px(48.),
            Self::Custom(rems) => theme.rems(rems),
        }
    }

    /// Padding on each side of the icon inside its square hit target.
    pub fn padding(self, theme: &Theme) -> Pixels {
        match self {
            Self::Indicator | Self::Custom(_) => DynamicSpacing::Base00.px(theme),
            _ => DynamicSpacing::Base02.px(theme),
        }
    }

    /// The side of the square hit target (icon + padding) — an `IconButton`'s
    /// size when its shape is square.
    pub fn square(self, theme: &Theme) -> Pixels {
        self.px(theme) + self.padding(theme) * 2.
    }
}

// ── Buttons (`ui/src/components/button/button_like.rs`) ────────────────

/// Button heights — Zed's `ButtonSize`. Also sizes non-button rows that must
/// line up with buttons (`Default` matches a dense list row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 32px.
    Large,
    /// 28px.
    Medium,
    /// 22px.
    #[default]
    Default,
    /// 18px.
    Compact,
    /// 16px.
    None,
}

impl ButtonSize {
    pub fn height(self, theme: &Theme) -> Pixels {
        match self {
            Self::Large => theme.ui_px(32.),
            Self::Medium => theme.ui_px(28.),
            Self::Default => theme.ui_px(22.),
            Self::Compact => theme.ui_px(18.),
            Self::None => theme.ui_px(16.),
        }
    }

    pub fn padding_x(self, theme: &Theme) -> Pixels {
        match self {
            Self::Large | Self::Medium => DynamicSpacing::Base08.px(theme),
            Self::Default | Self::Compact => DynamicSpacing::Base04.px(theme),
            // Zed's `px_px()`: one unscaled pixel.
            Self::None => px(1.),
        }
    }

    /// The [`IconSize`] an icon inside a button frame draws at — the one
    /// mapping every icon in a `button_frame` / `icon_button_frame` resolves
    /// through, so icon buttons, labeled buttons, and chips share one scale
    /// instead of each surface picking its own glyph size.
    ///
    /// The icon grows with the frame: `None` 16px → Indicator 10px, `Compact`
    /// 18px → XSmall 12px, `Default` 22px and `Medium` 28px → Small 14px, and
    /// `Large` 32px → Medium 16px. A chevron or other disclosure glyph inside
    /// a button is not its icon: those take `IconSize::XSmall` (see the fold
    /// rows), and dense tree rows keep `IconSize::Indicator`.
    pub fn icon_size(self) -> IconSize {
        match self {
            Self::None => IconSize::Indicator,
            Self::Compact => IconSize::XSmall,
            Self::Default | Self::Medium => IconSize::Small,
            Self::Large => IconSize::Medium,
        }
    }
}

/// Metrics every [`ButtonSize`] shares.
pub mod button {
    use super::*;

    /// Between a button's icon and its label.
    pub fn gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    /// Between a button's label and its keybinding.
    pub fn keybinding_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base06.px(theme)
    }

    /// Per corner for a full-size button. Compact frames take
    /// [`Radius::Medium`] instead — see [`radius`].
    pub const RADIUS: Radius = Radius::Large;

    /// Per corner by frame size: compact controls read tighter than the
    /// normal button scale, whose corners match every other control's.
    pub fn radius(size: ButtonSize) -> Radius {
        match size {
            ButtonSize::Compact | ButtonSize::None => Radius::Medium,
            _ => Radius::Large,
        }
    }

    /// A button label's size. Zed's `Button` defaults to a Default label and
    /// its compact buttons pass `LabelSize::Small`; Orbit encodes that
    /// convention by size so a label always fits its button.
    pub fn label_size(size: ButtonSize) -> TextSize {
        match size {
            ButtonSize::Large | ButtonSize::Medium => TextSize::Default,
            ButtonSize::Default | ButtonSize::Compact | ButtonSize::None => TextSize::Small,
        }
    }
}

// ── Lists (`ui/src/components/list/{list,list_item,list_separator}.rs`) ──

/// Vertical padding a list row adds around its content — Zed's
/// `ListItemSpacing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListItemSpacing {
    /// No extra padding.
    #[default]
    Dense,
    /// −1px each side (Zed's `py_neg_px`, unscaled).
    ExtraDense,
    /// 4px each side (`py_1`).
    Sparse,
}

impl ListItemSpacing {
    pub fn padding_y(self, theme: &Theme) -> Pixels {
        match self {
            Self::Dense => px(0.),
            Self::ExtraDense => px(-1.),
            Self::Sparse => theme.rems(0.25),
        }
    }

    /// A row's height: Zed's list item has no fixed height; it is one line
    /// of its label plus this spacing above and below.
    pub fn row_height(self, line_height: Pixels, theme: &Theme) -> Pixels {
        line_height + self.padding_y(theme) * 2.
    }
}

/// `ListItem` metrics.
pub mod list_item {
    use super::*;

    /// Inner horizontal padding.
    pub fn padding_x(theme: &Theme) -> Pixels {
        DynamicSpacing::Base06.px(theme)
    }

    /// Outer horizontal inset of an `.inset(true)` item (every menu entry).
    pub fn inset(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    /// Indent per nesting level; fixed px in Zed.
    pub const INDENT_STEP: Pixels = px(12.);

    /// Between the disclosure/start slot and the content (`gap_1`).
    pub fn slot_gap(theme: &Theme) -> Pixels {
        theme.rems(0.25)
    }

    /// Inside the content slot.
    pub fn content_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base06.px(theme)
    }

    /// `.rounded()` / `.outlined()` / inset items — the same corner as a
    /// sidebar destination.
    pub const RADIUS: Radius = Radius::Medium;
}

/// `List` and `ListSeparator` metrics.
pub mod list {
    use super::*;

    /// Above the first and below the last row.
    pub fn padding_y(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    /// Above and below a 1px separator.
    pub fn separator_margin_y(theme: &Theme) -> Pixels {
        DynamicSpacing::Base06.px(theme)
    }

    /// A `ListSubHeader`'s label row (`h_5`).
    pub fn sub_header_height(theme: &Theme) -> Pixels {
        theme.rems(1.25)
    }

    /// A `ListSubHeader`'s outer horizontal padding.
    pub fn sub_header_padding_x(theme: &Theme) -> Pixels {
        DynamicSpacing::Base02.px(theme)
    }

    /// An inset `ListSubHeader`'s inner horizontal padding (`px_2`).
    pub fn sub_header_inset_x(theme: &Theme) -> Pixels {
        theme.rems(0.5)
    }

    /// Below a `ListSubHeader`.
    pub fn sub_header_padding_bottom(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    /// A `ListSubHeader`'s label: Small and muted.
    pub const SUB_HEADER_TEXT: TextSize = TextSize::Small;
}

// ── Pickers (`picker/src/{picker,render}.rs`) ───────────────────────────────

/// `Picker` metrics: a search row over a list of inset, `Sparse` list items.
pub mod picker {
    use super::*;

    /// Zed pickers draw the modal elevation even as popovers.
    pub const ELEVATION: ElevationIndex = ElevationIndex::ModalSurface;

    /// `DEFAULT_MODAL_WIDTH`: 34rem.
    pub fn default_width(theme: &Theme) -> Pixels {
        theme.rems(34.)
    }

    /// `DEFAULT_MODAL_MAX_HEIGHT`: 24rem.
    pub fn default_max_height(theme: &Theme) -> Pixels {
        theme.rems(24.)
    }

    /// The search row (`h_9`), with a hairline between it and the list.
    pub fn search_height(theme: &Theme) -> Pixels {
        theme.rems(2.25)
    }

    /// The search row's horizontal padding (`px_2p5`).
    pub fn search_padding_x(theme: &Theme) -> Pixels {
        theme.rems(0.625)
    }

    /// Above the first and below the last row.
    pub fn list_padding_y(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    /// Row spacing: pickers use `Sparse` list items.
    pub const ROW_SPACING: ListItemSpacing = ListItemSpacing::Sparse;

    /// A row's label size.
    pub const TEXT: TextSize = TextSize::Default;

    /// A row's secondary line (description, path, metadata).
    pub const SECONDARY_TEXT: TextSize = TextSize::Small;

    /// A one-line row: one Comfortable line of Default text plus `Sparse`
    /// padding — 31px at the default size.
    pub fn entry_height(theme: &Theme) -> Pixels {
        ROW_SPACING.row_height(BufferLineHeight::Comfortable.resolve(TEXT.px(theme)), theme)
    }

    /// A two-line row: a Default label over a Small secondary line.
    pub fn two_line_entry_height(theme: &Theme) -> Pixels {
        let line = |size: TextSize| BufferLineHeight::Comfortable.resolve(size.px(theme));
        ROW_SPACING.row_height(line(TEXT) + line(SECONDARY_TEXT), theme)
    }
}

// ── Popovers + menus (`popover.rs`, `popover_menu.rs`, `context_menu.rs`) ─

/// `Popover` and `PopoverMenu` metrics.
pub mod popover {
    use super::*;

    /// `POPOVER_Y_PADDING`: vertical padding added around a popover's
    /// contents, split between top and bottom. Fixed px.
    pub const Y_PADDING: Pixels = px(8.);

    /// Padding on each of the top and bottom edges.
    pub fn padding_y() -> Pixels {
        Y_PADDING * 0.5
    }

    /// Horizontal padding of a popover's aside panel (`px_1`).
    pub fn aside_padding_x(theme: &Theme) -> Pixels {
        theme.rems(0.25)
    }

    /// Between a popover and its aside (`gap_1`).
    pub fn aside_gap(theme: &Theme) -> Pixels {
        theme.rems(0.25)
    }

    pub const ELEVATION: ElevationIndex = ElevationIndex::ElevatedSurface;

    /// `PopoverMenu`'s default distance from its trigger: 4px of padding plus
    /// 1px of border compensation.
    pub const MENU_OFFSET: Pixels = px(5.);

    /// Margin kept from the window edges when a menu snaps into view
    /// (`snap_to_window_with_margin`).
    pub const WINDOW_MARGIN: Pixels = px(8.);
}

/// `ContextMenu` metrics.
pub mod context_menu {
    use super::*;

    /// Minimum width when the menu has no fixed width. Fixed px in Zed.
    pub const MIN_WIDTH: Pixels = px(200.);

    /// Tallest the menu grows, as a fraction of the viewport height.
    pub const MAX_HEIGHT_FRACTION: f32 = 0.75;

    /// Entry label size (Zed's default `Label`).
    pub const TEXT: TextSize = TextSize::Default;

    /// Forced on the whole menu, whatever the text style it is deferred from.
    pub const LINE_HEIGHT: BufferLineHeight = BufferLineHeight::Comfortable;

    /// Default entry icon.
    pub const ICON: IconSize = IconSize::Small;

    /// A link entry's trailing arrow.
    pub const LINK_ICON: IconSize = IconSize::XSmall;

    /// Between an entry's icon and its label (`gap_1p5`).
    pub fn icon_gap(theme: &Theme) -> Pixels {
        theme.rems(0.375)
    }

    /// Before an entry's keybinding (`ml_4`).
    pub fn keybinding_gap(theme: &Theme) -> Pixels {
        theme.rems(1.)
    }

    /// One entry: a Comfortable line of Default text in a dense list item —
    /// 23px at the default size.
    pub fn entry_height(theme: &Theme) -> Pixels {
        ListItemSpacing::Dense.row_height(LINE_HEIGHT.resolve(TEXT.px(theme)), theme)
    }

    /// Width of the documentation aside: `max_w_96` in a wide window,
    /// `max_w_48` otherwise.
    pub fn aside_width(theme: &Theme, wide_window: bool) -> Pixels {
        if wide_window {
            theme.rems(24.)
        } else {
            theme.rems(12.)
        }
    }

    /// Window width from which the aside sits beside the menu, not above it.
    pub fn wide_window_min_width(theme: &Theme) -> Pixels {
        theme.ui_px(800.)
    }
}

// ── Toasts + notifications ─────────────────────────────────────────────

/// Metrics shared by transient notifications and actionable update notices.
pub mod toast {
    use super::*;

    /// One width keeps every toast in a stack aligned to the same edge.
    pub const WIDTH: Pixels = px(360.);
    pub const BADGE_RADIUS: Radius = Radius::Small;
    pub const ENTER: Duration = AnimationDuration::Fast.duration();
    pub const TITLE: TextSize = TextSize::Default;
    pub const BODY: TextSize = TextSize::Small;
    pub const ICON: IconSize = IconSize::Small;
    pub const CLOSE_BUTTON: ButtonSize = ButtonSize::Default;
    pub const ACTION_BUTTON: ButtonSize = ButtonSize::Medium;
    pub const ICON_RADIUS: Radius = Radius::Large;

    pub fn surface<E: Styled>(element: E, theme: &Theme) -> E {
        element.elevation_2(theme)
    }

    pub fn stack_inset(theme: &Theme) -> Pixels {
        DynamicSpacing::Base16.px(theme)
    }

    pub fn stack_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    pub fn padding_x(theme: &Theme) -> Pixels {
        DynamicSpacing::Base12.px(theme)
    }

    pub fn padding_y(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    pub fn content_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    pub fn icon_tile_size(theme: &Theme) -> Pixels {
        ACTION_BUTTON.height(theme)
    }

    pub fn title_body_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base02.px(theme)
    }

    pub fn header_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    pub fn package_row_padding_y(theme: &Theme) -> Pixels {
        DynamicSpacing::Base06.px(theme)
    }

    pub fn package_row_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    pub fn version_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    pub fn version_text_size(theme: &Theme) -> Pixels {
        theme.code_px(11.)
    }

    pub fn badge_height(theme: &Theme) -> Pixels {
        ButtonSize::Default.height(theme)
    }

    pub fn badge_min_width(theme: &Theme) -> Pixels {
        badge_height(theme)
    }

    pub fn badge_padding_x(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    pub fn separator_size() -> Pixels {
        BORDER_WIDTH
    }

    pub fn action_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base06.px(theme)
    }
}

// ── Modals (`ui/src/components/modal.rs`) ───────────────────────────────

/// `Modal`, `ModalHeader`, `Section`, and `ModalFooter` metrics.
pub mod modal {
    use super::*;

    pub fn header_padding_x(theme: &Theme) -> Pixels {
        DynamicSpacing::Base12.px(theme)
    }

    pub fn header_padding_top(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    pub fn header_padding_bottom(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    /// Between the header's back button, headline column, and dismiss button.
    pub fn header_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    /// The header's headline (drawn muted in Zed).
    pub const HEADLINE: HeadlineSize = HeadlineSize::Small;

    /// Between sections.
    pub fn body_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    /// A section's horizontal padding: Base12 when contained, otherwise
    /// Base06 + Base06 (the same 12px at Default density).
    pub fn section_padding_x(theme: &Theme, contained: bool) -> Pixels {
        if contained {
            DynamicSpacing::Base12.px(theme)
        } else {
            DynamicSpacing::Base06.px(theme) * 2.
        }
    }

    /// Below a section's content (`pb_2`).
    pub fn section_padding_bottom(theme: &Theme) -> Pixels {
        theme.rems(0.5)
    }

    /// Between a section's rows.
    pub fn section_gap(theme: &Theme) -> Pixels {
        DynamicSpacing::Base04.px(theme)
    }

    /// A `SectionHeader` row (`h_7`).
    pub fn section_header_height(theme: &Theme) -> Pixels {
        theme.rems(1.75)
    }

    pub fn section_header_padding_x(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    /// All four sides of the footer, which carries a 1px top border.
    pub fn footer_padding(theme: &Theme) -> Pixels {
        DynamicSpacing::Base08.px(theme)
    }

    /// Between the footer's start and end slots (`gap_1`).
    pub fn footer_gap(theme: &Theme) -> Pixels {
        theme.rems(0.25)
    }

    pub const ELEVATION: ElevationIndex = ElevationIndex::ModalSurface;
}

// ── Tooltips (`ui/src/components/tooltip.rs`) ───────────────────────────

/// `Tooltip` metrics.
pub mod tooltip {
    use super::*;

    /// Left offset that keeps the tooltip off the cursor (`pl_2`).
    pub fn offset_x(theme: &Theme) -> Pixels {
        theme.rems(0.5)
    }

    /// Top offset that keeps the tooltip off the cursor (`pt_2p5`).
    pub fn offset_y(theme: &Theme) -> Pixels {
        theme.rems(0.625)
    }

    /// `px_2`.
    pub fn padding_x(theme: &Theme) -> Pixels {
        theme.rems(0.5)
    }

    /// `py_1`.
    pub fn padding_y(theme: &Theme) -> Pixels {
        theme.rems(0.25)
    }

    /// The title's wrap width (`max_w_72`).
    pub fn max_width(theme: &Theme) -> Pixels {
        theme.rems(18.)
    }

    /// Between the title and its keybinding (`gap_4`).
    pub fn keybinding_gap(theme: &Theme) -> Pixels {
        theme.rems(1.)
    }

    pub const TEXT: TextSize = TextSize::Default;

    /// The optional muted line under the title.
    pub const META_TEXT: TextSize = TextSize::Small;

    pub const ELEVATION: ElevationIndex = ElevationIndex::ElevatedSurface;
}

// ── Text inputs (`ui_input/src/input_field.rs`) ─────────────────────────

/// `InputField` metrics.
pub mod input {
    use super::*;

    /// Fixed px in Zed.
    pub const MIN_WIDTH: Pixels = px(192.);

    /// `min_h_8`.
    pub fn min_height(theme: &Theme) -> Pixels {
        theme.rems(2.)
    }

    /// `px_2`.
    pub fn padding_x(theme: &Theme) -> Pixels {
        theme.rems(0.5)
    }

    /// `py_1p5`.
    pub fn padding_y(theme: &Theme) -> Pixels {
        theme.rems(0.375)
    }

    /// Between the label and the field, and between a start icon and the
    /// text (`gap_1`).
    pub fn gap(theme: &Theme) -> Pixels {
        theme.rems(0.25)
    }

    pub const RADIUS: Radius = Radius::Large;
    pub const LABEL: TextSize = TextSize::Small;
    pub const ICON: IconSize = IconSize::Small;
}

// ── Elevation (`ui/src/styles/elevation.rs`, `ui/src/traits/styled_ext.rs`)

/// How close a surface sits to the user — Zed's `ElevationIndex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationIndex {
    /// The app background, under panels and panes.
    Background,
    /// Panels, panes, title bar, and tab bar.
    Surface,
    /// The editable content area.
    EditorSurface,
    /// Popovers, menus, palettes, notifications, floating panels.
    ElevatedSurface,
    /// Modals and dialogs, above the scrim.
    ModalSurface,
}

impl ElevationIndex {
    /// Zed's shadow stack for this layer. Only the two elevated layers cast
    /// one; the last layer of each is a 1px contact line with no blur.
    pub fn shadow(self, theme: &Theme) -> Vec<BoxShadow> {
        let light = theme.mode == ThemeMode::Light;
        match self {
            Self::ElevatedSurface => vec![
                drop_shadow(2., 3., 0.12),
                drop_shadow(1., 0., if light { 0.03 } else { 0.06 }),
            ],
            Self::ModalSurface => vec![
                drop_shadow(2., 3., if light { 0.06 } else { 0.12 }),
                drop_shadow(3., 6., if light { 0.06 } else { 0.08 }),
                drop_shadow(6., 12., 0.04),
                drop_shadow(1., 0., if light { 0.04 } else { 0.12 }),
            ],
            Self::Background | Self::Surface | Self::EditorSurface => Vec::new(),
        }
    }

    /// This layer's fill in Orbit's palette: the window chrome for the base
    /// layers, the content pane for the editor surface, and the floating
    /// fill for both elevated layers (Zed's `elevated_surface_background`).
    pub fn bg(self, theme: &Theme) -> Hsla {
        match self {
            Self::Background | Self::Surface => theme.surface,
            Self::EditorSurface => theme.background,
            Self::ElevatedSurface | Self::ModalSurface => theme.surface_elevated,
        }
    }
}

fn drop_shadow(offset_y: f32, blur: f32, alpha: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(offset_y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
    }
}

/// Zed's `StyledExt::elevation_*`: the elevated fill, a 12px [`Radius::XLarge`]
/// corner, a 1px hairline, and the layer's shadow. The hairline is Orbit's
/// floating-surface stroke (`border_strong`), the role Zed's `border_variant`
/// plays.
pub trait StyledExt: Styled + Sized {
    /// Title bar, panels, tab bar.
    fn elevation_1(self, theme: &Theme) -> Self {
        elevated(self, theme, ElevationIndex::Surface, true)
    }

    fn elevation_1_borderless(self, theme: &Theme) -> Self {
        elevated(self, theme, ElevationIndex::Surface, false)
    }

    /// Popovers, menus, tooltips, palettes, notifications.
    fn elevation_2(self, theme: &Theme) -> Self {
        elevated(self, theme, ElevationIndex::ElevatedSurface, true)
    }

    fn elevation_2_borderless(self, theme: &Theme) -> Self {
        elevated(self, theme, ElevationIndex::ElevatedSurface, false)
    }

    /// Modals and dialogs: anything that must be answered or dismissed before
    /// the rest of the window responds.
    fn elevation_3(self, theme: &Theme) -> Self {
        elevated(self, theme, ElevationIndex::ModalSurface, true)
    }

    fn elevation_3_borderless(self, theme: &Theme) -> Self {
        elevated(self, theme, ElevationIndex::ModalSurface, false)
    }
}

impl<E: Styled> StyledExt for E {}

// ── Raised buttons ──────────────────────────────────────────────────────

/// A button's fill as a soft top-lit gradient of its own color: a touch
/// lighter at the top edge, a touch deeper at the bottom, so the face reads
/// as a raised key rather than a flat sticker. The hue never changes — the
/// ramp only moves lightness (and, for translucent washes, alpha) — so every
/// button keeps the identity of the color it was given.
pub fn raised_fill(color: Hsla) -> Background {
    const LIFT: f32 = 0.02;
    const SINK: f32 = 0.012;
    let translucent = color.a < 0.95;
    let mut top = color;
    let mut bottom = color;
    top.l = (color.l + LIFT).min(1.);
    bottom.l = (color.l - SINK).max(0.);
    if translucent {
        top.a = (color.a * 1.12).min(1.);
        bottom.a = color.a * 0.94;
    }
    linear_gradient(
        180.,
        linear_color_stop(top, 0.),
        linear_color_stop(bottom, 1.),
    )
}

/// The shadow that lifts a button off its surface: a 1px contact line plus a
/// short, soft ambient layer. Tight on purpose — a button sits just above the
/// page, not over it (menus and modals use [`ElevationIndex`]).
pub fn button_shadow(theme: &Theme) -> Vec<BoxShadow> {
    let mut contact = theme.shadow_contact;
    contact.a *= 0.45;
    let mut ambient = theme.shadow_contact;
    ambient.a *= 0.25;
    vec![
        BoxShadow {
            color: contact,
            offset: point(px(0.), px(1.)),
            blur_radius: px(1.),
            spread_radius: px(0.),
        },
        BoxShadow {
            color: ambient,
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(-1.),
        },
    ]
}

/// Raised-button fills. Works on elements and on hover/active refinements
/// alike (`.hover(|s| s.raised(theme.surface_hover, &theme))`).
pub trait RaisedExt: Styled + Sized {
    /// Gradient face in `color` plus the lift shadow — a standalone button.
    fn raised(self, color: Hsla, theme: &Theme) -> Self {
        self.bg(raised_fill(color)).shadow(button_shadow(theme))
    }

    /// Gradient face only — for segments of a grouped control, where a
    /// per-segment shadow would spill onto the neighbours.
    fn raised_flat(self, color: Hsla) -> Self {
        self.bg(raised_fill(color))
    }
}

impl<E: Styled> RaisedExt for E {}

fn elevated<E: Styled>(el: E, theme: &Theme, index: ElevationIndex, bordered: bool) -> E {
    let el = el
        .bg(ElevationIndex::ElevatedSurface.bg(theme))
        .rounded(Radius::XLarge.px(theme))
        .shadow(index.shadow(theme));
    if bordered {
        el.border_1().border_color(theme.border_strong)
    } else {
        el
    }
}

// ── Motion (`ui/src/styles/animation.rs`) ───────────────────────────────

/// Zed's `AnimationDuration`.
///
/// Only one-shot transitions use this scale. Orbit's looping affordances (the
/// spinner, shimmer, streaming pulses) and its feedback timers (copy / refresh
/// acknowledgment, gesture guards) run their own cadences, which Zed's
/// three-value scale does not cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimationDuration {
    Instant = 50,
    /// The default for animate-in transitions.
    Fast = 150,
    Slow = 300,
}

impl AnimationDuration {
    pub const fn duration(self) -> Duration {
        Duration::from_millis(self as u64)
    }
}

impl From<AnimationDuration> for Duration {
    fn from(duration: AnimationDuration) -> Self {
        duration.duration()
    }
}

/// Zed's default easing for one-shot transitions. Re-exported vocabulary,
/// like the rest of this module, before any surface animates with it.
#[allow(unused_imports)]
pub use gpui::ease_out_quint;

/// How far an animate-in travels.
pub const SLIDE_IN_DISTANCE: Pixels = px(40.);

/// Where a fading animate-in starts.
pub const FADE_IN_START_OPACITY: f32 = 0.4;

// ── Scrollbars (`ui/src/components/scrollbar.rs`) ───────────────────────

/// Zed's `ScrollbarStyle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbarStyle {
    #[default]
    Regular,
    Editor,
}

impl ScrollbarStyle {
    pub const fn width(self) -> Pixels {
        match self {
            Self::Regular => px(6.),
            Self::Editor => px(15.),
        }
    }
}

/// Scrollbar metrics shared by both styles.
pub mod scrollbar {
    use super::*;

    /// Around the thumb.
    pub const PADDING: Pixels = px(4.);
    pub const TRACK_BORDER: Pixels = BORDER_WIDTH;
    pub const MIN_THUMB_LENGTH: Pixels = px(25.);
    /// Idle time before an auto-hiding scrollbar fades.
    pub const AUTOHIDE_DELAY: Duration = Duration::from_millis(1000);
    pub const HIDE_DURATION: Duration = Duration::from_millis(400);
    pub const SHOW_DURATION: Duration = Duration::from_millis(50);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::UiPrefs;

    fn theme_with(ui_font_size: f32, spacing_density: u32) -> Theme {
        let ui = UiPrefs {
            ui_font_size,
            spacing_density,
            ..UiPrefs::default()
        };
        Theme::dark().with_ui(ui)
    }

    fn default_theme() -> Theme {
        Theme::dark()
    }

    fn close(a: Pixels, b: f32) -> bool {
        (f32::from(a) - b).abs() < 0.001
    }

    #[test]
    fn default_settings_resolve_to_zeds_default_pixels() {
        let theme = default_theme();
        let expected = [
            0., 1., 2., 3., 4., 6., 8., 12., 16., 20., 24., 32., 40., 48.,
        ];
        for (spacing, px) in DynamicSpacing::ALL.into_iter().zip(expected) {
            assert!(close(spacing.px(&theme), px), "{spacing:?}");
        }
        assert!(close(TextSize::Large.px(&theme), 16.));
        assert!(close(TextSize::Default.px(&theme), 14.));
        assert!(close(TextSize::Small.px(&theme), 12.));
        assert!(close(TextSize::XSmall.px(&theme), 10.));
        assert!(close(ButtonSize::Default.height(&theme), 22.));
        assert!(close(ButtonSize::Medium.height(&theme), 28.));
        assert!(close(Radius::Small.px(&theme), 4.));
        assert!(close(Radius::Large.px(&theme), 8.));
        assert!(close(tooltip::max_width(&theme), 288.));
        assert!(close(HeadlineSize::Small.px(&theme), 16.));
    }

    #[test]
    fn density_anchors_hit_zeds_three_tables() {
        for spacing in DynamicSpacing::ALL {
            for density in [
                UiDensity::Compact,
                UiDensity::Default,
                UiDensity::Comfortable,
            ] {
                assert_eq!(
                    spacing.at_percent(density.percent()),
                    spacing.at(density),
                    "{spacing:?} at {density:?}"
                );
            }
        }
        // Past the outer anchors the table is held, not extrapolated.
        assert_eq!(DynamicSpacing::Base08.at_percent(60), 4.);
        assert_eq!(DynamicSpacing::Base08.at_percent(140), 10.);
    }

    #[test]
    fn density_interpolates_between_anchors() {
        // Base12 is (10, 12, 14): halfway to Compact is 11, to Comfortable 13.
        assert_eq!(DynamicSpacing::Base12.at_percent(90), 11.);
        assert_eq!(DynamicSpacing::Base12.at_percent(110), 13.);
        let compact = theme_with(14., 80);
        assert!(close(DynamicSpacing::Base06.px(&compact), 3.));
    }

    #[test]
    fn tokens_scale_with_the_ui_font_like_rems() {
        // 16px UI font is 16/14 of Orbit's authored default.
        let theme = theme_with(16., 100);
        let scale = 16. / 14.;
        assert!(close(DynamicSpacing::Base08.px(&theme), 8. * scale));
        assert!(close(TextSize::Default.px(&theme), 14. * scale));
        assert!(close(IconSize::Small.px(&theme), 14. * scale));
        assert!(close(theme.rems(1.), 16. * scale));
        // Fixed-px tokens do not move.
        assert_eq!(context_menu::MIN_WIDTH, px(200.));
        assert_eq!(list_item::INDENT_STEP, px(12.));
        assert!(close(TextSize::Ui.px(&theme), 16.));
    }

    #[test]
    fn icon_squares_add_their_padding() {
        let theme = default_theme();
        assert!(close(IconSize::Indicator.square(&theme), 10.));
        assert!(close(IconSize::XSmall.square(&theme), 16.));
        assert!(close(IconSize::Small.square(&theme), 18.));
        assert!(close(IconSize::Medium.square(&theme), 20.));
        assert!(close(IconSize::XLarge.square(&theme), 52.));
    }

    #[test]
    fn button_padding_follows_size() {
        let theme = default_theme();
        assert!(close(ButtonSize::Large.padding_x(&theme), 8.));
        assert!(close(ButtonSize::Default.padding_x(&theme), 4.));
        assert_eq!(ButtonSize::None.padding_x(&theme), px(1.));
    }

    #[test]
    fn raised_buttons_cast_a_short_shadow_in_both_modes() {
        for theme in [default_theme(), Theme::light()] {
            let shadow = button_shadow(&theme);
            assert_eq!(shadow.len(), 2, "contact line + soft ambient");
            for layer in &shadow {
                assert!(layer.color.a > 0., "{:?} shadow is visible", theme.mode);
                assert!(
                    layer.blur_radius <= px(4.),
                    "a button sits just above the page"
                );
            }
        }
    }

    #[test]
    fn button_icons_grow_with_the_frame() {
        let scale = [
            (ButtonSize::None, IconSize::Indicator),
            (ButtonSize::Compact, IconSize::XSmall),
            (ButtonSize::Default, IconSize::Small),
            (ButtonSize::Medium, IconSize::Small),
            (ButtonSize::Large, IconSize::Medium),
        ];
        for (button, icon) in scale {
            assert_eq!(button.icon_size(), icon, "{button:?}");
        }
    }

    #[test]
    fn menu_entries_are_one_comfortable_line_tall() {
        let theme = default_theme();
        // 14px × 1.618 = 22.65, rounded by GPUI to 23.
        assert!(close(context_menu::entry_height(&theme), 23.));
        let line = px(20.);
        assert_eq!(ListItemSpacing::Dense.row_height(line, &theme), px(20.));
        assert_eq!(
            ListItemSpacing::ExtraDense.row_height(line, &theme),
            px(18.)
        );
        assert!(close(ListItemSpacing::Sparse.row_height(line, &theme), 28.));
    }

    #[test]
    fn elevation_shadows_match_zed() {
        let dark = Theme::dark();
        let light = Theme::light();
        for index in [
            ElevationIndex::Background,
            ElevationIndex::Surface,
            ElevationIndex::EditorSurface,
        ] {
            assert!(index.shadow(&dark).is_empty(), "{index:?} casts no shadow");
        }
        let elevated = ElevationIndex::ElevatedSurface.shadow(&dark);
        assert_eq!(elevated.len(), 2);
        assert_eq!(elevated[0].offset, point(px(0.), px(2.)));
        assert_eq!(elevated[0].blur_radius, px(3.));
        assert_eq!(elevated[1].blur_radius, px(0.));
        assert!((elevated[1].color.a - 0.06).abs() < 1e-6);
        let elevated_light = ElevationIndex::ElevatedSurface.shadow(&light);
        assert!((elevated_light[1].color.a - 0.03).abs() < 1e-6);

        let modal = ElevationIndex::ModalSurface.shadow(&dark);
        assert_eq!(modal.len(), 4);
        assert_eq!(modal[2].offset, point(px(0.), px(6.)));
        assert_eq!(modal[2].blur_radius, px(12.));
        for shadow in elevated.iter().chain(&modal) {
            assert_eq!(shadow.spread_radius, px(0.));
            assert_eq!(shadow.offset.x, px(0.));
        }
    }

    #[test]
    fn density_buckets_for_branching_ui() {
        assert_eq!(UiDensity::nearest(80), UiDensity::Compact);
        assert_eq!(UiDensity::nearest(85), UiDensity::Compact);
        assert_eq!(UiDensity::nearest(90), UiDensity::Default);
        assert_eq!(UiDensity::nearest(110), UiDensity::Default);
        assert_eq!(UiDensity::nearest(115), UiDensity::Comfortable);
    }

    #[test]
    fn durations_are_zeds() {
        assert_eq!(
            Duration::from(AnimationDuration::Instant),
            Duration::from_millis(50)
        );
        assert_eq!(
            AnimationDuration::Fast.duration(),
            Duration::from_millis(150)
        );
        assert_eq!(
            AnimationDuration::Slow.duration(),
            Duration::from_millis(300)
        );
    }
}
