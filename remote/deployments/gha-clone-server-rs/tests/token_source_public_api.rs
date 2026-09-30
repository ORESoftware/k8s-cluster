use gha_clone_server::credentials::TokenSource;

#[test]
fn public_token_source_validates_reads_and_redacts_inline_tokens() {
    let token = "example_installation_token_value_123456";
    let source =
        TokenSource::from_values(Some(token.to_string()), None, "INLINE_TOKEN", "TOKEN_FILE")
            .expect("valid token source")
            .expect("configured token source");

    assert_eq!(source.kind(), "environment");
    assert_eq!(source.read().expect("read token"), token);

    let debug = format!("{source:?}");
    assert_eq!(debug, "Inline(<redacted>)");
    assert!(!debug.contains(token));
}

#[test]
fn public_token_source_fails_closed_on_ambiguous_configuration() {
    let error = TokenSource::from_values(
        Some("example_installation_token_value_123456".to_string()),
        Some("/var/run/example/token".to_string()),
        "INLINE_TOKEN",
        "TOKEN_FILE",
    )
    .expect_err("ambiguous token sources must be rejected");

    assert!(error.contains("configure exactly one"));
    assert!(error.contains("INLINE_TOKEN"));
    assert!(error.contains("TOKEN_FILE"));
}
