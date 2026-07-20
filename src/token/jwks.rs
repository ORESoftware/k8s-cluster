//! Derive our public JWKS from the EC signing key so downstream services can
//! verify the tokens we mint. Only the public half is ever exposed.

use p256::pkcs8::DecodePrivateKey;
use p256::SecretKey;

/// The `{ "keys": [ … ] }` document served at `/.well-known/jwks.json`.
#[derive(Clone)]
pub struct PublicJwks {
    json: serde_json::Value,
}

impl PublicJwks {
    /// Build the JWKS from the PKCS#8 EC private PEM, advertising `kid`/ES256.
    pub fn from_ec_pem(pem: &str, kid: &str) -> anyhow::Result<Self> {
        let secret = SecretKey::from_pkcs8_pem(pem)
            .map_err(|e| anyhow::anyhow!("parsing EC signing key: {e}"))?;

        // p256 emits the public coordinates as {kty, crv, x, y}; we enrich it
        // with the sig-use metadata verifiers select on.
        let jwk = secret.public_key().to_jwk();
        let mut obj = serde_json::to_value(&jwk)?;
        if let Some(map) = obj.as_object_mut() {
            map.insert(
                "kid".to_string(),
                serde_json::Value::String(kid.to_string()),
            );
            map.insert(
                "use".to_string(),
                serde_json::Value::String("sig".to_string()),
            );
            map.insert(
                "alg".to_string(),
                serde_json::Value::String("ES256".to_string()),
            );
        }

        Ok(Self {
            json: serde_json::json!({ "keys": [obj] }),
        })
    }

    pub fn as_json(&self) -> &serde_json::Value {
        &self.json
    }
}
