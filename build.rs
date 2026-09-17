fn main() {
    println!("cargo:rerun-if-changed=vendor/flags2env/parser.c");
    println!("cargo:rerun-if-changed=vendor/flags2env/parser.h");

    cc::Build::new()
        .file("vendor/flags2env/parser.c")
        .include("vendor/flags2env")
        .flag_if_supported("-std=c99")
        .compile("flags2env");
}
