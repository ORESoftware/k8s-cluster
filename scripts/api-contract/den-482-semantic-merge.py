#!/usr/bin/env python3
"""Merge the wal-gateway contract changes into the newer fleet generator.

The original DEN-482 patch was prepared before the browser-test Fastify
contract landed on main. This script applies only the non-overlapping service
patch, then conceptually merges the wal-gateway security/visibility semantics
with the newer Node/Fastify route hints. Every replacement is exact and
fail-closed so upstream drift cannot silently discard either implementation.
"""

import os
from pathlib import Path


def replace_once(source: str, old: str, new: str, label: str) -> str:
    count = source.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return source.replace(old, new, 1)


def merge_generator() -> None:
    path = Path("remote/tools/generate-api-docs.mjs")
    source = path.read_text()

    methods_marker = """const OPENAPI_DOCUMENT_METHODS = new Set([
  'get',
  'post',
  'put',
  'patch',
  'delete',
  'head',
  'options',
  'trace',
]);

function extractOpenApiRoutes(document, sourceFile) {"""
    auth_helper = """const OPENAPI_DOCUMENT_METHODS = new Set([
  'get',
  'post',
  'put',
  'patch',
  'delete',
  'head',
  'options',
  'trace',
]);

function openApiAuthHint(operation, method, path) {
  const security = operation.security;
  if (security === undefined) {
    return undefined;
  }
  if (!Array.isArray(security)) {
    throw new Error(`${method.toUpperCase()} ${path} has a non-array OpenAPI security value`);
  }
  if (
    security.length === 0 ||
    security.some(
      (requirement) =>
        requirement &&
        typeof requirement === 'object' &&
        !Array.isArray(requirement) &&
        Object.keys(requirement).length === 0,
    )
  ) {
    return 'public';
  }

  const schemes = [
    ...new Set(
      security.flatMap((requirement) => {
        if (!requirement || typeof requirement !== 'object' || Array.isArray(requirement)) {
          throw new Error(`${method.toUpperCase()} ${path} has an invalid security requirement`);
        }
        return Object.keys(requirement);
      }),
    ),
  ].sort();
  if (schemes.length === 0) {
    return 'public';
  }
  if (schemes.every((scheme) => scheme === 'runtime_config_server_auth' || scheme === 'serverAuth')) {
    return 'X-Server-Auth (RUNTIME_CONFIG_SERVER_SECRET)';
  }
  if (schemes.every((scheme) => scheme === 'operatorSecret')) {
    return 'operator secret';
  }
  if (schemes.every((scheme) => scheme === 'webhookSignature')) {
    return 'webhook signature';
  }
  return undefined;
}

function extractOpenApiRoutes(document, sourceFile) {"""
    source = replace_once(source, methods_marker, auth_helper, "insert OpenAPI auth helper")

    operation_marker = """      const operationId = operation.operationId;
      if (typeof operationId !== 'string' || operationId.length === 0) {
        throw new Error(`${method.toUpperCase()} ${path} has no stable operationId`);
      }
      routes.push({"""
    operation_validated = """      const operationId = operation.operationId;
      if (typeof operationId !== 'string' || operationId.length === 0) {
        throw new Error(`${method.toUpperCase()} ${path} has no stable operationId`);
      }
      const visibilityHint = operation['x-dd-visibility'];
      if (visibilityHint !== undefined && !['public', 'internal'].includes(visibilityHint)) {
        throw new Error(
          `${method.toUpperCase()} ${path} has invalid x-dd-visibility ${JSON.stringify(visibilityHint)}`,
        );
      }
      const explicitAuthHint = operation['x-dd-auth'];
      if (
        explicitAuthHint !== undefined &&
        (typeof explicitAuthHint !== 'string' || explicitAuthHint.length === 0)
      ) {
        throw new Error(`${method.toUpperCase()} ${path} has an invalid x-dd-auth value`);
      }
      routes.push({"""
    source = replace_once(source, operation_marker, operation_validated, "validate OpenAPI hints")
    source = replace_once(
        source,
        "        visibilityHint: operation['x-dd-visibility'],\n        authHint: operation['x-dd-auth'],",
        "        visibilityHint,\n        authHint: explicitAuthHint ?? openApiAuthHint(operation, method, path),",
        "compose explicit and inferred hints",
    )

    merge_marker = """    current.sourceFiles.add(route.sourceFile);
    if (route.purposeHint && !current.purposeHint) {"""
    merge_hints = """    current.sourceFiles.add(route.sourceFile);
    for (const hint of ['visibilityHint', 'authHint', 'routeTypeHint']) {
      if (route[hint] === undefined) continue;
      if (current[hint] !== undefined && current[hint] !== route[hint]) {
        throw new Error(
          `ambiguous ${hint} for ${route.path}: ${JSON.stringify(current[hint])} versus ${JSON.stringify(route[hint])}`,
        );
      }
      current[hint] = route[hint];
    }
    if (route.purposeHint && !current.purposeHint) {"""
    source = replace_once(source, merge_marker, merge_hints, "merge route hints")
    path.write_text(source)


def record_runtime_auth_boundaries() -> None:
    path = Path("remote/deployments/wal-gateway-rs/src/docs.rs")
    source = path.read_text()
    visibility_marker = """            operation.insert(
                \"x-dd-visibility\".to_string(),
                Value::String(if public.contains(path.as_str()) {
                    \"public\"
                } else {
                    \"internal\"
                }
                .to_string()),
            );"""
    visibility_auth = """            let visibility = if public.contains(path.as_str()) {
                \"public\"
            } else {
                \"internal\"
            };
            operation.insert(
                \"x-dd-visibility\".to_string(),
                Value::String(visibility.to_string()),
            );
            if !operation.contains_key(\"x-dd-auth\") {
                let auth = if visibility == \"public\" {
                    Some(\"public\")
                } else {
                    match path.as_str() {
                        \"/healthz\" | \"/readyz\" | \"/metrics\" => Some(\"cluster-network-policy\"),
                        _ => None,
                    }
                };
                if let Some(auth) = auth {
                    operation.insert(
                        \"x-dd-auth\".to_string(),
                        Value::String(auth.to_string()),
                    );
                }
            }"""
    source = replace_once(source, visibility_marker, visibility_auth, "record route auth boundary")
    path.write_text(source)


def canonicalize_export_order() -> None:
    path = Path("remote/deployments/wal-gateway-rs/src/docs.rs")
    source = path.read_text()
    old = """pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    let mut json = serde_json::to_string_pretty(openapi)?;
    json.push('\\n');
    Ok(json)
}"""
    new = """fn sort_json_objects(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut entries = std::mem::take(object).into_iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (_, child) in &mut entries {
                sort_json_objects(child);
            }
            object.extend(entries);
        }
        Value::Array(items) => {
            for item in items {
                sort_json_objects(item);
            }
        }
        _ => {}
    }
}

pub fn canonical_json(openapi: &OpenApi) -> Result<String, serde_json::Error> {
    // Serializing the Utoipa model directly can expose randomized map order.
    // Normalize every JSON object while preserving semantically ordered arrays.
    let mut value = serde_json::to_value(openapi)?;
    sort_json_objects(&mut value);
    let mut json = serde_json::to_string_pretty(&value)?;
    json.push('\\n');
    Ok(json)
}"""
    source = replace_once(source, old, new, "canonicalize OpenAPI export order")
    path.write_text(source)


def pin_shared_contract_revision() -> None:
    path = Path("remote/api-contracts/manifest.json")
    source = path.read_text()
    merged_commit = os.environ.get("SHARED_COMMIT")
    if not merged_commit:
        raise SystemExit("SHARED_COMMIT is required to pin shared contract provenance")
    source = replace_once(
        source,
        "2504b054ac92becf265762c1bd1f0679a10de893",
        merged_commit,
        "pin merged shared runtime-config revision",
    )
    path.write_text(source)


def enforce_private_operation_security() -> None:
    path = Path("remote/tools/check-openapi-contracts.mjs")
    source = path.read_text()
    security_marker = """      if (!PUBLIC_PATHS.has(path)) {
        if (!Array.isArray(security) || security.length === 0) {
          throw new Error(`${name}: ${method.toUpperCase()} ${path} has no security requirement`);
        }
      }"""
    security_boundary = """      if (!PUBLIC_PATHS.has(path)) {
        const networkBoundary =
          operation['x-dd-auth'] === 'cluster-network-policy' &&
          (security === undefined || (Array.isArray(security) && security.length === 0));
        if ((!Array.isArray(security) || security.length === 0) && !networkBoundary) {
          throw new Error(`${name}: ${method.toUpperCase()} ${path} has no security requirement`);
        }
      }"""
    source = replace_once(
        source,
        security_marker,
        security_boundary,
        "allow explicit cluster network boundary",
    )
    path.write_text(source)


def main() -> None:
    merge_generator()
    record_runtime_auth_boundaries()
    canonicalize_export_order()
    pin_shared_contract_revision()
    enforce_private_operation_security()


if __name__ == "__main__":
    main()
