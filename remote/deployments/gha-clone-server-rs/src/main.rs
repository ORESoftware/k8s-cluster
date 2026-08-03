// Keep each included file at complete Rust item boundaries. The fragments share
// one crate scope, making validation logic directly testable without widening
// visibility merely to satisfy a module split.
include!("parts/part01.rs");
include!("parts/part02.rs");
include!("parts/part03.rs");
include!("parts/part04.rs");
