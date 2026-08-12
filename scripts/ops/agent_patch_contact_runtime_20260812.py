#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


root = Path(__file__).resolve().parents[2]
main_path = root / "remote/deployments/dd-email-sms-contact-rs/src/main.rs"
readme_path = root / "remote/deployments/dd-email-sms-contact-rs/readme.md"
deployment_path = root / "remote/argocd/dd-next-runtime/dd-email-sms-contact-rs.deployment.yaml"
external_secret_path = root / "remote/argocd/dd-next-runtime/dd-email-sms-contact-rs.externalsecret.yaml"

main = main_path.read_text(encoding="utf-8")

main = replace_once(
    main,
    '//   sms:     TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, TWILIO_FROM_NUMBER.\n',
    '//   sms:     TWILIO_ACCOUNT_SID, TWILIO_API_KEY_SID + TWILIO_API_KEY_SECRET (preferred),\n'
    '//            TWILIO_AUTH_TOKEN (compatibility fallback), TWILIO_FROM_NUMBER.\n',
    "top-level Twilio env documentation",
)

main = replace_once(
    main,
    '''    twilio_sid: Option<String>,
    twilio_token: Option<String>,
    twilio_from: Option<String>,
''',
    '''    twilio_sid: Option<String>,
    twilio_credentials: Option<TwilioCredentials>,
    twilio_from: Option<String>,
''',
    "AppState Twilio credential fields",
)

main = replace_once(
    main,
    '''struct CachedToken {
    value: String,
    expires_at: Instant,
}

struct TokenBucket {
''',
    '''struct CachedToken {
    value: String,
    expires_at: Instant,
}

#[derive(Clone)]
enum TwilioCredentials {
    AuthToken { token: String },
    ApiKey { sid: String, secret: String },
}

struct TokenBucket {
''',
    "TwilioCredentials enum insertion",
)

main = replace_once(
    main,
    '''    let state = AppState {
        http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"),
        auth_secret: non_empty(env::var("SERVER_AUTH_SECRET").ok()),
        nats_secret: non_empty(env::var("NATS_SHARED_SECRET").ok()),
        sendgrid_key: non_empty(env::var("SENDGRID_API_KEY").ok()).filter(|k| !k.contains("REPLACE")),
        email_from: env::var("EMAIL_FROM").unwrap_or_else(|_| "outreach@dancingdragons.cc".to_string()),
        twilio_sid: non_empty(env::var("TWILIO_ACCOUNT_SID").ok()),
        twilio_token: non_empty(env::var("TWILIO_AUTH_TOKEN").ok()),
        twilio_from: non_empty(env::var("TWILIO_FROM_NUMBER").ok()),
''',
    '''    let twilio_credentials = select_twilio_credentials(
        non_empty(env::var("TWILIO_API_KEY_SID").ok()),
        non_empty(env::var("TWILIO_API_KEY_SECRET").ok()),
        non_empty(env::var("TWILIO_AUTH_TOKEN").ok()),
    );

    let state = AppState {
        http: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("http client"),
        auth_secret: non_empty(env::var("SERVER_AUTH_SECRET").ok()),
        nats_secret: non_empty(env::var("NATS_SHARED_SECRET").ok()),
        sendgrid_key: non_empty(env::var("SENDGRID_API_KEY").ok()).filter(|k| !k.contains("REPLACE")),
        email_from: env::var("EMAIL_FROM").unwrap_or_else(|_| "outreach@dancingdragons.cc".to_string()),
        twilio_sid: non_empty(env::var("TWILIO_ACCOUNT_SID").ok()),
        twilio_credentials,
        twilio_from: non_empty(env::var("TWILIO_FROM_NUMBER").ok()),
''',
    "AppState construction",
)

main = replace_once(
    main,
    '''fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn build_webpush_config() -> Option<WebPushConfig> {
''',
    '''fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn select_twilio_credentials(
    api_key_sid: Option<String>,
    api_key_secret: Option<String>,
    auth_token: Option<String>,
) -> Option<TwilioCredentials> {
    match (api_key_sid, api_key_secret) {
        (Some(sid), Some(secret)) => Some(TwilioCredentials::ApiKey { sid, secret }),
        (None, None) => auth_token.map(|token| TwilioCredentials::AuthToken { token }),
        _ => {
            tracing::error!(
                "TWILIO_API_KEY_SID and TWILIO_API_KEY_SECRET must be configured together; Twilio disabled"
            );
            None
        }
    }
}

fn build_webpush_config() -> Option<WebPushConfig> {
''',
    "Twilio credential selector",
)

main = replace_once(
    main,
    '''    match s.http.post("https://api.sendgrid.com/v3/mail/send").bearer_auth(key).json(&body).send().await {
        Ok(r) if r.status().is_success() => Outcome { ok: true, transport: "sendgrid", upstream_status: Some(r.status().as_u16()), error: None, rate_limited: false },
        Ok(r) => {
            let code = r.status().as_u16();
            let txt = cap(r.text().await.unwrap_or_default());
            Outcome { ok: false, transport: "sendgrid", upstream_status: Some(code), error: Some(txt), rate_limited: false }
        }
        Err(e) => Outcome { ok: false, transport: "sendgrid", upstream_status: None, error: Some(format!("request failed: {e}")), rate_limited: false },
    }
}

async fn sms_send(s: &AppState, to: &str, sms_body: &str) -> Outcome {
    let (Some(sid), Some(token), Some(from)) = (s.twilio_sid.clone(), s.twilio_token.clone(), s.twilio_from.clone()) else {
        return Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some("Twilio not configured".into()), rate_limited: false };
    };
    if !s.sms_bucket.lock().await.try_take() {
        return Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some("sms rate limit exceeded".into()), rate_limited: true };
    }
    let url = format!("https://api.twilio.com/2010-04-01/Accounts/{sid}/Messages.json");
    let form = [("To", to), ("From", from.as_str()), ("Body", sms_body)];
    match s.http.post(url).basic_auth(sid, Some(token)).form(&form).send().await {
        Ok(r) if r.status().is_success() => Outcome { ok: true, transport: "twilio", upstream_status: Some(r.status().as_u16()), error: None, rate_limited: false },
        Ok(r) => {
            let code = r.status().as_u16();
            let txt = cap(r.text().await.unwrap_or_default());
            Outcome { ok: false, transport: "twilio", upstream_status: Some(code), error: Some(txt), rate_limited: false }
        }
        Err(e) => Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some(format!("request failed: {e}")), rate_limited: false },
    }
}

// ── push transports ─────────────────────────────────────────────────────────────
''',
    '''    match s.http.post("https://api.sendgrid.com/v3/mail/send").bearer_auth(key).json(&body).send().await {
        Ok(r) if r.status().is_success() => Outcome { ok: true, transport: "sendgrid", upstream_status: Some(r.status().as_u16()), error: None, rate_limited: false },
        Ok(r) => provider_rejection("sendgrid", r.status().as_u16()),
        Err(_) => provider_transport_failure("sendgrid"),
    }
}

async fn sms_send(s: &AppState, to: &str, sms_body: &str) -> Outcome {
    let (Some(account_sid), Some(credentials), Some(from)) = (
        s.twilio_sid.clone(),
        s.twilio_credentials.clone(),
        s.twilio_from.clone(),
    ) else {
        return Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some("Twilio not configured".into()), rate_limited: false };
    };
    if !s.sms_bucket.lock().await.try_take() {
        return Outcome { ok: false, transport: "twilio", upstream_status: None, error: Some("sms rate limit exceeded".into()), rate_limited: true };
    }
    let (username, password) = match credentials {
        TwilioCredentials::AuthToken { token } => (account_sid.clone(), token),
        TwilioCredentials::ApiKey { sid, secret } => (sid, secret),
    };
    let url = format!("https://api.twilio.com/2010-04-01/Accounts/{account_sid}/Messages.json");
    let form = [("To", to), ("From", from.as_str()), ("Body", sms_body)];
    match s.http.post(url).basic_auth(username, Some(password)).form(&form).send().await {
        Ok(r) if r.status().is_success() => Outcome { ok: true, transport: "twilio", upstream_status: Some(r.status().as_u16()), error: None, rate_limited: false },
        Ok(r) => provider_rejection("twilio", r.status().as_u16()),
        Err(_) => provider_transport_failure("twilio"),
    }
}

fn provider_rejection(transport: &'static str, code: u16) -> Outcome {
    let (message, rate_limited) = match code {
        401 | 403 => ("provider authentication failed", false),
        429 => ("provider rate limited request", true),
        400..=499 => ("provider rejected request", false),
        500..=599 => ("provider unavailable", false),
        _ => ("provider request failed", false),
    };
    Outcome {
        ok: false,
        transport,
        upstream_status: Some(code),
        error: Some(message.to_string()),
        rate_limited,
    }
}

fn provider_transport_failure(transport: &'static str) -> Outcome {
    Outcome {
        ok: false,
        transport,
        upstream_status: None,
        error: Some("provider request failed".to_string()),
        rate_limited: false,
    }
}

// ── push transports ─────────────────────────────────────────────────────────────
''',
    "SendGrid and Twilio safe provider handling",
)

main = replace_once(
    main,
    '''fn outcome_json(channel: &str, to: &str, o: &Outcome) -> Value {
    json!({"ok": o.ok, "channel": channel, "to": to, "transport": o.transport, "upstreamStatus": o.upstream_status, "error": o.error, "rateLimited": o.rate_limited})
}
''',
    '''fn result_target(channel: &str, to: &str) -> String {
    match channel {
        "email" => "email-recipient".to_string(),
        "sms" => "sms-recipient".to_string(),
        _ => to.to_string(),
    }
}

fn outcome_json(channel: &str, to: &str, o: &Outcome) -> Value {
    json!({"ok": o.ok, "channel": channel, "to": result_target(channel, to), "transport": o.transport, "upstreamStatus": o.upstream_status, "error": o.error, "rateLimited": o.rate_limited})
}
''',
    "result target redaction",
)

main = replace_once(
    main,
    '''        "sms": {"twilio_configured": s.twilio_sid.is_some() && s.twilio_token.is_some() && s.twilio_from.is_some()},
''',
    '''        "sms": {"twilio_configured": s.twilio_sid.is_some() && s.twilio_credentials.is_some() && s.twilio_from.is_some()},
''',
    "Twilio readyz check",
)

main = replace_once(
    main,
    '''        publish_result(client, reply, json!({"ok": false, "channel": "email", "to": req.to, "error": e, "idempotency_key": idempotency_key})).await;
''',
    '''        publish_result(client, reply, json!({"ok": false, "channel": "email", "to": result_target("email", &req.to), "error": e, "idempotency_key": idempotency_key})).await;
''',
    "email validation target redaction",
)

main = replace_once(
    main,
    '''        publish_result(client, None, json!({"ok": false, "channel": "sms", "to": req.to, "error": e})).await;
''',
    '''        publish_result(client, None, json!({"ok": false, "channel": "sms", "to": result_target("sms", &req.to), "error": e})).await;
''',
    "sms validation target redaction",
)

tests = r'''

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_failures_are_classified_without_upstream_bodies() {
        let auth = provider_rejection("sendgrid", 401);
        assert_eq!(auth.upstream_status, Some(401));
        assert_eq!(auth.error.as_deref(), Some("provider authentication failed"));
        assert!(!auth.rate_limited);

        let rate_limited = provider_rejection("twilio", 429);
        assert_eq!(rate_limited.upstream_status, Some(429));
        assert_eq!(rate_limited.error.as_deref(), Some("provider rate limited request"));
        assert!(rate_limited.rate_limited);

        let unavailable = provider_rejection("sendgrid", 503);
        assert_eq!(unavailable.error.as_deref(), Some("provider unavailable"));

        let network = provider_transport_failure("twilio");
        assert_eq!(network.upstream_status, None);
        assert_eq!(network.error.as_deref(), Some("provider request failed"));
    }

    #[test]
    fn email_and_sms_result_targets_are_redacted() {
        let outcome = provider_rejection("sendgrid", 400);
        let email = outcome_json("email", "person@example.com", &outcome).to_string();
        assert!(!email.contains("person@example.com"));
        assert!(email.contains("email-recipient"));

        let sms = outcome_json("sms", "+15551234567", &outcome).to_string();
        assert!(!sms.contains("+15551234567"));
        assert!(sms.contains("sms-recipient"));
    }

    #[test]
    fn twilio_api_keys_are_preferred_and_partial_pairs_fail_closed() {
        let credentials = select_twilio_credentials(
            Some("SK_test".to_string()),
            Some("api-secret".to_string()),
            Some("account-token".to_string()),
        )
        .expect("complete API key pair should configure Twilio");
        match credentials {
            TwilioCredentials::ApiKey { sid, secret } => {
                assert_eq!(sid, "SK_test");
                assert_eq!(secret, "api-secret");
            }
            TwilioCredentials::AuthToken { .. } => panic!("API key pair must be preferred"),
        }

        assert!(select_twilio_credentials(
            Some("SK_partial".to_string()),
            None,
            Some("account-token".to_string()),
        )
        .is_none());

        let fallback = select_twilio_credentials(
            None,
            None,
            Some("account-token".to_string()),
        )
        .expect("account token should remain a compatibility fallback");
        assert!(matches!(fallback, TwilioCredentials::AuthToken { .. }));
    }
}
'''

if "#[cfg(test)]\nmod tests {" in main:
    raise SystemExit("tests module already exists")
main = main.rstrip() + tests + "\n"
main_path.write_text(main, encoding="utf-8")

deployment = deployment_path.read_text(encoding="utf-8")
deployment = replace_once(
    deployment,
    '''            - name: TWILIO_AUTH_TOKEN
              valueFrom:
                secretKeyRef:
                  name: dd-email-sms-contact-secrets
                  key: TWILIO_AUTH_TOKEN
                  optional: true
            - name: TWILIO_FROM_NUMBER
''',
    '''            # Prefer a restricted Twilio API key over the account auth token. The service
            # fails closed if only one half of the API-key pair is present.
            - name: TWILIO_API_KEY_SID
              valueFrom:
                secretKeyRef:
                  name: dd-email-sms-contact-secrets
                  key: TWILIO_API_KEY_SID
                  optional: true
            - name: TWILIO_API_KEY_SECRET
              valueFrom:
                secretKeyRef:
                  name: dd-email-sms-contact-secrets
                  key: TWILIO_API_KEY_SECRET
                  optional: true
            - name: TWILIO_AUTH_TOKEN
              valueFrom:
                secretKeyRef:
                  name: dd-email-sms-contact-secrets
                  key: TWILIO_AUTH_TOKEN
                  optional: true
            - name: TWILIO_FROM_NUMBER
''',
    "Twilio API key environment mapping",
)
deployment_path.write_text(deployment, encoding="utf-8")

external_secret = external_secret_path.read_text(encoding="utf-8")
external_secret = replace_once(
    external_secret,
    '''    #   TWILIO_ACCOUNT_SID / TWILIO_AUTH_TOKEN / TWILIO_FROM_NUMBER — SMS via Twilio
''',
    '''    #   TWILIO_ACCOUNT_SID / TWILIO_FROM_NUMBER — SMS account and sender
    #   TWILIO_API_KEY_SID / TWILIO_API_KEY_SECRET — preferred restricted API key pair
    #   TWILIO_AUTH_TOKEN — compatibility fallback only; do not use when an API key pair is present
''',
    "ExternalSecret Twilio documentation",
)
external_secret_path.write_text(external_secret, encoding="utf-8")

readme = readme_path.read_text(encoding="utf-8")
readme = replace_once(
    readme,
    '''- `dd.remote.contact.results` (publish) — per-send result summary `{ ok, channel, to, transport, upstreamStatus, error, rateLimited }`
''',
    '''- `dd.remote.contact.results` (publish) — per-send result summary `{ ok, channel, to, transport, upstreamStatus, error, rateLimited }`; `to` is a redacted channel label, not recipient PII
''',
    "README result schema",
)
readme = replace_once(
    readme,
    '''- Upstream error text is truncated to 1 KiB before it is returned or published onto the results bus.
- Result summaries carry only a redacted target (token prefix or `scheme://host/…`) — never the full
  device token or per-subscription endpoint path.
''',
    '''- SendGrid/Twilio response bodies and request errors are never returned or published; callers receive
  only the upstream status and a static error category. Push-provider error text remains bounded to 1 KiB.
- Result summaries carry redacted targets for every channel: generic email/SMS labels, or a token prefix /
  `scheme://host/…` for push — never recipient PII, a full device token, or a subscription endpoint path.
''',
    "README hardening controls",
)
readme = replace_once(
    readme,
    '''| `TWILIO_ACCOUNT_SID` / `TWILIO_AUTH_TOKEN` / `TWILIO_FROM_NUMBER` | SMS via Twilio |
''',
    '''| `TWILIO_ACCOUNT_SID` / `TWILIO_FROM_NUMBER` | Twilio account and approved sender |
| `TWILIO_API_KEY_SID` / `TWILIO_API_KEY_SECRET` | preferred restricted Twilio API key pair; both values are required |
| `TWILIO_AUTH_TOKEN` | compatibility fallback only; a partial API-key pair disables Twilio rather than falling back |
''',
    "README Twilio env table",
)
readme = replace_once(
    readme,
    '''- **Bounded error text**: all upstream (SendGrid/Twilio/push) error bodies are `cap()`-ed
  before they reach the HTTP response or the `CONTACT_SEND_RESULTS_SUBJECT` bus.
- **Secrets never logged/echoed**; reqwest uses rustls (TLS verify on) + a 20s timeout;
''',
    '''- **Provider-safe error boundaries**: SendGrid/Twilio response bodies and request errors are discarded;
  only status codes and static classifications cross the HTTP/NATS boundary. Push-provider errors remain
  `cap()`-ed before they reach callers or `CONTACT_SEND_RESULTS_SUBJECT`.
- **Recipient PII is redacted** from result summaries; email and SMS use generic target labels.
- **Secrets never logged/echoed**; reqwest uses rustls (TLS verify on) + a 20s timeout;
''',
    "README security posture",
)
readme = replace_once(
    readme,
    '''Send credentials come from the `dd-email-sms-contact-secrets` k8s Secret (External Secrets →
AWS Secrets Manager `dd/remote-dev/email-sms-contact-secrets`). The deployment marks every key
`optional: true`, so the pod boots and reports readiness before the bundle is populated; `/readyz`
shows which transports are live. **Populate that AWS secret with a SendGrid key that has the
`mail.send` scope** (the dd-next-1 `.env.local` key is admin-only and cannot send).
''',
    '''Send credentials come from the `dd-email-sms-contact-secrets` k8s Secret (External Secrets →
AWS Secrets Manager `dd/remote-dev/email-sms-contact-secrets`). The deployment marks every key
`optional: true`, so the pod boots and reports readiness before the bundle is populated; `/readyz`
shows which transports are live. **Populate that AWS secret with a service-specific SendGrid key
limited to `mail.send` and a verified sender.** For Twilio, prefer a restricted API key SID/secret
pair; retain the account auth token only as a temporary compatibility fallback.
''',
    "README secret guidance",
)
readme_path.write_text(readme, encoding="utf-8")

print("contact runtime provider hardening applied")
