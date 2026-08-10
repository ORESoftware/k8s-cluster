#!/usr/bin/env python3
"""Apply the exact, temporary NATS bridge header-consistency patch."""

from pathlib import Path
from textwrap import dedent


def main() -> None:
    source_path = Path("remote/nats-bridge/src/main.rs")
    source = source_path.read_text(encoding="utf-8")

    handlers_start = source.index("fn request_message_id(")
    handlers_end = source.index("fn constant_time_eq(", handlers_start)
    handlers = dedent(
        r'''
        fn normalize_message_id(value: &axum::http::HeaderValue) -> Result<String, String> {
            let value = value
                .to_str()
                .map_err(|_| "message id header must be valid ASCII".to_string())?
                .trim();
            if value.is_empty() || value.len() > 128 {
                return Err("message id must be 1-128 characters".into());
            }
            if !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/'))
            {
                return Err(
                    "message id may contain only ASCII alphanumerics, '-', '_', '.', ':', or '/'".into(),
                );
            }
            Ok(value.to_string())
        }

        fn request_message_id(headers: &HeaderMap) -> Result<Option<String>, String> {
            let mut canonical: Option<String> = None;
            for name in ["x-message-id", "idempotency-key", "nats-msg-id"] {
                for value in headers.get_all(name).iter() {
                    let candidate = normalize_message_id(value)?;
                    match canonical.as_deref() {
                        None => canonical = Some(candidate),
                        Some(existing) if existing == candidate.as_str() => {}
                        Some(_) => return Err("conflicting message id headers".into()),
                    }
                }
            }
            Ok(canonical)
        }

        fn caller_authorized(headers: &HeaderMap, expected: Option<&str>) -> bool {
            let Some(expected) = expected else {
                return true; // BRIDGE_ALLOW_INSECURE was explicitly set at startup.
            };

            let mut saw_credential = false;
            for value in headers.get_all("authorization").iter() {
                let Ok(value) = value.to_str() else {
                    return false;
                };
                let Some(token) = value.strip_prefix("Bearer ") else {
                    return false;
                };
                let token = token.trim();
                if token.is_empty() || !constant_time_eq(token, expected) {
                    return false;
                }
                saw_credential = true;
            }
            for value in headers.get_all("x-bridge-token").iter() {
                let Ok(token) = value.to_str() else {
                    return false;
                };
                let token = token.trim();
                if token.is_empty() || !constant_time_eq(token, expected) {
                    return false;
                }
                saw_credential = true;
            }
            saw_credential
        }
        '''
    ).lstrip()
    source = source[:handlers_start] + handlers + "\n" + source[handlers_end:]

    test_start = source.index(
        "    #[test]\n    fn bearer_and_header_tokens_are_accepted()"
    )
    test_end = source.index(
        "    // ---------------------------------------------------------------------\n"
        "    // Added security-surface tests",
        test_start,
    )
    tests = dedent(
        r'''
        #[test]
        fn bearer_and_header_tokens_are_accepted() {
            let mut h = HeaderMap::new();
            h.insert("authorization", "Bearer tok-abcdef123456".parse().unwrap());
            assert!(caller_authorized(&h, Some("tok-abcdef123456")));

            let mut h2 = HeaderMap::new();
            h2.insert("x-bridge-token", "tok-abcdef123456".parse().unwrap());
            assert!(caller_authorized(&h2, Some("tok-abcdef123456")));

            assert!(!caller_authorized(
                &HeaderMap::new(),
                Some("tok-abcdef123456")
            ));
        }

        #[test]
        fn matching_message_id_aliases_are_accepted() {
            let mut headers = HeaderMap::new();
            headers.insert("x-message-id", "job-42".parse().unwrap());
            headers.insert("idempotency-key", "job-42".parse().unwrap());
            headers.insert("nats-msg-id", "job-42".parse().unwrap());
            assert_eq!(request_message_id(&headers).unwrap().as_deref(), Some("job-42"));
        }

        #[test]
        fn conflicting_message_id_aliases_are_rejected() {
            let mut headers = HeaderMap::new();
            headers.insert("x-message-id", "job-42".parse().unwrap());
            headers.insert("idempotency-key", "job-43".parse().unwrap());
            assert_eq!(
                request_message_id(&headers).unwrap_err(),
                "conflicting message id headers"
            );
        }

        #[test]
        fn repeated_message_id_values_must_all_match() {
            let mut matching = HeaderMap::new();
            matching.append("x-message-id", "job-42".parse().unwrap());
            matching.append("x-message-id", "job-42".parse().unwrap());
            assert_eq!(request_message_id(&matching).unwrap().as_deref(), Some("job-42"));

            let mut conflicting = HeaderMap::new();
            conflicting.append("x-message-id", "job-42".parse().unwrap());
            conflicting.append("x-message-id", "job-43".parse().unwrap());
            assert!(request_message_id(&conflicting).is_err());
        }

        #[test]
        fn matching_dual_bridge_credentials_are_accepted() {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", "Bearer tok-abcdef123456".parse().unwrap());
            headers.insert("x-bridge-token", "tok-abcdef123456".parse().unwrap());
            assert!(caller_authorized(&headers, Some("tok-abcdef123456")));
        }

        #[test]
        fn conflicting_dual_bridge_credentials_are_rejected() {
            let mut headers = HeaderMap::new();
            headers.insert("authorization", "Bearer tok-abcdef123456".parse().unwrap());
            headers.insert("x-bridge-token", "tok-wrong-credential".parse().unwrap());
            assert!(!caller_authorized(&headers, Some("tok-abcdef123456")));
        }

        #[test]
        fn repeated_authorization_values_must_all_match() {
            let mut headers = HeaderMap::new();
            headers.append("authorization", "Bearer tok-abcdef123456".parse().unwrap());
            headers.append("authorization", "Bearer tok-wrong-credential".parse().unwrap());
            assert!(!caller_authorized(&headers, Some("tok-abcdef123456")));
        }
        '''
    ).strip("\n")
    tests = "\n".join(f"    {line}" if line else "" for line in tests.splitlines())
    source_path.write_text(
        source[:test_start] + tests + "\n\n" + source[test_end:], encoding="utf-8"
    )

    e2e_path = Path("remote/nats-bridge/scripts/e2e.sh")
    e2e = e2e_path.read_text(encoding="utf-8")
    insertion_anchor = 'assert_code "off-allowlist subject -> 403" 403 \\\n'
    if e2e.count(insertion_anchor) != 1:
        raise RuntimeError("off-allowlist e2e anchor drifted")
    added_rejections = dedent(
        r'''
        assert_code "conflicting auth headers -> 401" 401 \
          -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
          -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" -H 'x-bridge-token: wrong-token-xxxxxx' \
          -H 'content-type: application/json' -d '{}'
        assert_code "conflicting message-id aliases -> 400" 400 \
          -X POST "${BRIDGE_URL}/publish/dd.vapi.tasks.call" \
          -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" \
          -H 'x-message-id: e2e-conflict-a' -H 'idempotency-key: e2e-conflict-b' \
          -H 'content-type: application/json' -d '{}'
        '''
    ).lstrip()
    e2e = e2e.replace(insertion_anchor, added_rejections + insertion_anchor, 1)

    count_comment = "# 7, not 8: the oversize body is refused by axum's DefaultBodyLimit layer\n"
    count_expr = "d['published_total']==0 and d['rejected_total']==7"
    if e2e.count(count_comment) != 1 or e2e.count(count_expr) != 1:
        raise RuntimeError("rejection counter e2e contract drifted")
    e2e = e2e.replace(
        count_comment,
        "# 9, not 10: the oversize body is refused by axum's DefaultBodyLimit layer\n",
        1,
    ).replace(count_expr, "d['published_total']==0 and d['rejected_total']==9", 1)

    fallback_start = e2e.index('assert_json "allowed subject -> 200 durable:false"')
    fallback_end = e2e.index('\n\nstep "3. Vapi worker', fallback_start)
    fallback = dedent(
        r'''
        assert_json "matching auth and message-id aliases -> 200 durable:false" \
          "d['ok'] is True and d['durable'] is False and d['messageId']=='e2e-matching-id-001'" \
          -X POST "${BRIDGE_URL}/publish/vxl.events.test" \
          -H "authorization: Bearer ${BRIDGE_TOKEN_VALUE}" \
          -H "x-bridge-token: ${BRIDGE_TOKEN_VALUE}" \
          -H 'x-message-id: e2e-matching-id-001' -H 'idempotency-key: e2e-matching-id-001' \
          -H 'content-type: application/json' -d '{"hello":"world"}'
        '''
    ).strip("\n")
    e2e_path.write_text(e2e[:fallback_start] + fallback + e2e[fallback_end:], encoding="utf-8")


if __name__ == "__main__":
    main()
