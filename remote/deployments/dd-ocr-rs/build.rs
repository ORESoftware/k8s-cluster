// Compiles + links the local Tesseract OCR bridge when the `tesseract-bridge`
// feature is on.
//
//   tesseract-bridge -> compile cpp/tesseract_bridge.cpp and link libtesseract
//                       + Leptonica (liblept), both discovered via pkg-config.
//
// Build scripts don't see Cargo features as `cfg`, only as `CARGO_FEATURE_*`
// env vars, so we gate on those. `cargo build --no-default-features` skips the
// shim, leaving a crate that needs no native OCR toolchain.
use std::process::Command;

fn main() {
    if std::env::var_os("CARGO_FEATURE_TESSERACT_BRIDGE").is_some() {
        build_tesseract_bridge();
    }
}

fn build_tesseract_bridge() {
    println!("cargo:rerun-if-changed=cpp/tesseract_bridge.cpp");

    // pkg-config gives us the include flags for the Tesseract C++ API and
    // Leptonica, plus the libraries to link against.
    let cflags = pkg_config(&["--cflags", "tesseract", "lept"]);
    let libs = pkg_config(&["--libs", "tesseract", "lept"]);

    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("cpp/tesseract_bridge.cpp")
        .flag("-std=c++17");
    for token in cflags.split_whitespace() {
        // -I... include paths for tesseract/baseapi.h and leptonica/allheaders.h.
        build.flag(token);
    }
    build.compile("dd_tesseract_bridge"); // emits the static-lib link directive

    // Emit link directives for the Tesseract + Leptonica shared libraries.
    for token in libs.split_whitespace() {
        if let Some(path) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        } else if let Some(lib) = token.strip_prefix("-l") {
            println!("cargo:rustc-link-lib=dylib={lib}");
        } else if token.starts_with("-Wl") {
            println!("cargo:rustc-link-arg={token}");
        }
    }
    // The shim itself is C++; make sure the C++ runtime is linked.
    println!("cargo:rustc-link-lib=dylib=stdc++");
}

fn pkg_config(args: &[&str]) -> String {
    let output = Command::new("pkg-config")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to run pkg-config {args:?}: {e}"));
    if !output.status.success() {
        panic!(
            "pkg-config {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
