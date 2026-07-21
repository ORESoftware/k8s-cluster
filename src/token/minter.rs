//! Sign unified OreSoftware JWTs (ES256).

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use p256::pkcs8::{DecodePrivateKey, EncodePublicKey, LineEnding};
use p256::SecretKey;

use crate::config::SigningConfig;
use crate::error::AuthError;

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
    pub fn mint(
        &self,
        shared_user_id: &str,
        project: &str,
        supabase_user_id: &str,
        email: Option<String>,
        email_verified: bool,
    ) -> Result<MintedToken, AuthError> {
        let now = now_secs();
        let expires_at = now.saturating_add(self.ttl_secs);
        let claims = OreClaims {
            sub: shared_user_id.to_string(),
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            iat: now,
            exp: expires_at,
            project: project.to_string(),
            supabase_user_id: supabase_user_id.to_string(),
            email,
            email_verified,
        };
        let token = encode(&self.header, &claims, &self.encoding_key).map_err(|err| {
            tracing::error!(error = %err, "token signing failed");
            AuthError::Internal
        })?;
        Ok(MintedToken { token, expires_at })
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
            .mint(
                "shared-42",
                "fiducia-cloud",
                "sub-1",
                Some("a@b.co".into()),
                true,
            )
            .unwrap();
        let claims = m.verify(&minted.token).unwrap();
        assert_eq!(claims.sub, "shared-42");
        assert_eq!(claims.project, "fiducia-cloud");
        assert_eq!(claims.supabase_user_id, "sub-1");
        assert_eq!(claims.email.as_deref(), Some("a@b.co"));
        assert!(claims.email_verified);
        assert!(claims.exp > claims.iat);
    }

    #[test]
    fn tampered_token_is_rejected() {
        let m = minter();
        let minted = m.mint("s", "p", "u", None, false).unwrap();
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
