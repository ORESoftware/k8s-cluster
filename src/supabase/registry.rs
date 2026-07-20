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
        let issuer = unverified_issuer(token).ok_or(AuthError::Unauthorized)?;
        let verifier = self.by_issuer.get(&issuer).ok_or(AuthError::Unauthorized)?;
        verifier.verify(http, token).await
    }
}

/// Read the `iss` claim WITHOUT verifying the signature — used only to pick a
/// verifier. The chosen verifier re-pins `iss` during real verification.
fn unverified_issuer(token: &str) -> Option<String> {
    let payload_b64 = token.split('.').nth(1)?;
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
