use shared_auth_server::config::{SupabaseApiKeys, SupabaseProject};
use shared_auth_server::supabase::ProjectRegistry;

use jsonwebtoken::{Algorithm, EncodingKey, Header};

const CUSTOMER_PROJECT: &str = "fiducia-customer";
const ADMIN_PROJECT: &str = "fiducia-admin";
const CUSTOMER_REF: &str = "fiduciacustomerref";
const ADMIN_REF: &str = "fiduciaadminref";
const CUSTOMER_SECRET: &str = "customer-test-secret-at-least-32-bytes";
const ADMIN_SECRET: &str = "admin-test-secret-at-least-32-bytes---";

fn project(name: &str, project_ref: &str, secret: &str) -> SupabaseProject {
    SupabaseProject {
        name: name.to_string(),
        project_ref: project_ref.to_string(),
        issuer: None,
        jwks_url: None,
        audience: "authenticated".to_string(),
        publishable_key_env: None,
        secret_key_env: None,
        service_role_key_env: None,
        jwt_secret_env: None,
        api_keys: SupabaseApiKeys::default(),
        hs256_secret: Some(secret.to_string()),
    }
}

fn token(project_ref: &str, secret: &str, subject: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = serde_json::json!({
        "sub": subject,
        "aud": "authenticated",
        "iss": format!("https://{project_ref}.supabase.co/auth/v1"),
        "iat": now,
        "exp": now + 300,
        "email": format!("{subject}@example.invalid"),
        "email_verified": true,
        "aal": "aal2",
        "amr": [{"method": "password"}, {"method": "otp"}],
        "app_metadata": {
            "fiducia_roles": if project_ref == ADMIN_REF { ["operator"] } else { ["customer"] }
        }
    });
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap()
}

#[tokio::test]
async fn fiducia_customer_and_admin_tokens_route_to_distinct_supabase_projects() {
    let registry = ProjectRegistry::from_projects(&[
        project(CUSTOMER_PROJECT, CUSTOMER_REF, CUSTOMER_SECRET),
        project(ADMIN_PROJECT, ADMIN_REF, ADMIN_SECRET),
    ])
    .unwrap();

    assert_eq!(registry.len(), 2);

    let customer = registry
        .verify(
            &reqwest::Client::new(),
            &token(CUSTOMER_REF, CUSTOMER_SECRET, "customer-user"),
        )
        .await
        .unwrap();
    assert_eq!(customer.project, CUSTOMER_PROJECT);
    assert_eq!(customer.supabase_user_id, "customer-user");
    assert_eq!(customer.auth_level, 2);

    let admin = registry
        .verify(
            &reqwest::Client::new(),
            &token(ADMIN_REF, ADMIN_SECRET, "admin-user"),
        )
        .await
        .unwrap();
    assert_eq!(admin.project, ADMIN_PROJECT);
    assert_eq!(admin.supabase_user_id, "admin-user");
    assert_eq!(admin.auth_level, 2);
}

#[tokio::test]
async fn fiducia_cross_project_signature_confusion_is_rejected() {
    let registry = ProjectRegistry::from_projects(&[
        project(CUSTOMER_PROJECT, CUSTOMER_REF, CUSTOMER_SECRET),
        project(ADMIN_PROJECT, ADMIN_REF, ADMIN_SECRET),
    ])
    .unwrap();

    // The unverified issuer selects the admin verifier, but the token was signed
    // with the customer secret. Routing must never become trust.
    let confused = token(ADMIN_REF, CUSTOMER_SECRET, "cross-plane-user");
    assert!(registry
        .verify(&reqwest::Client::new(), &confused)
        .await
        .is_err());
}

#[test]
fn shared_auth_postgres_contract_owns_users_sessions_roles_and_provider_links() {
    let schema = include_str!("../db/schema.sql");
    for relation in [
        "shared_auth.principals",
        "shared_auth.provider_identities",
        "shared_auth.sessions",
        "shared_auth.roles",
    ] {
        assert!(
            schema.contains(relation),
            "missing required relation {relation}"
        );
    }
    let compact = schema.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(compact.contains("unique (provider, provider_tenant, provider_subject)"));
    assert!(compact.contains("refresh_token_hash text not null unique"));
    assert!(compact.contains("unique (shared_user_id, role_name)"));
    assert!(compact.contains("references shared_auth.principals(shared_user_id) on delete cascade"));
}
