//! The new-task page's backdrop: a living ordered-dither field.
//!
//! The page is a landing moment — an empty column, a headline, a folder row,
//! and the composer — so its background has one job: give the whole page the
//! texture of printed paper, densest where the eye lands and thinning into the
//! window's edges, without ever competing with the text on it.
//!
//! It is the same ordered-dither language the photo backdrop already speaks
//! ([`crate::dither`]: the Aceternity-style Bayer pass), reduced to its
//! simplest form: no image, just a soft density field quantized through an 8×8
//! Bayer matrix into four dot sizes. Eight-by-eight rather than four-by-four
//! because the coarser matrix reads as a crosshatch on a field this large; 64
//! thresholds give a ramp smooth enough to look printed rather than patterned.
//!
//! The field covers the page edge to edge — a slow two-axis wave over a gentle
//! downward bias, so the densest band sits behind the composer rather than
//! across the headline. An elliptical vignette calms the window's edges and
//! corners without ever emptying them: the screen is the page's surface, so it
//! runs to the frame, and the drift is visible everywhere the reader looks.
//!
//! **The mark is screened into it.** Where the app monogram sits, the same
//! screen takes over the shape: the glyphs hold a quiet density at rest, and a
//! soft band of light travels across them, so each letter fills with the
//! largest dots in turn and settles back. The mask is sampled from
//! [`crate::app_icon::MARK_PNG`], not redrawn by hand — the backdrop cannot
//! drift from the mark it claims to show, and a `cargo run` build never grows
//! the Alpha pill.
//!
//! **The motion is the dither.** The wave drifts, so dots cross their
//! thresholds a few at a time — roughly 2% of the screen a second, a handful of
//! cells per frame — and the page appears to breathe instead of strobing.
//! Nothing moves; only the screen fills in and out, and the light crosses the
//! mark about every twenty seconds. Reduce Motion freezes it on the frame it
//! was showing, which is a perfectly good poster of itself.
//!
//! A configured background image (Settings → Appearance) replaces this
//! entirely — see [`crate::dither`].

use gpui::{
    canvas, div, fill, point, px, size, Background, Bounds, Corners, Hsla, IntoElement,
    ParentElement, Pixels, Styled, Window,
};

use crate::theme::{Theme, ThemeMode};

/// Dither cell pitch, in logical pixels: the screen's ruling. Fine enough to
/// read as texture at arm's length and to keep the monogram's counters open;
/// coarse enough that the four dot sizes stay distinct. A full 1280×820 layer
/// is ~142 × 91 cells, so the field issues roughly thirteen thousand quads per
/// frame at the default UI scale.
const CELL: f32 = 9.;
/// Dot levels the field quantizes into. Level 0 is bare canvas.
const LEVELS: usize = 4;
/// Dot radius per level, as a fraction of [`CELL`]. Small on purpose: the dot
/// grid this replaced read as texture, and a halftone carrying much more ink
/// than that stops being texture and becomes a pattern. The steps are uneven
/// because a halftone's area grows with the square of the radius.
const RADII: [f32; LEVELS] = [0., 0.10, 0.15, 0.20];
/// Ink strength per level, as a fraction of the layer's ink colour. Even steps
/// keep the gradient smooth once the radii are uneven.
const INK: [f32; LEVELS] = [0., 0.45, 0.70, 1.];
/// The field's resting density: how much of the page carries ink before the
/// wave moves it. Low enough that the screen stays a texture at every point of
/// its drift — the wave supplies the variation, the base supplies the floor.
const DENSITY: f32 = 0.30;
/// The slow wave's amplitude. With [`DENSITY`] it spans roughly 0.08–0.52, so
/// the screen ranges from sparse stipple to small dense dots and never reaches
/// a solid field.
const WAVE: f32 = 0.22;
/// Downward bias: the densest band sits below the middle, behind the composer,
/// keeping the headline's band the calmest part of the page.
const BIAS: f32 = 0.10;
/// How much of the field survives at the vignette's darkest point. The screen is
/// a surface: it runs to the window's edges and into its corners, quieter there
/// but never absent — a feathered edge read as the backdrop stopping short.
const EDGE_FLOOR: f32 = 0.65;

/// The mark's width as a fraction of the layer, capped in pixels — the
/// monogram has to be wide enough for its counters to survive a 9px screen
/// (below roughly 200px it stops being a wordmark), and small enough to stay a
/// watermark on a wide display.
const MARK_W: f32 = 0.36;
const MARK_MAX_W: f32 = 560.;
const MARK_MIN_W: f32 = 200.;
/// Where the mark's centre sits, as a fraction of the layer's height: the gap
/// between the empty-state column and the floating composer.
const MARK_CY: f32 = 0.57;
/// Density inside the glyphs when the light is elsewhere — faint enough to be
/// a watermark, present enough that the wordmark is always legible.
const MARK_REST: f32 = 0.18;
/// Density inside a glyph the light is crossing: the largest dot the screen
/// has.
const MARK_LIT: f32 = 0.85;
/// The mark's ink as a fraction of the page's. The watermark is screened in the
/// same colour, one step quieter than the grain around it.
const MARK_INK: f32 = 0.85;
/// Seconds for the light to cross the mark and come round again.
const SWEEP_PERIOD: f32 = 20.;
/// The light band's half-width in mark coordinates, and where it fades to
/// nothing. Wide and soft: a spotlight, not a scanline.
const LIT_IN: f32 = 0.30;
const LIT_OUT: f32 = 0.55;
/// The mask's resolution, in glyph-local columns. Enough to keep the O and P
/// counters open at any layer size; one bilinear sample per cell is the whole
/// cost.
const MASK_COLS: usize = 64;

/// Standard 8×8 Bayer matrix, 0..63. Thresholds are sampled at
/// `(value + 0.5) / 64` so a perfectly even field dithers to exactly its own
/// coverage instead of biasing toward one level.
const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// The backdrop layer, absolutely filling its parent, for the given elapsed
/// `phase` in seconds. `phase` freezes at any value for Reduce Motion.
pub fn layer(theme: Theme, phase: f32) -> impl IntoElement + use<> {
    // The ink is the tertiary text weight, as the dot grid always was: paper
    // texture, not a second content layer. Light mode needs more of it to
    // carry the same weight on a bright canvas.
    let ink = match theme.mode {
        ThemeMode::Light => theme.text_muted.opacity(0.62),
        ThemeMode::Dark => theme.text_muted.opacity(0.55),
    };
    // The cell pitch rides the UI scale so the screen's ruling keeps its
    // proportion on a dense or a roomy interface.
    let pitch = f32::from(theme.ui_px(CELL));
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| paint(bounds, phase, pitch, ink, window),
            )
            .size_full(),
        )
}

/// One cell's worth of the layer: the density the screen quantizes, and how
/// much of the mark covers it (for the mark's quieter ink).
struct Sample {
    value: f32,
    mark: f32,
}

/// Paint the field: one quad per cell that carries ink.
fn paint(bounds: Bounds<Pixels>, phase: f32, pitch: f32, ink: Hsla, window: &mut Window) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width < pitch || height < pitch {
        return;
    }
    let frame = Frame::new(width, height);
    let columns = (width / pitch).ceil() as usize;
    let rows = (height / pitch).ceil() as usize;

    for row in 0..rows {
        for column in 0..columns {
            let u = (column as f32 + 0.5) * pitch / width;
            let v = (row as f32 + 0.5) * pitch / height;
            let cell = sample(u, v, phase, &frame);
            if cell.value <= 0. {
                continue;
            }
            let level = dither(cell.value, BAYER[row % 8][column % 8]);
            let radius = pitch * RADII[level];
            if radius <= 0. {
                continue;
            }
            // The mark paints in the same colour, one step quieter than the
            // grain it sits in: a watermark, never a second layer of ink.
            let alpha = ink.opacity(INK[level] * (1. + (MARK_INK - 1.) * cell.mark));
            let center = point(
                bounds.origin.x + px((column as f32 + 0.5) * pitch),
                bounds.origin.y + px((row as f32 + 0.5) * pitch),
            );
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(center.x - px(radius), center.y - px(radius)),
                        size(px(radius * 2.), px(radius * 2.)),
                    ),
                    Background::from(alpha),
                )
                .corner_radii(Corners::all(px(radius))),
            );
        }
    }
}

/// Where one layer puts things, in normalized layer coordinates.
struct Frame {
    /// The mark's rect as `(min_u, min_v, max_u, max_v)`, or `None` when the
    /// layer is too small to hold the monogram without turning it to mush.
    mark: Option<(f32, f32, f32, f32)>,
}

impl Frame {
    fn new(width: f32, height: f32) -> Self {
        // Aspect-preserving: the mask is a wide block, and a stretched
        // wordmark is worse than no wordmark.
        let aspect = Mark::get().aspect();
        let mark_w = (width * MARK_W).clamp(0., MARK_MAX_W);
        let mark_h = mark_w / aspect;
        let mark_w = mark_w.clamp(0., width * 0.8);
        // The mark needs room for itself plus the empty-state column above and
        // the composer below; past this it would grow into both.
        let fits = mark_w >= MARK_MIN_W && mark_h < height * 0.45;
        if !fits {
            return Self { mark: None };
        }
        let half_u = mark_w / width / 2.;
        let half_v = mark_h / height / 2.;
        Self {
            mark: Some((
                0.5 - half_u,
                MARK_CY - half_v,
                0.5 + half_u,
                MARK_CY + half_v,
            )),
        }
    }
}

/// The layer's density field plus the mark at normalized layer coordinates
/// `(u, v)` and elapsed `phase` seconds.
///
/// Two slow waves (a broad one across the page, a slower one running against it
/// so the pattern never repeats on an obvious axis) ride a base density and a
/// downward bias. The ellipse vignette and the four edge fades keep the screen
/// from ending on a hard line where the layer meets the window's hairlines.
/// Inside the mark the page's density gives way to the glyph's own, lit by the
/// travelling band.
fn sample(u: f32, v: f32, phase: f32, frame: &Frame) -> Sample {
    let tau = std::f32::consts::TAU;
    let broad = (tau * (u * 0.75 + phase * 0.016)).sin() * (tau * (v * 0.55 - phase * 0.011)).cos();
    let cross = (tau * ((u * 0.45 - v * 0.32) + phase * 0.007)).sin()
        * (tau * ((v * 0.38 + u * 0.21) - phase * 0.005)).cos();
    let wave = 0.68 * broad + 0.32 * cross;
    // Elliptical vignette, centred just under the page's middle so the densest
    // band reads as light falling on the composer. Floored, not feathered: the
    // corners stay screened at EDGE_FLOOR of the centre's density.
    let vignette = smoothstep(1.15, 0.05, ((u - 0.5) / 0.80).hypot((v - 0.52) / 0.86));
    let vignette = EDGE_FLOOR + (1. - EDGE_FLOOR) * vignette;
    let page = ((DENSITY + WAVE * wave + BIAS * (v - 0.5)) * vignette).clamp(0., 1.);

    let Some((min_u, min_v, max_u, max_v)) = frame.mark else {
        return Sample {
            value: page,
            mark: 0.,
        };
    };
    let mark_u = (u - min_u) / (max_u - min_u);
    let mark_v = (v - min_v) / (max_v - min_v);
    if !(0. ..=1.).contains(&mark_u) || !(0. ..=1.).contains(&mark_v) {
        return Sample {
            value: page,
            mark: 0.,
        };
    }
    let coverage = Mark::get().sample(mark_u, mark_v);
    if coverage <= 0. {
        return Sample {
            value: page,
            mark: 0.,
        };
    }
    // The light travels from left of the mark to right of it and starts again,
    // pausing off both ends so the mark is fully dark and fully lit each pass.
    let sweep = -0.4 + (phase / SWEEP_PERIOD).fract() * 1.8;
    let lit = smoothstep(LIT_OUT, LIT_IN, (mark_u - sweep).abs());
    let glyph = MARK_REST + (MARK_LIT - MARK_REST) * lit;
    Sample {
        value: (page * (1. - coverage) + glyph * coverage).clamp(0., 1.),
        mark: coverage,
    }
}

/// The app monogram as a dense coverage mask, derived once from the embedded
/// production mark — never redrawn by hand, so it cannot drift from the icon.
struct Mark {
    columns: usize,
    rows: usize,
    /// Row-major coverage, 0..=1.
    cells: Vec<f32>,
}

impl Mark {
    fn get() -> &'static Self {
        static MARK: std::sync::OnceLock<Mark> = std::sync::OnceLock::new();
        MARK.get_or_init(Mark::derive)
    }

    fn aspect(&self) -> f32 {
        self.columns as f32 / self.rows as f32
    }

    /// Bilinear coverage at glyph-local `(u, v)`, sharpened so a glyph's
    /// counter stays open instead of greying out at the mask's resolution.
    fn sample(&self, u: f32, v: f32) -> f32 {
        let x = u * self.columns as f32 - 0.5;
        let y = v * self.rows as f32 - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let at = |column: f32, row: f32| {
            if column < 0. || row < 0. || column >= self.columns as f32 || row >= self.rows as f32 {
                return 0.;
            }
            self.cells[row as usize * self.columns + column as usize]
        };
        let raw = at(x0, y0) * (1. - fx) * (1. - fy)
            + at(x0 + 1., y0) * fx * (1. - fy)
            + at(x0, y0 + 1.) * (1. - fx) * fy
            + at(x0 + 1., y0 + 1.) * fx * fy;
        smoothstep(0.35, 0.65, raw)
    }

    /// Threshold the mark, drop the plate's thin highlight ring, crop to the
    /// glyphs, and box-average down to [`MASK_COLS`] columns.
    fn derive() -> Self {
        // The mark is compile-time embedded: a decode failure is a broken
        // build, not a runtime condition to degrade around.
        let image = image::load_from_memory(crate::app_icon::MARK_PNG)
            .expect("the embedded mark PNG must decode");
        let luma = image.to_luma8();
        let (width, height) = luma.dimensions();
        let on: Vec<bool> = luma.pixels().map(|pixel| pixel.0[0] > 128).collect();
        let index = |x: u32, y: u32| (y * width + x) as usize;
        // Two separable passes of a 7-wide "all neighbours on" test: the plate's
        // bevel ring is a couple of pixels thick and vanishes, while the chunky
        // glyphs survive.
        let radius = 3;
        let mut horizontal = vec![false; on.len()];
        for y in 0..height {
            for x in 0..width {
                let mut keep = on[index(x, y)];
                let mut step = 0;
                while keep && step <= radius {
                    if x < step || x + step >= width {
                        keep = false;
                        break;
                    }
                    keep = on[index(x - step, y)] && on[index(x + step, y)];
                    step += 1;
                }
                horizontal[index(x, y)] = keep;
            }
        }
        let mut eroded = vec![false; on.len()];
        for y in 0..height {
            for x in 0..width {
                let mut keep = horizontal[index(x, y)];
                let mut step = 0;
                while keep && step <= radius {
                    if y < step || y + step >= height {
                        keep = false;
                        break;
                    }
                    keep = horizontal[index(x, y - step)] && horizontal[index(x, y + step)];
                    step += 1;
                }
                eroded[index(x, y)] = keep;
            }
        }
        // The glyphs' bounding box.
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, 0, 0);
        for y in 0..height {
            for x in 0..width {
                if eroded[index(x, y)] {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(
            min_x < max_x && min_y < max_y,
            "the embedded mark must erode to a non-empty glyph box"
        );
        let box_w = max_x - min_x + 1;
        let box_h = max_y - min_y + 1;
        let columns = MASK_COLS;
        let rows = ((columns as f32 * box_h as f32 / box_w as f32).round() as usize).max(1);
        let mut cells = vec![0.; columns * rows];
        for row in 0..rows {
            for column in 0..columns {
                // The source rect this mask cell covers, averaged by area.
                let x0 = min_x + (column as f32 * box_w as f32 / columns as f32) as u32;
                let x1 = min_x
                    + (((column + 1) as f32 * box_w as f32 / columns as f32).ceil() as u32)
                        .min(box_w);
                let y0 = min_y + (row as f32 * box_h as f32 / rows as f32) as u32;
                let y1 = min_y
                    + (((row + 1) as f32 * box_h as f32 / rows as f32).ceil() as u32).min(box_h);
                let (mut on_cells, mut count) = (0., 0.);
                for y in y0..y1.max(y0 + 1) {
                    for x in x0..x1.max(x0 + 1) {
                        if x <= max_x && y <= max_y && eroded[index(x, y)] {
                            on_cells += 1.;
                        }
                        count += 1.;
                    }
                }
                cells[row * columns + column] = if count > 0. { on_cells / count } else { 0. };
            }
        }
        Self {
            columns,
            rows,
            cells,
        }
    }
}

/// Quantize one cell's field value through the ordered matrix: the whole part
/// picks the level, the fractional part decides whether this cell rounds up.
fn dither(value: f32, threshold: u8) -> usize {
    let scaled = value * LEVELS as f32;
    let base = scaled.floor();
    let threshold = (threshold as f32 + 0.5) / 64.;
    let level = if scaled - base > threshold {
        base + 1.
    } else {
        base
    };
    (level.max(0.) as usize).min(LEVELS - 1)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::prelude::*;

    /// A layer shape the tests exercise the field on: wide enough to carry the
    /// mark, which is what a running window looks like.
    fn frame() -> Frame {
        Frame::new(1280., 820.)
    }

    /// The page's own density, with no mark in the way.
    fn page(u: f32, v: f32, phase: f32) -> f32 {
        sample(u, v, phase, &Frame { mark: None }).value
    }

    /// The mask comes from the shipped mark, so it must look like the mark: a
    /// wide block, mostly ink, with the O's counter open.
    #[test]
    fn the_mark_mask_is_the_production_monogram() {
        let mark = Mark::get();
        assert!(mark.columns >= 48, "{} columns", mark.columns);
        assert_eq!(mark.columns, MASK_COLS);
        assert!(
            (2.0..2.4).contains(&mark.aspect()),
            "the monogram is a wide block: aspect {}",
            mark.aspect()
        );
        let coverage: f32 = mark.cells.iter().sum::<f32>() / mark.cells.len() as f32;
        assert!(
            (0.55..0.85).contains(&coverage),
            "the glyphs cover most of their box: {coverage}"
        );
        // The O's counter is a hole in the ink; the P's stem is solid.
        assert!(
            mark.sample(0.21, 0.45) < 0.5,
            "the O's counter stays open: {}",
            mark.sample(0.21, 0.45)
        );
        assert!(
            mark.sample(0.56, 0.90) > 0.5,
            "the P's stem is solid: {}",
            mark.sample(0.56, 0.90)
        );
    }

    /// The screen covers the page edge to edge. Two regressions live here: a
    /// field that only rises off the bottom edge (bare canvas above it), and a
    /// field feathered away from the window's borders (a backdrop that stops
    /// short of the frame). Every point of the layer — corners included — must
    /// carry ink at every phase.
    #[test]
    fn the_field_covers_the_whole_page_edge_to_edge() {
        let frame = frame();
        // The outermost dotted cell sits half a pitch in from the border, so
        // these are the layer's first and last cells.
        for u in [0.004, 0.05, 0.25, 0.5, 0.75, 0.95, 0.996] {
            for v in [0.004, 0.05, 0.25, 0.5, 0.75, 0.95, 0.996] {
                for phase in [0., 7., 31.5, 300.] {
                    let value = sample(u, v, phase, &frame).value;
                    assert!(
                        value > 0.02,
                        "no ink at ({u}, {v}) on phase {phase}: {value}"
                    );
                }
            }
        }
    }

    /// ...and the drift is visible at the frame too, not just in the middle: a
    /// corner's cell changes with the phase like any other.
    #[test]
    fn the_field_drifts_at_the_edges() {
        let frame = frame();
        let screen = |u: f32, v: f32, phase: f32| {
            let value = sample(u, v, phase, &frame).value;
            (0..64)
                .map(|threshold| dither(value, threshold))
                .collect::<Vec<_>>()
        };
        for (u, v) in [
            (0.004, 0.004),
            (0.996, 0.004),
            (0.004, 0.996),
            (0.996, 0.996),
            (0.004, 0.5),
            (0.996, 0.5),
            (0.5, 0.004),
            (0.5, 0.996),
        ] {
            // The two waves beat against each other on a minute's timescale, so
            // an edge cell is checked across five minutes rather than one — the
            // point is that it moves at all, not that it moves fast.
            let seen: std::collections::BTreeSet<Vec<usize>> =
                (0..60).map(|step| screen(u, v, step as f32 * 5.)).collect();
            assert!(
                seen.len() > 1,
                "the edge at ({u}, {v}) never changes: {seen:?}"
            );
        }
    }

    /// ...and it is a screen, not a wash: most cells carry a dot, so the page
    /// reads as halftone rather than a few scattered specks.
    #[test]
    fn most_of_the_page_is_screened() {
        let frame = frame();
        let mut marked = 0;
        let mut total = 0;
        for row in 0..60 {
            for column in 0..60 {
                let u = 0.15 + (column as f32 + 0.5) * 0.70 / 60.;
                let v = 0.12 + (row as f32 + 0.5) * 0.80 / 60.;
                let value = sample(u, v, 0., &frame).value;
                if dither(value, BAYER[row % 8][column % 8]) > 0 {
                    marked += 1;
                }
                total += 1;
            }
        }
        let ratio = marked as f32 / total as f32;
        assert!(ratio > 0.6, "only {ratio} of the page is screened");
    }

    /// The page never saturates: the composer's band is dense but the page
    /// never turns into a solid slab of ink wherever the wave peaks.
    #[test]
    fn the_field_never_saturates() {
        for phase in [0., 7., 31.5, 300.] {
            for row in 0..40 {
                for column in 0..40 {
                    let u = (column as f32 + 0.5) / 40.;
                    let v = (row as f32 + 0.5) / 40.;
                    let value = page(u, v, phase);
                    assert!(
                        value < 0.60,
                        "solid ink at ({u}, {v}) on phase {phase}: {value}"
                    );
                }
            }
        }
    }

    /// The screen's weight tracks the field: a stronger field paints bigger
    /// dots, never more of them (this is a halftone, so the cell is always
    /// marked once it carries ink and the *area* is what ramps).
    #[test]
    fn the_screen_weight_tracks_the_field() {
        let mean_ink = |value: f32| {
            let total: f32 = BAYER
                .iter()
                .flatten()
                .map(|&threshold| INK[dither(value, threshold)])
                .sum();
            total / 64.
        };
        assert_eq!(mean_ink(0.), 0., "a bare field paints nothing");
        assert_eq!(
            mean_ink(1.),
            INK[LEVELS - 1],
            "a full field is solid at the top level"
        );
        let mut previous = 0.;
        let mut levels = std::collections::BTreeSet::new();
        for step in 0..=40 {
            let value = step as f32 / 40.;
            let ink = mean_ink(value);
            assert!(
                ink >= previous - f32::EPSILON,
                "ink dipped at {value}: {ink} after {previous}"
            );
            previous = ink;
            for &threshold in BAYER.iter().flatten() {
                levels.insert(dither(value, threshold));
            }
        }
        assert_eq!(levels.len(), LEVELS, "every level is reachable: {levels:?}");
    }

    /// The point of the effect: two phases are not the same screen. If this
    /// ever fails the backdrop is a still painting with a timer attached.
    #[test]
    fn a_later_phase_redraws_part_of_the_screen() {
        let screen = |phase: f32| -> Vec<usize> {
            let mut cells = Vec::new();
            for row in 0..80 {
                for column in 0..80 {
                    let u = (column as f32 + 0.5) / 80.;
                    let v = (row as f32 + 0.5) / 80.;
                    cells.push(dither(page(u, v, phase), BAYER[row % 8][column % 8]));
                }
            }
            cells
        };
        let first = screen(0.);
        let second = screen(1.);
        let changed = first
            .iter()
            .zip(second.iter())
            .filter(|(a, b)| a != b)
            .count();
        // Measured on the real field: ~1% of cells per second. Assert a band,
        // not a number — "some, and not all at once".
        let ratio = changed as f32 / first.len() as f32;
        assert!(
            (0.002..0.04).contains(&ratio),
            "{ratio} of the screen changed in one second"
        );
    }

    /// The mark is screened in, and the light crosses it: the lit side carries
    /// the largest dot while the far side rests, and half a sweep later it is
    /// the other way round.
    #[test]
    fn the_light_crosses_the_mark() {
        let frame = frame();
        let (min_u, min_v, max_u, max_v) = frame.mark.expect("a window this size carries the mark");
        let at = |mark_u: f32, phase: f32| {
            let u = min_u + (max_u - min_u) * mark_u;
            let v = min_v + (max_v - min_v) * 0.2;
            sample(u, v, phase, &frame)
        };
        // The sweep starts left of the mark at phase 0 and reaches its middle
        // after a quarter of the period; the glyphs' left and right ends take
        // turns being lit.
        let early_left = at(0.05, 5.);
        let early_right = at(0.95, 5.);
        assert!(
            early_left.value > early_right.value,
            "the left glyph lights first: {} vs {}",
            early_left.value,
            early_right.value
        );
        assert!(
            early_left.mark > 0.5 && early_right.mark > 0.5,
            "both samples sit on glyphs"
        );
        let late_left = at(0.05, 15.);
        let late_right = at(0.95, 15.);
        assert!(
            late_right.value > late_left.value,
            "and the right glyph lights later: {} vs {}",
            late_right.value,
            late_left.value
        );
        // Resting glyphs stay visible: a watermark, not an absence.
        assert!(
            early_right.value >= MARK_REST * 0.9,
            "the unlit glyph keeps its rest density: {}",
            early_right.value
        );
    }

    /// A layer too small for the monogram drops it rather than stretching a
    /// wordmark into mush — the screen alone is still a complete backdrop.
    #[test]
    fn a_small_layer_has_no_mark() {
        assert!(Frame::new(360., 300.).mark.is_none(), "narrow");
        assert!(Frame::new(1280., 360.).mark.is_none(), "short");
        assert!(
            Frame::new(1280., 820.).mark.is_some(),
            "a window carries it"
        );
    }

    /// The layer paints into a real window: a panic inside the quad path (a
    /// bad bounds, a zero radius) would otherwise only show up on screen.
    #[gpui::test]
    fn the_layer_draws(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|_, cx| cx.set_global(Theme::for_id(crate::theme::ThemeId::Orbit)));
        let view = cx.update(|_, cx| {
            cx.new(|_| {
                struct Layer(Theme);
                impl gpui::Render for Layer {
                    fn render(
                        &mut self,
                        _: &mut Window,
                        _: &mut gpui::Context<Self>,
                    ) -> impl IntoElement {
                        div().size_full().child(super::layer(self.0, 12.5))
                    }
                }
                Layer(Theme::for_id(crate::theme::ThemeId::Orbit))
            })
        });
        let _ = cx.draw(point(px(0.), px(0.)), size(px(900.), px(620.)), |_, _| {
            view.clone()
        });
    }
}
