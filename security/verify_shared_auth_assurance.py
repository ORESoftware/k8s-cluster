#!/usr/bin/env python3
"""Fail-closed, dependency-free checks for the Shared Auth assurance boundary.

This executable contract is intentionally narrower than Rust compilation and tests.
It exists so CI still exercises the security-critical policy when the private
generated interfaces checkout is unavailable. Locked Cargo check, Clippy, and
Rust tests remain the stronger release gate and must not be represented as
having run when they were skipped.
"""

from __future__ import annotations

import argparse
import ast
import operator
import re
import sys
from pathlib import Path
from typing import Callable

EXPECTED_ACR = '"urn:oresoftware:loa:2"'
EXPECTED_NUMERIC_CONSTANTS = {
    "SHARED_AUTH_MAX_AUTH_AGE_SECONDS": 15 * 60,
    "SHARED_AUTH_CLOCK_SKEW_SECONDS": 60,
    "SHARED_AUTH_MAX_AMR_METHODS": 16,
    "SHARED_AUTH_MAX_AMR_METHOD_BYTES": 64,
}
EXPECTED_DENIED_METHODS = {"pwd", "password"}
EXPECTED_PRIMARY_METHODS = {
    "federated",
    "email_otp",
    "otp",
    "magiclink",
    "magic_link",
    "email/signup",
}
EXPECTED_STRONG_SECOND_METHODS = {"totp", "sms_otp", "passkey", "webauthn"}
EXPECTED_TESTS = {
    "accepts_passwordless_primary_with_approved_independent_second_factor",
    "rejects_numeric_aal2_without_the_canonical_method_chain",
    "rejects_missing_wrong_stale_or_future_assurance_context",
    "shared_auth_introspection_requires_active_aal2_identity",
}

_NUMERIC_BINOPS: dict[type[ast.operator], Callable[[int, int], int]] = {
    ast.Add: operator.add,
    ast.Sub: operator.sub,
    ast.Mult: operator.mul,
    ast.FloorDiv: operator.floordiv,
}


def _compact(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip()


def _matching_brace(source: str, open_index: int) -> int:
    """Return the matching `}` while ignoring comments and string literals."""

    if source[open_index] != "{":
        raise ValueError("open_index must point to an opening brace")

    depth = 0
    index = open_index
    state = "code"
    block_comment_depth = 0
    raw_terminator = ""

    while index < len(source):
        char = source[index]
        nxt = source[index + 1] if index + 1 < len(source) else ""

        if state == "line-comment":
            if char == "\n":
                state = "code"
            index += 1
            continue

        if state == "block-comment":
            if char == "/" and nxt == "*":
                block_comment_depth += 1
                index += 2
                continue
            if char == "*" and nxt == "/":
                block_comment_depth -= 1
                index += 2
                if block_comment_depth == 0:
                    state = "code"
                continue
            index += 1
            continue

        if state == "string":
            if char == "\\":
                index += 2
                continue
            if char == '"':
                state = "code"
            index += 1
            continue

        if state == "raw-string":
            if source.startswith(raw_terminator, index):
                index += len(raw_terminator)
                state = "code"
            else:
                index += 1
            continue

        if state == "char":
            if char == "\\":
                index += 2
                continue
            if char == "'":
                state = "code"
            index += 1
            continue

        if char == "/" and nxt == "/":
            state = "line-comment"
            index += 2
            continue
        if char == "/" and nxt == "*":
            state = "block-comment"
            block_comment_depth = 1
            index += 2
            continue

        raw_match = re.match(r'(?:br|rb|r)(?P<hashes>#{0,32})"', source[index:])
        if raw_match and (
            index == 0
            or not (source[index - 1].isalnum() or source[index - 1] == "_")
        ):
            raw_terminator = '"' + raw_match.group("hashes")
            state = "raw-string"
            index += raw_match.end()
            continue

        if char == "b" and nxt == '"':
            state = "string"
            index += 2
            continue
        if char == '"':
            state = "string"
            index += 1
            continue

        # Treat a quote as a char literal only when a nearby closing quote
        # exists. This avoids mistaking Rust lifetimes such as `'a` for chars.
        if char == "'":
            closing = index + 1
            escaped = False
            while closing < min(len(source), index + 10) and source[closing] != "\n":
                current = source[closing]
                if current == "'" and not escaped:
                    state = "char"
                    break
                escaped = current == "\\" and not escaped
                if current != "\\":
                    escaped = False
                closing += 1
            if state == "char":
                index += 1
                continue

        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index
            if depth < 0:
                break
        index += 1

    raise ValueError("unbalanced Rust item")


def _extract_item(source: str, pattern: str, label: str) -> str:
    match = re.search(pattern, source, flags=re.MULTILINE)
    if not match:
        raise ValueError(f"missing {label}")
    open_index = source.find("{", match.end())
    if open_index < 0:
        raise ValueError(f"missing body for {label}")
    close_index = _matching_brace(source, open_index)
    return source[match.start() : close_index + 1]


def _constant_expression(source: str, name: str) -> str | None:
    match = re.search(
        rf"\bconst\s+{re.escape(name)}\s*:\s*[^=;]+=\s*(?P<value>[^;]+);",
        source,
    )
    return match.group("value").strip() if match else None


def _numeric_expression_value(expression: str) -> int:
    if not re.fullmatch(r"[0-9_+\-*/()\s]+", expression):
        raise ValueError("unsupported numeric constant expression")

    def evaluate(node: ast.AST) -> int:
        if isinstance(node, ast.Expression):
            return evaluate(node.body)
        if isinstance(node, ast.Constant) and type(node.value) is int:
            return node.value
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
            return -evaluate(node.operand)
        if isinstance(node, ast.BinOp) and type(node.op) in _NUMERIC_BINOPS:
            left = evaluate(node.left)
            right = evaluate(node.right)
            if isinstance(node.op, ast.FloorDiv) and right == 0:
                raise ValueError("division by zero")
            return _NUMERIC_BINOPS[type(node.op)](left, right)
        # Rust `/` between integer constants has integer semantics.
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Div):
            right = evaluate(node.right)
            if right == 0:
                raise ValueError("division by zero")
            return evaluate(node.left) // right
        raise ValueError("unsupported numeric constant expression")

    return evaluate(ast.parse(expression.replace("_", ""), mode="eval"))


def _string_literals(value: str) -> set[str]:
    return set(re.findall(r'"([^"\\]*(?:\\.[^"\\]*)*)"', value))


def _assignment_region(function: str, variable: str, next_marker: str) -> str | None:
    match = re.search(
        rf"\blet\s+{re.escape(variable)}\s*=\s*(?P<body>.*?)\s*;\s*{next_marker}",
        function,
        flags=re.DOTALL,
    )
    return match.group("body") if match else None


def validate_source(source: str) -> list[str]:
    errors: list[str] = []

    acr = _constant_expression(source, "SHARED_AUTH_REQUIRED_ACR")
    if acr is None or _compact(acr) != EXPECTED_ACR:
        errors.append("ACR: SHARED_AUTH_REQUIRED_ACR must remain urn:oresoftware:loa:2")

    for name, expected in EXPECTED_NUMERIC_CONSTANTS.items():
        expression = _constant_expression(source, name)
        if expression is None:
            errors.append(f"CONST_{name}: missing {name}")
            continue
        try:
            actual = _numeric_expression_value(expression)
        except (SyntaxError, ValueError) as error:
            errors.append(f"CONST_{name}: cannot evaluate {name}: {error}")
            continue
        if actual != expected:
            errors.append(f"CONST_{name}: {name} must equal {expected}, found {actual}")

    try:
        introspection = _extract_item(
            source,
            r"\bstruct\s+SharedAuthIntrospection\b",
            "SharedAuthIntrospection",
        )
    except ValueError as error:
        errors.append(f"INTROSPECTION: {error}")
        introspection = ""

    field_patterns = {
        "aal": r'#\s*\[\s*serde\s*\(\s*default\s*=\s*"default_auth_assurance_level"\s*\)\s*\]\s*aal\s*:\s*u8',
        "amr": r"#\s*\[\s*serde\s*\(\s*default\s*\)\s*\]\s*amr\s*:\s*Vec\s*<\s*String\s*>",
        "acr": r"#\s*\[\s*serde\s*\(\s*default\s*\)\s*\]\s*acr\s*:\s*Option\s*<\s*String\s*>",
        "iat": r"#\s*\[\s*serde\s*\(\s*default\s*\)\s*\]\s*iat\s*:\s*Option\s*<\s*u64\s*>",
    }
    for field, pattern in field_patterns.items():
        if introspection and not re.search(pattern, introspection, flags=re.DOTALL):
            errors.append(
                f"INTROSPECTION_{field.upper()}: "
                f"{field} must retain its bounded typed/defaulted contract"
            )

    try:
        assurance = _extract_item(
            source,
            r"\bfn\s+validate_shared_auth_assurance\s*\(",
            "validate_shared_auth_assurance",
        )
    except ValueError as error:
        errors.append(f"ASSURANCE: {error}")
        assurance = ""

    compact = _compact(assurance)
    required_fragments = {
        "AAL_GATE": "if introspection.aal < required_aal",
        "AAL1_COMPAT": "if required_aal < 2",
        "ACR_GATE": "introspection.acr.as_deref() != Some(SHARED_AUTH_REQUIRED_ACR)",
        "EMPTY_AMR": "introspection.amr.is_empty()",
        "AMR_COUNT": "introspection.amr.len() > SHARED_AUTH_MAX_AMR_METHODS",
        "NORMALIZATION": "raw.trim().to_ascii_lowercase()",
        "METHOD_LENGTH": "method.len() > SHARED_AUTH_MAX_AMR_METHOD_BYTES",
        "METHOD_CHARSET": 'b"_:-./".contains(&byte)',
        "DUPLICATE_AMR": "methods.contains(&method)",
        "IAT_REQUIRED": "introspection.iat.ok_or(ServiceError::MfaRequired)?",
        "FUTURE_IAT": "issued_at > now.saturating_add(SHARED_AUTH_CLOCK_SKEW_SECONDS)",
        "STALE_IAT": "now.saturating_sub(issued_at) > SHARED_AUTH_MAX_AUTH_AGE_SECONDS",
    }
    for code, fragment in required_fragments.items():
        if assurance and fragment not in compact:
            errors.append(f"{code}: required fail-closed assurance check is missing")

    denied_match = re.search(
        r"if\s+methods\s*\.iter\(\)\s*\.any\((?P<body>.*?)\)\s*\{",
        assurance,
        flags=re.DOTALL,
    )
    denied = _string_literals(denied_match.group("body")) if denied_match else set()
    if denied != EXPECTED_DENIED_METHODS:
        errors.append(
            "DENIED_METHODS: password-derived methods must be exactly "
            + ", ".join(sorted(EXPECTED_DENIED_METHODS))
        )

    primary_region = _assignment_region(
        assurance,
        "passwordless_primary",
        r"let\s+strong_second",
    )
    primary = _string_literals(primary_region) if primary_region else set()
    if primary != EXPECTED_PRIMARY_METHODS:
        errors.append(
            "PRIMARY_METHODS: passwordless primary allow-list changed without contract review"
        )

    strong_region = _assignment_region(
        assurance,
        "strong_second",
        r"if\s+!passwordless_primary",
    )
    strong = _string_literals(strong_region) if strong_region else set()
    if strong != EXPECTED_STRONG_SECOND_METHODS:
        errors.append(
            "STRONG_SECOND_METHODS: independent strong-factor allow-list changed "
            "without contract review"
        )

    try:
        production = _extract_item(
            source,
            r"\basync\s+fn\s+introspect_shared_auth\s*\(",
            "introspect_shared_auth",
        )
    except ValueError as error:
        errors.append(f"PRODUCTION_WIRING: {error}")
        production = ""
    if production and "validate_shared_auth_assurance(" not in production:
        errors.append(
            "PRODUCTION_WIRING: introspect_shared_auth must invoke "
            "validate_shared_auth_assurance"
        )

    for test_name in EXPECTED_TESTS:
        if not re.search(rf"\bfn\s+{re.escape(test_name)}\s*\(", source):
            errors.append(f"TEST_{test_name}: focused regression test is missing")

    return errors


GOOD_FIXTURE = r"""
const SHARED_AUTH_REQUIRED_ACR: &str = "urn:oresoftware:loa:2";
const SHARED_AUTH_MAX_AUTH_AGE_SECONDS: u64 = 15 * 60;
const SHARED_AUTH_CLOCK_SKEW_SECONDS: u64 = 60;
const SHARED_AUTH_MAX_AMR_METHODS: usize = 16;
const SHARED_AUTH_MAX_AMR_METHOD_BYTES: usize = 64;

#[derive(Debug, Deserialize)]
struct SharedAuthIntrospection {
    #[serde(default = "default_auth_assurance_level")]
    aal: u8,
    #[serde(default)]
    amr: Vec<String>,
    #[serde(default)]
    acr: Option<String>,
    #[serde(default)]
    iat: Option<u64>,
}

fn validate_shared_auth_assurance(
    introspection: &SharedAuthIntrospection,
    required_aal: u8,
    now: u64,
) -> Result<(), ServiceError> {
    if introspection.aal < required_aal {
        return Err(ServiceError::MfaRequired);
    }
    if required_aal < 2 {
        return Ok(());
    }
    if introspection.acr.as_deref() != Some(SHARED_AUTH_REQUIRED_ACR)
        || introspection.amr.is_empty()
        || introspection.amr.len() > SHARED_AUTH_MAX_AMR_METHODS
    {
        return Err(ServiceError::MfaRequired);
    }
    let mut methods = Vec::with_capacity(introspection.amr.len());
    for raw in &introspection.amr {
        let method = raw.trim().to_ascii_lowercase();
        if method.is_empty()
            || method.len() > SHARED_AUTH_MAX_AMR_METHOD_BYTES
            || !method.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_:-./".contains(&byte)
            })
            || methods.contains(&method)
        {
            return Err(ServiceError::MfaRequired);
        }
        methods.push(method);
    }
    if methods
        .iter()
        .any(|method| matches!(method.as_str(), "pwd" | "password"))
    {
        return Err(ServiceError::MfaRequired);
    }
    let passwordless_primary = methods.iter().any(|method| {
        matches!(
            method.as_str(),
            "federated" | "email_otp" | "otp" | "magiclink" | "magic_link" | "email/signup"
        )
    });
    let strong_second = methods
        .iter()
        .any(|method| matches!(method.as_str(), "totp" | "sms_otp" | "passkey" | "webauthn"));
    if !passwordless_primary || !strong_second {
        return Err(ServiceError::MfaRequired);
    }
    let issued_at = introspection.iat.ok_or(ServiceError::MfaRequired)?;
    if issued_at > now.saturating_add(SHARED_AUTH_CLOCK_SKEW_SECONDS)
        || now.saturating_sub(issued_at) > SHARED_AUTH_MAX_AUTH_AGE_SECONDS
    {
        return Err(ServiceError::MfaRequired);
    }
    Ok(())
}

async fn introspect_shared_auth() {
    validate_shared_auth_assurance(&introspection, required_aal, now)?;
}

fn accepts_passwordless_primary_with_approved_independent_second_factor() {}
fn rejects_numeric_aal2_without_the_canonical_method_chain() {}
fn rejects_missing_wrong_stale_or_future_assurance_context() {}
fn shared_auth_introspection_requires_active_aal2_identity() {}
"""


def _replace_once(source: str, old: str, new: str, mutation: str) -> str:
    if old not in source:
        raise AssertionError(f"self-test mutation {mutation!r} could not find its target")
    return source.replace(old, new, 1)


def run_self_test() -> None:
    baseline_errors = validate_source(GOOD_FIXTURE)
    if baseline_errors:
        raise AssertionError("good fixture failed:\n" + "\n".join(baseline_errors))

    mutations: list[tuple[str, str, str, str]] = [
        (
            "weaken canonical ACR",
            '"urn:oresoftware:loa:2"',
            '"urn:oresoftware:loa:1"',
            "ACR:",
        ),
        (
            "extend auth freshness to a day",
            "15 * 60",
            "24 * 60 * 60",
            "CONST_SHARED_AUTH_MAX_AUTH_AGE_SECONDS:",
        ),
        (
            "drop password alias rejection",
            '"pwd" | "password"',
            '"pwd"',
            "DENIED_METHODS:",
        ),
        (
            "accept a primary method as the second factor",
            '"totp" | "sms_otp" | "passkey" | "webauthn"',
            '"totp" | "sms_otp" | "passkey" | "webauthn" | "email_otp"',
            "STRONG_SECOND_METHODS:",
        ),
        (
            "remove duplicate AMR rejection",
            "            || methods.contains(&method)\n",
            "",
            "DUPLICATE_AMR:",
        ),
        (
            "default a missing issued-at time",
            "introspection.iat.ok_or(ServiceError::MfaRequired)?",
            "introspection.iat.unwrap_or(now)",
            "IAT_REQUIRED:",
        ),
        (
            "remove production validation wiring",
            "    validate_shared_auth_assurance(&introspection, required_aal, now)?;\n",
            "",
            "PRODUCTION_WIRING:",
        ),
        (
            "remove the accepted-chain regression",
            "fn accepts_passwordless_primary_with_approved_independent_second_factor() {}",
            "fn accepted_chain_test_removed() {}",
            "TEST_accepts_passwordless_primary_with_approved_independent_second_factor:",
        ),
        (
            "allow an unbounded AMR list",
            "const SHARED_AUTH_MAX_AMR_METHODS: usize = 16;",
            "const SHARED_AUTH_MAX_AMR_METHODS: usize = 1600;",
            "CONST_SHARED_AUTH_MAX_AMR_METHODS:",
        ),
        (
            "allow oversized AMR values",
            "const SHARED_AUTH_MAX_AMR_METHOD_BYTES: usize = 64;",
            "const SHARED_AUTH_MAX_AMR_METHOD_BYTES: usize = 4096;",
            "CONST_SHARED_AUTH_MAX_AMR_METHOD_BYTES:",
        ),
    ]

    for mutation, old, new, expected_error_prefix in mutations:
        candidate = _replace_once(GOOD_FIXTURE, old, new, mutation)
        errors = validate_source(candidate)
        if not any(error.startswith(expected_error_prefix) for error in errors):
            raise AssertionError(
                f"mutation {mutation!r} escaped; expected "
                f"{expected_error_prefix!r}, got {errors!r}"
            )

    print(f"self-test: PASS ({len(mutations)} adversarial mutations detected)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "source",
        nargs="?",
        default="src/service.rs",
        help="Rust source file to validate (default: src/service.rs)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run adversarial mutation tests for this validator",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        try:
            run_self_test()
        except AssertionError as error:
            print(f"self-test: FAIL\n{error}", file=sys.stderr)
            return 1
        return 0

    source_path = Path(args.source)
    try:
        source = source_path.read_text(encoding="utf-8")
    except OSError as error:
        print(f"contract: FAIL: cannot read {source_path}: {error}", file=sys.stderr)
        return 2

    errors = validate_source(source)
    if errors:
        print("shared-auth assurance contract: FAIL", file=sys.stderr)
        for error in errors:
            print(f" - {error}", file=sys.stderr)
        return 1

    print("shared-auth assurance contract: PASS")
    print(
        "note: this dependency-free contract does not replace locked Cargo check, "
        "Clippy, or Rust tests"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
