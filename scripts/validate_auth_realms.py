#!/usr/bin/env python3
"""Secret-neutral validation for the two Shared Auth runtime realms."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

REQUIRED_REALMS = {"admin", "customer"}
DISJOINT_FIELDS = {
    "deployment",
    "issuer",
    "databaseEndpointHost",
    "databaseResourceRef",
    "databaseSecretRef",
    "signingKeyRef",
    "sessionCookieName",
    "supabaseProjectRef",
}
REQUIRED_SCHEMA_TABLES = {
    "applications",
    "application_accounts",
    "oauth_clients",
    "application_consents",
    "session_application_grants",
}
REQUIRED_REALM_SOURCE_TOKENS = {
    'required_env("AUTH_REALM")',
    'required_env("AUTH_REALM_DEPLOYMENT")',
    'required_env("AUTH_DATABASE_RESOURCE_REF")',
    'required_env("AUTH_DATABASE_SECRET_REF")',
    'required_env("AUTH_SIGNING_KEY_REF")',
    'required_env("AUTH_SESSION_COOKIE_NAME")',
    'required_env("AUTH_REALM_SUPABASE_PROJECT_REF")',
    "AUTH_APPLICATION_DATABASE_URL is forbidden in shared-auth",
    "provider_refs.len() != 1",
    "AUTH_DATABASE_URL must target the selected realm PostgreSQL endpoint",
}


def validate_contract(
    contract: Any,
    schema_sql: str,
    realm_source: str,
) -> list[str]:
    errors: list[str] = []
    if not isinstance(contract, dict):
        return ["contract must be a JSON object"]
    if contract.get("schemaVersion") != 1:
        errors.append("schemaVersion must equal 1")
    if contract.get("applicationDatabaseFallbackAllowed") is not False:
        errors.append("applicationDatabaseFallbackAllowed must be false")
    if contract.get("providerProjectsPerRealm") != 1:
        errors.append("providerProjectsPerRealm must equal 1")
    if contract.get("productAuthorizationOwner") != "application-databases":
        errors.append("productAuthorizationOwner must be application-databases")

    profiles = contract.get("profiles")
    if not isinstance(profiles, list):
        return [*errors, "profiles must be an array"]
    if len(profiles) != 2:
        errors.append("exactly two realm profiles are required")

    seen_realms: set[str] = set()
    for index, profile in enumerate(profiles):
        prefix = f"profiles[{index}]"
        if not isinstance(profile, dict):
            errors.append(f"{prefix} must be an object")
            continue
        realm = _text(profile.get("realm"))
        if realm not in REQUIRED_REALMS:
            errors.append(f"{prefix}.realm must be admin or customer")
        if realm in seen_realms:
            errors.append(f"{prefix}.realm duplicates {realm}")
        seen_realms.add(realm)

        for field in DISJOINT_FIELDS:
            if not _text(profile.get(field)):
                errors.append(f"{prefix}.{field} is required")

        issuer = urlparse(_text(profile.get("issuer")))
        if issuer.scheme != "https" or not issuer.hostname:
            errors.append(f"{prefix}.issuer must be an HTTPS URL")
        elif realm == "admin" and "admin-auth" not in issuer.hostname.split(".")[0]:
            errors.append(f"{prefix}.issuer must use an admin-auth host")
        elif realm == "customer" and "admin-auth" in issuer.hostname.split(".")[0]:
            errors.append(f"{prefix}.issuer must not use an admin-auth host")

        if realm and realm not in _text(profile.get("deployment")):
            errors.append(f"{prefix}.deployment must name its realm")
        if realm and realm not in _text(profile.get("databaseEndpointHost")):
            errors.append(f"{prefix}.databaseEndpointHost must name its realm")
        if realm and realm not in _text(profile.get("databaseResourceRef")):
            errors.append(f"{prefix}.databaseResourceRef must name its realm")
        if realm and f"/{realm}/" not in _text(profile.get("databaseSecretRef")):
            errors.append(f"{prefix}.databaseSecretRef must be realm scoped")
        if realm and f"/{realm}/" not in _text(profile.get("signingKeyRef")):
            errors.append(f"{prefix}.signingKeyRef must be realm scoped")

        cookie = _text(profile.get("sessionCookieName"))
        if not cookie.startswith("__Host-") or (realm and realm not in cookie):
            errors.append(f"{prefix}.sessionCookieName must be a realm-specific __Host- name")
        project_ref = _text(profile.get("supabaseProjectRef"))
        if not (6 <= len(project_ref) <= 64 and project_ref.isalnum()):
            errors.append(f"{prefix}.supabaseProjectRef must be alphanumeric")

    for realm in REQUIRED_REALMS:
        if realm not in seen_realms:
            errors.append(f"missing {realm} realm profile")
    for field in DISJOINT_FIELDS:
        values = [_text(profile.get(field)) for profile in profiles if isinstance(profile, dict)]
        values = [value for value in values if value]
        if len(values) != len(set(values)):
            errors.append(f"{field} must be distinct across realms")

    lowered_schema = schema_sql.lower()
    for table in REQUIRED_SCHEMA_TABLES:
        token = f"create table if not exists shared_auth.{table}"
        if token not in lowered_schema:
            errors.append(f"schema is missing {table}")
    if "product authorization remains in each application database" not in lowered_schema:
        errors.append("schema must preserve product-local authorization ownership")

    for token in REQUIRED_REALM_SOURCE_TOKENS:
        if token not in realm_source:
            errors.append(f"realm source is missing guard: {token}")

    return errors


def _text(value: Any) -> str:
    return value.strip() if isinstance(value, str) else ""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--contract", required=True, type=Path)
    parser.add_argument("--schema", required=True, type=Path)
    parser.add_argument("--realm-source", required=True, type=Path)
    args = parser.parse_args()

    try:
        contract = json.loads(args.contract.read_text(encoding="utf-8"))
        schema_sql = args.schema.read_text(encoding="utf-8")
        realm_source = args.realm_source.read_text(encoding="utf-8")
    except (OSError, json.JSONDecodeError) as error:
        print(f"unable to load realm contract inputs: {error}")
        return 66

    errors = validate_contract(contract, schema_sql, realm_source)
    if errors:
        for error in errors:
            print(f"- {error}")
        return 1
    print("shared-auth runtime realms and application federation schema are valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
