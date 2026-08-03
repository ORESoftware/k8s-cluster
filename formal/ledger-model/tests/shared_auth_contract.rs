//! Shared Auth → Quaestor authorization-contract tests.
//!
//! The main billing crate currently depends on private sibling crates, so this
//! standalone manifest is the repository's always-buildable CI boundary. These
//! tests lock in the exact signed JSON shape that Shared Auth emits and the
//! tenant/fresh-step-up decisions Quaestor must make from it.

use serde_json::Value;
use uuid::Uuid;

const TENANT_A: &str = "11111111-1111-4111-8111-111111111111";
const TENANT_B: &str = "22222222-2222-4222-8222-222222222222";
const BILLING_WRITE: &str = "billing:write";
const MAX_STEP_UP_AGE_SECS: u64 = 15 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Read,
    Mutate,
}

#[derive(Debug, PartialEq, Eq)]
struct AuthorizationContext {
    tenant_ids: Vec<Uuid>,
    scopes: Vec<String>,
    aal2: bool,
    step_up_at: Option<u64>,
}

impl AuthorizationContext {
    fn from_signed_claims(claims: &Value) -> Self {
        let mut tenant_ids = Vec::new();
        let mut scopes = Vec::new();

        // Authorization deliberately reads app_metadata only. Supabase clients
        // can write user_metadata, so identically named fields there must never
        // grant billing access.
        if let Some(metadata) = claims.get("app_metadata").and_then(Value::as_object) {
            if let Some(raw) = metadata.get("tenant_id").and_then(Value::as_str) {
                push_tenant(&mut tenant_ids, raw);
            }
            if let Some(values) = metadata.get("tenant_ids").and_then(Value::as_array) {
                for raw in values.iter().filter_map(Value::as_str) {
                    push_tenant(&mut tenant_ids, raw);
                }
            }
            if let Some(values) = metadata
                .get("financial_scopes")
                .and_then(Value::as_array)
            {
                for raw in values.iter().filter_map(Value::as_str) {
                    let scope = raw.trim();
                    if !scope.is_empty() && !scopes.iter().any(|known| known == scope) {
                        scopes.push(scope.to_string());
                    }
                }
            }
        }

        let step_up_at = claims
            .get("amr")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("timestamp").and_then(Value::as_u64))
            .max();

        Self {
            tenant_ids,
            scopes,
            aal2: claims.get("aal").and_then(Value::as_str) == Some("aal2"),
            step_up_at,
        }
    }

    fn authorize(
        &self,
        tenant_id: Uuid,
        action: Action,
        now: u64,
    ) -> Result<(), &'static str> {
        if !self.tenant_ids.contains(&tenant_id) {
            return Err("wrong tenant");
        }
        if action == Action::Read {
            return Ok(());
        }
        if !self.aal2 {
            return Err("aal2 required");
        }
        let Some(step_up_at) = self.step_up_at else {
            return Err("fresh step-up required");
        };
        if now.saturating_sub(step_up_at) > MAX_STEP_UP_AGE_SECS {
            return Err("fresh step-up required");
        }
        if !self.scopes.iter().any(|scope| scope == BILLING_WRITE) {
            return Err("billing:write required");
        }
        Ok(())
    }
}

fn push_tenant(out: &mut Vec<Uuid>, raw: &str) {
    let Ok(id) = Uuid::parse_str(raw.trim()) else {
        return;
    };
    if !out.contains(&id) {
        out.push(id);
    }
}

fn shared_auth_v1_claims() -> Value {
    serde_json::json!({
        "sub": "shared-42",
        "iss": "https://auth.oresoftware.dev",
        "aud": "oresoftware",
        "iat": 1_000,
        "nbf": 1_000,
        "exp": 4_600,
        "jti": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "token_use": "access",
        "ver": 1,
        "project": "fiducia-cloud",
        "supabase_user_id": "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "email": "operator@example.com",
        "email_verified": true,
        "role": "authenticated",
        "aal": "aal2",
        "amr": [
            {"method": "password", "timestamp": 900},
            {"method": "totp", "timestamp": 1_000}
        ],
        "app_metadata": {
            "tenant_ids": [TENANT_A],
            "financial_scopes": [BILLING_WRITE]
        }
    })
}

#[test]
fn shared_auth_v1_allows_only_entitled_fresh_scoped_mutations() {
    let context = AuthorizationContext::from_signed_claims(&shared_auth_v1_claims());
    let tenant_a = Uuid::parse_str(TENANT_A).unwrap();
    let tenant_b = Uuid::parse_str(TENANT_B).unwrap();

    assert_eq!(context.authorize(tenant_a, Action::Read, 1_100), Ok(()));
    assert_eq!(context.authorize(tenant_a, Action::Mutate, 1_100), Ok(()));
    assert_eq!(
        context.authorize(tenant_b, Action::Read, 1_100),
        Err("wrong tenant")
    );
    assert_eq!(
        context.authorize(tenant_b, Action::Mutate, 1_100),
        Err("wrong tenant")
    );
}

#[test]
fn missing_or_stale_step_up_fails_closed_without_blocking_reads() {
    let tenant_a = Uuid::parse_str(TENANT_A).unwrap();

    let mut claims = shared_auth_v1_claims();
    claims["aal"] = Value::String("aal1".into());
    let context = AuthorizationContext::from_signed_claims(&claims);
    assert_eq!(context.authorize(tenant_a, Action::Read, 1_100), Ok(()));
    assert_eq!(
        context.authorize(tenant_a, Action::Mutate, 1_100),
        Err("aal2 required")
    );

    let context = AuthorizationContext::from_signed_claims(&shared_auth_v1_claims());
    assert_eq!(
        context.authorize(tenant_a, Action::Mutate, 1_901),
        Err("fresh step-up required")
    );
}

#[test]
fn user_metadata_and_malformed_ids_never_grant_a_tenant() {
    let claims = serde_json::json!({
        "aal": "aal2",
        "amr": [{"timestamp": 1_000}],
        "user_metadata": {
            "tenant_ids": [TENANT_A],
            "financial_scopes": [BILLING_WRITE]
        },
        "app_metadata": {
            "tenant_ids": ["not-a-uuid"],
            "financial_scopes": [BILLING_WRITE]
        }
    });
    let context = AuthorizationContext::from_signed_claims(&claims);
    assert!(context.tenant_ids.is_empty());
    assert_eq!(
        context.authorize(Uuid::parse_str(TENANT_A).unwrap(), Action::Mutate, 1_100),
        Err("wrong tenant")
    );
}

#[test]
fn legacy_shared_auth_token_has_no_implicit_billing_grants() {
    // Version-zero tokens minted before authorization propagation contain only
    // stable identity fields. Quaestor must parse them as unentitled/AAL1 rather
    // than treating absent claims as wildcards.
    let legacy = serde_json::json!({
        "sub": "shared-42",
        "iss": "https://auth.oresoftware.dev",
        "aud": "oresoftware",
        "iat": 1_000,
        "exp": 4_600,
        "project": "fiducia-cloud",
        "supabase_user_id": "upstream-user"
    });
    let context = AuthorizationContext::from_signed_claims(&legacy);
    assert!(context.tenant_ids.is_empty());
    assert!(context.scopes.is_empty());
    assert!(!context.aal2);
    assert!(context.step_up_at.is_none());
}

#[test]
fn production_source_still_uses_the_same_load_bearing_claim_names() {
    // This companion tripwire makes a token-contract rename an intentional
    // two-repository change instead of a silently green issuer/consumer drift.
    let verifier = include_str!("../../../src/supabase_auth.rs");
    let authorization = include_str!("../../../src/api/auth.rs");

    for field in [
        "tenant_id",
        "tenant_ids",
        "financial_scopes",
        "aal",
        "amr",
    ] {
        assert!(
            verifier.contains(field),
            "production verifier no longer references Shared Auth field {field:?}"
        );
    }
    assert!(authorization.contains("billing:write"));
    assert!(authorization.contains("MAX_STEP_UP_AGE_SECS"));
}
