//! Page background: an optional image, ordered-dithered (the Aceternity
//! "dither shader" trick, CPU-side) and shown behind the whole window.
//!
//! GPUI has no public fragment-shader hook, so the pass runs once when the
//! setting changes: the chosen file is copied into `~/.orbit-pi/`, downscaled,
//! dithered cell-by-cell in **color** — each channel quantized with the Bayer
//! threshold, like the shader's `colorMode: "original"` — and cached as a
//! [`RenderImage`]. Painting it afterwards is a plain `img()` — no per-frame
//! work. The view adds the dim + bottom fade that drops it into the page.
//!
//! Three tuning values ride beside the source name in `background.json`, all
//! exposed in Settings → Appearance: the blur applied after the pass, the
//! dither cell edge (the pixelation size), and the bottom fade. Blur is a
//! continuous slider ([`BLUR_MAX`], snapped to [`BLUR_STEP`]); the cell edge
//! and fade are named presets, so the pixelation and the drop stay looks
//! rather than a parameter dump. The defaults reproduce the original
//! hardcoded pass exactly, except the fade: it now washes rather than
//! curtains, so the picture stays visible behind and below the floating
//! composer.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use gpui::RenderImage;
use image::{Frame, RgbaImage};

/// Default dither cell size, in source pixels — the shader's `gridSize`.
const GRID: u32 = 4;
/// Default bottom-fade preset. A wash rather than a curtain, so the picture
/// reads below the floating composer on a fresh install.
const DEFAULT_FADE: usize = 2;
/// Longest source edge kept; bigger only costs dither time.
const MAX_EDGE: u32 = 1600;
/// Brightness multiplier: the backdrop sits behind the whole UI, so it runs
/// dim — the chroma, not the brightness, is what carries it.
const DIM: f32 = 0.70;
/// Quantization levels per channel (3 → 27 colors, 4 → 64). Fewer levels
/// make the ordered pattern read more strongly.
const LEVELS: u32 = 4;
/// Contrast stretch applied before quantizing, so photos use the full ramp
/// instead of dithering everything to the middle.
const CONTRAST: f32 = 1.3;

/// Bayer 4×4 ordered-dither thresholds (0..15).
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

// ── tuning ─────────────────────────────────────────────────────────────────

/// Backdrop tuning, persisted beside the source name in `background.json`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Gaussian sigma applied *after* the dither pass; 0 disables blur.
    pub blur: f32,
    /// Dither cell edge, in source pixels — the pixelation size.
    pub cell: u32,
    /// Bottom fade height, as a fraction of the page.
    pub fade: f32,
}

/// Highest gaussian sigma the blur slider reaches. The blur runs *after* the
/// dither, so it softens the ordered pattern into a glow rather than being
/// re-hardened by quantization — the value stays modest because a little
/// goes a long way once the cells are blended.
pub const BLUR_MAX: f32 = 6.;
/// Blur quantization step, in sigma. The slider scrubs continuously but the
/// dither pass only rebuilds when the value crosses a step, so a drag stays
/// cheap and a persisted value can never land between two painted ones.
pub const BLUR_STEP: f32 = 0.5;
/// Dither cell edges, in source pixels. Small reads as halftone, large as
/// chunky pixels.
pub const CELL_SIZES: [u32; 5] = [2, 3, 4, 6, 8];
/// Pixel-size preset names.
pub const CELL_LABELS: [&str; 5] = ["Fine", "Small", "Medium", "Large", "Coarse"];
/// Bottom fade: how far up the column the scrim reaches, as a fraction of the
/// column, paired with the alpha it reaches at the bottom edge. The lighter
/// presets wash rather than curtain, so the picture reads behind and below the
/// floating composer; the strongest reaches full page colour for a bright image
/// that needs settling.
pub const FADE_HEIGHTS: [f32; 5] = [0., 0.08, 0.14, 0.22, 0.32];
/// Peak alpha of each fade preset, at the bottom edge.
pub const FADE_PEAKS: [f32; 5] = [0., 0.55, 0.72, 0.88, 1.];
/// Bottom-fade preset names.
pub const FADE_LABELS: [&str; 5] = ["None", "Subtle", "Medium", "Deep", "Full"];

impl Default for Tuning {
    fn default() -> Self {
        Self {
            blur: 0.,
            cell: GRID,
            fade: FADE_HEIGHTS[DEFAULT_FADE],
        }
    }
}

impl Tuning {
    /// Index of the active pixel-size preset, for the dropdown chip.
    pub fn cell_index(self) -> usize {
        index_of_u32(&CELL_SIZES, self.cell)
    }

    /// Index of the active bottom-fade preset, for the dropdown chip.
    pub fn fade_index(self) -> usize {
        index_of_f32(&FADE_HEIGHTS, self.fade)
    }

    /// Peak alpha of the active fade preset. The gradient stops short of full
    /// page colour at the lighter settings, so the picture is washed rather
    /// than hidden below the composer.
    pub fn fade_peak(self) -> f32 {
        FADE_PEAKS[self.fade_index()]
    }
}

/// Normalized slider position (0..1) for a blur sigma, for the Settings
/// slider's handle and fill.
pub fn blur_norm(sigma: f32) -> f32 {
    (sigma / BLUR_MAX).clamp(0., 1.)
}

/// Blur sigma for a normalized slider position, snapped to [`BLUR_STEP`].
/// This is the single mapping the slider, the config, and the pass share.
pub fn blur_from_norm(norm: f32) -> f32 {
    ((norm.clamp(0., 1.) * BLUR_MAX) / BLUR_STEP).round() * BLUR_STEP
}

fn index_of_f32(values: &[f32], value: f32) -> usize {
    values
        .iter()
        .position(|candidate| (candidate - value).abs() < 0.001)
        .unwrap_or(0)
}

fn index_of_u32(values: &[u32], value: u32) -> usize {
    values
        .iter()
        .position(|candidate| *candidate == value)
        .unwrap_or(0)
}

// ── paths & config ─────────────────────────────────────────────────────────

fn default_dir() -> PathBuf {
    crate::platform::home_dir().join(".orbit-pi")
}

fn image_path(dir: &Path) -> PathBuf {
    dir.join("new-task-background.png")
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join("background.json")
}

/// The picked file's display name plus the tuning, from `background.json`.
#[derive(Debug, Clone, PartialEq, Default)]
struct Config {
    source: Option<String>,
    tuning: Tuning,
}

/// Read `background.json`, falling back per field. Tuning values are only
/// accepted when they are one of the presets, so a hand-edited file can
/// never leave the dropdown chip disagreeing with the painted result.
fn read_config(dir: &Path) -> Config {
    let Ok(raw) = std::fs::read_to_string(config_path(dir)) else {
        return Config::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Config::default();
    };
    let mut config = Config {
        source: value
            .get("source")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        ..Config::default()
    };
    if let Some(blur) = value.get("blur").and_then(serde_json::Value::as_f64) {
        let blur = blur as f32;
        // Blur is continuous, so a value outside the slider's range (an older
        // config that ran to 16, say) clamps to the nearest end rather than
        // snapping the user back to Off.
        if blur.is_finite() {
            config.tuning.blur = blur_from_norm(blur_norm(blur));
        }
    }
    if let Some(cell) = value.get("cell").and_then(serde_json::Value::as_u64) {
        let cell = cell as u32;
        if CELL_SIZES.contains(&cell) {
            config.tuning.cell = cell;
        }
    }
    if let Some(fade) = value.get("fade").and_then(serde_json::Value::as_f64) {
        let fade = fade as f32;
        if FADE_HEIGHTS.contains(&fade) {
            config.tuning.fade = fade;
        }
    }
    config
}

fn write_config(dir: &Path, config: &Config) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let _ = std::fs::write(
        config_path(dir),
        serde_json::json!({
            "source": config.source,
            "blur": config.tuning.blur,
            "cell": config.tuning.cell,
            "fade": config.tuning.fade,
        })
        .to_string(),
    );
}

// ── setting changes ────────────────────────────────────────────────────────

/// Copy `source` into `dir` as the processed PNG, remembering its name and
/// keeping the tuning already chosen. Returns that name for the settings card.
pub fn choose(dir: &Path, source: &Path) -> Result<String, String> {
    let decoded = image::open(source).map_err(|err| tr!("dither.could_not_read", error = err))?;
    let fitted = fit(decoded);
    std::fs::create_dir_all(dir).map_err(|err| tr!("dither.could_not_save", error = err))?;
    fitted
        .into_rgba8()
        .save(image_path(dir))
        .map_err(|err| tr!("dither.could_not_save", error = err))?;
    let label = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_string());
    let mut config = read_config(dir);
    config.source = Some(label.clone());
    write_config(dir, &config);
    Ok(label)
}

/// Drop the background — the dot grid returns, and with it the default tuning.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(image_path(dir));
    let _ = std::fs::remove_file(config_path(dir));
}

/// Downscale to [`MAX_EDGE`] so the dither pass stays cheap; smaller images
/// pass through untouched.
fn fit(image: image::DynamicImage) -> image::DynamicImage {
    if image.width() > MAX_EDGE || image.height() > MAX_EDGE {
        image.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Triangle)
    } else {
        image
    }
}

// ── the backdrop ───────────────────────────────────────────────────────────

/// The configured setting's display name for the Settings card.
pub fn configured_label() -> Option<String> {
    read_config(&default_dir()).source
}

/// The active tuning, for the Settings controls and the view's fade.
pub fn tuning() -> Tuning {
    read_config(&default_dir()).tuning
}

/// Persist backdrop tuning and drop the memoized pass so the next
/// [`background`] rebuilds it. Returns whether anything actually changed.
pub fn set_tuning(tuning: Tuning) -> bool {
    set_tuning_in(&default_dir(), tuning)
}

fn set_tuning_in(dir: &Path, tuning: Tuning) -> bool {
    let mut config = read_config(dir);
    if config.tuning == tuning {
        return false;
    }
    config.tuning = tuning;
    write_config(dir, &config);
    true
}

pub fn choose_file(source: &Path) -> Result<String, String> {
    choose(&default_dir(), source)
}

pub fn clear_all() {
    clear(&default_dir())
}

/// The dithered background, if one is configured. Decodes and dithers on
/// first call (or after a change), then serves the cached image.
pub fn background() -> Option<Arc<RenderImage>> {
    cached(&default_dir())
}

struct Cached {
    key: Key,
    image: Option<Arc<RenderImage>>,
}

/// Only the inputs the dither pass actually reads: the source file's
/// identity plus its blur and cell edge. The bottom fade is painted by the
/// view, not baked here, so changing it must not rebuild the pass.
#[derive(PartialEq)]
struct Key {
    path: PathBuf,
    modified: u64,
    len: u64,
    blur: f32,
    cell: u32,
}

static CACHE: RwLock<Option<Cached>> = RwLock::new(None);

/// Load + dither the stored PNG, memoized on the file's `(path, mtime, len)`
/// and the tuning — choosing, clearing, and re-tuning all change the key, so
/// stale entries fall out on mismatch without explicit invalidation.
fn cached(dir: &Path) -> Option<Arc<RenderImage>> {
    let path = image_path(dir);
    let (modified, len) = std::fs::metadata(&path)
        .map(|meta| {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_millis() as u64)
                .unwrap_or(0);
            (modified, meta.len())
        })
        .unwrap_or((0, 0));
    let tuning = read_config(dir).tuning;
    let key = Key {
        path: path.clone(),
        modified,
        len,
        blur: tuning.blur,
        cell: tuning.cell,
    };

    let cache = CACHE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cached) = cache.as_ref() {
        if cached.key == key {
            return cached.image.clone();
        }
    }
    drop(cache);

    let image = load(&path).map(|source| {
        let dithered = dither(&source, tuning.cell);
        let softened = soften(&dithered, tuning.blur);
        // GPUI's sprite atlas samples BGRA, not RGBA.
        let mut bgra = softened.into_owned();
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        Arc::new(RenderImage::new(vec![Frame::new(bgra)]))
    });
    *CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Cached {
        key,
        image: image.clone(),
    });
    image
}

fn load(path: &Path) -> Option<RgbaImage> {
    image::open(path).ok().map(|image| image.into_rgba8())
}

/// Soften the dithered result, or borrow it untouched when blur is off.
/// `image::imageops::blur` clamps sigma 0 to 0.8, so "Off" must skip the
/// pass entirely to stay a true no-op.
///
/// The blur is applied *after* the dither, not before it: a pre-pass only
/// smooths the source, and the ordered threshold then re-introduces the
/// high-frequency pattern, which reads as noise. Blurring the posterized
/// cells is what actually softens the backdrop into a glow.
fn soften(source: &RgbaImage, sigma: f32) -> Cow<'_, RgbaImage> {
    if sigma > 0. {
        Cow::Owned(image::imageops::fast_blur(source, sigma))
    } else {
        Cow::Borrowed(source)
    }
}

/// Ordered-dither `source` in color, in `grid`-sized cells each sampled at
/// its centre. Every channel is dimmed, contrast-stretched, and quantized
/// with the cell's Bayer threshold — hue survives, the posterized ordered
/// pattern is what shows. Returns an RGBA buffer the size of the source.
fn dither(source: &RgbaImage, grid: u32) -> RgbaImage {
    let grid = grid.max(1);
    let (width, height) = source.dimensions();
    let mut out = RgbaImage::new(width, height);
    for cell_y in 0..height.div_ceil(grid) {
        for cell_x in 0..width.div_ceil(grid) {
            let sample_x = (cell_x * grid + grid / 2).min(width - 1);
            let sample_y = (cell_y * grid + grid / 2).min(height - 1);
            let sample = source.get_pixel(sample_x, sample_y).0;
            let threshold = (BAYER[cell_y as usize % 4][cell_x as usize % 4] as f32 + 0.5) / 16.;
            let color = [
                quantize(sample[0], threshold),
                quantize(sample[1], threshold),
                quantize(sample[2], threshold),
                sample[3],
            ];
            for y in cell_y * grid..(cell_y * grid + grid).min(height) {
                for x in cell_x * grid..(cell_x * grid + grid).min(width) {
                    out.put_pixel(x, y, image::Rgba(color));
                }
            }
        }
    }
    out
}

/// Dim, stretch, and quantize one channel using a Bayer threshold: the
/// fractional part of the scaled value decides whether it rounds up in this
/// cell, which is what turns a smooth ramp into an ordered pattern.
fn quantize(channel: u8, threshold: f32) -> u8 {
    let value = contrast((channel as f32 / 255. * DIM).clamp(0., 1.));
    let levels = (LEVELS - 1) as f32;
    let scaled = value * levels;
    let base = scaled.floor();
    let level = if scaled - base > threshold {
        base + 1.
    } else {
        base
    };
    ((level.clamp(0., levels) / levels) * 255.)
        .round()
        .clamp(0., 255.) as u8
}

fn contrast(value: f32) -> f32 {
    ((value - 0.5) * CONTRAST + 0.5).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, value: u8) -> RgbaImage {
        RgbaImage::from_pixel(width, height, image::Rgba([value, value, value, 255]))
    }

    /// Unique scratch directory per call — tests share a process id.
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("orbit-dither-{tag}-{nanos}"))
    }

    #[test]
    fn bayer_is_a_permutation_of_zero_to_fifteen() {
        let mut seen: Vec<u8> = BAYER.iter().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0u8..16).collect::<Vec<_>>());
    }

    #[test]
    fn dither_keeps_size_and_quantizes_every_channel() {
        // 16×16 so every Bayer row/column is exercised (a shorter image can
        // miss the extremes of the matrix).
        let source = solid(16, 16, 128);
        let out = dither(&source, 4);
        assert_eq!(out.dimensions(), (16, 16));
        let levels: Vec<u8> = (0..LEVELS)
            .map(|level| (level * 255 / (LEVELS - 1)) as u8)
            .collect();
        for pixel in out.pixels() {
            for channel in &pixel.0[..3] {
                assert!(
                    levels.contains(channel),
                    "{channel} is not a quantized level"
                );
            }
        }
        // A mid-gray sits between two levels, so it must dither: more than
        // one level appears across the cells.
        let distinct: std::collections::HashSet<u8> =
            out.pixels().map(|pixel| pixel.0[0]).collect();
        assert!(distinct.len() > 1, "mid-gray should dither across levels");
    }

    #[test]
    fn color_survives_the_pass() {
        // The reference look keeps the photo's chroma — a red stays red
        // (dominant channel), rather than collapsing into two theme tones.
        let red = RgbaImage::from_pixel(8, 8, image::Rgba([210, 40, 30, 255]));
        let out = dither(&red, 4);
        assert!(out.pixels().all(|pixel| {
            let [r, g, b, _] = pixel.0;
            r > g && r > b
        }));
        // And a teal stays teal (the screenshot's hair color).
        let teal = RgbaImage::from_pixel(8, 8, image::Rgba([40, 190, 200, 255]));
        let out = dither(&teal, 4);
        assert!(out.pixels().all(|pixel| {
            let [r, g, b, _] = pixel.0;
            g > r && b > r
        }));
    }

    #[test]
    fn bright_input_stays_brighter_than_dark() {
        let bright = dither(&solid(16, 16, 255), 4);
        let dark = dither(&solid(16, 16, 0), 4);
        let mean = |image: &RgbaImage| {
            let sum: u32 = image.pixels().map(|pixel| pixel.0[0] as u32).sum();
            sum as f32 / (image.width() * image.height()) as f32
        };
        assert!(mean(&bright) > mean(&dark), "white must out-rank black");
        // Dimmed: even a full-white source averages below full white.
        assert!(mean(&bright) < 255., "dimming must hold white back");
    }

    #[test]
    fn grid_cells_are_uniform() {
        // Every grid-sized cell carries exactly one color.
        let source = solid(16, 16, 128);
        let out = dither(&source, 4);
        for cell_y in 0..4 {
            for cell_x in 0..4 {
                let first = out.get_pixel(cell_x * 4, cell_y * 4).0;
                for y in cell_y * 4..cell_y * 4 + 4 {
                    for x in cell_x * 4..cell_x * 4 + 4 {
                        assert_eq!(out.get_pixel(x, y).0, first, "cell ({cell_x},{cell_y})");
                    }
                }
            }
        }
    }

    /// The pixel-size presets all have to produce real blocks — a cell edge
    /// that does not divide the canvas must still fill to the last row/column.
    #[test]
    fn every_cell_preset_produces_uniform_blocks() {
        // 24 = lcm(2,3,4,6,8), so every preset divides the canvas evenly.
        let source = solid(24, 24, 128);
        for cell in CELL_SIZES {
            let out = dither(&source, cell);
            for cell_y in 0..24 / cell {
                for cell_x in 0..24 / cell {
                    let first = out.get_pixel(cell_x * cell, cell_y * cell).0;
                    for y in cell_y * cell..(cell_y + 1) * cell {
                        for x in cell_x * cell..(cell_x + 1) * cell {
                            assert_eq!(
                                out.get_pixel(x, y).0,
                                first,
                                "cell {cell} ({cell_x},{cell_y})"
                            );
                        }
                    }
                }
            }
        }
    }

    /// "Off" must not touch a single byte: `image::imageops::blur` clamps
    /// sigma 0 to 0.8, which is why the pass is skipped instead.
    #[test]
    fn blur_off_is_a_true_no_op() {
        let source = RgbaImage::from_fn(32, 32, |x, _| {
            image::Rgba([if x < 16 { 20 } else { 240 }, 0, 0, 255])
        });
        let dithered = dither(&source, 4);
        let softened = soften(&dithered, 0.);
        assert!(matches!(softened, Cow::Borrowed(_)));
        assert_eq!(softened.as_ref(), &dithered);
    }

    /// The slider's normalized position and the stored sigma are one mapping:
    /// the ends hit 0 and [`BLUR_MAX`] exactly, and every value lands on the
    /// [`BLUR_STEP`] grid so the dropdown-free control can never persist a
    /// value the pass would round differently.
    #[test]
    fn blur_slider_mapping_is_a_rounded_round_trip() {
        assert_eq!(blur_from_norm(0.), 0.);
        assert_eq!(blur_from_norm(1.), BLUR_MAX);
        assert_eq!(blur_norm(0.), 0.);
        assert_eq!(blur_norm(BLUR_MAX), 1.);
        // Out-of-range norms clamp rather than extrapolate.
        assert_eq!(blur_from_norm(-1.), 0.);
        assert_eq!(blur_from_norm(2.), BLUR_MAX);
        for step in 0..=((BLUR_MAX / BLUR_STEP) as u32) {
            let norm = step as f32 / (BLUR_MAX / BLUR_STEP);
            let sigma = blur_from_norm(norm);
            assert_eq!(sigma / BLUR_STEP, (sigma / BLUR_STEP).round());
            assert_eq!(blur_norm(sigma), norm);
        }
    }

    /// A hand-edited config can never land between steps: the slider, the
    /// stored value, and the painted pass agree. Out-of-range values clamp to
    /// an end, so an older 0–16 config keeps its blur instead of resetting.
    #[test]
    fn blur_config_value_is_snapped_to_the_step() {
        let dir = temp_dir("blur-snap");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            config_path(&dir),
            r#"{"source":"x.png","blur":3.31,"cell":4,"fade":0}"#,
        )
        .unwrap();
        assert_eq!(read_config(&dir).tuning.blur, 3.5);

        std::fs::write(
            config_path(&dir),
            r#"{"source":"x.png","blur":99.9,"cell":4,"fade":0}"#,
        )
        .unwrap();
        assert_eq!(read_config(&dir).tuning.blur, BLUR_MAX);

        std::fs::write(
            config_path(&dir),
            r#"{"source":"x.png","blur":-4,"cell":4,"fade":0}"#,
        )
        .unwrap();
        assert_eq!(read_config(&dir).tuning.blur, 0.);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Blur softens the dithered edge, so the transition between the two
    /// posterized plateaus widens and no single adjacent step can jump the
    /// full range any more. Applying it after the pass is what makes the
    /// backdrop read as a glow instead of a hard halftone.
    #[test]
    fn blur_softens_the_edge() {
        let source = RgbaImage::from_fn(64, 64, |x, _| {
            image::Rgba([if x < 32 { 20 } else { 240 }, 0, 0, 255])
        });
        let sharp = dither(&source, 4);
        let soft = soften(&sharp, 4.).into_owned();
        // Largest jump between horizontally adjacent pixels.
        let max_step = |image: &RgbaImage| {
            (0..image.width() - 1)
                .map(|x| {
                    let left = image.get_pixel(x, image.height() / 2).0[0] as i32;
                    let right = image.get_pixel(x + 1, image.height() / 2).0[0] as i32;
                    (right - left).abs()
                })
                .max()
                .unwrap()
        };
        assert!(
            max_step(&soft) < max_step(&sharp),
            "blur must soften the edge (soft {} vs sharp {})",
            max_step(&soft),
            max_step(&sharp)
        );
    }

    /// Every preset is a real option, and the defaults sit inside them — the
    /// dropdowns and the painted result can never disagree.
    #[test]
    #[allow(clippy::assertions_on_constants)] // deliberate preset invariants
    fn presets_cover_the_defaults() {
        assert!((0. ..=BLUR_MAX).contains(&Tuning::default().blur));
        assert_eq!(Tuning::default().blur, 0.);
        assert!(CELL_SIZES.contains(&Tuning::default().cell));
        assert!(FADE_HEIGHTS.contains(&Tuning::default().fade));
        assert_eq!(CELL_LABELS.len(), CELL_SIZES.len());
        assert_eq!(FADE_LABELS.len(), FADE_HEIGHTS.len());
        assert_eq!(FADE_PEAKS.len(), FADE_HEIGHTS.len());
        for fade in FADE_HEIGHTS {
            assert!((0. ..=1.).contains(&fade), "fade {fade} is off-page");
        }
        for peak in FADE_PEAKS {
            assert!((0. ..=1.).contains(&peak), "peak {peak} is not an alpha");
        }
        // Ascending, so the dropdown reads as a scale on both axes.
        assert!(BLUR_MAX > 0.);
        assert!(BLUR_STEP > 0. && BLUR_STEP < BLUR_MAX);
        assert!(CELL_SIZES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(FADE_HEIGHTS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(FADE_PEAKS.windows(2).all(|pair| pair[0] < pair[1]));
        // The chip resolves the default to its own preset index, and a fresh
        // install washes rather than curtains — the picture stays visible
        // below the floating composer.
        assert_eq!(index_of_u32(&CELL_SIZES, Tuning::default().cell), 2);
        assert_eq!(
            index_of_f32(&FADE_HEIGHTS, Tuning::default().fade),
            DEFAULT_FADE
        );
        assert!(Tuning::default().fade_peak() < 1.);
        assert_eq!(Tuning::default().fade_peak(), FADE_PEAKS[DEFAULT_FADE]);
    }

    #[test]
    fn choose_and_clear_round_trip() {
        let dir = temp_dir("round-trip");
        let source = dir.join("picked.png");
        std::fs::create_dir_all(&dir).unwrap();
        solid(4, 4, 90).save(&source).unwrap();

        let label = choose(&dir, &source).unwrap();
        assert_eq!(label, "picked.png");
        assert!(image_path(&dir).exists());
        assert_eq!(read_config(&dir).source.as_deref(), Some("picked.png"));

        clear(&dir);
        assert!(!image_path(&dir).exists());
        assert_eq!(read_config(&dir).source, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tuning lives beside the source: replacing the image keeps it, and a
    /// reset drops back to the defaults.
    #[test]
    fn tuning_survives_a_replace_but_not_a_reset() {
        let dir = temp_dir("tuning");
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.png");
        solid(4, 4, 90).save(&first).unwrap();
        choose(&dir, &first).unwrap();
        assert_eq!(read_config(&dir).tuning, Tuning::default());

        let tuned = Tuning {
            blur: BLUR_MAX,
            cell: CELL_SIZES[4],
            fade: FADE_HEIGHTS[1],
        };
        assert!(set_tuning_in(&dir, tuned));
        assert_eq!(read_config(&dir).tuning, tuned);
        assert!(!set_tuning_in(&dir, tuned), "re-setting is a no-op");

        let second = dir.join("second.png");
        solid(4, 4, 10).save(&second).unwrap();
        choose(&dir, &second).unwrap();
        assert_eq!(read_config(&dir).tuning, tuned, "replace keeps tuning");
        assert_eq!(read_config(&dir).source.as_deref(), Some("second.png"));

        clear(&dir);
        assert_eq!(read_config(&dir).tuning, Tuning::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hand-edited or corrupt config can never land between presets: the
    /// chip and the painted pass read the same validated value.
    #[test]
    fn off_preset_tuning_falls_back_to_defaults() {
        let dir = temp_dir("off-preset");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            config_path(&dir),
            r#"{"source":"x.png","blur":0,"cell":5,"fade":0.333}"#,
        )
        .unwrap();
        let config = read_config(&dir);
        assert_eq!(config.tuning, Tuning::default());
        assert_eq!(config.source.as_deref(), Some("x.png"));

        std::fs::write(config_path(&dir), "not json at all").unwrap();
        assert_eq!(read_config(&dir), Config::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choose_rejects_unreadable_files() {
        let dir = temp_dir("bad");
        std::fs::create_dir_all(&dir).unwrap();
        let bogus = dir.join("not-an-image.txt");
        std::fs::write(&bogus, "hello").unwrap();
        assert!(choose(&dir, &bogus).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Synthetic scene: a tonal ramp, color bands, and a bright disc —
    /// enough to eyeball dimming, hue retention, the dither grid, and now
    /// the blur / pixel-size presets.
    fn debug_scene(width: u32, height: u32) -> RgbaImage {
        let mut source = RgbaImage::new(width, height);
        let band_h = height.div_ceil(5).max(1);
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            let ramp = (x as f32 / (width - 1) as f32 * 255.) as u8;
            let band = match y / band_h {
                0 => [ramp, ramp / 2, ramp / 3], // warm
                1 => [ramp / 3, ramp, ramp / 2], // green
                2 => [40, 190, 200],             // teal
                3 => [210, 60, 70],              // red
                _ => [ramp / 2, ramp / 3, ramp], // violet
            };
            *pixel = image::Rgba([band[0], band[1], band[2], 255]);
        }
        let cx = width as f32 * 0.75;
        let cy = height as f32 / 3.;
        let r = height as f32 / 6.;
        for (x, y, pixel) in source.enumerate_pixels_mut() {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy < r * r {
                *pixel = image::Rgba([240, 240, 240, 255]);
            }
        }
        source
    }

    /// Composite the backdrop the way the view does — cover-fit the dithered
    /// image into `width`×`height`, then lay the bottom fade over it — so the
    /// composer region can be judged without launching the app. Dark palette
    /// only (`#1A1A1A`); it exists to eyeball the fade, not to be a screenshot.
    fn page_mock(source: &RgbaImage, tuning: Tuning, width: u32, height: u32) -> RgbaImage {
        let dithered = dither(source, tuning.cell);
        let dithered = soften(&dithered, tuning.blur);
        let dithered = dithered.as_ref();

        // `ObjectFit::Cover`: scale to fill both axes, then centre-crop.
        let scale = f32::max(
            width as f32 / dithered.width() as f32,
            height as f32 / dithered.height() as f32,
        );
        let scaled = image::imageops::resize(
            dithered,
            (dithered.width() as f32 * scale).round().max(1.) as u32,
            (dithered.height() as f32 * scale).round().max(1.) as u32,
            image::imageops::FilterType::Triangle,
        );
        let mut out = RgbaImage::new(width, height);
        let crop_x = (scaled.width() as i64 - width as i64) / 2;
        let crop_y = (scaled.height() as i64 - height as i64) / 2;
        image::imageops::replace(&mut out, &scaled, -crop_x, -crop_y);

        // The view's bottom scrim: `background` from transparent at the top of
        // the fade to `fade_peak()` at the bottom edge.
        let page = [0x1a, 0x1a, 0x1a];
        let fade = (tuning.fade * height as f32).round() as u32;
        let peak = tuning.fade_peak();
        for row in 0..fade {
            let alpha = peak * (row + 1) as f32 / fade as f32;
            let y = height - fade + row;
            for x in 0..width {
                let px = out.get_pixel(x, y).0;
                let blend = |channel: u8, target: u8| {
                    (channel as f32 * (1. - alpha) + target as f32 * alpha).round() as u8
                };
                out.put_pixel(
                    x,
                    y,
                    image::Rgba([
                        blend(px[0], page[0]),
                        blend(px[1], page[1]),
                        blend(px[2], page[2]),
                        255,
                    ]),
                );
            }
        }
        out
    }

    #[test]
    #[ignore] // manual: cargo test -p orbit-pi debug_render -- --ignored --nocapture
    fn debug_render_to_tmp() {
        let source = debug_scene(480, 270);
        let write = |name: &str, tuning: Tuning| {
            let dithered = dither(&source, tuning.cell);
            let out = soften(&dithered, tuning.blur).into_owned();
            let path = format!("/tmp/orbit-dither-{name}.png");
            out.save(&path).unwrap();
            println!("{path} blur={} cell={}", tuning.blur, tuning.cell);
        };
        write("base", Tuning::default());
        write(
            "cell-coarse",
            Tuning {
                cell: CELL_SIZES[4],
                ..Tuning::default()
            },
        );
        write(
            "blur-heavy",
            Tuning {
                blur: BLUR_MAX,
                ..Tuning::default()
            },
        );
        write(
            "blur-heavy-coarse",
            Tuning {
                blur: BLUR_MAX,
                cell: CELL_SIZES[4],
                ..Tuning::default()
            },
        );
        // The composed page: what the bottom of the new-task window looks
        // like at each fade preset. The default must still read as picture,
        // not as the flat slab the backdrop used to sit on.
        for (name, fade) in [
            ("fade-none", FADE_HEIGHTS[0]),
            ("fade-default", FADE_HEIGHTS[DEFAULT_FADE]),
            ("fade-full", FADE_HEIGHTS[4]),
        ] {
            let tuning = Tuning {
                fade,
                ..Tuning::default()
            };
            let path = format!("/tmp/orbit-page-{name}.png");
            page_mock(&source, tuning, 900, 656).save(&path).unwrap();
            println!(
                "{path} fade={fade} span={}px peak={:.2}",
                (fade * 656.).round(),
                tuning.fade_peak()
            );
        }
    }
}
