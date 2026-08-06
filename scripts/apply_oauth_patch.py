#!/usr/bin/env python3
from __future__ import annotations

import base64
import gzip
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

DB_OAUTH = "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"
SCHEMA_BLOCK = "H4sIAAjlc2oC/71XTW/jNhC9+1fMoUBswPImXm/6sWiBtEBb9NAA6z212Ao0NbbYUKRKUrGdov3tOyQlS3IUr9OmFYJAlsg3w5n3ZkajJIFlro1LpLjHDFZGby0aYJWjp+KBOaEVOMOUZdzf2xm8zxG4FKgcGMyEQe6AqWxEUJbrEi0wQyt0KQhwbXRBbwF3zC8jjHukbRthCTSAMw/zR4XWgRMFTsHlqDyYwXsmRcYcwWzpmX9BsBmCsPRnK3pOdumRslWB2Qy+rb3/bvnue7DIDTrbuNY/kYeJflqn6RSgldwDs7D88SaZv7mGFbN4vaiMhExsyDc7GxEc+QKOrSS5sAalHR2LDmLB5gSVpd7GTPv/ac9cWh/QwngEdNU/U5FBe1VV+7M0omBmD3e4hwzXrJIONkg4dBhdpH7peDINWDETfShwuHPNvXdTVVKS1TUaVByH/I0wdnyAm1BIyLZEOjJnlrMMp7XzMedpZcRpgzxHfgdjiWrj8nF33wRW6LZIOb0KKZxfLr6oz2OdD/LRdQpec4curY2E3cfoV5fzRRMtynvKcyb9ejwD/WjD33Dx2683yS8sebhMvkyTD38uXv/12UXje6R///rdarU6Qo/LlaZkDB01vq61mEYipzmz+VmBHtg3ga9h8bqJQeAxZd91zZL0KHhF6R5a3IZ6Sm8buuGupCzaT29u4l2UnkI9a53l9arof4hU6vYl6vU4xtI7fsGMYfsmxr214c0h9/WOo+Rf9zfGoFMFCSfUBnr8CW+PId5czfsYnSB80wnnZDR5O2rKhFAZ7o7KxKnSkMbaSMrbBVOkvvOrykG1006CJgGHKmeoxp081IcnX6ku3liruSCPqRySr6ZgFBWFiU8SYMGE9CfMCF7Qm61weazlVC5pWUPSxxW20zNm8AMqNIJDyazdapNJtLau22VYTV5LwQWheiTccVn5Al2wsqTi7PQdKjulsoj33hG1IV9Lo7OKeooVG5UIBVKou9huClGv8VjkIKMeQdsM+W/JcPAuN7ra5KGl/LS8/RlQZaWmfc8s8wXbCJ5608c1PvjcFW23pj9Rijto8cjjFuVEPW6bSegh5xX8YSqNW7QnDXbrx6fLxnmaGIhj2rpyQhGDCWh3TrveEqP4JLL+tmS0hLijNhKTyuLQfDCDWz8VeI7EIcArJ84LYQqq6RVGjoZYxktG2YZwb8PukEXP6pz5JhJdCnrDrQeJ8n3Fqkz4dCW2RC7WpBeiP8kiutRYWKHU2381jcThZ9w2xMjSwW7bZW2/zRx29prL0GjTnWwOJKmU8Bn4jzja9sAQtFYcZw5F9T47bgHO0sNLtNNT3TROuuc005fsUYEw/6BB1UR7qiW1h+l1pGUMebLSFTXfrgRqdLefwTtcE2QORrtGsfS9YUluhEXJJ8156cXviFIyjoX/YGlExJwuCFXK/RSs9h8oXTumBo+65fRWUeMxJD2PV5UbQ8nPKHzRCK1nsriwR72mkfNztdo60tC3EesjNp/RU57F5M63RCgCL/3t0IQEHhmoWXtY0B+0P7S3s/SrVzR2z6dX88+HJ+84cB8p51k963+ZSs8U4gAb0kOgTylxkEadQfFxY/wINlIPhw0QAAA="
EMAIL = "H4sIAILlc2oC/9VX3W/bNhB/91/BqJgrAY60YUAfFMdDkQRFgHYd4uZhaAOClmibjSSyJBXHS/2/746SJdpxgiLbHuYgjnh3uo/fffCSJEfko7JCVqwgU17l77TISc4Lccf1msylJlfn0+MZy255ThQzZiV1XnBjiBGL6lhU8SBJjvCXfFpyotmKyIofW1FyYuUtr4gwpOKgjRRysQAloNJYqXkekzNZzcWi1gwdQEHZupI6haslvm6RwWaGV3YEuos1sWCoZAuRHReiuiWaf6u5sQS8V1JUTr6u2B0TBZsVPB4MasOJ4Trn9KuRVZri94mj1rXI0/Qavk8asQyc4WmaOc/S9AOaeQ9WGldPfBmutdRp+ra2ywt8BA3wGjgyvfj9/N3V5Tn98PbyPcUTvb56n5KhsZqckmBprTJpkjAlYggrXwDmcSbL5O7XpASvEyQGoE7VM8LMusrIvCJIpC5simGHAwIfVAWKAYIVQJCmZ4VAnByvjYEM94JouJpnQqFw41dDdBnzCdKp745omM6Y4VRCRsFtnrblM0ahSSPFABGpxV8urbRNDwWgt7KIN8hG5HhCrripCzsOoxHpkJyQB6doL2RqZRP1NvJRd8q8yPBzOAU9v4u+J7nY+6P01T+Ou+c9FW0jEbnvmK2YsIPNYPDqMysKuQqzQii1TlMrJURYrSnTi7oEh0x0M3gq5x0AL0/7tkn8rP4fSqHglkC/0Fu+hhZqYuxS0HURbUV6DjM055rPw6inyVsqddiZgP7vp0X020lnb65lSTl2ZGcy7mme6h/W6ObVKfEyWusifLKK90ryH1Sh5wM3GVM8p60vzZEubVmEQ6RFvajl9xZE4BYomT3q/QyuYQraJQ7s7ax32nC0i/vjXCxgamcyxyvA3RME7gm8H9aE3yuhOZzJw4aUoqotN+mXCn467cBBwpnEkmqePeblnKxlTXK4pippu+HvvHF5GTl+xsDoopJoy8bB/qSIrS2o4ZkhCXnz8y7EOzOgga+HBHE6CMlYTf5dVOJxoiZe4GBgzMgSCu70SwCwBJNpo2OcsMlj2Qa+MbSirBaTh804aR8fi74IUlTzw7D6NfcsvIqtC8lyQBjv6KPwoQdYcW1wN2hr3AQp+dyznYiVjkgC5zU8d5ONbG460c1N70KA/QyCe4q27/fdPtoVqFjJge8PBST1NjwTpp595ZkF8eBPWevt4uSKw0MwAGUWXMUQdoxBPHat0FyADZmogokqGJHgjhU1kl2bbkbPvoR167/j6njTvdKgs4FMdKnQ3CiAmUMu8Lrpx6eSxobbe8QbqzPONNcUJ1HYzmGPi/kMh21+o93R7Q/n5qbsjiVTFGZd+N1tW9/3EmU1y0QFe9qKaaiWn5wQRNltsgfWxDlkk+dBW3Tbjz+7FXQKZ2WfzO34FPMOldhYZmsTRuTolHT38NQRXeulb8/OLv74dHHuubzn7o4DjT5y+tgCXjT1L2/gWtytwS5IzbG++E60TQV3L3jRam5rXREINjwQdCvYVMbH2zCMIlxbYBc5dGk9v2s8WiH+u51hajUA+8TegBYB2MfWvZVAh20v91IQpXfHv2B/KGtLQAnxCuRaF2mqmDY8RBtRX9/0u1+DlzAKNAy7rTr0BXDQa6qY0IaCaiwMpXBBRFIYOLShxd3fqCtYdGQqSx72QEbg0VMge9V60OZOCe460CsBL4b9KbaSGpcfQHGvvNAGTBHZl5m/j7hh1RSQS3aT5dZFx+yTojnMxYyHr4evYQAMWalOgugAO3Dsb7W0h/ljxy+e4E4cd+G4bpvP5osQLmsLW/urz4rZJf6D55qPItnE2gQ3g1LmxB1PBn8DFjnav3IPAAA="
EMAIL_TESTS = "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"
SESSION_TOKENS = "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"


def decoded(value: str) -> str:
    return gzip.decompress(base64.b64decode(value)).decode("utf-8")


def read(path: str) -> str:
    return (ROOT / path).read_text()


def write(path: str, content: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content)


def replace_once(path: str, old: str, new: str) -> None:
    content = read(path)
    if new in content:
        return
    count = content.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one patch seam, found {count}: {old[:100]!r}")
    write(path, content.replace(old, new, 1))


write("src/db/oauth.rs", decoded(DB_OAUTH))
write("src/email.rs", decoded(EMAIL))
write("src/email_tests.rs", decoded(EMAIL_TESTS))
write("src/http/session_tokens.rs", decoded(SESSION_TOKENS))

replace_once(
    "src/db/mod.rs",
    "use crate::supabase::VerifiedIdentity;\n",
    "use crate::supabase::VerifiedIdentity;\n\nmod oauth;\npub use oauth::*;\n",
)

replace_once(
    "src/db/mod.rs",
    "UPDATE shared_auth.magic_link_tokens SET consumed_at = now() \\\n                 WHERE token_hash = $1 AND consumed_at IS NULL AND expires_at > now() \\\n                 RETURNING shared_user_id",
    "UPDATE shared_auth.magic_link_tokens AS ml SET consumed_at = now() \\\n                 WHERE ml.token_hash = $1 AND ml.consumed_at IS NULL AND ml.expires_at > now() \\\n                   AND NOT EXISTS ( \\\n                     SELECT 1 FROM shared_auth.oauth_magic_link_requests om \\\n                     WHERE om.token_hash = ml.token_hash \\\n                   ) \\\n                 RETURNING ml.shared_user_id",
)

replace_once(
    "src/db/mod.rs",
    "SELECT token_hash FROM shared_auth.magic_link_tokens \\\n                    WHERE identifier_hash = $1 AND otp_hash = $2 \\\n                      AND consumed_at IS NULL AND expires_at > now() \\\n                      AND failed_attempts < 5 \\\n                    ORDER BY created_at DESC LIMIT 1 FOR UPDATE",
    "SELECT ml.token_hash FROM shared_auth.magic_link_tokens ml \\\n                    WHERE ml.identifier_hash = $1 AND ml.otp_hash = $2 \\\n                      AND ml.consumed_at IS NULL AND ml.expires_at > now() \\\n                      AND ml.failed_attempts < 5 \\\n                      AND NOT EXISTS ( \\\n                        SELECT 1 FROM shared_auth.oauth_magic_link_requests om \\\n                        WHERE om.token_hash = ml.token_hash \\\n                      ) \\\n                    ORDER BY ml.created_at DESC LIMIT 1 FOR UPDATE OF ml",
)

replace_once(
    "src/db/mod.rs",
    "SELECT token_hash FROM shared_auth.magic_link_tokens \\\n                        WHERE identifier_hash = $1 AND consumed_at IS NULL \\\n                          AND expires_at > now() \\\n                        ORDER BY created_at DESC LIMIT 1 FOR UPDATE",
    "SELECT ml.token_hash FROM shared_auth.magic_link_tokens ml \\\n                        WHERE ml.identifier_hash = $1 AND ml.consumed_at IS NULL \\\n                          AND ml.expires_at > now() \\\n                          AND NOT EXISTS ( \\\n                            SELECT 1 FROM shared_auth.oauth_magic_link_requests om \\\n                            WHERE om.token_hash = ml.token_hash \\\n                          ) \\\n                        ORDER BY ml.created_at DESC LIMIT 1 FOR UPDATE OF ml",
)

schema = read("db/schema.sql")
if "create table if not exists shared_auth.oauth_authorization_requests" not in schema:
    marker = (
        "create index if not exists session_application_grants_active_idx\n"
        "    on shared_auth.session_application_grants (application_id, client_id, last_used_at desc)\n"
        "    where revoked_at is null;\n"
    )
    if schema.count(marker) != 1:
        raise SystemExit("db/schema.sql: session grant marker not found exactly once")
    schema = schema.replace(marker, marker + decoded(SCHEMA_BLOCK), 1)
    write("db/schema.sql", schema)

replace_once(
    "src/http/passwordless.rs",
    "            &token.plaintext,\n            &token.otp,\n        )",
    "            &token.plaintext,\n            &token.otp,\n            None,\n            None,\n        )",
)

replace_once(
    "src/http/mod.rs",
    "mod mfa;\nmod passwordless;\n",
    "mod mfa;\nmod oauth;\nmod passwordless;\n",
)
replace_once(
    "src/http/mod.rs",
    "        .route(\"/ui/exchange\", post(ui::ui_exchange))\n        .route(\"/docs/api\", get(docs::api_docs))",
    "        .route(\"/ui/exchange\", post(ui::ui_exchange))\n"
    "        .route(\"/authorize\", get(oauth::authorize))\n"
    "        .route(\n"
    "            \"/authorize/passwordless/request\",\n"
    "            post(oauth::request_passwordless),\n"
    "        )\n"
    "        .route(\n"
    "            \"/authorize/passwordless/consume\",\n"
    "            post(oauth::consume_otp),\n"
    "        )\n"
    "        .route(\"/authorize/consume\", get(oauth::consume_link))\n"
    "        .route(\"/oauth/token\", post(oauth::token))\n"
    "        .route(\"/docs/api\", get(docs::api_docs))",
)

replace_once(
    "src/http/local.rs",
    """    let session = db
        .rotate_session(
            &hash_token(&request.refresh_token),
            &replacement.hash,
            expires_at,
        )
        .await?;
    let access = session_tokens::mint_for_session(&state, &session)?;
    Ok(Json(SessionResponse {
        access_token: access.token,
        token_type: "Bearer",
        expires_at: access.expires_at,
        refresh_token: replacement.plaintext,
        refresh_expires_at: expires_at.timestamp() as u64,
        shared_user_id: session.identity.shared_user_id.to_string(),
        provider: session.identity.provider,
        roles: session.identity.roles,
        amr: access.amr,
        acr: access.acr,
    }))""",
    """    let session = db
        .rotate_session_with_application(
            &hash_token(&request.refresh_token),
            &replacement.hash,
            expires_at,
        )
        .await?;
    let access = session_tokens::mint_for_oauth_session(&state, &session)?;
    let identity = session.session.identity;
    Ok(Json(SessionResponse {
        access_token: access.token,
        token_type: "Bearer",
        expires_at: access.expires_at,
        refresh_token: replacement.plaintext,
        refresh_expires_at: expires_at.timestamp() as u64,
        shared_user_id: identity.shared_user_id.to_string(),
        provider: identity.provider,
        roles: identity.roles,
        amr: access.amr,
        acr: access.acr,
    }))""",
)

mint_application = r"""
    /// Mint a product access token directly from a completed first-party
    /// authorization-code ceremony. Unlike delegation, this does not require
    /// exposing a broad central token to the application. The resulting token
    /// is bound to one audience, authorized party, scope set, and session.
    pub fn mint_application(
        &self,
        context: MintContext,
        audience: &str,
        client_id: &str,
        scopes: &[String],
    ) -> Result<MintedToken, AuthError> {
        let mut seen = std::collections::HashSet::new();
        let valid_scopes = scopes.iter().all(|scope| {
            !scope.is_empty()
                && scope.len() <= 128
                && scope.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.')
                })
                && seen.insert(scope.as_str())
        });
        if audience.is_empty()
            || audience == self.audience
            || audience.len() > 128
            || client_id.is_empty()
            || client_id.len() > 128
            || scopes.is_empty()
            || scopes.len() > 16
            || !valid_scopes
            || !seen.contains("openid")
        {
            return Err(AuthError::Forbidden);
        }

        let now = now_secs();
        let expires_at = now.saturating_add(self.ttl_secs);
        let is_supabase = context.provider == "supabase";
        let include_email = seen.contains("email");
        let email = if include_email { context.email } else { None };
        let email_verified = include_email && context.email_verified;
        let assurance = context.assurance;
        let claims = OreClaims {
            sub: context.shared_user_id,
            iss: self.issuer.clone(),
            aud: audience.to_owned(),
            iat: now,
            exp: expires_at,
            nbf: now.saturating_sub(5),
            jti: uuid::Uuid::new_v4().to_string(),
            sid: context.session_id.map(|id| id.to_string()),
            project: is_supabase.then(|| context.provider_tenant.clone()),
            supabase_user_id: is_supabase.then(|| context.provider_subject.clone()),
            provider: context.provider,
            provider_tenant: context.provider_tenant,
            provider_subject: context.provider_subject,
            email,
            email_verified,
            roles: context.roles,
            aal: assurance.level(),
            amr: assurance.amr.clone(),
            acr: assurance.acr.clone(),
            auth_time: Some(now),
            scope: scopes.join(" "),
            azp: Some(client_id.to_owned()),
            parent_jti: None,
        };
        let token = self.sign(&claims)?;
        Ok(MintedToken {
            token,
            expires_at,
            amr: assurance.amr,
            acr: assurance.acr,
        })
    }

"""
minter = read("src/token/minter.rs")
if "pub fn mint_application(" not in minter:
    marker = "    /// Mint a narrow product token from an already verified, revocation-aware\n"
    if minter.count(marker) != 1:
        raise SystemExit("src/token/minter.rs: delegation marker not found")
    minter = minter.replace(marker, mint_application + marker, 1)

app_token_test = r"""
    #[test]
    fn application_token_is_exact_audience_and_scope_bound() {
        let m = minter();
        let minted = m
            .mint_application(
                context(AuthenticationAssurance::local_password()),
                "canonical-api",
                "canonical-web",
                &["openid".into(), "quote:read".into()],
            )
            .unwrap();
        assert!(m.verify(&minted.token).is_err());
        let claims = m
            .verify_for_audience(&minted.token, "canonical-api")
            .unwrap();
        assert_eq!(claims.azp.as_deref(), Some("canonical-web"));
        assert_eq!(claims.scope, "openid quote:read");
        assert!(claims.email.is_none());
        assert!(!claims.email_verified);
        assert!(claims.parent_jti.is_none());
        assert!(claims.is_delegated());
    }

"""
if "fn application_token_is_exact_audience_and_scope_bound" not in minter:
    marker = (
        "    #[test]\n"
        "    fn delegation_preserves_subject_session_and_assurance_but_narrows_authority()"
    )
    if minter.count(marker) != 1:
        raise SystemExit("src/token/minter.rs: test insertion marker not found")
    minter = minter.replace(marker, app_token_test + marker, 1)
write("src/token/minter.rs", minter)

readme = read("README.md")
if "## Browser authorization code + PKCE" not in readme:
    readme += r"""

## Browser authorization code + PKCE

First-party web, desktop, and mobile applications use the browser flow instead
of receiving a realm-wide token in a redirect URL:

1. The application creates a high-entropy `state`, PKCE verifier, and S256
   challenge, then sends the browser to `GET /authorize` with an exact
   registered `client_id`, `redirect_uri`, and scope set.
2. shared-auth renders a script-free email magic-link/OTP form. Form posts are
   bound to a per-request `__Host-` transaction cookie; email links are bound to
   the same durable authorization request and remain single-use.
3. The exact registered callback receives only `code` and the original `state`.
4. The application exchanges the 90-second code at `POST /oauth/token` with the
   original verifier. Postgres atomically consumes the code and creates a
   client/audience/scope-bound application session.
5. `POST /auth/refresh` preserves that application authority during rotation;
   it can never mint the realm's central audience from an application refresh
   token.

Required deployment settings for browser passwordless authorization include
`AUTH_OAUTH_MAGIC_LINK_BASE_URL` (for example,
`https://auth.example.com/authorize/consume`) and the realm-distinct
`AUTH_SESSION_COOKIE_NAME`. Register redirect URIs exactly in
`shared_auth.oauth_clients`; wildcard callbacks are not supported.
"""
    write("README.md", readme)

print("OAuth/PKCE patch applied")
