#!/usr/bin/env python3
"""Fail CI when Fiducia customer/admin identity planes drift together."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SECRETS = ROOT / "remote/argocd/fiducia/fiducia-runtime-secrets.externalsecret.yaml"
CUSTOMER_DEPLOYMENT = ROOT / "remote/argocd/fiducia/fiducia-backend.deployment.yaml"
ADMIN_DEPLOYMENT = ROOT / "remote/argocd/fiducia/fiducia-admin.deployment.yaml"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"fiducia auth boundary check failed: {message}")


def external_secret_document(text: str, name: str) -> str:
    for document in re.split(r"^---\s*$", text, flags=re.MULTILINE):
        if re.search(rf"^\s*name:\s*{re.escape(name)}\s*$", document, re.MULTILINE):
            return document
    raise SystemExit(f"fiducia auth boundary check failed: missing ExternalSecret {name}")


def deployment_secret_ref(text: str, env_name: str) -> tuple[str, str]:
    pattern = re.compile(
        rf"-\s+name:\s*{re.escape(env_name)}\s+"
        rf"valueFrom:\s+secretKeyRef:\s*\{{\s*name:\s*([^,\s]+),\s*"
        rf"key:\s*([^,\s}}]+)",
        re.MULTILINE,
    )
    match = pattern.search(text)
    if not match:
        raise SystemExit(
            f"fiducia auth boundary check failed: {env_name} is not sourced from a Secret"
        )
    return match.group(1), match.group(2)


def main() -> None:
    secrets_text = SECRETS.read_text(encoding="utf-8")
    customer_secret = external_secret_document(secrets_text, "fiducia-backend-secrets")
    admin_secret = external_secret_document(secrets_text, "fiducia-admin-secrets")

    require(
        "key: dd/remote-dev/fiducia-backend-secrets" in customer_secret,
        "customer secrets must use the customer remote object",
    )
    require(
        "key: dd/remote-dev/fiducia-admin-secrets" in admin_secret,
        "admin secrets must use the admin remote object",
    )
    require(
        "property: FIDUCIA_CUSTOMER_SUPABASE_URL" in customer_secret,
        "customer Supabase URL must have a plane-specific remote property",
    )
    require(
        "property: FIDUCIA_CUSTOMER_SUPABASE_PUBLISHABLE_KEY" in customer_secret,
        "customer Supabase key must have a plane-specific remote property",
    )
    require(
        "property: FIDUCIA_ADMIN_SUPABASE_URL" in admin_secret,
        "admin Supabase URL must have a plane-specific remote property",
    )
    require(
        "property: FIDUCIA_ADMIN_SUPABASE_PUBLISHABLE_KEY" in admin_secret,
        "admin Supabase key must have a plane-specific remote property",
    )
    require(
        "dd.dev/supabase-plane: customer" in customer_secret,
        "customer target Secret must carry its plane label",
    )
    require(
        "dd.dev/supabase-plane: admin" in admin_secret,
        "admin target Secret must carry its plane label",
    )

    customer_deployment = CUSTOMER_DEPLOYMENT.read_text(encoding="utf-8")
    admin_deployment = ADMIN_DEPLOYMENT.read_text(encoding="utf-8")
    customer_url_ref = deployment_secret_ref(customer_deployment, "SUPABASE_URL")
    customer_key_ref = deployment_secret_ref(
        customer_deployment, "SUPABASE_PUBLISHABLE_KEY"
    )
    admin_url_ref = deployment_secret_ref(admin_deployment, "SUPABASE_URL")
    admin_key_ref = deployment_secret_ref(admin_deployment, "SUPABASE_PUBLISHABLE_KEY")

    require(
        customer_url_ref == ("fiducia-backend-secrets", "supabase-url"),
        f"customer SUPABASE_URL has unexpected source {customer_url_ref}",
    )
    require(
        customer_key_ref == ("fiducia-backend-secrets", "supabase-publishable-key"),
        f"customer publishable key has unexpected source {customer_key_ref}",
    )
    require(
        admin_url_ref == ("fiducia-admin-secrets", "supabase-url"),
        f"admin SUPABASE_URL has unexpected source {admin_url_ref}",
    )
    require(
        admin_key_ref == ("fiducia-admin-secrets", "supabase-publishable-key"),
        f"admin publishable key has unexpected source {admin_key_ref}",
    )
    require(
        customer_url_ref[0] != admin_url_ref[0],
        "customer and admin must never consume the same Kubernetes Secret",
    )

    print("fiducia auth boundary check passed")


if __name__ == "__main__":
    main()
