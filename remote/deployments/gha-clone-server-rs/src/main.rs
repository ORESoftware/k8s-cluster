// The executable is split into reviewed source fragments so GitHub's contents
// API can publish the implementation without opaque archives. build.rs joins
// them byte-for-byte into OUT_DIR before rustc runs.
include!(concat!(env!("OUT_DIR"), "/main_generated.rs"));
