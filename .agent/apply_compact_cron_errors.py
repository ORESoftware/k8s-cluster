from pathlib import Path
from textwrap import dedent

path = Path("src/cron.rs")
source = path.read_text()


def replace_exact(old: str, new: str, expected: int, label: str) -> None:
    global source
    old = dedent(old)
    new = dedent(new)
    count = source.count(old)
    if count != expected:
        raise SystemExit(f"{label}: expected {expected} matches, found {count}")
    source = source.replace(old, new, expected)


def replace_region(start_marker: str, end_marker: str, replacement: str, label: str) -> None:
    global source
    start = source.find(start_marker)
    if start < 0:
        raise SystemExit(f"{label}: start marker missing")
    end = source.find(end_marker, start)
    if end < 0:
        raise SystemExit(f"{label}: end marker missing")
    source = source[:start] + dedent(replacement).rstrip() + "\n\n" + source[end:]


replace_exact(
    """
    let name = match take_schedule_name(&mut body) {
        Ok(name) => name,
        Err(response) => return response,
    };
    """,
    """
    let name = match take_schedule_name(&mut body) {
        Ok(name) => name,
        Err(code) => return bad_request(code),
    };
    """,
    1,
    "take_schedule_name caller",
)

replace_exact(
    """
    let idempotency = match required_idempotency(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    """,
    """
    let idempotency = match required_idempotency(&headers) {
        Ok(value) => value,
        Err(code) => return bad_request(code),
    };
    """,
    1,
    "create schedule idempotency caller",
)

replace_exact(
    """
    let idempotency = match required_idempotency(headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    """,
    """
    let idempotency = match required_idempotency(headers) {
        Ok(value) => value,
        Err(code) => return bad_request(code),
    };
    """,
    1,
    "shared write idempotency caller",
)

replace_exact(
    """
    let body = match validated_function_body(body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    """,
    """
    let body = match validated_function_body(body) {
        Ok(body) => body,
        Err(code) => return bad_request(code),
    };
    """,
    2,
    "function API validation callers",
)

replace_exact(
    """
    let body = match validated_function_body(json!({
        "slug": form.slug.trim(),
        "displayName": form.display_name.trim(),
        "runtime": "nodejs",
        "functionBody": form.function_body,
        "maxRunMs": form.max_run_ms.unwrap_or(30_000),
        "labels": ["cron"]
    })) {
        Ok(body) => body,
        Err(response) => return response,
    };
    """,
    """
    let body = match validated_function_body(json!({
        "slug": form.slug.trim(),
        "displayName": form.display_name.trim(),
        "runtime": "nodejs",
        "functionBody": form.function_body,
        "maxRunMs": form.max_run_ms.unwrap_or(30_000),
        "labels": ["cron"]
    })) {
        Ok(body) => body,
        Err(code) => return bad_request(code),
    };
    """,
    1,
    "function form validation caller",
)

replace_region(
    "fn required_idempotency(",
    "#[allow(clippy::too_many_arguments)]",
    """
    fn required_idempotency(headers: &HeaderMap) -> Result<HeaderValue, &'static str> {
        let value = headers
            .get(IDEMPOTENCY_KEY_HEADER)
            .ok_or("idempotency_key_required")?;
        let valid = value.to_str().is_ok_and(|value| {
            !value.is_empty()
                && value.len() <= 200
                && value.bytes().all(|byte| matches!(byte, 0x21..=0x7e))
        });
        if !valid {
            return Err("invalid_idempotency_key");
        }
        Ok(value.clone())
    }
    """,
    "required_idempotency helper",
)

replace_region(
    "fn take_schedule_name(",
    "fn validated_function_body(",
    """
    fn take_schedule_name(body: &mut Value) -> Result<String, &'static str> {
        let object = body.as_object_mut().ok_or("invalid_schedule")?;
        let name = object
            .remove("name")
            .and_then(|value| value.as_str().map(str::to_string))
            .ok_or("schedule_name_required")?;
        if !valid_schedule_name(&name) {
            return Err("invalid_schedule_name");
        }
        Ok(name)
    }
    """,
    "take_schedule_name helper",
)

replace_region(
    "fn validated_function_body(",
    "fn valid_schedule_name(",
    """
    fn validated_function_body(mut body: Value) -> Result<Value, &'static str> {
        let object = body.as_object_mut().ok_or("invalid_function")?;
        if let Some(runtime) = object.get("runtime").and_then(Value::as_str) {
            if runtime != "nodejs" {
                return Err("unsupported_function_runtime");
            }
        }
        object.insert("runtime".to_string(), Value::String("nodejs".to_string()));
        if object
            .get("entryCommand")
            .or_else(|| object.get("entry_command"))
            .is_some()
            || object.get("environment").is_some()
            || object.get("container").is_some()
        {
            return Err("unsupported_function_configuration");
        }
        let slug = object
            .get("slug")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !valid_slug(slug) {
            return Err("invalid_function_slug");
        }
        let source = object
            .get("functionBody")
            .or_else(|| object.get("function_body"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if source.is_empty() || source.len() > MAX_FUNCTION_SOURCE_BYTES {
            return Err("invalid_function_source");
        }
        let max_run_ms = object
            .get("maxRunMs")
            .or_else(|| object.get("max_run_ms"))
            .and_then(Value::as_u64)
            .unwrap_or(30_000);
        if max_run_ms == 0 || max_run_ms > MAX_RUN_MS {
            return Err("invalid_function_timeout");
        }
        Ok(body)
    }
    """,
    "validated_function_body helper",
)

for forbidden in (
    "fn required_idempotency(headers: &HeaderMap) -> Result<HeaderValue, Response>",
    "fn take_schedule_name(body: &mut Value) -> Result<String, Response>",
    "fn validated_function_body(mut body: Value) -> Result<Value, Response>",
):
    if forbidden in source:
        raise SystemExit(f"large error type remains: {forbidden}")

path.write_text(source)
