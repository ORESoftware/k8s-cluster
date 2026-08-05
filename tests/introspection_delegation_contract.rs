//! Source-level contract guard for the protected introspection response.
//!
//! The full HTTP/integration suites exercise authentication and token validity.
//! This focused check prevents a later response refactor from dropping either
//! side of the delegated token lineage required by downstream resource servers.

#[test]
fn protected_introspection_exposes_current_and_parent_token_ids() {
    let source = include_str!("../src/http/introspect.rs");

    assert!(
        source.contains("\"jti\": claims.jti"),
        "active protected introspection must expose the delegated token jti"
    );
    assert!(
        source.contains("\"parent_jti\": claims.parent_jti"),
        "active protected introspection must preserve the parent token lineage"
    );
    assert!(
        source.contains("authorize_caller(&state, &headers)"),
        "full token lineage must stay behind the independent service credential"
    );
}
