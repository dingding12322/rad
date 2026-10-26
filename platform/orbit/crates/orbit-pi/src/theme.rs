//! App theme: a GPUI [`Global`] with a selectable palette.
//!
//! Colors are semantic roles (backgrounds, text, borders, transcript
//! surfaces) rather than raw steps, so every paint site reads from one
//! place. A [`ThemeId`] names a palette; [`AppearancePrefs`] chooses which
//! light or dark palette to use, optionally following the system. The UI
//! face is Zed's bundled IBM Plex Sans (`.ZedSans`); code surfaces use
//! Zed's Lilex (`.ZedMono`).

use std::path::PathBuf;
use std::sync::RwLock;

use gpui::{hsla, point, px, rgb, App, BoxShadow, Global, Hsla, Pixels, SharedString};
use serde_json::Value;

use crate::highlight::TokenClass;

mod appearance;
pub use appearance::*;

/// Zed's design tokens (spacing, type, icons, buttons, elevation, motion),
/// resolved through this theme.
pub mod tokens;

/// Dark or light appearance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

/// One of the selectable Orbit palettes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeId {
    Orbit,
    OrbitLight,
    OrbitPaper,
    OrbitContrast,
    OrbitDarker,
    /// Graphite canvas, cool ink, and an electric-blue accent. Formerly
    /// `OrbitMinimal`; renamed so "minimal" names the interface, never a hue.
    OrbitCobalt,
    /// The blue redesign that briefly held the `Orbit` name: charcoal canvas,
    /// neutral surfaces, restrained blue accent, accent-filled primary buttons.
    OrbitAzure,
    /// The one ported editor theme that stays: warm paper ink on a near-black
    /// ground, with a light counterpart.
    Flexoki,
    FlexokiLight,
    /// Light counterpart to [`Self::OrbitAzure`].
    OrbitAzureLight,
}

impl ThemeId {
    /// Every palette the app ships, dark first, in the order the settings
    /// dropdown paints them.
    pub const ALL: [ThemeId; 10] = [
        Self::Orbit,
        Self::OrbitDarker,
        Self::OrbitCobalt,
        Self::OrbitAzure,
        Self::Flexoki,
        Self::OrbitLight,
        Self::OrbitAzureLight,
        Self::OrbitPaper,
        Self::OrbitContrast,
        Self::FlexokiLight,
    ];

    /// Persisted key; also accepts legacy theme names (mapped to Orbit).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Orbit => "orbit",
            Self::OrbitLight => "orbit-light",
            Self::OrbitPaper => "orbit-paper",
            Self::OrbitContrast => "orbit-contrast",
            Self::OrbitDarker => "orbit-darker",
            Self::OrbitCobalt => "orbit-cobalt",
            Self::OrbitAzure => "orbit-azure",
            Self::Flexoki => "flexoki",
            Self::FlexokiLight => "flexoki-light",
            Self::OrbitAzureLight => "orbit-azure-light",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            // Current keys, plus legacy dark theme names → Orbit.
            "orbit"
            | "dark"
            | "one-dark"
            | "zedokai"
            | "zedokai-darker"
            | "zedokai-filter-octagon"
            | "zedokai-darker-filter-octagon"
            | "zedokai-filter-spectrum"
            | "zedokai-darker-filter-spectrum" => Some(Self::Orbit),
            // Current key, plus the legacy generic light aliases → Orbit Light.
            "orbit-light" | "light" => Some(Self::OrbitLight),
            "orbit-paper" => Some(Self::OrbitPaper),
            "orbit-contrast" | "orbit-high-contrast" => Some(Self::OrbitContrast),
            "orbit-darker" => Some(Self::OrbitDarker),
            // Graphite-and-blue dark. `orbit-minimal` is the pre-rename key,
            // and the rest name the palette's origin; all three still load.
            "orbit-cobalt" | "orbit-minimal" | "codex" | "codex-dark" => Some(Self::OrbitCobalt),
            "orbit-azure" | "azure" => Some(Self::OrbitAzure),
            "orbit-azure-light" | "azure-light" => Some(Self::OrbitAzureLight),
            // Retired editor themes. Their saved keys still load: dark ones
            // land on Orbit, light ones on Orbit Light.
            "vague"
            | "batsignal-dark"
            | "batsignal"
            | "ashwood"
            | "obsidian-dark"
            | "obsidian"
            | "matte-black"
            | "matte-black-theme"
            | "adwaita-pastel-dark"
            | "adwaita-pastel"
            | "ashen"
            | "discord-dark"
            | "dark-discord"
            | "discord"
            | "catppuccin"
            | "catppuccin-mocha"
            | "catppuccin-dark"
            | "dracula"
            | "dracula-dark"
            | "gruvbox"
            | "gruvbox-dark"
            | "nord"
            | "nord-dark"
            | "tokyo-night"
            | "tokyonight"
            | "tokyo-night-storm"
            | "one-dark-pro"
            | "onedarkpro"
            | "one-dark-pro-max"
            | "onedarkpromax"
            | "odpm"
            | "monokai"
            | "solarized-dark"
            | "solarized"
            | "ayu-dark"
            | "ayu"
            | "kanagawa"
            | "kanagawa-wave"
            | "aura"
            | "aura-dark"
            | "carbonfox"
            | "carbonfox-dark"
            | "fields-of-the-shire"
            | "fields-of-the-shire-dark"
            | "jetbrains"
            | "jetbrains-dark"
            | "mono"
            | "mono-plus"
            | "night-owl"
            | "nightowl"
            | "night-owl-dark"
            | "open-chamber"
            | "openchamber"
            | "vesper"
            | "vitesse" => Some(Self::Orbit),
            "catppuccin-latte" | "catppuccin-light" | "latte" | "solarized-light" | "one-light"
            | "one-light-pro" | "nord-light" | "nord-snow-storm" | "gruvbox-light"
            | "tokyo-night-light" | "tokyonight-light" | "tokyo-night-day" | "ayu-light" => {
                Some(Self::OrbitLight)
            }
            "flexoki" | "flexoki-dark" => Some(Self::Flexoki),
            "flexoki-light" => Some(Self::FlexokiLight),
            _ => None,
        }
    }

    /// Human label for the settings dropdown.
    pub fn label(self) -> &'static str {
        match self {
            Self::Orbit => "Orbit",
            Self::OrbitLight => "Orbit Light",
            Self::OrbitPaper => "Orbit Paper",
            Self::OrbitContrast => "Orbit Contrast",
            Self::OrbitDarker => "Orbit Darker",
            Self::OrbitCobalt => "Orbit Cobalt",
            Self::OrbitAzure => "Orbit Azure",
            Self::Flexoki => "Flexoki",
            Self::FlexokiLight => "Flexoki Light",
            Self::OrbitAzureLight => "Orbit Azure Light",
        }
    }

    pub fn appearance(self) -> ThemeMode {
        match self {
            Self::OrbitLight
            | Self::OrbitAzureLight
            | Self::OrbitPaper
            | Self::OrbitContrast
            | Self::FlexokiLight => ThemeMode::Light,
            Self::Orbit
            | Self::OrbitAzure
            | Self::OrbitDarker
            | Self::OrbitCobalt
            | Self::Flexoki => ThemeMode::Dark,
        }
    }
}

/// Semantic palette. Copy so list closures and hover styles can capture it
/// without a lifetime.
///
/// The first group is the canonical design-system surface: one semantic role
/// per meaning, resolved from the active [`ThemeId`] and [`ThemeMode`]. Paint
/// sites should read these names rather than raw color values. The remaining
/// fields are specialized roles (code washes, syntax, selection washes) that
/// do not belong to the general surface ramp.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub theme_id: ThemeId,
    pub mode: ThemeMode,
    /// UI customization (General settings): language + font sizes.
    pub ui: UiPrefs,

    // ── Surfaces ────────────────────────────────────────────────────────
    /// Root window and workspace canvas.
    pub background: Hsla,
    /// Primary panel surface: sidebars, panes, composer.
    pub surface: Hsla,
    /// Nested/secondary containers and subtle wells.
    pub surface_secondary: Hsla,
    /// Raised cards, chips, menus, popovers, modals.
    pub surface_elevated: Hsla,
    /// Pointer-hover feedback.
    pub surface_hover: Hsla,
    /// Pressed/open/highlighted feedback.
    pub surface_active: Hsla,
    /// Hairline between regions.
    pub border: Hsla,
    /// Quiet separator inside a surface.
    pub border_subtle: Hsla,

    // ── Text ─────────────────────────────────────────────────────────────
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub text_disabled: Hsla,

    // ── Accent ───────────────────────────────────────────────────────────
    /// The palette's identity hue: primary fills, selection washes, focus,
    /// tints, and accent-colored text derive from it.
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_pressed: Hsla,
    /// Foreground on an accent or error fill.
    pub accent_fg: Hsla,
    /// Accent-colored text/icons on canvas or panel surfaces.
    pub accent_text: Hsla,
    /// Selected navigation/list row wash.
    pub selection: Hsla,
    /// Foreground on [`Self::selection`].
    pub selection_fg: Hsla,
    /// Keyboard focus indicator.
    pub focus_ring: Hsla,

    // ── Primary action ───────────────────────────────────────────────────
    /// Fill of a primary action button (send, commit, Save, Update). A palette
    /// may key it to its accent or to a neutral step, so an existing identity
    /// keeps its primary buttons while the accent stays the selection/text hue.
    pub action: Hsla,
    pub action_hover: Hsla,
    pub action_pressed: Hsla,
    /// Foreground on [`Self::action`].
    pub action_fg: Hsla,

    // ── Status ───────────────────────────────────────────────────────────
    pub success: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub error_hover: Hsla,
    /// Foreground on a solid error fill.
    pub error_fg: Hsla,
    pub info: Hsla,

    // ── Specialized surfaces and roles ───────────────────────────────────
    /// Knob of a toggle switch. Kept light in both modes so it reads on the
    /// accent (on) and the elevated track (off).
    pub toggle_knob: Hsla,
    /// Translucent hover wash over content rows.
    pub overlay: Hsla,
    /// Stronger translucent hover/selection wash.
    pub overlay_strong: Hsla,
    /// Ink-derived strong hairline for floating surfaces.
    pub border_strong: Hsla,
    pub assistant_text: Hsla,
    pub code_bg: Hsla,
    pub code_text: Hsla,
    /// Syntax token colors (transcript code blocks + the review diff). A
    /// palette sets these explicitly so a ported editor theme keeps its own
    /// syntax identity instead of borrowing the UI's semantic roles.
    pub syn_string: Hsla,
    pub syn_number: Hsla,
    pub syn_function: Hsla,
    pub syn_type: Hsla,
    pub syn_comment: Hsla,
    pub syn_literal: Hsla,
    pub syn_meta: Hsla,
    pub syn_operator: Hsla,
    /// Inline-code chip wash. The wash is the accent at low alpha; the
    /// glyphs ride the body ink, so code stays distinct from links.
    pub inline_code_bg: Hsla,
    pub shadow_contact: Hsla,
}

impl Global for Theme {}

/// Interface language for the workbench chrome. Re-exported from
/// [`crate::i18n`] so every persisted pref reads as a locale id while the
/// `Language` name stays stable for the settings surface.
pub use crate::i18n::AppLanguage as Language;

/// Orbit UI preferences: language, type sizes, density, and composer behavior.
/// Every size is px with the built-in defaults (UI 14, terminal / editor 13);
/// only spacing density remains a percentage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiPrefs {
    pub language: Language,
    /// Interface text size, px ("UI font size").
    pub ui_font_size: f32,
    /// Terminal / tool-output size, px ("Terminal Font Size").
    pub terminal_font_size: f32,
    /// Editor / diff / code-block size, px ("Editor Font Size").
    pub editor_font_size: f32,
    /// Global spacing multiplier, percent ("Spacing Density").
    pub spacing_density: u32,
    /// Honor reduce-motion: perpetual/looping animations (spinners, the
    /// running-session shimmer, the drop-overlay fade) render static.
    pub reduce_motion: bool,
    /// Enter's delivery mode while the agent is working (Orbit-only).
    pub composer_send_mode: crate::composer_send::SendMode,
}

/// The interface text size authored against, px. Chrome sizes scale relative
/// to this, like a rem authored at its default UI font size.
pub const DEFAULT_UI_FONT_SIZE: f32 = 14.;
/// Selectable interface / terminal / editor font sizes, px.
pub const FONT_SIZES: [f32; 8] = [11., 12., 13., 14., 15., 16., 18., 20.];
/// Selectable spacing densities, percent.
pub const SPACING_DENSITIES: [u32; 9] = [80, 85, 90, 95, 100, 105, 110, 115, 120];

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            language: Language::System,
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            terminal_font_size: 13.,
            editor_font_size: 13.,
            spacing_density: 100,
            reduce_motion: false,
            composer_send_mode: crate::composer_send::SendMode::default(),
        }
    }
}

impl UiPrefs {
    fn persist_path() -> PathBuf {
        crate::platform::home_dir()
            .join(".orbit-pi")
            .join("ui.json")
    }

    fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(Self::persist_path()) else {
            return Self::default();
        };
        serde_json::from_str::<Value>(&raw)
            .map(|value| Self::from_value(&value))
            .unwrap_or_default()
    }

    /// Nearest selectable font size, for migrating legacy percentage scales.
    fn nearest_size(value: f32) -> f32 {
        // A NaN in a legacy file must not panic startup; fall back to the
        // default size (a NaN-safe `total_cmp` alone would pick
        // `FONT_SIZES[0]`, because NaN compares greater than everything).
        if value.is_nan() {
            return DEFAULT_UI_FONT_SIZE;
        }
        FONT_SIZES
            .into_iter()
            .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
            .unwrap_or(DEFAULT_UI_FONT_SIZE)
    }

    /// Parse prefs from JSON, falling back to defaults per field. A legacy
    /// `interface_scale` percentage migrates onto the equivalent px size.
    fn from_value(value: &Value) -> Self {
        let mut prefs = Self::default();
        if let Some(language) = value
            .get("language")
            .and_then(Value::as_str)
            .and_then(Language::parse)
        {
            prefs.language = language;
        }
        if let Some(size) = value.get("ui_font_size").and_then(Value::as_f64) {
            let size = size as f32;
            if FONT_SIZES.contains(&size) {
                prefs.ui_font_size = size;
            }
        } else if let Some(scale) = value.get("interface_scale").and_then(Value::as_u64) {
            let px = scale as f32 / 100. * DEFAULT_UI_FONT_SIZE;
            prefs.ui_font_size = Self::nearest_size(px);
        }
        if let Some(size) = value.get("terminal_font_size").and_then(Value::as_f64) {
            if FONT_SIZES.contains(&(size as f32)) {
                prefs.terminal_font_size = size as f32;
            }
        }
        if let Some(size) = value.get("editor_font_size").and_then(Value::as_f64) {
            if FONT_SIZES.contains(&(size as f32)) {
                prefs.editor_font_size = size as f32;
            }
        } else if let Some(size) = value.get("code_font_size").and_then(Value::as_f64) {
            let size = size as f32;
            if FONT_SIZES.contains(&size) {
                prefs.editor_font_size = size;
            }
        }
        if let Some(density) = value.get("spacing_density").and_then(Value::as_u64) {
            let density = density as u32;
            if SPACING_DENSITIES.contains(&density) {
                prefs.spacing_density = density;
            }
        }
        if let Some(reduce) = value.get("reduce_motion").and_then(Value::as_bool) {
            prefs.reduce_motion = reduce;
        }
        if let Some(mode) = value
            .get("composer_send_mode")
            .and_then(Value::as_str)
            .and_then(crate::composer_send::SendMode::parse)
        {
            prefs.composer_send_mode = mode;
        }
        prefs
    }

    fn persist(self) {
        let path = Self::persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, self.to_value().to_string());
    }

    fn to_value(self) -> Value {
        serde_json::json!({
            "language": self.language.as_str(),
            "ui_font_size": self.ui_font_size,
            "terminal_font_size": self.terminal_font_size,
            "editor_font_size": self.editor_font_size,
            "spacing_density": self.spacing_density,
            "reduce_motion": self.reduce_motion,
            "composer_send_mode": self.composer_send_mode.as_str(),
        })
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::for_id(ThemeId::OrbitAzure)
    }
}

/// Current UI / code font families. Stored as statics (readable without an
/// app handle) so transcript helpers, which don't carry `cx`, can look them
/// up — keeping [`Theme`] itself `Copy`. Persisted to `~/.orbit-pi/fonts.json`.
static UI_FONT_FAMILY: RwLock<SharedString> = RwLock::new(SharedString::new_static(".ZedSans"));
static CODE_FONT_FAMILY: RwLock<SharedString> = RwLock::new(SharedString::new_static(".ZedMono"));

/// The persisted font-families payload.
#[derive(Debug, Clone, PartialEq)]
pub struct FontPrefs {
    /// UI face family (`.ZedSans` → bundled IBM Plex Sans by default).
    pub ui_font_family: SharedString,
    /// Code/monospace face family (`.ZedMono` → bundled Lilex by default).
    pub code_font_family: SharedString,
}

impl Default for FontPrefs {
    fn default() -> Self {
        Self {
            ui_font_family: ".ZedSans".into(),
            code_font_family: ".ZedMono".into(),
        }
    }
}

impl FontPrefs {
    fn persist_path() -> PathBuf {
        crate::platform::home_dir()
            .join(".orbit-pi")
            .join("fonts.json")
    }

    fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(Self::persist_path()) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Self::default();
        };
        let mut prefs = Self::default();
        if let Some(family) = value.get("ui_font_family").and_then(Value::as_str) {
            if !family.is_empty() {
                prefs.ui_font_family = family.to_string().into();
            }
        }
        if let Some(family) = value.get("code_font_family").and_then(Value::as_str) {
            if !family.is_empty() {
                prefs.code_font_family = family.to_string().into();
            }
        }
        prefs
    }

    fn persist(&self) {
        let path = Self::persist_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            path,
            serde_json::json!({
                "ui_font_family": self.ui_font_family.to_string(),
                "code_font_family": self.code_font_family.to_string(),
            })
            .to_string(),
        );
    }
}

/// Current UI face family (readable without `cx`).
pub fn ui_font_family() -> SharedString {
    UI_FONT_FAMILY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// Current code/mono face family (readable without `cx`).
pub fn code_font_family() -> SharedString {
    CODE_FONT_FAMILY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// A named entry in the curated Interface / Code font pickers. `family` is
/// the real family gpui resolves; `label` is what the settings chip shows.
#[derive(Debug, Clone, Copy)]
pub struct FontChoice {
    pub label: &'static str,
    pub family: &'static str,
}

/// Orbit's curated interface faces (plus a System escape hatch).
pub const UI_FONTS: [FontChoice; 10] = [
    FontChoice {
        label: "Inter",
        family: "Inter",
    },
    FontChoice {
        label: "Fixel Text",
        family: "Fixel Text",
    },
    FontChoice {
        label: "Geist Sans",
        family: "Geist",
    },
    FontChoice {
        label: "Atkinson Hyperlegible",
        family: "Atkinson Hyperlegible",
    },
    FontChoice {
        label: "Source Sans 3",
        family: "Source Sans 3",
    },
    FontChoice {
        label: "Roboto",
        family: "Roboto",
    },
    FontChoice {
        label: "Noto Sans",
        family: "Noto Sans",
    },
    FontChoice {
        label: "DM Sans",
        family: "DM Sans",
    },
    FontChoice {
        label: "Manrope",
        family: "Manrope",
    },
    FontChoice {
        label: "IBM Plex Sans",
        family: ".ZedSans",
    },
];

/// Orbit's curated code faces (plus a System Mono escape hatch).
pub const CODE_FONTS: [FontChoice; 16] = [
    FontChoice {
        label: "JetBrains Mono",
        family: "JetBrains Mono",
    },
    FontChoice {
        label: "Fira Code",
        family: "Fira Code",
    },
    FontChoice {
        label: "Geist Mono",
        family: "Geist Mono",
    },
    FontChoice {
        label: "Commit Mono",
        family: "CommitMono",
    },
    FontChoice {
        label: "Source Code Pro",
        family: "Source Code Pro",
    },
    FontChoice {
        label: "Cascadia Code",
        family: "Cascadia Code",
    },
    FontChoice {
        label: "Roboto Mono",
        family: "Roboto Mono",
    },
    FontChoice {
        label: "Iosevka",
        family: "Iosevka",
    },
    FontChoice {
        label: "DM Mono",
        family: "DM Mono",
    },
    FontChoice {
        label: "IBM Plex Mono",
        family: "IBM Plex Mono",
    },
    FontChoice {
        label: "Inconsolata",
        family: "Inconsolata",
    },
    FontChoice {
        label: "Noto Sans Mono",
        family: "Noto Sans Mono",
    },
    FontChoice {
        label: "Space Mono",
        family: "Space Mono",
    },
    FontChoice {
        label: "Anonymous Pro",
        family: "Anonymous Pro",
    },
    FontChoice {
        label: "Martian Mono",
        family: "Martian Mono",
    },
    FontChoice {
        label: "Lilex",
        family: ".ZedMono",
    },
];

/// Display label for a stored family name: a curated label when one exists,
/// otherwise the raw family (so legacy/installed choices still read right).
pub fn font_choice_label(family: &str) -> String {
    if family == ".ZedSans" {
        return "IBM Plex Sans".into();
    }
    if family == ".ZedMono" {
        return "Lilex".into();
    }
    for choice in UI_FONTS.iter().chain(CODE_FONTS.iter()) {
        if choice.family == family {
            return choice.label.into();
        }
    }
    family.to_string()
}

/// Current font families as a struct (for the settings UI).
pub fn font_prefs() -> FontPrefs {
    FontPrefs {
        ui_font_family: ui_font_family(),
        code_font_family: code_font_family(),
    }
}

/// The raw color tokens for one palette. Derived washes (hover overlay,
/// strong border, inline-code wash) and shadows are computed from these in
/// [`Theme::build`], so a palette only carries its actual colors.
#[derive(Clone, Copy)]
struct Palette {
    background: u32,
    surface: u32,
    surface_secondary: u32,
    elevated: u32,
    hover: u32,
    active: u32,
    selection: u32,
    selection_fg: u32,
    border: u32,
    border_subtle: u32,
    text: u32,
    text_secondary: u32,
    text_muted: u32,
    text_disabled: u32,
    success: u32,
    error: u32,
    error_hover: u32,
    error_fg: u32,
    accent: u32,
    accent_hover: u32,
    accent_pressed: u32,
    accent_fg: u32,
    accent_text: u32,
    focus_ring: u32,
    /// Primary-action button fill, separate from the identity accent.
    action: u32,
    action_hover: u32,
    action_pressed: u32,
    action_fg: u32,
    warning: u32,
    info: u32,
    assistant_text: u32,
    code_bg: u32,
    code_text: u32,
    syn_string: u32,
    syn_number: u32,
    syn_function: u32,
    syn_type: u32,
    syn_comment: u32,
    syn_literal: u32,
    syn_meta: u32,
    syn_operator: u32,
}

/// Orbit dark: near-black canvas, neutral charcoal surfaces, the ember
/// interaction accent, and light primary buttons with dark ink. This is the
/// default dark appearance; Orbit Azure carries the blue design.
const ORBIT: Palette = Palette {
    background: 0x1A1A1A,
    surface: 0x181818,
    surface_secondary: 0x1E1E1E,
    elevated: 0x232323,
    hover: 0x262626,
    active: 0x2A2A2A,
    selection: 0x282828,
    selection_fg: 0xE2E2E2,
    border: 0x2A2A2A,
    border_subtle: 0x232323,
    text: 0xE2E2E2,
    text_secondary: 0xA3A3A3,
    text_muted: 0x8A8A8A,
    text_disabled: 0x606060,
    success: 0x62C987,
    error: 0xE2726A,
    error_hover: 0xEC8A82,
    error_fg: 0x17181C,
    accent: 0xE2795B,
    accent_hover: 0xEC8B72,
    accent_pressed: 0xD9634A,
    accent_fg: 0x17181C,
    accent_text: 0xE2795B,
    focus_ring: 0xE2E2E2,
    action: 0xE7E9EC,
    action_hover: 0xF2F3F6,
    action_pressed: 0xDDE0E4,
    action_fg: 0x17181C,
    warning: 0xE0B36A,
    info: 0x7FA7D9,
    assistant_text: 0xE2E2E2,
    code_bg: 0x151515,
    code_text: 0xE2E2E2,
    syn_string: 0x62C987,
    syn_number: 0xE0B36A,
    syn_function: 0xE2E2E2,
    syn_type: 0xE2E2E2,
    syn_comment: 0xA3A3A3,
    syn_literal: 0xE2795B,
    syn_meta: 0xE2726A,
    syn_operator: 0x8A8A8A,
};

/// Orbit Darker ("Premium Graphite"): a deeper cut of Orbit — near-black
/// graphite surfaces, lighter hairlines, restrained ember accent.
const ORBIT_DARKER: Palette = Palette {
    background: 0x111111,
    surface: 0x0D0D0D,
    surface_secondary: 0x131313,
    elevated: 0x191919,
    hover: 0x1E1E1E,
    active: 0x232323,
    selection: 0x222A38,
    selection_fg: 0xF0F0F0,
    border: 0x252525,
    border_subtle: 0x1B1B1B,
    text: 0xE8E8E8,
    text_secondary: 0xA1A1A1,
    text_muted: 0x8C8C8C,
    text_disabled: 0x5A5A5A,
    success: 0x69C58A,
    error: 0xE16F68,
    error_hover: 0xEB827A,
    error_fg: 0x111214,
    accent: 0xE07A5F,
    accent_hover: 0xEA8A72,
    accent_pressed: 0xC9644B,
    accent_fg: 0x111214,
    accent_text: 0xE9A288,
    focus_ring: 0xE9A288,
    action: 0xE9EAEC,
    action_hover: 0xFFFFFF,
    action_pressed: 0xDCDEE1,
    action_fg: 0x111214,
    warning: 0xDDB56A,
    info: 0x6EA0D8,
    assistant_text: 0xE4E4E4,
    code_bg: 0x0C0C0C,
    code_text: 0xE4E4E4,
    syn_string: 0x69C58A,
    syn_number: 0xDDB56A,
    syn_function: 0xE4E4E4,
    syn_type: 0xD7D7D7,
    syn_comment: 0x8D8D8D,
    syn_literal: 0xE07A5F,
    syn_meta: 0xE16F68,
    syn_operator: 0x8A8A8A,
};

/// Orbit Cobalt (formerly "Orbit Minimal", codex-inspired): a deep graphite
/// canvas with cool-gray ink and an electric-blue interaction accent.
const ORBIT_COBALT: Palette = Palette {
    background: 0x181818,
    surface: 0x121212,
    surface_secondary: 0x171717,
    elevated: 0x1E1E1E,
    hover: 0x242424,
    active: 0x292929,
    selection: 0x1D2B3F,
    selection_fg: 0xF1F1F1,
    border: 0x2A2A2A,
    border_subtle: 0x202020,
    text: 0xF1F1F1,
    text_secondary: 0xA7A7A7,
    text_muted: 0x8E8E8E,
    text_disabled: 0x5C5C5C,
    success: 0x40C977,
    error: 0xFA423E,
    error_hover: 0xFF625E,
    error_fg: 0x111111,
    accent: 0x339CFF,
    accent_hover: 0x4FA8FF,
    accent_pressed: 0x2B8AE6,
    accent_fg: 0x111111,
    accent_text: 0x5EAEFF,
    focus_ring: 0x6EA0FF,
    action: 0xF1F1F1,
    action_hover: 0xFFFFFF,
    action_pressed: 0xE4E4E4,
    action_fg: 0x111111,
    warning: 0xD9B45D,
    info: 0x65A4E8,
    assistant_text: 0xE8E8E8,
    code_bg: 0x101010,
    code_text: 0xE7E7E7,
    // Syntax keeps the editor's cool-blue keywords with semantic green/amber.
    syn_string: 0x40C977,
    syn_number: 0xD9B45D,
    syn_function: 0xFFFFFF,
    syn_type: 0xD7D7D7,
    syn_comment: 0x888888,
    syn_literal: 0x5EAEFF,
    syn_meta: 0xF06A6A,
    syn_operator: 0x8A8A8A,
};

/// Orbit Azure: charcoal canvas with a restrained blue interaction accent —
/// the blue counterpart to Orbit. Primary buttons ride the accent.
const ORBIT_AZURE: Palette = Palette {
    background: 0x181818,
    surface: 0x1F1F1F,
    surface_secondary: 0x242424,
    elevated: 0x292929,
    hover: 0x303030,
    active: 0x363636,
    // One step below the reference #303A4B so accent-colored labels on the
    // selected row still clear 4.5:1.
    selection: 0x2F3949,
    selection_fg: 0xE5E5E5,
    border: 0x343434,
    border_subtle: 0x292929,
    text: 0xE5E5E5,
    text_secondary: 0xA3A3A3,
    text_muted: 0x909090,
    text_disabled: 0x606060,
    success: 0x4CAF86,
    error: 0xE47478,
    error_hover: 0xE98A8D,
    error_fg: 0x171717,
    accent: 0x286CF0,
    accent_hover: 0x2D6FEF,
    accent_pressed: 0x1F5DD8,
    accent_fg: 0xFFFFFF,
    accent_text: 0x6EA0FF,
    focus_ring: 0x6EA0FF,
    action: 0x286CF0,
    action_hover: 0x2D6FEF,
    action_pressed: 0x1F5DD8,
    action_fg: 0xFFFFFF,
    warning: 0xE5AD55,
    info: 0x65A4E8,
    assistant_text: 0xE5E5E5,
    code_bg: 0x151515,
    code_text: 0xE5E5E5,
    syn_string: 0x4CAF86,
    syn_number: 0xE5AD55,
    syn_function: 0xE5E5E5,
    syn_type: 0xE5E5E5,
    syn_comment: 0xA3A3A3,
    syn_literal: 0xE2795B,
    syn_meta: 0xE16D70,
    syn_operator: 0x8A8A8A,
};

/// Orbit Azure Light: the same blue on a cool near-white canvas. Primary
/// buttons ride the accent, so hover and pressed step deeper.
const ORBIT_AZURE_LIGHT: Palette = Palette {
    background: 0xF5F5F5,
    surface: 0xFFFFFF,
    surface_secondary: 0xF8F8F8,
    elevated: 0xF0F0F0,
    hover: 0xEBEBEB,
    active: 0xE2E2E2,
    selection: 0xE8EEFA,
    selection_fg: 0x171717,
    border: 0xE5E5E5,
    border_subtle: 0xEEEEEE,
    text: 0x171717,
    text_secondary: 0x525252,
    text_muted: 0x696969,
    text_disabled: 0xA3A3A3,
    success: 0x247A56,
    error: 0xC03940,
    error_hover: 0xB83239,
    error_fg: 0xFFFFFF,
    accent: 0x286CF0,
    accent_hover: 0x1F5DD8,
    accent_pressed: 0x194BB5,
    accent_fg: 0xFFFFFF,
    accent_text: 0x1F5DD8,
    focus_ring: 0x286CF0,
    action: 0x286CF0,
    action_hover: 0x1F5DD8,
    action_pressed: 0x194BB5,
    action_fg: 0xFFFFFF,
    warning: 0x94600D,
    info: 0x2864A8,
    assistant_text: 0x171717,
    code_bg: 0xEEEEEE,
    code_text: 0x171717,
    syn_string: 0x297E48,
    syn_number: 0x99631E,
    syn_function: 0x171717,
    syn_type: 0x171717,
    syn_comment: 0x525252,
    syn_literal: 0xB55035,
    syn_meta: 0xC03940,
    syn_operator: 0x5F5F5F,
};

/// Orbit Light: the ember accent on a warm off-white canvas, with dark
/// primary buttons. Orbit Azure Light carries the blue design.
const ORBIT_LIGHT: Palette = Palette {
    background: 0xF6F5F6,
    surface: 0xF3F3F3,
    surface_secondary: 0xEFEFEF,
    elevated: 0xECECEC,
    hover: 0xE8E8E8,
    active: 0xE2E2E2,
    selection: 0xE5E5E5,
    selection_fg: 0x242424,
    border: 0xE2E2E2,
    border_subtle: 0xECECEC,
    text: 0x242424,
    text_secondary: 0x666666,
    text_muted: 0x696969,
    text_disabled: 0xA3A3A3,
    success: 0x277845,
    error: 0xB94038,
    error_hover: 0xB83239,
    error_fg: 0xFFFFFF,
    accent: 0xB55035,
    accent_hover: 0xA8482B,
    accent_pressed: 0x8F3A22,
    accent_fg: 0xFFFFFF,
    accent_text: 0xA4462C,
    focus_ring: 0x242424,
    action: 0x202227,
    action_hover: 0x34363C,
    action_pressed: 0x171A1E,
    action_fg: 0xF8F8F9,
    warning: 0x915E1C,
    info: 0x2C6E9B,
    assistant_text: 0x242424,
    code_bg: 0xE6E6E6,
    code_text: 0x242424,
    syn_string: 0x297E48,
    syn_number: 0x99631E,
    syn_function: 0x242424,
    syn_type: 0x242424,
    syn_comment: 0x666666,
    syn_literal: 0xB55035,
    syn_meta: 0xC3433B,
    syn_operator: 0x6F6F6F,
};

/// Orbit Paper: a cooler, neutral paper canvas in the Orbit grammar — a light
/// counterpart to Orbit's warm off-white, with the same ember accent.
const ORBIT_PAPER: Palette = Palette {
    background: 0xF7F7F8,
    surface: 0xF1F1F3,
    surface_secondary: 0xF4F4F6,
    elevated: 0xE9E9EC,
    hover: 0xE3E3E7,
    active: 0xDBDBE0,
    selection: 0xE3E9F7,
    selection_fg: 0x232327,
    border: 0xE0E0E4,
    border_subtle: 0xE8E8EB,
    text: 0x232327,
    text_secondary: 0x5A5A62,
    text_muted: 0x686870,
    text_disabled: 0xA0A0A8,
    success: 0x277644,
    error: 0xB63F37,
    error_hover: 0xB83239,
    error_fg: 0xF7F7F8,
    accent: 0xB55035,
    accent_hover: 0xB24E30,
    accent_pressed: 0x9A3F27,
    accent_fg: 0xF7F7F8,
    accent_text: 0x9A3F27,
    focus_ring: 0xB55035,
    action: 0xB55035,
    action_hover: 0xB24E30,
    action_pressed: 0x9A3F27,
    action_fg: 0xF7F7F8,
    warning: 0x8F5D1C,
    info: 0x2864A8,
    assistant_text: 0x232327,
    code_bg: 0xEDEDF0,
    code_text: 0x232327,
    syn_string: 0x297E48,
    syn_number: 0x99631E,
    syn_function: 0x232327,
    syn_type: 0x232327,
    syn_comment: 0x62626A,
    syn_literal: 0xB55035,
    syn_meta: 0xC3433B,
    syn_operator: 0x5F5F5F,
};

/// Orbit Contrast: a near-white, high-contrast light palette with deeper ink
/// and a stronger hairline, for maximum legibility.
const ORBIT_CONTRAST: Palette = Palette {
    background: 0xFFFFFF,
    surface: 0xF7F7F7,
    surface_secondary: 0xF5F5F5,
    elevated: 0xEDEDED,
    hover: 0xE6E6E6,
    active: 0xDBDBDB,
    selection: 0xE4EBFA,
    selection_fg: 0x0F0F0F,
    border: 0xC9C9C9,
    border_subtle: 0xE2E2E2,
    text: 0x0F0F0F,
    text_secondary: 0x454545,
    text_muted: 0x666666,
    text_disabled: 0x8A8A8A,
    success: 0x1E6B39,
    error: 0xB02A22,
    error_hover: 0xC84037,
    error_fg: 0xFFFFFF,
    accent: 0xA8431F,
    accent_hover: 0xBA5430,
    accent_pressed: 0x8E3214,
    accent_fg: 0xFFFFFF,
    accent_text: 0x8E3214,
    focus_ring: 0xA8431F,
    action: 0xA8431F,
    action_hover: 0xBA5430,
    action_pressed: 0x96391A,
    action_fg: 0xFFFFFF,
    warning: 0x8A5310,
    info: 0x1F5A99,
    assistant_text: 0x0F0F0F,
    code_bg: 0xF2F2F2,
    code_text: 0x0F0F0F,
    syn_string: 0x1E6B39,
    syn_number: 0x8A5310,
    syn_function: 0x0F0F0F,
    syn_type: 0x0F0F0F,
    syn_comment: 0x5A5A5A,
    syn_literal: 0xA8431F,
    syn_meta: 0xB02A22,
    syn_operator: 0x5F5F5F,
};

/// Flexoki Light — the canonical light counterpart to Flexoki.
const FLEXOKI_LIGHT: Palette = Palette {
    background: 0xFFFCF0,
    surface: 0xF2F0E5,
    surface_secondary: 0xF7F4E9,
    elevated: 0xE6E4D9,
    hover: 0xDAD8CE,
    active: 0xCECDC3,
    selection: 0xE7E4D6,
    selection_fg: 0x282726,
    border: 0xDAD8CE,
    border_subtle: 0xE6E4D9,
    text: 0x282726,
    text_secondary: 0x575653,
    text_muted: 0x66645E,
    text_disabled: 0xA09E98,
    success: 0x566C09,
    error: 0xAF3029,
    error_hover: 0xC4483F,
    error_fg: 0xFFFCF0,
    accent: 0xBC5215,
    accent_hover: 0xB24E0F,
    accent_pressed: 0xA84712,
    accent_fg: 0xFFFCF0,
    accent_text: 0xA84712,
    focus_ring: 0xBC5215,
    action: 0xBC5215,
    action_hover: 0xB24E0F,
    action_pressed: 0xA84712,
    action_fg: 0xFFFCF0,
    warning: 0x7E6000,
    info: 0x2A6B8A,
    assistant_text: 0x282726,
    code_bg: 0xF2F0E5,
    code_text: 0x282726,
    syn_string: 0x24837B,
    syn_number: 0x5E409D,
    syn_function: 0xBC5215,
    syn_type: 0xA07901,
    syn_comment: 0x878580,
    syn_literal: 0x5E409D,
    syn_meta: 0xA02F6F,
    syn_operator: 0xAF3029,
};

/// Flexoki — ported from OpenChamber's `flexoki-dark`.
const FLEXOKI: Palette = Palette {
    background: 0x171515,
    surface: 0x1C1B1A,
    surface_secondary: 0x201E1D,
    elevated: 0x252221,
    hover: 0x2D2B2B,
    active: 0x3E3B3B,
    selection: 0x26242C,
    selection_fg: 0xCECDC3,
    border: 0x2D2C2A,
    border_subtle: 0x252221,
    text: 0xCECDC3,
    text_secondary: 0x9A9893,
    text_muted: 0x8C8C8C,
    text_disabled: 0x5A5855,
    success: 0xA0AF54,
    error: 0xD96A60,
    error_hover: 0xD96D63,
    error_fg: 0x0B0B0B,
    accent: 0xDA702C,
    accent_hover: 0xDE8044,
    accent_pressed: 0xC76322,
    accent_fg: 0x171515,
    accent_text: 0xDA702C,
    focus_ring: 0xDA702C,
    action: 0xDA702C,
    action_hover: 0xDE8044,
    action_pressed: 0xC76322,
    action_fg: 0x171515,
    warning: 0xD0A215,
    info: 0x6B99B8,
    assistant_text: 0xCECDC3,
    code_bg: 0x1C1B1A,
    code_text: 0xCECDC3,
    syn_string: 0x3AA99F,
    syn_number: 0x8B7EC8,
    syn_function: 0xDA702C,
    syn_type: 0xD0A215,
    syn_comment: 0x878580,
    syn_literal: 0x8B7EC8,
    syn_meta: 0xDA702C,
    syn_operator: 0xD14D41,
};

fn palette(id: ThemeId) -> Palette {
    match id {
        ThemeId::Orbit => ORBIT,
        ThemeId::OrbitLight => ORBIT_LIGHT,
        ThemeId::OrbitPaper => ORBIT_PAPER,
        ThemeId::OrbitContrast => ORBIT_CONTRAST,
        ThemeId::OrbitDarker => ORBIT_DARKER,
        ThemeId::OrbitCobalt => ORBIT_COBALT,
        ThemeId::OrbitAzure => ORBIT_AZURE,
        ThemeId::Flexoki => FLEXOKI,
        ThemeId::FlexokiLight => FLEXOKI_LIGHT,
        ThemeId::OrbitAzureLight => ORBIT_AZURE_LIGHT,
    }
}

impl Theme {
    pub fn for_id(id: ThemeId) -> Self {
        Self::build(id.appearance(), UiPrefs::default(), id, palette(id))
    }

    #[cfg(test)]
    pub fn dark() -> Self {
        Self::for_id(ThemeId::OrbitAzure)
    }

    #[cfg(test)]
    pub fn light() -> Self {
        Self::for_id(ThemeId::OrbitAzureLight)
    }

    /// Keep the current UI customization across a palette switch.
    pub fn with_ui(mut self, ui: UiPrefs) -> Self {
        self.ui = ui;
        self
    }

    /// Scale an interface text size by the UI font size setting, the way
    /// rems resolve against the UI font size (`ui_font_size /
    /// DEFAULT_UI_FONT_SIZE`). Chrome sizes are authored at the 14px default,
    /// so at the default setting this resolves to the authored pixel value.
    pub fn ui_px(&self, value: f32) -> Pixels {
        px(value * (self.ui.ui_font_size / DEFAULT_UI_FONT_SIZE))
    }

    /// Scale an editor-surface size (code blocks, diffs) by the Editor font
    /// size setting (`editor_font_size / 13`, the default).
    pub fn code_px(&self, value: f32) -> Pixels {
        px(value * (self.ui.editor_font_size / 13.))
    }

    /// Scale a terminal/tool-output size by the Terminal font size setting
    /// (`terminal_font_size / 13`, the default).
    pub fn term_px(&self, value: f32) -> Pixels {
        px(value * (self.ui.terminal_font_size / 13.))
    }

    /// Paint color for one syntax token, shared by transcript code blocks
    /// and the review diff so both read as the same editor. Each palette
    /// carries its own `syn_*` colors (so a ported editor theme keeps its
    /// syntax identity); comments are explicitly legible on the code wash.
    pub fn token_color(self, class: TokenClass) -> Hsla {
        match class {
            TokenClass::Keyword => self.accent_text,
            TokenClass::Literal => self.syn_literal,
            TokenClass::String => self.syn_string,
            TokenClass::Comment => self.syn_comment,
            TokenClass::Number => self.syn_number,
            TokenClass::Type => self.syn_type,
            TokenClass::Function => self.syn_function,
            TokenClass::Meta => self.syn_meta,
            // Terminal vocabulary: command, flag, path, control operator.
            TokenClass::Command => self.accent_text,
            TokenClass::Flag => self.syn_number,
            TokenClass::Path => self.text_primary,
            TokenClass::Operator => self.syn_operator,
            TokenClass::Added => self.success,
            TokenClass::Removed => self.error,
        }
    }

    /// Paint color for a `/command` token in the composer. Commands are
    /// actions, so they take the readable accent foreground — the role
    /// terminal command words and syntax keywords already carry.
    pub fn mention_command(self) -> Hsla {
        self.accent_text
    }

    /// Paint color for an `@file` mention in the composer. Files take the
    /// accent's complement, so an `@reference` never reads as a `/command`
    /// while still riding the palette's tuned chroma and lightness.
    pub fn mention_file(self) -> Hsla {
        if self.accent_text.s < 0.15 {
            return self.accent_text.opacity(0.7);
        }
        let mut color = self.accent_text;
        color.h = (color.h + 0.5).fract();
        color.s = color.s.max(0.5);
        color
    }

    /// Build a full [`Theme`] from its appearance + raw palette tokens.
    fn build(mode: ThemeMode, ui: UiPrefs, id: ThemeId, p: Palette) -> Self {
        let text = hex(p.text);
        let accent = hex(p.accent);
        let (wash, wash_strong, border_strong, contact) = match mode {
            ThemeMode::Dark => (0.05, 0.09, 0.14, 0.32),
            ThemeMode::Light => (0.06, 0.10, 0.16, 0.10),
        };
        Self {
            theme_id: id,
            mode,
            ui,
            background: hex(p.background),
            surface: hex(p.surface),
            surface_secondary: hex(p.surface_secondary),
            surface_elevated: hex(p.elevated),
            surface_hover: hex(p.hover),
            surface_active: hex(p.active),
            border: hex(p.border),
            border_subtle: hex(p.border_subtle),
            text_primary: text,
            text_secondary: hex(p.text_secondary),
            text_muted: hex(p.text_muted),
            text_disabled: hex(p.text_disabled),
            accent,
            accent_hover: hex(p.accent_hover),
            accent_pressed: hex(p.accent_pressed),
            accent_fg: hex(p.accent_fg),
            accent_text: hex(p.accent_text),
            selection: hex(p.selection),
            selection_fg: hex(p.selection_fg),
            focus_ring: hex(p.focus_ring),
            action: hex(p.action),
            action_hover: hex(p.action_hover),
            action_pressed: hex(p.action_pressed),
            action_fg: hex(p.action_fg),
            success: hex(p.success),
            warning: hex(p.warning),
            error: hex(p.error),
            error_hover: hex(p.error_hover),
            error_fg: hex(p.error_fg),
            info: hex(p.info),
            toggle_knob: match mode {
                ThemeMode::Light => hex(0xFFFFFF),
                ThemeMode::Dark => text,
            },
            overlay: text.opacity(wash),
            overlay_strong: text.opacity(wash_strong),
            border_strong: text.opacity(border_strong),
            assistant_text: hex(p.assistant_text),
            code_bg: hex(p.code_bg),
            code_text: hex(p.code_text),
            syn_string: hex(p.syn_string),
            syn_number: hex(p.syn_number),
            syn_function: hex(p.syn_function),
            syn_type: hex(p.syn_type),
            syn_comment: hex(p.syn_comment),
            syn_literal: hex(p.syn_literal),
            syn_meta: hex(p.syn_meta),
            syn_operator: hex(p.syn_operator),
            inline_code_bg: accent.opacity(0.10),
            shadow_contact: hsla(0., 0., 0., contact),
        }
    }

    /// Soft lift for the floating composer: one tight contact layer. The
    /// hairline border carries definition, so no wide ambient here (a
    /// border under a broad shadow reads as a ghost card).
    pub fn composer_shadow(self) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: self.shadow_contact,
            offset: point(px(0.), px(2.)),
            blur_radius: px(10.),
            spread_radius: px(-3.),
        }]
    }

    /// Backdrop dim under a floating search surface (command palette):
    /// present enough to separate the card from the page, light enough that
    /// the workspace still reads through.
    pub fn scrim(self) -> Hsla {
        match self.mode {
            ThemeMode::Dark => hsla(0., 0., 0., 0.26),
            ThemeMode::Light => hsla(0., 0., 0., 0.14),
        }
    }

    /// Backdrop dim under a blocking modal (extension dialogs, custom-UI
    /// surfaces, the update prompt): a step stronger than [`Self::scrim`]
    /// because a modal *must* read as the only live layer.
    pub fn scrim_modal(self) -> Hsla {
        match self.mode {
            ThemeMode::Dark => hsla(0., 0., 0., 0.32),
            ThemeMode::Light => hsla(0., 0., 0., 0.18),
        }
    }

    /// Backdrop dim under the image lightbox: near-opaque, so a viewed
    /// attachment is what the eye lands on, not the transcript behind it.
    pub fn scrim_media(self) -> Hsla {
        match self.mode {
            ThemeMode::Dark => hsla(0., 0., 0., 0.72),
            ThemeMode::Light => hsla(0., 0., 0., 0.6),
        }
    }
}

fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

fn persist_path() -> PathBuf {
    crate::platform::home_dir().join(".orbit-pi").join("theme")
}

/// Install the persisted appearance (System by default) and font preferences.
pub fn init(cx: &mut App) {
    let prefs = AppearancePrefs::load();
    cx.set_global(prefs);
    cx.set_global(Theme::for_id(prefs.resolve(cx.window_appearance())).with_ui(UiPrefs::load()));
    let prefs = FontPrefs::load();
    *UI_FONT_FAMILY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = prefs.ui_font_family;
    *CODE_FONT_FAMILY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = prefs.code_font_family;
}

/// Current theme. Panics if [`init`] has not run.
pub fn get(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// Whether the user asked to reduce motion. Looping animations should render
/// static when this is true.
pub fn reduce_motion(cx: &App) -> bool {
    get(cx).ui.reduce_motion
}

/// Apply new UI / code font families to the statics and persist them.
pub fn set_font_prefs(prefs: FontPrefs) {
    *UI_FONT_FAMILY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = prefs.ui_font_family.clone();
    *CODE_FONT_FAMILY
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = prefs.code_font_family.clone();
    prefs.persist();
}

/// Apply new UI customization (language / font sizes), persist it, and
/// notify global observers — every paint site reads sizes through the
/// theme, so the change is live.
pub fn set_ui_prefs(cx: &mut App, ui: UiPrefs) {
    let previous = get(cx).ui;
    if previous == ui {
        return;
    }
    // Adopt the new locale before the theme global changes so the repaint
    // driven by that change already reads the right strings, then rebuild
    // the native menu bar (which only GPUI can refresh).
    crate::i18n::set_language(ui.language);
    crate::set_app_menus(cx);
    ui.persist();
    let id = get(cx).theme_id;
    cx.set_global(Theme::for_id(id).with_ui(ui));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_theme_id() {
        assert_eq!(ThemeId::parse("orbit"), Some(ThemeId::Orbit));
        assert_eq!(ThemeId::parse("orbit-light"), Some(ThemeId::OrbitLight));
        assert_eq!(ThemeId::parse("system"), None);
        assert_eq!(ThemeId::Flexoki.as_str(), "flexoki");
        assert_eq!(ThemeId::Flexoki.appearance(), ThemeMode::Dark);
        // Legacy theme names map onto Orbit / Orbit Light.
        assert_eq!(ThemeId::parse("dark"), Some(ThemeId::Orbit));
        assert_eq!(ThemeId::parse("light"), Some(ThemeId::OrbitLight));
        assert_eq!(ThemeId::parse("one-dark"), Some(ThemeId::Orbit));
        assert_eq!(ThemeId::parse("zedokai"), Some(ThemeId::Orbit));
        assert_eq!(ThemeId::parse("flexoki-dark"), Some(ThemeId::Flexoki));
        assert_eq!(ThemeId::parse("flexoki"), Some(ThemeId::Flexoki));
        assert_eq!(ThemeId::parse("flexoki-light"), Some(ThemeId::FlexokiLight));
        // Orbit-family keys.
        assert_eq!(ThemeId::parse("orbit-paper"), Some(ThemeId::OrbitPaper));
        assert_eq!(
            ThemeId::parse("orbit-contrast"),
            Some(ThemeId::OrbitContrast)
        );
        assert_eq!(ThemeId::parse("orbit-darker"), Some(ThemeId::OrbitDarker));
        assert_eq!(ThemeId::OrbitDarker.as_str(), "orbit-darker");
        assert_eq!(ThemeId::OrbitDarker.appearance(), ThemeMode::Dark);
        assert_eq!(ThemeId::parse("orbit-cobalt"), Some(ThemeId::OrbitCobalt));
        // The pre-rename key and the palette's origin still load.
        assert_eq!(ThemeId::parse("orbit-minimal"), Some(ThemeId::OrbitCobalt));
        assert_eq!(ThemeId::parse("codex"), Some(ThemeId::OrbitCobalt));
        assert_eq!(ThemeId::OrbitCobalt.as_str(), "orbit-cobalt");
        // Retired editor themes still load: dark onto Orbit, light onto Orbit
        // Light.
        assert_eq!(ThemeId::parse("dracula"), Some(ThemeId::Orbit));
        assert_eq!(ThemeId::parse("nord"), Some(ThemeId::Orbit));
        assert_eq!(ThemeId::parse("catppuccin-mocha"), Some(ThemeId::Orbit));
        assert_eq!(ThemeId::parse("one-light"), Some(ThemeId::OrbitLight));
        assert_eq!(ThemeId::parse("nord-snow-storm"), Some(ThemeId::OrbitLight));
        assert_eq!(ThemeId::parse("tokyo-night-day"), Some(ThemeId::OrbitLight));
        // Every selectable theme round-trips through its persisted key.
        for id in ThemeId::ALL {
            assert_eq!(ThemeId::parse(id.as_str()), Some(id));
        }
    }

    /// Composer mention tokens stay legible and distinct from each other in
    /// every palette, including the monochrome ones (Ashwood, Mono) where the
    /// split is by ink rather than hue.
    #[test]
    fn mention_token_colors_are_distinct_across_palettes() {
        for id in ThemeId::ALL {
            let theme = Theme::for_id(id);
            let command = theme.mention_command();
            let file = theme.mention_file();
            assert!(
                command != file,
                "mention colors collide in {id:?}: {command:?}"
            );
            assert!(command.a > 0., "command token is transparent in {id:?}");
            assert!(file.a > 0., "file token is transparent in {id:?}");
        }
    }

    /// The toggle knob must read on both the accent (on) and the raised track
    /// (off) in every palette. Light mode used `text` — a near-black dot on
    /// the pale track — so the knob is its own light token there.
    #[test]
    fn toggle_knob_stays_light_on_light_tracks() {
        for id in ThemeId::ALL {
            let theme = Theme::for_id(id);
            match theme.mode {
                ThemeMode::Light => assert!(
                    theme.toggle_knob.l > theme.surface_elevated.l,
                    "toggle knob is not lighter than the off track in {id:?}"
                ),
                ThemeMode::Dark => assert_eq!(
                    theme.toggle_knob, theme.text_primary,
                    "dark toggle knob drifted in {id:?}"
                ),
            }
        }
    }

    /// Every shipped palette is selectable under its name.
    #[test]
    fn theme_catalog_labels() {
        let labels: Vec<&str> = ThemeId::ALL.iter().map(|id| id.label()).collect();
        assert_eq!(
            labels,
            vec![
                "Orbit",
                "Orbit Darker",
                "Orbit Cobalt",
                "Orbit Azure",
                "Flexoki",
                "Orbit Light",
                "Orbit Azure Light",
                "Orbit Paper",
                "Orbit Contrast",
                "Flexoki Light",
            ]
        );
    }

    fn rel_lum(v: u32) -> f64 {
        let f = |c: u32| {
            let c = c as f64 / 255.;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f((v >> 16) & 0xff) + 0.7152 * f((v >> 8) & 0xff) + 0.0722 * f(v & 0xff)
    }

    fn contrast(a: u32, b: u32) -> f64 {
        let (l1, l2) = (rel_lum(a), rel_lum(b));
        let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Every palette must keep its foreground readable against the surfaces
    /// it is painted on. The bars are the delivered floor: ink clears AA on
    /// the canvas, the accent stays legible as text (`accent_text`, the role
    /// links and inline code read) rather than only as a wash, and every state
    /// pair (selected row, primary button) holds at 4.5:1.
    #[test]
    fn every_theme_has_readable_foregrounds() {
        for id in ThemeId::ALL {
            let p = palette(id);
            let c = |a, b| contrast(a, b);
            assert!(
                c(p.text, p.background) >= 7.0,
                "{id:?}: body text on canvas"
            );
            assert!(c(p.text, p.elevated) >= 4.5, "{id:?}: body text on raised");
            assert!(
                c(p.text_secondary, p.background) >= 4.0,
                "{id:?}: secondary text"
            );
            assert!(
                c(p.text_muted, p.background) >= 3.0,
                "{id:?}: tertiary/label text"
            );
            assert!(
                c(p.accent_text, p.background) >= 4.5,
                "{id:?}: accent as text"
            );
            assert!(
                c(p.selection_fg, p.selection) >= 4.5,
                "{id:?}: selected row"
            );
            assert!(c(p.action_fg, p.action) >= 4.5, "{id:?}: primary button");
            assert!(c(p.accent_fg, p.accent) >= 4.5, "{id:?}: accent fill");
            assert!(
                c(p.border, p.background) >= 1.15,
                "{id:?}: hairline vs canvas"
            );
            // Syntax must stay legible on the code wash, comments included.
            for (label, token) in [
                ("string", p.syn_string),
                ("number", p.syn_number),
                ("function", p.syn_function),
                ("type", p.syn_type),
                ("literal", p.syn_literal),
                ("meta", p.syn_meta),
                ("operator", p.syn_operator),
            ] {
                assert!(
                    c(token, p.code_bg) >= 3.5,
                    "{id:?}: syntax {label} on code wash"
                );
            }
            assert!(
                c(p.syn_comment, p.code_bg) >= 3.0,
                "{id:?}: comment on code wash"
            );
            // Status and diff colors are painted on the dark work surfaces as
            // badge dots and +/− counts. AA-large is the floor there; labels
            // ride ink, so the hue is reinforcement, never the only signal.
            if id.appearance() == ThemeMode::Dark {
                for (label, token) in [
                    ("success", p.success),
                    ("danger", p.error),
                    ("caution", p.warning),
                ] {
                    assert!(
                        c(token, p.background) >= 3.0,
                        "{id:?}: status {label} on canvas"
                    );
                    assert!(
                        c(token, p.elevated) >= 2.5,
                        "{id:?}: status {label} on raised"
                    );
                }
            }
            // A run's failed/cancelled badge and a deleted diff line both use
            // red; the badge's dot + label is what separates them, never the
            // hue alone. Guard the one collision that would erase that:
            // `error` (failure) must not equal `text_muted` (cancel).
            assert_ne!(
                p.error, p.text_muted,
                "{id:?}: failure red collides with the muted cancel ink"
            );
        }
    }

    /// The two default appearances ship the agreed neutral/blue design
    /// tokens. `text_muted` and the primary hover step are the two values
    /// adjusted from the first draft: the draft muted gray failed AA on
    /// elevated cards, and the draft hover blue failed AA with a white label.
    #[test]
    fn default_palettes_ship_the_agreed_tokens() {
        let dark = Theme::dark();
        let light = Theme::light();
        let expected_dark = [
            ("background", dark.background, 0x181818),
            ("surface", dark.surface, 0x1F1F1F),
            ("surface_secondary", dark.surface_secondary, 0x242424),
            ("surface_elevated", dark.surface_elevated, 0x292929),
            ("surface_hover", dark.surface_hover, 0x303030),
            ("surface_active", dark.surface_active, 0x363636),
            ("border", dark.border, 0x343434),
            ("border_subtle", dark.border_subtle, 0x292929),
            ("text_primary", dark.text_primary, 0xE5E5E5),
            ("text_secondary", dark.text_secondary, 0xA3A3A3),
            ("text_muted", dark.text_muted, 0x909090),
            ("text_disabled", dark.text_disabled, 0x606060),
            ("accent", dark.accent, 0x286CF0),
            ("accent_hover", dark.accent_hover, 0x2D6FEF),
            ("accent_pressed", dark.accent_pressed, 0x1F5DD8),
            ("selection", dark.selection, 0x2F3949),
            ("focus_ring", dark.focus_ring, 0x6EA0FF),
            ("success", dark.success, 0x4CAF86),
            ("warning", dark.warning, 0xE5AD55),
            ("error", dark.error, 0xE47478),
            ("info", dark.info, 0x65A4E8),
        ];
        for (label, actual, expected) in expected_dark {
            assert_eq!(actual, hex(expected), "dark {label}");
        }
        let expected_light = [
            ("background", light.background, 0xF5F5F5),
            ("surface", light.surface, 0xFFFFFF),
            ("surface_secondary", light.surface_secondary, 0xF8F8F8),
            ("surface_elevated", light.surface_elevated, 0xF0F0F0),
            ("surface_hover", light.surface_hover, 0xEBEBEB),
            ("surface_active", light.surface_active, 0xE2E2E2),
            ("border", light.border, 0xE5E5E5),
            ("border_subtle", light.border_subtle, 0xEEEEEE),
            ("text_primary", light.text_primary, 0x171717),
            ("text_secondary", light.text_secondary, 0x525252),
            ("text_muted", light.text_muted, 0x696969),
            ("text_disabled", light.text_disabled, 0xA3A3A3),
            ("accent", light.accent, 0x286CF0),
            ("accent_hover", light.accent_hover, 0x1F5DD8),
            ("accent_pressed", light.accent_pressed, 0x194BB5),
            ("selection", light.selection, 0xE8EEFA),
            ("focus_ring", light.focus_ring, 0x286CF0),
            ("success", light.success, 0x247A56),
            ("warning", light.warning, 0x94600D),
            ("error", light.error, 0xC03940),
            ("info", light.info, 0x2864A8),
        ];
        for (label, actual, expected) in expected_light {
            assert_eq!(actual, hex(expected), "light {label}");
        }
        // The canvas is charcoal / warm-neutral, never pure black or white.
        assert_ne!(dark.background, hex(0x000000));
        assert_ne!(light.background, hex(0xFFFFFF));
        assert_ne!(dark.text_primary, light.text_primary);
    }

    /// Every palette resolves the complete semantic vocabulary. A missing or
    /// transparent role is a defect in the palette, not in the caller.
    #[test]
    fn every_palette_exposes_the_semantic_vocabulary() {
        for id in ThemeId::ALL {
            let t = Theme::for_id(id);
            let colors = [
                ("background", t.background),
                ("surface", t.surface),
                ("surface_secondary", t.surface_secondary),
                ("surface_elevated", t.surface_elevated),
                ("surface_hover", t.surface_hover),
                ("surface_active", t.surface_active),
                ("border", t.border),
                ("border_subtle", t.border_subtle),
                ("text_primary", t.text_primary),
                ("text_secondary", t.text_secondary),
                ("text_muted", t.text_muted),
                ("text_disabled", t.text_disabled),
                ("accent", t.accent),
                ("accent_hover", t.accent_hover),
                ("accent_pressed", t.accent_pressed),
                ("accent_fg", t.accent_fg),
                ("accent_text", t.accent_text),
                ("selection", t.selection),
                ("selection_fg", t.selection_fg),
                ("focus_ring", t.focus_ring),
                ("action", t.action),
                ("action_hover", t.action_hover),
                ("action_pressed", t.action_pressed),
                ("action_fg", t.action_fg),
                ("success", t.success),
                ("warning", t.warning),
                ("error", t.error),
                ("error_hover", t.error_hover),
                ("error_fg", t.error_fg),
                ("info", t.info),
            ];
            for (name, color) in colors {
                assert!(color.a > 0., "{id:?}: {name} is transparent");
            }
            assert_ne!(t.selection, t.surface_active, "{id:?}: selection role");
            assert_ne!(t.text_disabled, t.text_muted, "{id:?}: disabled role");
            assert_ne!(t.accent_text, t.text_primary, "{id:?}: accent text role");
        }
    }

    /// Muted metadata is painted on cards, not only on the canvas; it must
    /// still clear AA there in every palette. The accent foreground and all
    /// three primary-button states carry the same floor.
    #[test]
    fn interactive_and_muted_foregrounds_clear_aa() {
        for id in ThemeId::ALL {
            let p = palette(id);
            let c = |a, b| contrast(a, b);
            assert!(
                c(p.text_muted, p.elevated) >= 4.5,
                "{id:?}: muted text on elevated surface"
            );
            assert!(
                c(p.accent_text, p.background) >= 4.5,
                "{id:?}: accent text on canvas"
            );
            assert!(
                c(p.accent_text, p.elevated) >= 4.5,
                "{id:?}: accent text on elevated surface"
            );
            assert!(
                c(p.accent_fg, p.accent) >= 4.5,
                "{id:?}: accent fill foreground"
            );
            assert!(
                c(p.action_fg, p.action) >= 4.5,
                "{id:?}: primary action rest state"
            );
            assert!(
                c(p.action_fg, p.action_hover) >= 4.5,
                "{id:?}: primary action hover state"
            );
            assert!(
                c(p.action_fg, p.action_pressed) >= 4.5,
                "{id:?}: primary action pressed state"
            );
            assert!(
                c(p.error_fg, p.error) >= 4.5,
                "{id:?}: error fill foreground"
            );
            assert!(
                c(p.error_fg, p.error_hover) >= 4.5,
                "{id:?}: error fill hover foreground"
            );
            assert!(
                c(p.focus_ring, p.background) >= 3.0,
                "{id:?}: focus ring on canvas"
            );
            assert!(
                c(p.focus_ring, p.elevated) >= 3.0,
                "{id:?}: focus ring on elevated surface"
            );
            // The primary-action fill is its own role (a palette may key it to
            // a neutral step rather than its accent), so its ink needs its own
            // floor. Hover is transient and not held here.
            assert!(
                c(p.action_fg, p.action) >= 4.5,
                "{id:?}: primary action rest state"
            );
        }
    }

    /// A selected row paints its metadata with `text_secondary`, never
    /// `text_muted`: the selection wash sits between the canvas and a card,
    /// where muted ink drops below AA. The same holds for the accent text on
    /// the selection wash. Guard both so a palette edit cannot silently
    /// reintroduce low-contrast metadata on the active row.
    #[test]
    fn selection_rows_keep_their_metadata_readable() {
        for id in ThemeId::ALL {
            let p = palette(id);
            let c = |a, b| contrast(a, b);
            assert!(
                c(p.text_secondary, p.selection) >= 4.5,
                "{id:?}: secondary text on the selection wash"
            );
            assert!(
                c(p.selection_fg, p.selection) >= 7.0,
                "{id:?}: primary ink on the selection wash"
            );
            assert!(
                c(p.accent_text, p.selection) >= 4.5,
                "{id:?}: accent text on the selection wash"
            );
        }
    }

    /// One surface grammar for every palette: on dark, each step up the ramp is
    /// lighter than the one below (canvas → raised → hover → active); on light
    /// it is the inverse. Menus paint on `surface_elevated`, which `Theme::build`
    /// sets to the palette's raised step by construction, and the primary
    /// button always answers a hover with a visible step.
    #[test]
    fn every_theme_holds_its_surface_grammar() {
        for id in ThemeId::ALL {
            let p = palette(id);
            let l = |v: u32| rel_lum(v);
            match id.appearance() {
                ThemeMode::Dark => {
                    assert!(
                        l(p.elevated) > l(p.background),
                        "{id:?}: raised must lift off the canvas"
                    );
                    assert!(
                        l(p.elevated) >= l(p.surface),
                        "{id:?}: raised must float above the chrome plane"
                    );
                    assert!(
                        l(p.hover) >= l(p.elevated),
                        "{id:?}: hover must sit above raised"
                    );
                    assert!(
                        l(p.active) >= l(p.hover),
                        "{id:?}: active must sit above hover"
                    );
                }
                ThemeMode::Light => {
                    assert!(
                        l(p.elevated) < l(p.background),
                        "{id:?}: raised must recess in light mode"
                    );
                    assert!(
                        l(p.elevated) <= l(p.surface),
                        "{id:?}: raised must sit below the light chrome plane"
                    );
                    assert!(
                        l(p.hover) <= l(p.elevated),
                        "{id:?}: hover must recess below raised"
                    );
                    assert!(
                        l(p.active) <= l(p.hover),
                        "{id:?}: active must recess below hover"
                    );
                }
            }
            assert_ne!(
                p.accent, p.accent_hover,
                "{id:?}: primary button hover feedback"
            );
            // The accent is the lone interactive hue; it can never double as
            // a status color or a second accent would read as state.
            assert_ne!(p.accent, p.success, "{id:?}: accent collides with success");
            assert_ne!(p.accent, p.error, "{id:?}: accent collides with error");
            assert_ne!(p.accent, p.warning, "{id:?}: accent collides with warning");
        }
    }

    /// Every palette must carry a real accent hue (the composer derives
    /// `@file` by inverting it), so a grey accent is a defect.
    #[test]
    fn accent_carries_chroma() {
        for id in ThemeId::ALL {
            let accent = Theme::for_id(id).accent;
            assert!(
                accent.s > 0.05,
                "{id:?}: accent is nearly achromatic (s = {:.3})",
                accent.s
            );
        }
    }

    #[test]
    fn theme_appearance_matches_label() {
        assert_eq!(ThemeId::Orbit.appearance(), ThemeMode::Dark);
        assert_eq!(ThemeId::OrbitLight.appearance(), ThemeMode::Light);
        assert_eq!(ThemeId::OrbitPaper.appearance(), ThemeMode::Light);
        assert_eq!(ThemeId::OrbitContrast.appearance(), ThemeMode::Light);
        assert_eq!(ThemeId::OrbitDarker.appearance(), ThemeMode::Dark);
        assert_eq!(ThemeId::OrbitCobalt.appearance(), ThemeMode::Dark);
        assert_eq!(ThemeId::FlexokiLight.appearance(), ThemeMode::Light);
        assert_eq!(ThemeId::OrbitAzure.appearance(), ThemeMode::Dark);
        assert_eq!(ThemeId::OrbitAzureLight.appearance(), ThemeMode::Light);
        assert_eq!(ThemeId::Orbit.label(), "Orbit");
        assert_eq!(ThemeId::OrbitLight.label(), "Orbit Light");
        assert_eq!(ThemeId::OrbitPaper.label(), "Orbit Paper");
        assert_eq!(ThemeId::OrbitContrast.label(), "Orbit Contrast");
        assert_eq!(ThemeId::OrbitDarker.label(), "Orbit Darker");
        assert_eq!(ThemeId::OrbitCobalt.label(), "Orbit Cobalt");
        assert_eq!(ThemeId::Flexoki.label(), "Flexoki");
        assert_eq!(ThemeId::FlexokiLight.label(), "Flexoki Light");
        assert_eq!(ThemeId::OrbitAzure.label(), "Orbit Azure");
        assert_eq!(ThemeId::OrbitAzureLight.label(), "Orbit Azure Light");
        // The reverted default is the ember identity; Azure is the blue design.
        assert_eq!(
            Theme::for_id(ThemeId::Orbit).accent,
            hex(0xE2795B),
            "Orbit keeps its ember accent"
        );
        assert_eq!(
            Theme::for_id(ThemeId::OrbitAzure).accent,
            hex(0x286CF0),
            "Orbit Azure keeps the blue accent"
        );
    }

    /// Flexoki is ported, so it keeps its own syntax colors rather than
    /// borrowing Orbit's semantic green/amber reuse. If the two ever collapse
    /// onto the same tokens, the theme has lost what makes it Flexoki.
    #[test]
    fn flexoki_keeps_its_own_syntax_palette() {
        let flexoki = Theme::for_id(ThemeId::Flexoki);
        let orbit = Theme::dark();
        assert_eq!(flexoki.mode, ThemeMode::Dark);
        assert_ne!(
            flexoki.token_color(TokenClass::String),
            orbit.token_color(TokenClass::String)
        );
        assert_eq!(flexoki.token_color(TokenClass::String), hex(0x3AA99F));
        assert_eq!(flexoki.token_color(TokenClass::Function), hex(0xDA702C));
    }

    #[test]
    fn parse_language() {
        assert_eq!(Language::parse("system"), Some(Language::System));
        assert_eq!(Language::parse("en"), Some(Language::English));
        assert_eq!(Language::parse("zh-CN"), Some(Language::SimplifiedChinese));
        assert_eq!(Language::parse("fr"), Some(Language::French));
        assert_eq!(Language::parse("xx"), None);
        assert_eq!(Language::English.label(), "English");
    }

    #[test]
    fn ui_prefs_parse_rejects_out_of_range_sizes() {
        use serde_json::json;
        let prefs = UiPrefs::from_value(&json!({
            "language": "en",
            "ui_font_size": 500,
            "terminal_font_size": 15,
            "editor_font_size": 12,
            "spacing_density": 100,
        }));
        // 500px isn't selectable — falls back to the default; the rest apply.
        assert_eq!(prefs.language, Language::English);
        assert_eq!(prefs.ui_font_size, 14.);
        assert_eq!(prefs.terminal_font_size, 15.);
        assert_eq!(prefs.editor_font_size, 12.);
        assert_eq!(prefs.spacing_density, 100);
        // Malformed payload → all defaults.
        assert_eq!(
            UiPrefs::from_value(&json!({"language": 3})),
            UiPrefs::default()
        );
        // Legacy percentage / px keys migrate onto the px scales.
        let legacy = UiPrefs::from_value(&json!({
            "interface_scale": 115,
            "code_font_size": 15,
        }));
        assert_eq!(legacy.ui_font_size, 16.);
        assert_eq!(legacy.editor_font_size, 15.);
    }

    #[test]
    fn composer_send_mode_round_trips_in_ui_prefs() {
        use crate::composer_send::SendMode;
        for mode in [SendMode::FollowUp, SendMode::Steer] {
            let prefs = UiPrefs {
                composer_send_mode: mode,
                language: Language::French,
                ui_font_size: 16.,
                ..UiPrefs::default()
            };
            let json = prefs.to_value().to_string();
            assert_eq!(
                UiPrefs::from_value(&serde_json::from_str(&json).unwrap()),
                prefs
            );
        }
    }

    #[test]
    fn composer_send_mode_missing_or_invalid_keeps_legacy_default() {
        use crate::composer_send::SendMode;
        for value in [
            serde_json::json!({}),
            serde_json::json!({"composer_send_mode": "unknown"}),
            serde_json::json!({"composer_send_mode": 42}),
            serde_json::json!({"composer_send_mode": null}),
        ] {
            assert_eq!(
                UiPrefs::from_value(&value).composer_send_mode,
                SendMode::FollowUp
            );
        }
    }

    #[test]
    fn font_scale_helpers() {
        fn close(a: Pixels, b: Pixels) -> bool {
            (f32::from(a) - f32::from(b)).abs() < 0.01
        }
        let mut theme = Theme::dark();
        // Defaults are unscaled.
        assert_eq!(theme.ui_px(14.), px(14.));
        assert_eq!(theme.code_px(13.), px(13.));
        assert_eq!(theme.term_px(13.), px(13.));
        theme.ui.ui_font_size = 18.;
        theme.ui.editor_font_size = 11.;
        theme.ui.terminal_font_size = 15.;
        theme.ui.spacing_density = 75;
        assert_eq!(theme.ui_px(14.), px(18.));
        assert!(close(theme.code_px(13.), px(11.)));
        assert!(close(theme.term_px(13.), px(15.)));
        // A palette switch carries UI customization over.
        let light = Theme::for_id(ThemeId::OrbitLight).with_ui(theme.ui);
        assert_eq!(light.theme_id, ThemeId::OrbitLight);
        assert_eq!(light.ui, theme.ui);
    }

    #[test]
    fn palettes_differ() {
        let dark = Theme::dark();
        let light = Theme::light();
        assert_ne!(dark.background, light.background);
        assert_ne!(dark.text_primary, light.text_primary);
        assert_eq!(dark.theme_id, ThemeId::OrbitAzure);
        assert_eq!(light.theme_id, ThemeId::OrbitAzureLight);
        // Every palette is distinct from the default.
        for id in ThemeId::ALL {
            assert_eq!(Theme::for_id(id).theme_id, id);
        }
        // Orbit is dark; Orbit Light is light.
        for id in ThemeId::ALL {
            assert_eq!(Theme::for_id(id).mode, id.appearance());
        }
    }
}
