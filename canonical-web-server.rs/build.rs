fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_TEST_AUTH");

    let test_auth = std::env::var_os("CARGO_FEATURE_TEST_AUTH").is_some();
    let release_profile = std::env::var("PROFILE").as_deref() == Ok("release");
    if test_auth && release_profile {
        panic!("the test-auth feature is forbidden in release builds");
    }
}
