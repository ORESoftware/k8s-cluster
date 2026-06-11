//! Pure-Rust image preprocessing for the local OCR path, built on the
//! open-source `image` and `imageproc` crates.
//!
//! We decode arbitrary input (PNG/JPEG/WebP/TIFF/BMP/GIF), convert to luma,
//! optionally upscale small scans so glyphs clear Tesseract's preferred
//! x-height, and Otsu-threshold to a clean bi-level image. Tesseract reads a
//! binarised, right-sized page far more accurately than a noisy colour scan.
//! The result is re-encoded as PNG so the Leptonica side of the bridge (and the
//! cloud backends) decode it losslessly.
//!
//! Security: input is untrusted. `imageproc` pulls `image` in with all default
//! codecs (incl. heavier/less-hardened AVIF/OpenEXR decoders), so we never call
//! the format-sniffing `load_from_memory`. Instead we guess the format, reject
//! anything outside an explicit allowlist, and decode under `image::Limits`
//! (dimension + allocation caps) so a decompression bomb can't exhaust memory.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use imageproc::contrast::{otsu_level, threshold, ThresholdType};

/// Formats we accept from callers. Decoders for everything else `image` was
/// compiled with (AVIF, OpenEXR, TGA, DDS, HDR, QOI, PNM, ...) are never reached
/// from untrusted bytes.
const ALLOWED_FORMATS: &[ImageFormat] = &[
    ImageFormat::Png,
    ImageFormat::Jpeg,
    ImageFormat::WebP,
    ImageFormat::Tiff,
    ImageFormat::Bmp,
    ImageFormat::Gif,
];

/// Decode limits applied to every untrusted image.
#[derive(Clone, Copy)]
pub struct DecodeLimits {
    /// Reject images whose width or height exceeds this (pixels).
    pub max_dim: u32,
    /// Cap the decoder's intermediate allocation (bytes).
    pub max_alloc_bytes: u64,
}

#[derive(Debug)]
pub enum PreprocessError {
    /// Could not determine the container format.
    UnknownFormat,
    /// Format recognised but not on the allowlist.
    UnsupportedFormat(&'static str),
    /// Decode failed or tripped a configured limit.
    Decode(image::ImageError),
    /// Re-encoding the normalised image failed.
    Encode(image::ImageError),
}

impl std::fmt::Display for PreprocessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreprocessError::UnknownFormat => write!(f, "could not determine the image format"),
            PreprocessError::UnsupportedFormat(fmt) => {
                write!(f, "unsupported image format: {fmt} (allowed: png, jpeg, webp, tiff, bmp, gif)")
            }
            PreprocessError::Decode(e) => write!(f, "decode failed: {e}"),
            PreprocessError::Encode(e) => write!(f, "encode failed: {e}"),
        }
    }
}

impl std::error::Error for PreprocessError {}

/// Outcome of preprocessing: a PNG-encoded, normalised image plus the dimensions
/// and whether binarisation was applied.
#[derive(Debug)]
pub struct Preprocessed {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub binarized: bool,
}

/// Cheaply identify the container format from magic bytes and enforce the
/// allowlist *without decoding*. Used to reject junk/disallowed input on every
/// path — including before spending a paid cloud API call or forwarding bytes
/// to a third party. Returns the canonical format name on success.
pub fn sniff_allowed(input: &[u8]) -> Result<&'static str, PreprocessError> {
    let reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| PreprocessError::UnknownFormat)?;
    let format = reader.format().ok_or(PreprocessError::UnknownFormat)?;
    if !ALLOWED_FORMATS.contains(&format) {
        return Err(PreprocessError::UnsupportedFormat(format_name(format)));
    }
    Ok(format_name(format))
}

/// Decode `input`, normalise it for OCR, and return a PNG.
///
/// * `binarize` — apply Otsu thresholding to a 1-bit-style black/white image.
/// * `min_dim` — if the shorter side is below this, integer-upscale (Lanczos3)
///   so small text is legible. `0` disables upscaling. Upscaling never grows a
///   side past `limits.max_dim`.
///
/// CPU-bound — invoke from `spawn_blocking`.
pub fn prepare(
    input: &[u8],
    binarize: bool,
    min_dim: u32,
    limits: DecodeLimits,
) -> Result<Preprocessed, PreprocessError> {
    // Guess the container format from magic bytes and gate it before decoding.
    let reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| PreprocessError::UnknownFormat)?;
    let format = reader.format().ok_or(PreprocessError::UnknownFormat)?;
    if !ALLOWED_FORMATS.contains(&format) {
        return Err(PreprocessError::UnsupportedFormat(format_name(format)));
    }

    // Cap dimensions + intermediate allocation so a small payload can't decode
    // into a multi-gigabyte buffer.
    let mut decode_limits = Limits::default();
    decode_limits.max_image_width = Some(limits.max_dim);
    decode_limits.max_image_height = Some(limits.max_dim);
    decode_limits.max_alloc = Some(limits.max_alloc_bytes);

    let mut reader = reader;
    reader.limits(decode_limits);
    let img = reader.decode().map_err(PreprocessError::Decode)?;

    let (w0, h0) = (img.width(), img.height());
    let short = w0.min(h0);

    // Integer upscale keeps glyph edges crisp for the thresholding pass, but is
    // clamped so the result never exceeds the configured dimension ceiling.
    let mut scale = if min_dim > 0 && short > 0 && short < min_dim {
        min_dim.div_ceil(short).max(1)
    } else {
        1
    };
    let long = w0.max(h0).max(1);
    while scale > 1 && long.saturating_mul(scale) > limits.max_dim {
        scale -= 1;
    }
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
    dynimg
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(PreprocessError::Encode)?;

    Ok(Preprocessed {
        png,
        width,
        height,
        binarized,
    })
}

fn format_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::WebP => "webp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Gif => "gif",
        ImageFormat::Avif => "avif",
        ImageFormat::OpenExr => "openexr",
        ImageFormat::Tga => "tga",
        ImageFormat::Dds => "dds",
        ImageFormat::Hdr => "hdr",
        ImageFormat::Pnm => "pnm",
        ImageFormat::Qoi => "qoi",
        ImageFormat::Ico => "ico",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: DecodeLimits = DecodeLimits {
        max_dim: 10_000,
        max_alloc_bytes: 256 * 1024 * 1024,
    };

    // 1x1 PNG (valid).
    fn tiny_png() -> Vec<u8> {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        B64.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGP4DwABAQEAsTj2FAAAAABJRU5ErkJggg==")
            .unwrap()
    }

    #[test]
    fn decodes_allowed_png() {
        let out = prepare(&tiny_png(), true, 0, LIMITS).expect("png decodes");
        assert_eq!((out.width, out.height), (1, 1));
        assert!(out.binarized);
        assert!(!out.png.is_empty());
    }

    #[test]
    fn rejects_unknown_bytes() {
        let err = prepare(b"not an image at all", false, 0, LIMITS).unwrap_err();
        assert!(matches!(err, PreprocessError::UnknownFormat));
    }

    #[test]
    fn rejects_disallowed_but_recognised_format() {
        // OpenEXR magic number: 0x76 0x2f 0x31 0x01.
        let exr = [0x76u8, 0x2f, 0x31, 0x01, 0, 0, 0, 0];
        let err = prepare(&exr, false, 0, LIMITS).unwrap_err();
        assert!(
            matches!(err, PreprocessError::UnsupportedFormat("openexr"))
                || matches!(err, PreprocessError::UnknownFormat),
            "expected unsupported/unknown, got {err:?}"
        );
    }
}
