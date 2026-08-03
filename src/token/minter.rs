//! Sign unified OreSoftware JWTs (ES256).

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use p256::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};
use p256::SecretKey;

use crate::config::SigningConfig;
use crate::error::AuthError;

use super::assurance::AuthenticationAssurance;
use super::claims::OreClaims;
use super::jwks::PublicJwks;

pub struct TokenMinter {
    encoding_key: EncodingKey,
    header: Header,
    issuer: String,
    audience: String,
    ttl_secs: u64,
    jwks: PublicJwks,
    /// Verification side, so this server can also validate the tokens it minted
    /// (`/auth/introspect`, `/auth/verify`) without a network round-trip.
    decoding_key: DecodingKey,
    validation: Validation,
}

/// A freshly minted token and its absolute expiry (unix seconds).
pub struct MintedToken {
    pub token: String,
    pub expires_at: u64,
    pub amr: Vec<String>,
    pub acr: Option<String>,
}

pub struct MintContext {
    pub shared_user_id: String,
    pub session_id: Option<uuid::Uuid>,
    pub provider: String,
    pub provider_tenant: String,
    pub provider_subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub roles: Vec<String>,
    pub assurance: AuthenticationAssurance,
}

impl TokenMinter {
    pub fn from_config(config: &SigningConfig) -> anyhow::Result<Self> {
        let encoding_key = EncodingKey::from_ec_pem(config.ec_private_pem.as_bytes())
            .map_err(|e| anyhow::anyhow!("loading EC signing key: {e}"))?;
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(config.key_id.clone());
        let jwks = PublicJwks::from_ec_pem(&config.ec_private_pem, &config.key_id)?;

        // Derive the public-key PEM once for our own verification side.
        let secret = SecretKey::from_pkcs8_pem(&config.ec_private_pem)
            .map_err(|e| anyhow::anyhow!("parsing EC signing key: {e}"))?;
        let public_pem = secret
            .public_key()
            .to_public_key_pem(LineEnding::LF)
            .map_err(|e| anyhow::anyhow!("encoding public key: {e}"))?;
        let decoding_key = DecodingKey::from_ec_pem(public_pem.as_bytes())
            .map_err(|e| anyhow::anyhow!("building decoding key: {e}"))?;
        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_issuer(&[config.issuer.as_str()]);
        validation.set_audience(&[config.audience.as_str()]);
        validation.validate_exp = true;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat", "nbf"]);

        Ok(Self {
            encoding_key,
            header,
            issuer: config.issuer.clone(),
            audience: config.audience.clone(),
            ttl_secs: config.ttl_secs,
            jwks,
            decoding_key,
            validation,
        })
    }

    /// Validate a token this server previously minted, returning its claims.
    pub fn verify(&self, token: &str) -> Result<OreClaims, AuthError> {
        decode::<OreClaims>(token, &self.decoding_key, &self.validation)
            .map(|data| data.claims)
            .map_err(|_| AuthError::Unauthorized)
    }

    /// The public JWKS document verifiers fetch.
    pub fn jwks(&self) -> &PublicJwks {
        &self.jwks
    }

    /// Mint a token for a resolved OreSoftware identity.
    ///
    /// A local AAL2 token is minted only after this process completed a verified
    /// step-up ceremony, so its `auth_time` is the current time. Provider
    /// exchanges that need to preserve an upstream factor timestamp use
    /// [`Self::mint_with_auth_time`] instead.
    pub fn mint(&self, context: MintContext) -> Result<MintedToken, AuthError> {
        self.mint_with_auth_time(context, None)
    }

    /// Mint while preserving a verified upstream authentication timestamp.
    ///
    /// `verified_auth_time` is load-bearing only for AAL2. AAL1 tokens never
    /// carry `auth_time`. For local AAL2 flows the caller passes `None`, and the
    /// completion time of the server-owned ceremony is used. Exchange adapters
    /// pass the newest timestamp extracted from the already verified provider
    /// token; if they cannot establish one they must downgrade assurance before
    /// calling this method.
    pub fn mint_with_auth_time(
        &self,
        context: MintContext,
        verified_auth_time: Option<u64>,
    ) -> Result<MintedToken, AuthError> {
        let now = now_secs();
        let expires_at = now.saturating_add(self.ttl_secs);
        let is_supabase = context.provider == "supabase";
        let assurance_level = context.assurance.level();
        let auth_time = (assurance_level >= 2).then(|| verified_auth_time.unwrap_or(now));
        let claims = OreClaims {
            sub: context.shared_user_id,
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            iat: now,
            exp: expires_at,
            nbf: now.saturating_sub(5),
            jti: uuid::Uuid::new_v4().to_string(),
            auth_time,
            sid: context.session_id.map(|id| id.to_string()),
            project: is_supabase.then(|| context.provider_tenant.clone()),
            supabase_user_id: is_supabase.then(|| context.provider_subject.clone()),
            provider: context.provider,
            provider_tenant: context.provider_tenant,
            provider_subject: context.provider_subject,
            email: context.email,
            email_verified: context.email_verified,
            roles: context.roles,
            // Derived from the ACR so `aal` and `acr` can never disagree.
            aal: assurance_level,
            amr: context.assurance.amr.clone(),
            acr: context.assurance.acr.clone(),
        };
        let token = encode(&self.header, &claims, &self.encoding_key).map_err(|err| {
            tracing::error!(error = %err, "token signing failed");
            AuthError::Internal
        })?;
        Ok(MintedToken {
            token,
            expires_at,
            amr: context.assurance.amr,
            acr: context.assurance.acr,
        })
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SigningConfig;
    use crate::token::{AuthenticationAssurance, ACR_LOA1, ACR_LOA2};

    use p256::pkcs8::{EncodePrivateKey, LineEnding};

    fn signing_pem() -> String {
        p256::SecretKey::from_slice(&[7u8; 32])
            .unwrap()
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string()
    }

    fn minter() -> TokenMinter {
        TokenMinter::from_config(&SigningConfig {
            ec_private_pem: signing_pem(),
            key_id: "test-kid".to_string(),
            issuer: "https://auth.test".to_string(),
            audience: "oresoftware".to_string(),
            ttl_secs: 3600,
        })
        .unwrap()
    }

    // Our minted tokens verify against our own key with NO Supabase dependency —
    // the half of tandem-resilience that keeps downstream auth alive even if
    // Supabase is fully down.
    #[test]
    fn mint_then_verify_roundtrip() {
        let m = minter();
        let minted = m
            .mint(MintContext {
                shared_user_id: "shared-42".into(),
                session_id: Some(uuid::Uuid::from_u128(42)),
                provider: "supabase".into(),
                provider_tenant: "fiducia-cloud".into(),
                provider_subject: "sub-1".into(),
                email: Some("a@b.co".into()),
                email_verified: true,
                roles: vec!["user".into()],
                assurance: AuthenticationAssurance::local_password(),
            })
            .unwrap();
        let claims = m.verify(&minted.token).unwrap();
        assert_eq!(claims.sub, "shared-42");
        assert_eq!(claims.project.as_deref(), Some("fiducia-cloud"));
        assert_eq!(claims.supabase_user_id.as_deref(), Some("sub-1"));
        assert_eq!(claims.provider, "supabase");
        assert_eq!(claims.roles, vec!["user"]);
        assert_eq!(claims.email.as_deref(), Some("a@b.co"));
        assert!(claims.email_verified);
        assert_eq!(claims.amr, vec!["pwd"]);
        assert_eq!(claims.acr.as_deref(), Some(ACR_LOA1));
        assert_eq!(claims.auth_time, None);
        assert_eq!(minted.amr, vec!["pwd"]);
        assert_eq!(minted.acr.as_deref(), Some(ACR_LOA1));
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn local_aal2_token_records_ceremony_completion_time() {
        let m = minter();
        let before = now_secs();
        let minted = m
            .mint(MintContext {
                shared_user_id: "shared-42".into(),
                session_id: Some(uuid::Uuid::from_u128(42)),
                provider: "local".into(),
                provider_tenant: "default".into(),
                provider_subject: "shared-42".into(),
                email: None,
                email_verified: false,
                roles: vec!["user".into()],
                assurance: AuthenticationAssurance::step_up(&["pwd".into()], "totp"),
            })
            .unwrap();
        let claims = m.verify(&minted.token).unwrap();
        assert_eq!(claims.acr.as_deref(), Some(ACR_LOA2));
        assert_eq!(claims.aal, 2);
        assert!(claims
            .auth_time
            .is_some_and(|at| at >= before && at <= now_secs()));
    }

    #[test]
    fn exchanged_aal2_token_preserves_verified_upstream_auth_time() {
        let m = minter();
        let upstream_auth_time = 1_700_000_003;
        let minted = m
            .mint_with_auth_time(
                MintContext {
                    shared_user_id: "shared-42".into(),
                    session_id: Some(uuid::Uuid::from_u128(42)),
                    provider: "supabase".into(),
                    provider_tenant: "fiducia-cloud".into(),
                    provider_subject: "sub-1".into(),
                    email: None,
                    email_verified: false,
                    roles: vec!["user".into()],
                    assurance: AuthenticationAssurance::step_up(&["pwd".into()], "totp"),
                },
                Some(upstream_auth_time),
            )
            .unwrap();
        let claims = m.verify(&minted.token).unwrap();
        assert_eq!(claims.auth_time, Some(upstream_auth_time));
    }

    #[test]
    fn aal1_ignores_supplied_auth_time() {
        let m = minter();
        let minted = m
            .mint_with_auth_time(
                MintContext {
                    shared_user_id: "shared-42".into(),
                    session_id: None,
                    provider: "local".into(),
                    provider_tenant: "default".into(),
                    provider_subject: "shared-42".into(),
                    email: None,
                    email_verified: false,
                    roles: vec![],
                    assurance: AuthenticationAssurance::local_password(),
                },
                Some(1_700_000_003),
            )
            .unwrap();
        assert_eq!(m.verify(&minted.token).unwrap().auth_time, None);
    }

    #[test]
    fn tampered_token_is_rejected() {
        let m = minter();
        let minted = m
            .mint(MintContext {
                shared_user_id: "s".into(),
                session_id: None,
                provider: "local".into(),
                provider_tenant: "default".into(),
                provider_subject: "u".into(),
                email: None,
                email_verified: false,
                roles: vec![],
                assurance: AuthenticationAssurance::local_password(),
            })
            .unwrap();
        let mut bad = minted.token.clone();
        bad.push('x'); // corrupt the signature segment
        assert!(m.verify(&bad).is_err());
    }

    #[test]
    fn jwks_advertises_our_kid_and_es256() {
        let m = minter();
        let jwks = m.jwks().as_json();
        let key = &jwks["keys"][0];
        assert_eq!(key["kid"], "test-kid");
        assert_eq!(key["alg"], "ES256");
        assert_eq!(key["kty"], "EC");
        assert_eq!(key["use"], "sig");
    }
}
