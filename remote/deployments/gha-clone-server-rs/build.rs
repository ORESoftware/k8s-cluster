use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let output = PathBuf::from(env::var("OUT_DIR").expect("out dir")).join("main_generated.rs");
    let parts = [
        "src/parts/part01.rs",
        "src/parts/part02.rs",
        "src/parts/part03.rs",
        "src/parts/part04.rs",
    ];
    let mut source = String::new();
    for relative in parts {
        println!("cargo:rerun-if-changed={relative}");
        source.push_str(
            &fs::read_to_string(manifest.join(relative))
                .unwrap_or_else(|error| panic!("failed to read {relative}: {error}")),
        );
    }
    fs::write(output, source).expect("write generated Rust source");
}
