//! Safe wrapper around the Tesseract C++ bridge (`cpp/tesseract_bridge.cpp`).
//!
//! Image bytes cross the FFI boundary as a raw buffer. The C++ side allocates
//! the result text / error string with `malloc`; we copy them into owned Rust
//! `String`s and hand the originals straight back to be freed.
//!
//! When built `--no-default-features` (no `tesseract-bridge`), every call
//! returns [`TesseractError::Disabled`] so the crate compiles and runs without
//! a local OCR toolchain — only the cloud backends are then usable.

#[cfg(feature = "tesseract-bridge")]
use std::ffi::{CStr, CString};
#[cfg(feature = "tesseract-bridge")]
use std::os::raw::{c_char, c_int};

#[cfg(feature = "tesseract-bridge")]
extern "C" {
    fn dd_tess_ocr(
        data: *const u8,
        len: usize,
        lang: *const c_char,
        psm: c_int,
        out_conf: *mut c_int,
        out_err: *mut *mut c_char,
    ) -> *mut c_char;
    fn dd_tess_version() -> *mut c_char;
    fn dd_tess_free_str(ptr: *mut c_char);
}

/// Whether the Tesseract bridge was compiled into this build.
pub const fn tesseract_enabled() -> bool {
    cfg!(feature = "tesseract-bridge")
}

#[derive(Debug)]
#[allow(dead_code)] // Disabled only constructed in --no-default-features builds.
pub enum TesseractError {
    Disabled,
    NulInput(&'static str),
    NullReturn,
    InvalidUtf8,
    /// Engine-side failure (decode/init/recognise) with a bridge message.
    Engine(String),
}

impl std::fmt::Display for TesseractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TesseractError::Disabled => {
                write!(f, "tesseract bridge is not compiled into this build")
            }
            TesseractError::NulInput(field) => write!(f, "{field} contained a NUL byte"),
            TesseractError::NullReturn => write!(f, "tesseract bridge returned a null pointer"),
            TesseractError::InvalidUtf8 => write!(f, "tesseract bridge returned invalid UTF-8"),
            TesseractError::Engine(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for TesseractError {}

/// Recognised text plus the engine's mean word confidence (0..100, or -1 when
/// Tesseract could not report one).
#[derive(Debug)]
pub struct OcrOutcome {
    pub text: String,
    pub confidence: i32,
}

/// Run Tesseract over `image` (any format Leptonica can decode) using the given
/// `lang` ("eng", "eng+deu", ...) and page-segmentation `psm`.
///
/// Blocking + CPU-bound — invoke from `spawn_blocking`.
#[cfg(feature = "tesseract-bridge")]
pub fn recognize(image: &[u8], lang: &str, psm: i32) -> Result<OcrOutcome, TesseractError> {
    let c_lang = CString::new(lang).map_err(|_| TesseractError::NulInput("lang"))?;
    let mut conf: c_int = -1;
    let mut err: *mut c_char = std::ptr::null_mut();

    // SAFETY: image/lang pointers are valid for the duration of the call; the
    // returned text pointer (and any error pointer) are owned by us and freed
    // via take_owned_string below.
    let raw = unsafe {
        dd_tess_ocr(
            image.as_ptr(),
            image.len(),
            c_lang.as_ptr(),
            psm as c_int,
            &mut conf,
            &mut err,
        )
    };

    if raw.is_null() {
        let message = if err.is_null() {
            "tesseract bridge failed without a message".to_string()
        } else {
            take_owned_string(err).unwrap_or_else(|_| "tesseract bridge failed".to_string())
        };
        return Err(TesseractError::Engine(message));
    }

    let text = take_owned_string(raw)?;
    Ok(OcrOutcome {
        text,
        confidence: conf,
    })
}

#[cfg(not(feature = "tesseract-bridge"))]
pub fn recognize(_image: &[u8], _lang: &str, _psm: i32) -> Result<OcrOutcome, TesseractError> {
    Err(TesseractError::Disabled)
}

/// The linked Tesseract version, e.g. `5.3.4`.
#[cfg(feature = "tesseract-bridge")]
pub fn version() -> Option<String> {
    // SAFETY: returns a malloc'd C string, freed in take_owned_string.
    let raw = unsafe { dd_tess_version() };
    take_owned_string(raw).ok()
}

#[cfg(not(feature = "tesseract-bridge"))]
pub fn version() -> Option<String> {
    None
}

/// Copy a bridge-owned C string into an owned `String` and free the original.
#[cfg(feature = "tesseract-bridge")]
fn take_owned_string(raw: *mut c_char) -> Result<String, TesseractError> {
    if raw.is_null() {
        return Err(TesseractError::NullReturn);
    }
    // SAFETY: `raw` is non-null and NUL-terminated; we copy it out then free it.
    unsafe {
        let copied = CStr::from_ptr(raw)
            .to_str()
            .map(|s| s.to_string())
            .map_err(|_| TesseractError::InvalidUtf8);
        dd_tess_free_str(raw);
        copied
    }
}
