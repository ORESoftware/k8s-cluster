//! Safe wrapper around the Pandoc Haskell bridge (`libdd-pandoc-bridge.so`).
//!
//! The bridge exposes a single rich entry point: `dd_pandoc_convert` takes a
//! UTF-8 JSON request and returns a UTF-8 JSON envelope. All document bytes
//! cross as base64, so the same path handles text *and* binary formats (docx,
//! odt, pptx, epub). Calls are NOT serialised — the threaded GHC RTS and
//! `runPure` (pure) make concurrent calls from `spawn_blocking` workers safe.
//!
//! The `standalone` foreign library does not auto-init the RTS, so we call
//! `hs_init` once via a `Once` before the first call.
//!
//! When built `--no-default-features` (no `haskell-bridge`), every call returns
//! [`BridgeError::Disabled`] so the crate compiles without GHC.

#[cfg(feature = "haskell-bridge")]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
#[cfg(feature = "haskell-bridge")]
use serde_json::{json, Value};
#[cfg(feature = "haskell-bridge")]
use std::ffi::{CStr, CString};
#[cfg(feature = "haskell-bridge")]
use std::os::raw::{c_char, c_int};
#[cfg(feature = "haskell-bridge")]
use std::sync::Once;

#[cfg(not(feature = "haskell-bridge"))]
use serde_json::Value;

#[cfg(feature = "haskell-bridge")]
extern "C" {
    // Provided by the GHC RTS bundled into libdd-pandoc-bridge.so. The standalone
    // foreign library does NOT auto-initialise the RTS.
    fn hs_init(argc: *mut c_int, argv: *mut *mut *mut c_char);
    fn dd_pandoc_convert(request: *const c_char) -> *mut c_char;
    fn dd_pandoc_make_pdf(request: *const c_char) -> *mut c_char;
    fn dd_pandoc_version() -> *mut c_char;
    fn dd_pandoc_free(ptr: *mut c_char);
}

#[cfg(feature = "haskell-bridge")]
static HS_INIT: Once = Once::new();

/// Initialise the GHC RTS exactly once (idempotent, thread-safe).
#[cfg(feature = "haskell-bridge")]
fn ensure_hs_init() {
    HS_INIT.call_once(|| {
        let argv_storage: &'static mut [*mut c_char] = Box::leak(Box::new([
            b"dd-document-rs\0".as_ptr() as *mut c_char,
            std::ptr::null_mut(),
        ]));
        let mut argc: c_int = 1;
        let mut argv: *mut *mut c_char = argv_storage.as_mut_ptr();
        // SAFETY: argc/argv reference 'static storage that outlives the process.
        unsafe { hs_init(&mut argc, &mut argv) };
    });
}

/// Whether the Haskell bridge was compiled in.
pub const fn bridge_enabled() -> bool {
    cfg!(feature = "haskell-bridge")
}

#[derive(Debug)]
#[allow(dead_code)] // Disabled only constructed in --no-default-features builds.
pub enum BridgeError {
    Disabled,
    NulInput(&'static str),
    NullReturn,
    InvalidUtf8,
    BadEnvelope(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Disabled => write!(f, "pandoc bridge is not compiled into this build"),
            BridgeError::NulInput(field) => write!(f, "{field} contained a NUL byte"),
            BridgeError::NullReturn => write!(f, "pandoc bridge returned a null pointer"),
            BridgeError::InvalidUtf8 => write!(f, "pandoc bridge returned invalid UTF-8"),
            BridgeError::BadEnvelope(msg) => write!(f, "pandoc bridge envelope error: {msg}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Outcome of a conversion. `ok == true` implies `output` is `Some(bytes)`;
/// otherwise `error` carries the Pandoc/Haskell-side message.
#[derive(Debug)]
pub struct ConvertOutcome {
    pub ok: bool,
    pub output: Option<Vec<u8>>,
    pub error: Option<String>,
}

/// Convert `content` from the `from` Pandoc format to the `to` Pandoc format.
///
/// `content` is raw bytes (UTF-8 text for text formats, the file bytes for
/// binary formats). `standalone` wraps the output in the format's default
/// template; `metadata` is injected into the document (title/author/date/...).
///
/// Blocking + CPU-bound — invoke from `spawn_blocking`.
#[cfg(feature = "haskell-bridge")]
pub fn convert(
    from: &str,
    to: &str,
    content: &[u8],
    standalone: bool,
    metadata: &Value,
) -> Result<ConvertOutcome, BridgeError> {
    ensure_hs_init();

    let request = json!({
        "from": from,
        "to": to,
        "contentB64": BASE64.encode(content),
        "standalone": standalone,
        "metadata": metadata,
    });
    let c_request =
        CString::new(request.to_string()).map_err(|_| BridgeError::NulInput("request"))?;

    // SAFETY: request pointer is valid for the call; the returned pointer is
    // owned by us and freed below.
    let raw = unsafe { dd_pandoc_convert(c_request.as_ptr()) };
    parse_envelope(take_owned_string(raw)?)
}

#[cfg(not(feature = "haskell-bridge"))]
pub fn convert(
    _from: &str,
    _to: &str,
    _content: &[u8],
    _standalone: bool,
    _metadata: &Value,
) -> Result<ConvertOutcome, BridgeError> {
    Err(BridgeError::Disabled)
}

/// Render `content` (from the `from` format) to a PDF via the Typst engine.
///
/// Requires `typst` on PATH in the image (opt-in). Not sandboxed — see the
/// Haskell side. Blocking + CPU-bound — invoke from `spawn_blocking`.
#[cfg(feature = "haskell-bridge")]
pub fn make_pdf(
    from: &str,
    content: &[u8],
    standalone: bool,
    metadata: &Value,
) -> Result<ConvertOutcome, BridgeError> {
    ensure_hs_init();
    let request = json!({
        "from": from,
        "to": "pdf",
        "contentB64": BASE64.encode(content),
        "standalone": standalone,
        "metadata": metadata,
    });
    let c_request =
        CString::new(request.to_string()).map_err(|_| BridgeError::NulInput("request"))?;
    // SAFETY: request pointer valid for the call; returned pointer owned by us.
    let raw = unsafe { dd_pandoc_make_pdf(c_request.as_ptr()) };
    parse_envelope(take_owned_string(raw)?)
}

#[cfg(not(feature = "haskell-bridge"))]
pub fn make_pdf(
    _from: &str,
    _content: &[u8],
    _standalone: bool,
    _metadata: &Value,
) -> Result<ConvertOutcome, BridgeError> {
    Err(BridgeError::Disabled)
}

/// Parse a `{ ok, outputB64?, error? }` envelope into a [`ConvertOutcome`].
#[cfg(feature = "haskell-bridge")]
fn parse_envelope(envelope: String) -> Result<ConvertOutcome, BridgeError> {
    let value: Value =
        serde_json::from_str(&envelope).map_err(|e| BridgeError::BadEnvelope(e.to_string()))?;
    let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let output = match value.get("outputB64").and_then(|v| v.as_str()) {
        Some(b64) => Some(
            BASE64
                .decode(b64)
                .map_err(|e| BridgeError::BadEnvelope(format!("bad outputB64: {e}")))?,
        ),
        None => None,
    };
    Ok(ConvertOutcome {
        ok,
        output,
        error: value
            .get("error")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// The linked Pandoc library version, e.g. `3.10`.
#[cfg(feature = "haskell-bridge")]
pub fn version() -> Result<String, BridgeError> {
    ensure_hs_init();
    // SAFETY: returns an owned malloc'd C string, freed in take_owned_string.
    let raw = unsafe { dd_pandoc_version() };
    take_owned_string(raw)
}

#[cfg(not(feature = "haskell-bridge"))]
pub fn version() -> Result<String, BridgeError> {
    Err(BridgeError::Disabled)
}

/// Copy a bridge-owned C string into an owned `String` and free the original.
#[cfg(feature = "haskell-bridge")]
fn take_owned_string(raw: *mut c_char) -> Result<String, BridgeError> {
    if raw.is_null() {
        return Err(BridgeError::NullReturn);
    }
    // SAFETY: `raw` is non-null and NUL-terminated; we copy it out then free it.
    unsafe {
        let copied = CStr::from_ptr(raw)
            .to_str()
            .map(|s| s.to_string())
            .map_err(|_| BridgeError::InvalidUtf8);
        dd_pandoc_free(raw);
        copied
    }
}
