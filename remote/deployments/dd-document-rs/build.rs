// Links the native bridges into the binary based on the enabled features.
//
//   haskell-bridge -> link the prebuilt libdd-pandoc-bridge.so (Pandoc SDK)
//   magick-bridge  -> compile cpp/magick_bridge.cpp and link Magick++ (the
//                     official ImageMagick C++ SDK), discovered via pkg-config
//
// Build scripts don't see Cargo features as `cfg`, only as `CARGO_FEATURE_*`
// env vars, so we gate on those. `cargo build --no-default-features` skips both,
// leaving a crate that needs no native toolchain.
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PANDOC_BRIDGE_LIB_DIR");

    if std::env::var_os("CARGO_FEATURE_HASKELL_BRIDGE").is_some() {
        link_pandoc_bridge();
    }
    if std::env::var_os("CARGO_FEATURE_MAGICK_BRIDGE").is_some() {
        build_magick_bridge();
    }
}

fn link_pandoc_bridge() {
    // Where `cabal`/`ghc` dropped libdd-pandoc-bridge.so. The Dockerfile sets
    // this; default to the in-tree build output for local linking.
    let lib_dir = std::env::var("PANDOC_BRIDGE_LIB_DIR")
        .unwrap_or_else(|_| format!("{}/haskell/dist", env!("CARGO_MANIFEST_DIR")));
    println!("cargo:rustc-link-search=native={lib_dir}");
    println!("cargo:rustc-link-lib=dylib=dd-pandoc-bridge");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir}");
}

fn build_magick_bridge() {
    println!("cargo:rerun-if-changed=cpp/magick_bridge.cpp");

    // pkg-config gives us the include/define flags for Magick++ and the libs to
    // link against (Magick++ + MagickWand + MagickCore, plus their deps).
    let cflags = pkg_config(&["--cflags", "Magick++"]);
    let libs = pkg_config(&["--libs", "Magick++"]);

    let mut build = cc::Build::new();
    build.cpp(true).file("cpp/magick_bridge.cpp").flag("-std=c++17");
    for token in cflags.split_whitespace() {
        // -I... and -D... (quantum depth / HDRI) both matter for Magick++.
        build.flag(token);
    }
    build.compile("dd_magick_bridge"); // emits the static lib link directive

    // Emit link directives for the Magick++ shared libraries.
    for token in libs.split_whitespace() {
        if let Some(path) = token.strip_prefix("-L") {
            println!("cargo:rustc-link-search=native={path}");
        } else if let Some(lib) = token.strip_prefix("-l") {
            println!("cargo:rustc-link-lib=dylib={lib}");
        } else if token.starts_with("-F") || token.starts_with("-Wl") {
            println!("cargo:rustc-link-arg={token}");
        }
    }
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
