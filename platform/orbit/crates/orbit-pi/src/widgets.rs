//! Extension widgets — pi's `setWidget` text blocks rendered natively.
//!
//! Extensions push a small block of text lines above or below the editor
//! (`ctx.ui.setWidget`). Over RPC only the string-array form is delivered —
//! component factories are dropped by pi — and those strings carry the SGR
//! sequences `theme.fg` / `bold` / `strikethrough` emit. This module holds the
//! per-session widget list and turns each raw line into styled runs, so an
//! extension's status block appears in Orbit instead of nowhere.
//!
//! Rendering is deliberately not a terminal: the lines are parsed down to
//! text + attributes (foreground, bold, …) and drawn with GPUI text, so the
//! block belongs to Orbit's surface while keeping the extension's intent.

use alacritty_terminal::vte::ansi::Rgb;
use gpui::{
    px, Font, FontFeatures, FontStyle, FontWeight, Hsla, StrikethroughStyle, TextRun,
    UnderlineStyle,
};

use crate::terminal::{rgb_to_hsla, Palette};
use crate::theme::{self, Theme};

/// Where a widget sits relative to the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetPlacement {
    AboveEditor,
    BelowEditor,
}

/// One extension widget, keyed by the extension's `widgetKey`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionWidget {
    pub key: String,
    pub placement: WidgetPlacement,
    /// Raw lines as sent by pi, SGR sequences intact.
    pub lines: Vec<String>,
}

/// A styled span of one widget line.
#[derive(Debug, Clone, Default, PartialEq)]
struct StyledSegment {
    text: String,
    fg: Option<Hsla>,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
    dim: bool,
}

/// The monospace face widgets render in.
pub fn widget_font() -> Font {
    Font {
        family: theme::code_font_family(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// Parse one raw line into its display text and GPUI runs. An empty line
/// yields a single space so its row keeps height.
pub fn styled_line(line: &str, theme: &Theme, font: &Font) -> (String, Vec<TextRun>) {
    let segments = parse_line(line, theme);
    let mut display = String::new();
    let mut runs: Vec<TextRun> = Vec::with_capacity(segments.len());
    for segment in &segments {
        if segment.text.is_empty() {
            continue;
        }
        runs.push(segment.run(theme, font));
        display.push_str(&segment.text);
    }
    if display.is_empty() {
        // An empty line still occupies a row. The fallback space needs a run:
        // `StyledText::with_runs` panics unless the runs cover the text
        // exactly, so a bare space with no runs aborts the whole render.
        let space = StyledSegment {
            text: " ".to_string(),
            ..Default::default()
        };
        runs.push(space.run(theme, font));
        display.push(' ');
    }
    (display, runs)
}

impl StyledSegment {
    fn run(&self, theme: &Theme, font: &Font) -> TextRun {
        let mut fg = self.fg.unwrap_or(theme.code_text);
        let mut bg = self.bg;
        if self.inverse {
            let swapped_fg = bg.unwrap_or(theme.code_bg);
            bg = Some(self.fg.unwrap_or(theme.code_text));
            fg = swapped_fg;
        }
        if self.dim {
            fg = fg.opacity(0.7);
        }
        TextRun {
            len: self.text.len(),
            font: Font {
                weight: if self.bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                style: if self.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
                ..font.clone()
            },
            color: fg,
            background_color: bg,
            underline: self.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            }),
            strikethrough: self.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
        }
    }
}

/// Current SGR state while scanning a line.
#[derive(Default, Clone, Copy)]
struct SgrState {
    fg: Option<Hsla>,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    inverse: bool,
    dim: bool,
}

/// Strip ANSI sequences from `line`, splitting it into styled segments.
fn parse_line(line: &str, theme: &Theme) -> Vec<StyledSegment> {
    let palette = Palette::from_theme(theme);
    let mut state = SgrState::default();
    let mut segments = Vec::new();
    let mut text = String::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            flush(&mut text, &state, &mut segments);
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'[' => {
                    // CSI: params up to a final byte in 0x40..=0x7e.
                    let start = i + 1;
                    let mut j = start;
                    while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                        j += 1;
                    }
                    if j < bytes.len() {
                        // Only SGR (`m`) carries style; cursor/mode sequences
                        // are terminal-only and simply dropped.
                        if bytes[j] == b'm' {
                            apply_sgr(&line[start..j], &mut state, &palette);
                        }
                        i = j + 1;
                    } else {
                        i = bytes.len();
                    }
                }
                // OSC, APC, DCS, PM, SOS: skip to BEL or ST.
                b']' | b'_' | b'P' | b'^' | b'X' => {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                // Two-byte escape (`ESC ( B`, …): drop the introducer only.
                _ => i += 1,
            }
        } else {
            let len = utf8_char_len(bytes[i]);
            let end = (i + len).min(bytes.len());
            text.push_str(&line[i..end]);
            i = end;
        }
    }
    flush(&mut text, &state, &mut segments);
    segments
}

fn flush(text: &mut String, state: &SgrState, segments: &mut Vec<StyledSegment>) {
    if text.is_empty() {
        return;
    }
    segments.push(StyledSegment {
        text: std::mem::take(text),
        fg: state.fg,
        bg: state.bg,
        bold: state.bold,
        italic: state.italic,
        underline: state.underline,
        strikethrough: state.strikethrough,
        inverse: state.inverse,
        dim: state.dim,
    });
}

/// Apply one `ESC [ … m` parameter list to the running style.
fn apply_sgr(params: &str, state: &mut SgrState, palette: &Palette) {
    let nums: Vec<u32> = params
        .split(';')
        .map(|part| part.parse::<u32>().unwrap_or(0))
        .collect();
    let mut i = 0;
    while i < nums.len() {
        let n = nums[i];
        match n {
            0 => *state = SgrState::default(),
            1 => state.bold = true,
            2 => state.dim = true,
            3 => state.italic = true,
            4 => state.underline = true,
            7 => state.inverse = true,
            9 => state.strikethrough = true,
            22 => {
                state.bold = false;
                state.dim = false;
            }
            23 => state.italic = false,
            24 => state.underline = false,
            27 => state.inverse = false,
            29 => state.strikethrough = false,
            30..=37 => state.fg = Some(palette.indexed((n - 30) as usize)),
            39 => state.fg = None,
            40..=47 => state.bg = Some(palette.indexed((n - 40) as usize)),
            49 => state.bg = None,
            90..=97 => state.fg = Some(palette.indexed((n - 90 + 8) as usize)),
            100..=107 => state.bg = Some(palette.indexed((n - 100 + 8) as usize)),
            38 | 48 => {
                let is_fg = n == 38;
                match nums.get(i + 1).copied() {
                    Some(5) => {
                        if let Some(index) = nums.get(i + 2) {
                            let color = Some(palette.indexed(*index as usize));
                            if is_fg {
                                state.fg = color;
                            } else {
                                state.bg = color;
                            }
                            i += 2;
                        }
                    }
                    Some(2) => {
                        if let (Some(r), Some(g), Some(b)) =
                            (nums.get(i + 2), nums.get(i + 3), nums.get(i + 4))
                        {
                            let color = Some(rgb_to_hsla(Rgb {
                                r: *r as u8,
                                g: *g as u8,
                                b: *b as u8,
                            }));
                            if is_fg {
                                state.fg = color;
                            } else {
                                state.bg = color;
                            }
                            i += 4;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// Byte length of the UTF-8 character starting with `byte`. A continuation or
/// invalid lead advances one byte so the scan can never stall.
fn utf8_char_len(byte: u8) -> usize {
    match byte {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeId;

    fn theme() -> Theme {
        Theme::for_id(ThemeId::Orbit)
    }

    fn runs(line: &str) -> (String, Vec<TextRun>) {
        styled_line(line, &theme(), &widget_font())
    }

    #[test]
    fn plain_text_passes_through() {
        let (text, runs) = runs("Loading…");
        assert_eq!(text, "Loading…");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len, "Loading…".len());
        assert!(runs[0].underline.is_none());
    }

    #[test]
    fn fg_256_color_becomes_a_colored_run() {
        let (text, runs) = runs("\x1b[38;5;2magain\x1b[39m");
        assert_eq!(text, "again");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].color, Palette::from_theme(&theme()).indexed(2));
    }

    #[test]
    fn truecolor_is_decoded() {
        let (text, runs) = runs("\x1b[38;2;255;0;0mred\x1b[39m");
        assert_eq!(text, "red");
        assert_eq!(runs[0].color, rgb_to_hsla(Rgb { r: 255, g: 0, b: 0 }));
    }

    #[test]
    fn attributes_apply_and_reset() {
        let (text, runs) = runs("\x1b[1m\x1b[4m\x1b[9mhot\x1b[0mcold");
        assert_eq!(text, "hotcold");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert!(runs[0].underline.is_some());
        assert!(runs[0].strikethrough.is_some());
        assert_eq!(runs[1].font.weight, FontWeight::NORMAL);
        assert!(runs[1].underline.is_none());
    }

    #[test]
    fn combined_parameters_are_honored() {
        let (text, runs) = runs("\x1b[1;3;38;5;1mwhy");
        assert_eq!(text, "why");
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert_eq!(runs[0].font.style, FontStyle::Italic);
        assert_eq!(runs[0].color, Palette::from_theme(&theme()).indexed(1));
    }

    #[test]
    fn osc_and_cursor_marker_are_stripped() {
        // Clipboard OSC (52) and pi's APC cursor marker must not leak as text.
        let (text, _) = runs("\x1b]52;c;YWJj\x07visible");
        assert_eq!(text, "visible");
        let (text, _) = runs("a\x1b_pi:c\x07b");
        assert_eq!(text, "ab");
    }

    #[test]
    fn empty_line_keeps_its_row() {
        let (text, runs) = runs("");
        assert_eq!(text, " ");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len, 1);
    }

    /// `StyledText::with_runs` panics unless the runs consume the text exactly,
    /// so every line — blank, SGR-only, or mixed — must come back covered.
    #[test]
    fn runs_always_cover_the_display_text() {
        for line in [
            "",
            "\x1b[0m",
            "\x1b[1mbold\x1b[0m plain",
            "\x1b[38;5;2mé中\x1b[39m",
            "\x1b]52;c;YWJj\x07",
            "   ",
        ] {
            let (text, runs) = runs(line);
            let covered: usize = runs.iter().map(|run| run.len).sum();
            assert_eq!(covered, text.len(), "runs must cover {line:?}");
        }
    }
}
