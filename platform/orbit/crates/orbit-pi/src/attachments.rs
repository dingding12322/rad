//! Image-attachment compression — bounded prompt payloads.
//!
//! A high-resolution PNG/JPEG screenshot is megabytes of base64 once it rides
//! a prompt, and it then stays in the session history for every later turn
//! (see `docs/provider-request-limits.md`). This module downscales and
//! re-encodes attached images at ingest time so the bytes Orbit sends pi are
//! bounded before they ever reach a provider.
//!
//! Policy — mirroring pi's own documented `inputLimits.images.resize`
//! defaults (1400 px, 512 KiB base64, JPEG quality 75):
//!
//! - The longest edge is capped at [`MAX_EDGE`]; smaller images are never
//!   upscaled.
//! - The **base64** payload (the shape pi sends) is capped at
//!   [`MAX_BASE64_BYTES`]; JPEG quality steps down to meet it.
//! - Opaque images are re-encoded as JPEG; images with an alpha channel are
//!   re-encoded as lossless WebP so transparency survives (the only WebP mode
//!   `image` 0.25 supports — so the byte cap is a soft target for that rare
//!   case, bounded by the resize).
//! - An image already under both caps passes through untouched — no quality
//!   loss and, for JPEGs, the EXIF orientation survives.
//! - Animated GIFs and other formats the encoder cannot safely rewrite
//!   (SVG, TIFF) pass through unchanged so frames and fidelity are never
//!   silently dropped.

use std::io::Cursor;

use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, ImageFormat};

/// Longest source edge kept, matching pi's documented resize default.
pub const MAX_EDGE: u32 = 1400;
/// Cap on the base64 payload (the form pi sends), matching pi's
/// `inputLimits.images.resize.maxBytes`.
pub const MAX_BASE64_BYTES: usize = 512 * 1024;
/// Starting JPEG quality; steps down toward [`JPEG_QUALITY_MIN`] to fit the cap.
const JPEG_QUALITY: u8 = 75;
const JPEG_QUALITY_MIN: u8 = 40;
const JPEG_QUALITY_STEP: u8 = 5;

/// A prepared, bounded image ready to ride a prompt.
pub struct CompressedImage {
    /// Encoded bytes — the re-encoded JPEG/WebP, or the source when it was
    /// small enough (or a format we refuse to rewrite) to pass through.
    pub bytes: Vec<u8>,
    /// Format of `bytes`, for the chip preview decode and the wire MIME type.
    pub format: gpui::ImageFormat,
    /// True when the image was resized or re-encoded; false when it passed
    /// through untouched.
    pub compressed: bool,
}

/// Prepare `input` (already tagged with `format`) for the prompt. Never
/// fails: an undecodable or un-rewritable image passes through unchanged
/// rather than being dropped.
pub fn compress_image(input: &[u8], format: gpui::ImageFormat) -> CompressedImage {
    // Animated GIFs decode to a single frame here; re-encoding would throw the
    // other frames away. SVG and TIFF have no reliable rewrite in `image`'s
    // default feature set. Pass these through untouched.
    if matches!(
        format,
        gpui::ImageFormat::Gif | gpui::ImageFormat::Svg | gpui::ImageFormat::Tiff
    ) {
        return passthrough(input, format);
    }

    let decoded = match image::load_from_memory(input) {
        Ok(decoded) => decoded,
        Err(_) => return passthrough(input, format),
    };

    let (width, height) = (decoded.width(), decoded.height());
    // Already within every bound: keep the exact source bytes.
    if width <= MAX_EDGE && height <= MAX_EDGE && base64_len(input.len()) <= MAX_BASE64_BYTES {
        return passthrough(input, format);
    }

    let (scaled_width, scaled_height) = fit(width, height, MAX_EDGE);
    let decoded = if scaled_width != width || scaled_height != height {
        decoded.resize(scaled_width, scaled_height, FilterType::Triangle)
    } else {
        decoded
    };

    if decoded.color().has_alpha() {
        // Lossless WebP preserves transparency and is smaller than PNG for the
        // same content; the resize above bounds its worst case.
        match encode_webp(&decoded) {
            Some(bytes) => CompressedImage {
                bytes,
                format: gpui::ImageFormat::Webp,
                compressed: true,
            },
            None => passthrough(input, format),
        }
    } else {
        match encode_jpeg_capped(&decoded) {
            Some(bytes) => CompressedImage {
                bytes,
                format: gpui::ImageFormat::Jpeg,
                compressed: true,
            },
            None => passthrough(input, format),
        }
    }
}

fn passthrough(input: &[u8], format: gpui::ImageFormat) -> CompressedImage {
    CompressedImage {
        bytes: input.to_vec(),
        format,
        compressed: false,
    }
}

/// Scale `(width, height)` down to fit inside a `max_edge` square, preserving
/// aspect ratio and never upscaling.
fn fit(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = max_edge as f64 / longest as f64;
    let w = ((width as f64) * scale).round().clamp(1.0, max_edge as f64) as u32;
    let h = ((height as f64) * scale).round().clamp(1.0, max_edge as f64) as u32;
    (w, h)
}

/// Encode as JPEG, lowering quality until the base64 payload fits the cap.
/// A large noisy image can still exceed the cap at the quality floor; the
/// smallest encoding wins rather than dropping the attachment.
fn encode_jpeg_capped(image: &DynamicImage) -> Option<Vec<u8>> {
    let mut quality = JPEG_QUALITY;
    loop {
        let mut buf = Vec::new();
        image
            .write_with_encoder(JpegEncoder::new_with_quality(&mut buf, quality))
            .ok()?;
        if base64_len(buf.len()) <= MAX_BASE64_BYTES || quality <= JPEG_QUALITY_MIN {
            return Some(buf);
        }
        quality = quality.saturating_sub(JPEG_QUALITY_STEP).max(JPEG_QUALITY_MIN);
    }
}

/// Encode as lossless WebP (the only WebP mode `image` 0.25 supports).
fn encode_webp(image: &DynamicImage) -> Option<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());
    image.write_to(&mut cursor, ImageFormat::WebP).ok()?;
    Some(cursor.into_inner())
}

/// The base64 length of `raw_len` raw bytes (no `data:` prefix).
fn base64_len(raw_len: usize) -> usize {
    raw_len.div_ceil(3) * 4
}

/// A compact human byte label for the compression status: `4.2 MB`, `176 KB`,
/// `640 B`.
pub fn human_bytes(bytes: usize) -> String {
    const UNITS: [(&str, f64); 4] = [
        ("GB", 1024.0 * 1024.0 * 1024.0),
        ("MB", 1024.0 * 1024.0),
        ("KB", 1024.0),
        ("B", 1.0),
    ];
    let bytes = bytes as f64;
    for (unit, scale) in UNITS {
        if bytes >= scale {
            let value = bytes / scale;
            let text = if value >= 10.0 {
                format!("{value:.0}")
            } else {
                format!("{value:.1}")
            };
            return format!("{text} {unit}");
        }
    }
    "0 B".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An incompressible RGB image so the source genuinely exceeds the caps.
    fn noise_rgb(width: u32, height: u32) -> Vec<u8> {
        let mut img = image::RgbImage::new(width, height);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([
                x.wrapping_mul(7).wrapping_add(y.wrapping_mul(3)) as u8,
                x.wrapping_mul(13).wrapping_add(y.wrapping_mul(5)) as u8,
                x.wrapping_mul(17).wrapping_add(y.wrapping_mul(11)) as u8,
            ]);
        }
        encode_png(image::DynamicImage::ImageRgb8(img))
    }

    fn noise_rgba(width: u32, height: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::new(width, height);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgba([
                x.wrapping_mul(7).wrapping_add(y.wrapping_mul(3)) as u8,
                x.wrapping_mul(13).wrapping_add(y.wrapping_mul(5)) as u8,
                x.wrapping_mul(17).wrapping_add(y.wrapping_mul(11)) as u8,
                128,
            ]);
        }
        encode_png(image::DynamicImage::ImageRgba8(img))
    }

    fn encode_png(img: DynamicImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn fit_scales_down_and_never_up() {
        assert_eq!(fit(10, 10, 1400), (10, 10));
        assert_eq!(fit(2800, 1400, 1400), (1400, 700));
        assert_eq!(fit(1400, 2800, 1400), (700, 1400));
        assert_eq!(fit(1, 1, 1400), (1, 1));
    }

    #[test]
    fn base64_len_matches_known_padding() {
        assert_eq!(base64_len(0), 0);
        assert_eq!(base64_len(1), 4);
        assert_eq!(base64_len(3), 4);
        assert_eq!(base64_len(4), 8);
    }

    #[test]
    fn small_image_passes_through_untouched() {
        let png = noise_rgb(16, 16);
        let out = compress_image(&png, gpui::ImageFormat::Png);
        assert!(!out.compressed);
        assert_eq!(out.bytes, png);
        assert_eq!(out.format, gpui::ImageFormat::Png);
    }

    #[test]
    fn oversized_opaque_image_becomes_smaller_jpeg() {
        let png = noise_rgb(1600, 1200);
        let out = compress_image(&png, gpui::ImageFormat::Png);
        assert!(out.compressed);
        assert_eq!(out.format, gpui::ImageFormat::Jpeg);
        let decoded = image::load_from_memory(&out.bytes).unwrap();
        assert!(decoded.width() <= MAX_EDGE);
        assert!(decoded.height() <= MAX_EDGE);
        assert!(out.bytes.len() < png.len(), "compression must shrink the payload");
    }

    #[test]
    fn oversized_alpha_image_keeps_alpha_as_webp() {
        let png = noise_rgba(1600, 1200);
        let out = compress_image(&png, gpui::ImageFormat::Png);
        assert!(out.compressed);
        assert_eq!(out.format, gpui::ImageFormat::Webp);
        let decoded = image::load_from_memory(&out.bytes).unwrap();
        assert!(decoded.color().has_alpha(), "alpha must survive");
        assert!(decoded.width() <= MAX_EDGE);
    }

    #[test]
    fn gif_passes_through_to_keep_animation() {
        let gif = b"GIF89a".to_vec();
        let out = compress_image(&gif, gpui::ImageFormat::Gif);
        assert!(!out.compressed);
        assert_eq!(out.bytes, gif);
        assert_eq!(out.format, gpui::ImageFormat::Gif);
    }

    #[test]
    fn human_bytes_reads_at_the_right_scale() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(640), "640 B");
        assert_eq!(human_bytes(180_000), "176 KB");
        assert_eq!(human_bytes(12_400_000), "12 MB");
    }
}
