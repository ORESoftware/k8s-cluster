//! Standalone compilation and cryptographic contract tests for billing auth.
//!
//! The billing binary currently has private sibling path dependencies, so a
//! standalone GitHub checkout cannot compile the whole application. This crate
//! imports the production auth source directly: a green result therefore proves
//! the exact verifier and authorization policy compile together, rather than a
//! copied test implementation compiling in their place.

#[path = "../../../src/supabase_auth.rs"]
pub mod supabase_auth;

/// The production middleware reads only these fields from the full application
/// config. Keeping the minimal shape here lets the exact middleware source
/// compile without pulling in unrelated database/provider configuration.
pub mod config {
    use crate::supabase_auth::SupabaseConfig;

    #[derive(Clone, Debug)]
    pub struct Config {
        pub api_auth_bearer: Option<String>,
        pub supabase: SupabaseConfig,
        pub tenant_routes_require_user_jwt: bool,
        pub step_up_required_for_mutations: bool,
    }
}

#[path = "../../../src/api/auth.rs"]
pub mod api_auth;

#[cfg(test)]
mod contract_tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::http::StatusCode;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use serde::Serialize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use uuid::Uuid;

    use crate::api_auth::{
        Action, AuthzPolicy, MAX_STEP_UP_AGE_SECS, Principal, TenantScope, authorize_request,
    };
    use crate::supabase_auth::{Aal, AuthError, SupabaseConfig, SupabaseVerifier};

    const ISSUER: &str = "https://shared-auth.contract.test";
    const AUDIENCE: &str = "oresoftware";
    const ACR_LOA2: &str = "urn:oresoftware:loa:2";
    const KEY_ID: &str = "contract-key";
    const PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgtHh2r6rPFBDDAWL2\nhEMj6Q29eVfuk8W63ogfB25dhbGhRANCAATQVDOM6hNuZGIc2Px0rMnOEVX0v7dt\nZ6TVPpLjRhoR9hwirXpUOYDC9QJ04JK1BwdigBz36DjJqhVyJQckv7qh\n-----END PRIVATE KEY-----\n";
    const JWKS: &str = r#"{"keys":[{"kty":"EC","crv":"P-256","x":"0FQzjOoTbmRiHNj8dKzJzhFV9L-3bWek1T6S40YaEfY","y":"HCKtelQ5gML1AnTgkrUHB2KAHPfoOMmqFXIlByS_uqE","use":"sig","alg":"ES256","kid":"contract-key"}]}"#;

    #[derive(Clone, Serialize)]
    struct Claims {
        sub: String,
        iss: String,
        aud: String,
        iat: u64,
        nbf: u64,
        exp: u64,
        roles: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        aal: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        acr: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        auth_time: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_tenant: Option<String>,
    }

    impl Claims {
        fn base(now: u64) -> Self {
            Self {
                sub: "shared-user-42".to_string(),
                iss: ISSUER.to_string(),
                aud: AUDIENCE.to_string(),
                iat: now,
                nbf: now.saturating_sub(5),
                exp: now.saturating_add(3_600),
                roles: Vec::new(),
                aal: Some(1),
                acr: Some("urn:oresoftware:loa:1".to_string()),
                auth_time: None,
                provider_tenant: None,
            }
        }

        fn entitled_writer(now: u64, tenant_id: Uuid) -> Self {
            let mut claims = Self::base(now);
            claims.roles = vec![
                format!("quaestor:tenant:{tenant_id}"),
                "quaestor:billing:write".to_string(),
            ];
            claims.aal = Some(2);
            claims.acr = Some(ACR_LOA2.to_string());
            claims.auth_time = Some(now);
            claims
        }
    }

    fn sign(claims: &Claims, kid: &str) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(kid.to_string());
        encode(
            &header,
            claims,
            &EncodingKey::from_ec_pem(PRIVATE_KEY_PEM.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn mutate_last_byte(token: &str) -> String {
        let mut bytes = token.as_bytes().to_vec();
        let last = bytes.last_mut().expect("JWT is non-empty");
        *last = if *last == b'a' { b'b' } else { b'a' };
        String::from_utf8(bytes).unwrap()
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    async fn serve_jwks(body: &'static str) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut request = [0_u8; 4_096];
                    let _ = socket.read(&mut request).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{address}/.well-known/jwks.json"), task)
    }

    fn policy() -> AuthzPolicy {
        AuthzPolicy {
            require_user_jwt: true,
            require_step_up_for_mutations: true,
        }
    }

    #[tokio::test]
    async fn shared_auth_signature_claims_and_billing_policy_are_one_contract() {
        let (jwks_url, server) = serve_jwks(JWKS).await;
        let verifier = SupabaseVerifier::from_config(&SupabaseConfig {
            url: None,
            audience: AUDIENCE.to_string(),
            issuer: Some(ISSUER.to_string()),
            jwks_url: Some(jwks_url),
            jwt_secret: None,
        })
        .expect("standalone contract config enables asymmetric verification");

        let now = now_secs();
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();

        // Happy path: signature, issuer, audience, namespaced tenant role,
        // namespaced write role, AAL2, and fresh auth_time all agree.
        let valid = sign(&Claims::entitled_writer(now, tenant_a), KEY_ID);
        let identity = verifier.verify(&valid).await.unwrap();
        assert_eq!(identity.assurance, Aal::Aal2);
        assert!(identity.is_entitled_to(tenant_a));
        assert!(!identity.is_entitled_to(tenant_b));
        assert!(identity.has_scope("billing:write"));
        assert_eq!(identity.step_up_age_secs(now), Some(0));

        let principal = Principal::User(Box::new(identity));
        assert_eq!(
            authorize_request(
                &principal,
                TenantScope::Tenant(tenant_a),
                Action::Read,
                policy(),
                now,
            ),
            Ok(())
        );
        assert_eq!(
            authorize_request(
                &principal,
                TenantScope::Tenant(tenant_a),
                Action::Mutate,
                policy(),
                now,
            ),
            Ok(())
        );
        assert_eq!(
            authorize_request(
                &principal,
                TenantScope::Tenant(tenant_b),
                Action::Read,
                policy(),
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        // Provider tenancy is identity-provider metadata, not billing tenancy.
        let mut provider_only = Claims::base(now);
        provider_only.provider_tenant = Some(tenant_a.to_string());
        let provider_only = verifier
            .verify(&sign(&provider_only, KEY_ID))
            .await
            .unwrap();
        assert!(provider_only.tenant_ids.is_empty());
        assert_eq!(
            authorize_request(
                &Principal::User(Box::new(provider_only)),
                TenantScope::Tenant(tenant_a),
                Action::Read,
                policy(),
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        // Tenant entitlement without the write role permits reads, not writes.
        let mut reader = Claims::base(now);
        reader.roles = vec![format!("quaestor:tenant:{tenant_a}")];
        reader.aal = Some(2);
        reader.acr = Some(ACR_LOA2.to_string());
        reader.auth_time = Some(now);
        let reader = Principal::User(Box::new(
            verifier.verify(&sign(&reader, KEY_ID)).await.unwrap(),
        ));
        assert_eq!(
            authorize_request(
                &reader,
                TenantScope::Tenant(tenant_a),
                Action::Read,
                policy(),
                now,
            ),
            Ok(())
        );
        assert_eq!(
            authorize_request(
                &reader,
                TenantScope::Tenant(tenant_a),
                Action::Mutate,
                policy(),
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        // The role is not enough without fresh AAL2.
        let mut aal1 = Claims::entitled_writer(now, tenant_a);
        aal1.aal = Some(1);
        aal1.acr = Some("urn:oresoftware:loa:1".to_string());
        aal1.auth_time = None;
        let aal1 = Principal::User(Box::new(
            verifier.verify(&sign(&aal1, KEY_ID)).await.unwrap(),
        ));
        assert_eq!(
            authorize_request(
                &aal1,
                TenantScope::Tenant(tenant_a),
                Action::Mutate,
                policy(),
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        let mut stale = Claims::entitled_writer(now, tenant_a);
        stale.auth_time = Some(now.saturating_sub(MAX_STEP_UP_AGE_SECS + 1));
        let stale = Principal::User(Box::new(
            verifier.verify(&sign(&stale, KEY_ID)).await.unwrap(),
        ));
        assert_eq!(
            authorize_request(
                &stale,
                TenantScope::Tenant(tenant_a),
                Action::Mutate,
                policy(),
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        let mut future = Claims::entitled_writer(now, tenant_a);
        future.auth_time = Some(now.saturating_add(31));
        let future = Principal::User(Box::new(
            verifier.verify(&sign(&future, KEY_ID)).await.unwrap(),
        ));
        assert_eq!(
            authorize_request(
                &future,
                TenantScope::Tenant(tenant_a),
                Action::Mutate,
                policy(),
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        let mut missing_time = Claims::entitled_writer(now, tenant_a);
        missing_time.auth_time = None;
        let missing_time = Principal::User(Box::new(
            verifier.verify(&sign(&missing_time, KEY_ID)).await.unwrap(),
        ));
        assert_eq!(
            authorize_request(
                &missing_time,
                TenantScope::Tenant(tenant_a),
                Action::Mutate,
                policy(),
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        // The anonymous process-wide bearer cannot mutate a tenant even if a
        // migration deployment temporarily allows it to pass tenant auth.
        assert_eq!(
            authorize_request(
                &Principal::Service,
                TenantScope::Tenant(tenant_a),
                Action::Mutate,
                AuthzPolicy {
                    require_user_jwt: false,
                    require_step_up_for_mutations: true,
                },
                now,
            ),
            Err(StatusCode::FORBIDDEN)
        );

        // Cryptographic and authority failures remain authentication failures,
        // never downgraded into an unscoped identity.
        let mut wrong_issuer = Claims::entitled_writer(now, tenant_a);
        wrong_issuer.iss = "https://attacker.invalid".to_string();
        assert_eq!(
            verifier.verify(&sign(&wrong_issuer, KEY_ID)).await,
            Err(AuthError::Unauthorized)
        );

        let mut wrong_audience = Claims::entitled_writer(now, tenant_a);
        wrong_audience.aud = "some-other-service".to_string();
        assert_eq!(
            verifier.verify(&sign(&wrong_audience, KEY_ID)).await,
            Err(AuthError::Unauthorized)
        );

        assert_eq!(
            verifier.verify(&mutate_last_byte(&valid)).await,
            Err(AuthError::Unauthorized)
        );
        assert_eq!(
            verifier
                .verify(&sign(
                    &Claims::entitled_writer(now, tenant_a),
                    "unknown-key",
                ))
                .await,
            Err(AuthError::Unauthorized)
        );

        let mut hs_header = Header::new(Algorithm::HS256);
        hs_header.kid = Some(KEY_ID.to_string());
        let hs_token = encode(
            &hs_header,
            &Claims::entitled_writer(now, tenant_a),
            &EncodingKey::from_secret(b"not-the-shared-auth-key"),
        )
        .unwrap();
        assert_eq!(
            verifier.verify(&hs_token).await,
            Err(AuthError::Unauthorized)
        );

        server.abort();
    }
}
