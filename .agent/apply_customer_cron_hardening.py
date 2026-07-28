from pathlib import Path

path = Path("src/cron.rs")
source = path.read_text()


def block(lines: list[str]) -> str:
    return "\n".join(lines) + "\n"


marker = "const MAX_RUN_MS: u64 = 120_000;\n"
helpers = block(
    [
        "const MAX_RUN_MS: u64 = 120_000;",
        "const MAX_TRACESTATE_BYTES: usize = 512;",
        "",
        "fn optional_env(name: &str) -> Option<String> {",
        "    std::env::var(name)",
        "        .ok()",
        "        .map(|value| value.trim().to_string())",
        "        .filter(|value| !value.is_empty())",
        "}",
        "",
        "fn valid_traceparent(value: &HeaderValue) -> bool {",
        "    let Ok(value) = value.to_str() else { return false; };",
        "    let bytes = value.as_bytes();",
        "    if bytes.len() != 55 || &bytes[0..2] != b\"00\" || bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {",
        "        return false;",
        "    }",
        "    let lower_hex = |byte: &u8| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase();",
        "    if !bytes[3..35].iter().all(lower_hex) || !bytes[36..52].iter().all(lower_hex) || !bytes[53..55].iter().all(lower_hex) {",
        "        return false;",
        "    }",
        "    bytes[3..35].iter().any(|byte| *byte != b'0') && bytes[36..52].iter().any(|byte| *byte != b'0')",
        "}",
        "",
        "fn valid_tracestate(value: &HeaderValue) -> bool {",
        "    value.to_str().is_ok_and(|value| !value.is_empty() && value.len() <= MAX_TRACESTATE_BYTES && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)))",
        "}",
        "",
        "fn insert_valid_trace_context(incoming: &HeaderMap, outgoing: &mut HeaderMap) {",
        "    let traceparent = HeaderName::from_static(\"traceparent\");",
        "    let tracestate = HeaderName::from_static(\"tracestate\");",
        "    let Some(value) = incoming.get(&traceparent).filter(|value| valid_traceparent(value)) else { return; };",
        "    outgoing.insert(traceparent, value.clone());",
        "    if let Some(value) = incoming.get(&tracestate).filter(|value| valid_tracestate(value)) {",
        "        outgoing.insert(tracestate, value.clone());",
        "    }",
        "}",
    ]
)
if "fn optional_env(name: &str)" not in source:
    if source.count(marker) != 1:
        raise SystemExit("customer cron helper insertion point changed")
    source = source.replace(marker, helpers, 1)

old = block(
    [
        '    for name in ["traceparent", "tracestate"] {',
        "        let header = HeaderName::from_static(name);",
        "        if let Some(value) = incoming.get(&header) {",
        "            headers.insert(header, value.clone());",
        "        }",
        "    }",
    ]
)
if old in source:
    source = source.replace(old, "    insert_valid_trace_context(incoming, &mut headers);\n", 1)

old = block(
    [
        "    cron_inventory_markup(",
        "        org_id,",
        "        &customer_csrf_token(config, customer),",
        "        schedules.as_ref().ok(),",
        "        functions.as_ref().ok(),",
        "        schedules.err().or_else(|| functions.err()),",
        "    )",
    ]
)
new = block(
    [
        "    let dependency_error = schedules",
        "        .as_ref()",
        "        .err()",
        "        .copied()",
        "        .or_else(|| functions.as_ref().err().copied());",
        "    cron_inventory_markup(",
        "        org_id,",
        "        &customer_csrf_token(config, customer),",
        "        schedules.as_ref().ok(),",
        "        functions.as_ref().ok(),",
        "        dependency_error,",
        "    )",
    ]
)
if old in source:
    source = source.replace(old, new, 1)

marker = block(["    #[test]", "    fn url_builder_encodes_path_segments() {"])
tests = block(
    [
        "    #[test]",
        "    fn outbound_headers_drop_invalid_browser_trace_context() {",
        "        let mut incoming = HeaderMap::new();",
        "        incoming.insert(\"traceparent\", HeaderValue::from_static(\"00-00000000000000000000000000000000-0123456789abcdef-01\"));",
        "        incoming.insert(\"tracestate\", HeaderValue::from_static(\"vendor=value\"));",
        "        let headers = outbound_headers(&service(), \"acme\", &incoming, None).unwrap();",
        "        assert!(headers.get(\"traceparent\").is_none());",
        "        assert!(headers.get(\"tracestate\").is_none());",
        "",
        "        incoming.insert(\"traceparent\", HeaderValue::from_static(\"00-0123456789abcdef0123456789abcdef-0123456789abcdef-01\"));",
        "        incoming.insert(\"tracestate\", HeaderValue::from_str(&\"x\".repeat(MAX_TRACESTATE_BYTES + 1)).unwrap());",
        "        let headers = outbound_headers(&service(), \"acme\", &incoming, None).unwrap();",
        "        assert!(headers.get(\"traceparent\").is_some());",
        "        assert!(headers.get(\"tracestate\").is_none());",
        "    }",
        "",
        "    #[test]",
        "    fn url_builder_encodes_path_segments() {",
    ]
)
if "fn outbound_headers_drop_invalid_browser_trace_context()" not in source:
    if source.count(marker) != 1:
        raise SystemExit("customer cron test insertion point changed")
    source = source.replace(marker, tests, 1)

required = [
    "fn optional_env(name: &str)",
    "fn valid_traceparent(value: &HeaderValue)",
    "insert_valid_trace_context(incoming, &mut headers);",
    "let dependency_error = schedules",
    "fn outbound_headers_drop_invalid_browser_trace_context()",
]
missing = [item for item in required if item not in source]
if missing:
    raise SystemExit(f"customer cron hardening incomplete: {missing}")

path.write_text(source)
