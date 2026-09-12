//! Real CAD/STL geometry engine for the fabrication server.
//!
//! Three capabilities, all computed deterministically from submitted STL
//! geometry (no licensed kernels, no external services, std-only math):
//!
//! * [`repair`]   — weld/heal a mesh into a watertight, consistently-wound shell
//! * [`toolpath`] — planar-slice the shell into perimeter motion programs
//! * [`cost`]     — estimate manufacturing cost from volume + toolpath time
//!
//! [`api`] adds the serde request/response glue that the HTTP handlers in
//! `main.rs` call. The lower layers stay JSON-free so they can be verified in
//! isolation.

pub mod api;
pub mod cost;
pub mod mesh;
pub mod repair;
pub mod toolpath;
