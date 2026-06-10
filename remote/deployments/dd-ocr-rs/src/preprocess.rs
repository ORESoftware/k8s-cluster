//! Pure-Rust image preprocessing for the local OCR path, built on the
//! open-source `image` and `imageproc` crates.
//!
//! We decode arbitrary input (PNG/JPEG/WebP/TIFF/BMP/GIF), convert to luma,
//! optionally upscale small scans so glyphs clear Tesseract's preferred
//! x-height, and Otsu-threshold to a clean bi-level image. Tesseract reads a
//! binarised, right-sized page far more accurately than a noisy colour scan.
//! The result is re-encoded as PNG so the Leptonica side of the bridge (and the
//! cloud backends) decode it losslessly.

use image::{DynamicImage, GenericImageView, ImageError, ImageFormat};
use imageproc::contrast::{otsu_level, threshold, ThresholdType};

/// Outcome of preprocessing: a PNG-encoded, normalised image plus the dimensions
/// and whether binarisation was applied.
pub struct Preprocessed {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub binarized: bool,
}

/// Decode `input`, normalise it for OCR, and return a PNG.
///
/// * `binarize` — apply Otsu thresholding to a 1-bit-style black/white image.
/// * `min_dim` — if the shorter side is below this, integer-upscale (Lanczos3)
///   so small text is legible. `0` disables upscaling.
///
/// CPU-bound — invoke from `spawn_blocking`.
pub fn prepare(input: &[u8], binarize: bool, min_dim: u32) -> Result<Preprocessed, ImageError> {
    let img = image::load_from_memory(input)?;
    let (w0, h0) = img.dimensions();
    let short = w0.min(h0);

    // Integer upscale keeps glyph edges crisp for the thresholding pass.
    let scale = if min_dim > 0 && short > 0 && short < min_dim {
        ((min_dim + short - 1) / short).max(1)
    } else {
        1
    };
    let scaled = if scale > 1 {
        img.resize(
            w0.saturating_mul(scale),
            h0.saturating_mul(scale),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    let mut gray = scaled.to_luma8();
    let mut binarized = false;
    if binarize {
        let level = otsu_level(&gray);
        gray = threshold(&gray, level, ThresholdType::Binary);
        binarized = true;
    }

    let (width, height) = (gray.width(), gray.height());
    let dynimg = DynamicImage::ImageLuma8(gray);
    let mut png = Vec::new();
    dynimg.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)?;

    Ok(Preprocessed {
        png,
        width,
        height,
        binarized,
    })
}
