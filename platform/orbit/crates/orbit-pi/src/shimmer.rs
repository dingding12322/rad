//! The running-session shimmer: a highlight band swept left-to-right across a
//! run of glyphs. Shared by the sidebar's running session rows and the
//! transcript's live "Working for" indicator — the same motion for the same
//! signal, defined once.

use gpui::{
    point, relative, App, Bounds, ContentMask, Element, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, Pixels, SharedString, Style, TextRun, Window, WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

/// Shimmer highlight width in pixels (shadcn's band is roughly 3ch + 40px).
const SHIMMER_BAND_PX: f32 = 56.0;

/// A single-line text whose ink catches a moving highlight. GPUI has no
/// `background-clip: text`, so the sweep is painted run-by-run — the shaped
/// layout is unchanged by the per-run colors. It ellipsizes like gpui's own
/// text element, so a long string still ends in `…`.
pub(crate) struct ShimmerText {
    pub(crate) text: SharedString,
    pub(crate) base: Hsla,
    pub(crate) highlight: Hsla,
    pub(crate) phase: f32,
}

impl ShimmerText {
    pub(crate) fn new(text: impl Into<SharedString>, base: Hsla, highlight: Hsla) -> Self {
        Self {
            text: text.into(),
            base,
            highlight,
            phase: 0.0,
        }
    }
}

impl IntoElement for ShimmerText {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ShimmerText {
    type RequestLayoutState = ();
    type PrepaintState = Vec<WrappedLine>;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let line_height = window.line_height();
        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        // Truncate to the row width with gpui's own helper, so the ellipsis
        // matches the rest of the UI. These runs are only shaped for glyph
        // geometry; the painted colors are rebuilt below.
        let mut runs = vec![run(self.text.len(), self.base)];
        let display = window
            .text_system()
            .line_wrapper(font.clone(), font_size)
            .truncate_line(self.text.clone(), bounds.size.width, "\u{2026}", &mut runs);

        let Ok(measured) =
            window
                .text_system()
                .shape_text(display.clone(), font_size, &runs, None, None)
        else {
            return Vec::new();
        };
        let Some(line) = measured.first() else {
            return Vec::new();
        };
        let total = f32::from(line.width()).max(1.);
        let half_band = (SHIMMER_BAND_PX / total).clamp(0.08, 0.5);
        let center = -half_band + self.phase * (1.0 + 2.0 * half_band);

        let mut colored = Vec::new();
        for (ix, grapheme) in display.grapheme_indices(true) {
            let x = line
                .position_for_index(ix, line_height)
                .map(|p| f32::from(p.x))
                .unwrap_or(0.);
            let t = band_intensity(x / total, center, half_band);
            colored.push(run(
                grapheme.len(),
                blend_hsla(self.base, self.highlight, t),
            ));
        }

        window
            .text_system()
            .shape_text(display, font_size, &colored, None, None)
            .map(|lines| lines.to_vec())
            .unwrap_or_default()
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = window.line_height();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for line in prepaint.iter() {
                line.paint(
                    point(bounds.origin.x, bounds.origin.y),
                    line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
            }
        });
    }
}

/// Smooth band falloff: 1 at the sweep center, easing to 0 at its edges.
fn band_intensity(pos: f32, center: f32, half_band: f32) -> f32 {
    let d = (pos - center).abs();
    if d >= half_band {
        return 0.0;
    }
    let t = 1.0 - d / half_band;
    t * t * (3.0 - 2.0 * t)
}

/// Mix two colors by `t` (0 = `a`, 1 = `b`). Hue takes the shortest arc, so
/// the sweep from gray ink to the accent never loops the color wheel.
fn blend_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    let mut dh = b.h - a.h;
    if dh > 0.5 {
        dh -= 1.0;
    } else if dh < -0.5 {
        dh += 1.0;
    }
    Hsla {
        h: (a.h + dh * t).rem_euclid(1.0),
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_band_peaks_at_center_and_fades_out() {
        // Full highlight at the sweep center, nothing past the band edge.
        assert_eq!(band_intensity(0.5, 0.5, 0.2), 1.0);
        assert_eq!(band_intensity(0.25, 0.5, 0.2), 0.0);
        // Smooth, monotonic falloff toward the edge.
        let near = band_intensity(0.45, 0.5, 0.2);
        let far = band_intensity(0.38, 0.5, 0.2);
        assert!(near > far && far > 0.0);
    }

    #[test]
    fn blend_hsla_hits_both_ends() {
        let ink = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.6,
            a: 1.0,
        };
        let accent = Hsla {
            h: 0.04,
            s: 0.7,
            l: 0.62,
            a: 1.0,
        };
        assert_eq!(blend_hsla(ink, accent, 0.0), ink);
        assert_eq!(blend_hsla(ink, accent, 1.0), accent);
        // Halfway gains saturation and meets the midpoint lightness.
        let mid = blend_hsla(ink, accent, 0.5);
        assert!(mid.s > ink.s && mid.s < accent.s);
        assert!((mid.l - 0.61).abs() < 1e-6);
    }
}
