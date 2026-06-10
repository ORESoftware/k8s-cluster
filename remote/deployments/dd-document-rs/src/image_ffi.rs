//! Safe wrapper around the Magick++ C++ bridge (`cpp/magick_bridge.cpp`).
//!
//! Image bytes cross the FFI boundary as raw buffers. The C++ side allocates
//! result buffers / error strings with `malloc`; we copy them into owned Rust
//! types and immediately hand the originals back to be freed.
//!
//! When built `--no-default-features` (no `magick-bridge`), every call returns
//! [`ImageError::Disabled`] so the crate compiles and runs without ImageMagick.

#[cfg(feature = "magick-bridge")]
use std::ffi::CStr;
use std::ffi::CString;
use std::os::raw::{c_char, c_double, c_int};

/// Mirror of the C++ `DdMagickOp` struct. Keep field order/layout in sync.
#[repr(C)]
struct DdMagickOp {
    out_format: *const c_char,
    resize: *const c_char,
    crop: *const c_char,
    rotate_degrees: c_double,
    quality: c_int,
    strip: c_int,
    grayscale: c_int,
    auto_orient: c_int,
    background: *const c_char,
}

#[cfg(feature = "magick-bridge")]
extern "C" {
    fn dd_magick_transform(
        input: *const u8,
        in_len: usize,
        op: *const DdMagickOp,
        out: *mut *mut u8,
        out_len: *mut usize,
        err: *mut *mut c_char,
    ) -> c_int;
    fn dd_magick_identify(
        input: *const u8,
        in_len: usize,
        out_json: *mut *mut c_char,
        err: *mut *mut c_char,
    ) -> c_int;
    fn dd_magick_version() -> *mut c_char;
    fn dd_magick_free_blob(ptr: *mut u8);
    fn dd_magick_free_str(ptr: *mut c_char);
}

// Concurrency is bounded by a semaphore in the HTTP layer; per-call resource
// limits cap cost, and MagickWandGenesis is done once on first use.
pub const fn image_enabled() -> bool {
    cfg!(feature = "magick-bridge")
}

#[derive(Debug)]
#[allow(dead_code)] // Disabled only constructed in --no-default-features builds.
pub enum ImageError {
    Disabled,
    NulInput(&'static str),
    InvalidUtf8,
    /// Non-zero return code with a message from the bridge.
    Magick(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Disabled => write!(f, "image bridge is not compiled into this build"),
            ImageError::NulInput(field) => write!(f, "{field} contained a NUL byte"),
            ImageError::InvalidUtf8 => write!(f, "image bridge returned invalid UTF-8"),
            ImageError::Magick(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// Optional, validated transform parameters.
#[derive(Default)]
pub struct ImageOps {
    pub out_format: Option<String>,
    pub resize: Option<String>,
    pub crop: Option<String>,
    pub rotate_degrees: f64,
    pub quality: i32,
    pub strip: bool,
    pub grayscale: bool,
    pub auto_orient: bool,
    pub background: Option<String>,
}

/// Transform `input` (encode/resize/rotate/strip/flatten) and return the bytes.
#[cfg(feature = "magick-bridge")]
pub fn transform(input: &[u8], ops: &ImageOps) -> Result<Vec<u8>, ImageError> {
    // Keep the CStrings alive for the duration of the call.
    let out_format = optional_cstring("out_format", ops.out_format.as_deref())?;
    let resize = optional_cstring("resize", ops.resize.as_deref())?;
    let crop = optional_cstring("crop", ops.crop.as_deref())?;
    let background = optional_cstring("background", ops.background.as_deref())?;

    let op = DdMagickOp {
        out_format: ptr_or_null(&out_format),
        resize: ptr_or_null(&resize),
        crop: ptr_or_null(&crop),
        rotate_degrees: ops.rotate_degrees,
        quality: ops.quality,
        strip: i32::from(ops.strip),
        grayscale: i32::from(ops.grayscale),
        auto_orient: i32::from(ops.auto_orient),
        background: ptr_or_null(&background),
    };

    let mut out: *mut u8 = std::ptr::null_mut();
    let mut out_len: usize = 0;
    let mut err: *mut c_char = std::ptr::null_mut();

    // SAFETY: all pointers are valid for the call; outputs are owned by us
    // and released below.
    let code = unsafe {
        dd_magick_transform(
            input.as_ptr(),
            input.len(),
            &op,
            &mut out,
            &mut out_len,
            &mut err,
        )
    };

    if code == 0 {
        // SAFETY: bridge returned success, so `out`/`out_len` describe a buffer
        // it allocated for us.
        let bytes = unsafe { copy_and_free_blob(out, out_len) };
        Ok(bytes)
    } else {
        Err(ImageError::Magick(take_error(err, code)))
    }
}

#[cfg(not(feature = "magick-bridge"))]
pub fn transform(_input: &[u8], _ops: &ImageOps) -> Result<Vec<u8>, ImageError> {
    Err(ImageError::Disabled)
}

/// Identify an image, returning the bridge's JSON description.
#[cfg(feature = "magick-bridge")]
pub fn identify(input: &[u8]) -> Result<String, ImageError> {
    let mut out_json: *mut c_char = std::ptr::null_mut();
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: input slice is valid; outputs owned by us and freed below.
    let code = unsafe { dd_magick_identify(input.as_ptr(), input.len(), &mut out_json, &mut err) };
    if code == 0 {
        take_string(out_json)
    } else {
        Err(ImageError::Magick(take_error(err, code)))
    }
}

#[cfg(not(feature = "magick-bridge"))]
pub fn identify(_input: &[u8]) -> Result<String, ImageError> {
    Err(ImageError::Disabled)
}

#[cfg(feature = "magick-bridge")]
pub fn version() -> Option<String> {
    // SAFETY: returns an owned malloc'd C string, freed in take_string.
    let raw = unsafe { dd_magick_version() };
    take_string(raw).ok()
}

#[cfg(not(feature = "magick-bridge"))]
pub fn version() -> Option<String> {
    None
}

fn optional_cstring(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<CString>, ImageError> {
    match value {
        Some(v) => Ok(Some(CString::new(v).map_err(|_| ImageError::NulInput(field))?)),
        None => Ok(None),
    }
}

fn ptr_or_null(value: &Option<CString>) -> *const c_char {
    value
        .as_ref()
        .map(|c| c.as_ptr())
        .unwrap_or(std::ptr::null())
}

#[cfg(feature = "magick-bridge")]
unsafe fn copy_and_free_blob(ptr: *mut u8, len: usize) -> Vec<u8> {
    if ptr.is_null() || len == 0 {
        if !ptr.is_null() {
            dd_magick_free_blob(ptr);
        }
        return Vec::new();
    }
    let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
    dd_magick_free_blob(ptr);
    bytes
}

#[cfg(feature = "magick-bridge")]
fn take_string(ptr: *mut c_char) -> Result<String, ImageError> {
    if ptr.is_null() {
        return Err(ImageError::Magick("bridge returned a null string".into()));
    }
    // SAFETY: ptr is a non-null, NUL-terminated malloc'd string from the bridge.
    unsafe {
        let result = CStr::from_ptr(ptr)
            .to_str()
            .map(|s| s.to_string())
            .map_err(|_| ImageError::InvalidUtf8);
        dd_magick_free_str(ptr);
        result
    }
}

/// Consume a bridge error string (or synthesize one from the code).
#[cfg(feature = "magick-bridge")]
fn take_error(ptr: *mut c_char, code: c_int) -> String {
    if ptr.is_null() {
        return format!("image bridge failed with code {code}");
    }
    // SAFETY: non-null malloc'd C string from the bridge.
    unsafe {
        let message = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        dd_magick_free_str(ptr);
        message
    }
}
