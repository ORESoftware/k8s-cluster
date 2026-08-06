// Hand-written package entrypoint for the generated Rust/WASM declarations.
// wasm-pack 0.15 initializes web packages through `__wbindgen_start`; this
// explicit no-op start hook satisfies that lifecycle without adding runtime
// network, storage, mutation, cookie, beacon, or probe behavior.

use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen(start)]
pub fn initialize_wasm_module() {}

include!("../generated/rust-wasm/src/lib.rs");
