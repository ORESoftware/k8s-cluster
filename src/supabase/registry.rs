//! Issuer-keyed registry of per-project verifiers.
//!
//! An incoming Supabase token is untrusted until verified, but its unverified
//! `iss` claim is enough to *route* it to the one project able to verify it.
//! We then verify against exactly that project (which pins `iss` again), so a
//! forged `iss` only ever selects a verifier that will reject the signature.

use std::collections::HashMap;

use crate::config::SupabaseProject;
use crate::error::AuthError;

use super::claims::VerifiedIdentity;
use super::verifier::SupabaseVerifier;

pub struct ProjectRegistry {
    by_issuer: HashMap<String, SupabaseVerifier>,
}

impl ProjectRegistry {
    pub fn from_projects(projects: &[SupabaseProject]) -> anyhow::Result<Self> {
        let mut by_issuer = HashMap::with_capacity(projects.len());
        for project in projects {
            let verifier = SupabaseVerifier::new(project);
            if by_issuer
                .insert(verifier.issuer().to_string(), verifier)
                .is_some()
            {
                anyhow::bail!("duplicate Supabase issuer for project {}", project.name);
            }
        }
        Ok(Self { by_issuer })
    }

    pub fn len(&self) -> usize {
        self.by_issuer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_issuer.is_empty()
    }

    /// Verify a token by routing on its issuer. Unknown or missing issuer →
    /// `Unauthorized`, indistinguishable from a bad signature.
    pub async fn verify(
        &self,
        http: &reqwest::Client,
        token: &str,
    ) -> Result<VerifiedIdentity, AuthError> {
        if token.len() > 16 * 1024 {
            return Err(AuthError::Unauthorized);
        }
        let issuer = unverified_issuer(token).ok_or(AuthError::Unauthorized)?;
        let verifier = self.by_issuer.get(&issuer).ok_or(AuthError::Unauthorized)?;
        verifier.verify(http, token).await
    }
}

/// Read the `iss` claim WITHOUT verifying the signature — used only to pick a
/// verifier. The chosen verifier re-pins `iss` during real verification.
fn unverified_issuer(token: &str) -> Option<String> {
    let payload_b64 = token.split('.').nth(1)?;
    if payload_b64.len() > 12 * 1024 {
        return None;
    }
    let bytes = base64_url_decode(payload_b64)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("iss")?.as_str().map(String::from)
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SupabaseProject;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};

    fn hs_project(name: &str, reff: &str, secret: &str) -> SupabaseProject {
        SupabaseProject {
            name: name.into(),
            project_ref: reff.into(),
            issuer: Some(format!("https://{reff}.supabase.co/auth/v1")),
            jwks_url: Some("http://127.0.0.1:1/jwks".into()),
            audience: "authenticated".into(),
            hs256_secret: Some(secret.into()),
        }
    }

    fn token(reff: &str, secret: &str, sub: &str) -> String {
        let claims = serde_json::json!({
            "sub": sub, "aud": "authenticated",
            "iss": format!("https://{reff}.supabase.co/auth/v1"),
            "exp": chrono::Utc::now().timestamp() + 3600,
        });
        jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap()
    }

    // Multi-project: a token is routed by its issuer to the right project and
    // verified there. This is the core multi-org Supabase integration.
    #[tokio::test]
    async fn routes_token_to_its_issuing_project() {
        let reg = ProjectRegistry::from_projects(&[
            hs_project("fiducia-cloud", "fid", "secret-a"),
            hs_project("3fa-app", "threefa", "secret-b"),
        ])
        .unwrap();
        assert_eq!(reg.len(), 2);

        let id = reg
            .verify(
                &reqwest::Client::new(),
                &token("threefa", "secret-b", "u-b"),
            )
            .await
            .unwrap();
        assert_eq!(id.project, "3fa-app");
        assert_eq!(id.supabase_user_id, "u-b");
    }

    #[tokio::test]
    async fn unknown_issuer_is_unauthorized() {
        let reg =
            ProjectRegistry::from_projects(&[hs_project("fiducia-cloud", "fid", "s")]).unwrap();
        // Valid signature but an issuer no project owns.
        let res = reg
            .verify(&reqwest::Client::new(), &token("other", "s", "u"))
            .await;
        assert!(res.is_err());
    }

    #[test]
    fn duplicate_issuer_is_rejected_at_build() {
        let dup = ProjectRegistry::from_projects(&[
            hs_project("a", "same", "s1"),
            hs_project("b", "same", "s2"),
        ]);
        assert!(dup.is_err());
    }
}
