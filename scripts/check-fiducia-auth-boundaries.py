#!/usr/bin/env python3
"""Fail CI when Fiducia customer/admin identity planes drift together."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SECRETS = ROOT / "remote/argocd/fiducia/fiducia-runtime-secrets.externalsecret.yaml"
CUSTOMER_DEPLOYMENT = ROOT / "remote/argocd/fiducia/fiducia-backend.deployment.yaml"
ADMIN_DEPLOYMENT = ROOT / "remote/argocd/fiducia/fiducia-admin.deployment.yaml"


def fail(message: str) -> None:
    raise SystemExit(f"fiducia auth boundary check failed: {message}")


def require(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def indentation(line: str) -> int:
    return len(line) - len(line.lstrip())


def yaml_scalar(value: str) -> str:
    """Read the simple scalar forms used by these deployment manifests."""
    value = value.strip()
    if " #" in value:
        value = value.split(" #", 1)[0].rstrip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
        return value[1:-1]
    return value


def list_entry_blocks(text: str, item_key: str, item_value: str) -> list[str]:
    """Return all YAML list entries beginning with ``- item_key: item_value``.

    This deliberately avoids adding a YAML dependency to the cluster contract
    job while still respecting indentation, comments, and duplicate entries.
    """
    lines = text.splitlines()
    pattern = re.compile(
        rf"^(?P<indent>\s*)-\s+{re.escape(item_key)}:\s*"
        rf"{re.escape(item_value)}\s*$"
    )
    blocks: list[str] = []
    for index, line in enumerate(lines):
        match = pattern.match(line)
        if not match:
            continue
        base_indent = len(match.group("indent"))
        end = index + 1
        while end < len(lines):
            candidate = lines[end]
            stripped = candidate.strip()
            if (
                stripped
                and not stripped.startswith("#")
                and indentation(candidate) <= base_indent
            ):
                break
            end += 1
        blocks.append("\n".join(lines[index:end]))
    return blocks


def nested_mapping(
    block: str,
    container: str,
    fields: tuple[str, ...],
    context: str,
) -> tuple[str, ...]:
    """Read a small nested mapping in either flow or block YAML style."""
    values: dict[str, str] = {}
    inline = re.search(
        rf"\b{re.escape(container)}:\s*\{{(?P<body>[^}}]*)\}}",
        block,
    )
    if inline:
        body = inline.group("body")
        for field in fields:
            match = re.search(
                rf"(?:^|,)\s*{re.escape(field)}:\s*([^,}}]+)",
                body,
            )
            if match:
                values[field] = yaml_scalar(match.group(1))
    else:
        lines = block.splitlines()
        for index, line in enumerate(lines):
            match = re.match(
                rf"^(?P<indent>\s*){re.escape(container)}:\s*$",
                line,
            )
            if not match:
                continue
            base_indent = len(match.group("indent"))
            for candidate in lines[index + 1 :]:
                stripped = candidate.strip()
                if (
                    stripped
                    and not stripped.startswith("#")
                    and indentation(candidate) <= base_indent
                ):
                    break
                field_match = re.match(
                    r"^\s*([A-Za-z0-9_-]+):\s*(.+?)\s*$",
                    candidate,
                )
                if field_match and field_match.group(1) in fields:
                    values[field_match.group(1)] = yaml_scalar(field_match.group(2))
            break

    missing = [field for field in fields if not values.get(field)]
    if missing:
        fail(
            f"{context} {container} must define {', '.join(fields)}; "
            f"missing {', '.join(missing)}"
        )
    return tuple(values[field] for field in fields)


def metadata_name(document: str) -> str | None:
    """Return only the top-level Kubernetes ``metadata.name`` value."""
    lines = document.splitlines()
    for index, line in enumerate(lines):
        if not re.match(r"^metadata:\s*$", line):
            continue
        for candidate in lines[index + 1 :]:
            stripped = candidate.strip()
            if stripped and indentation(candidate) == 0:
                break
            match = re.match(r"^\s+name:\s*(.+?)\s*$", candidate)
            if match:
                return yaml_scalar(match.group(1))
        break
    return None


def external_secret_document(text: str, name: str) -> str:
    matches = [
        document
        for document in re.split(r"^---\s*$", text, flags=re.MULTILINE)
        if re.search(r"^kind:\s*ExternalSecret\s*$", document, re.MULTILINE)
        and metadata_name(document) == name
    ]
    if len(matches) != 1:
        fail(f"expected exactly one ExternalSecret {name}, found {len(matches)}")
    return matches[0]


def external_secret_ref(document: str, secret_key: str) -> tuple[str, str]:
    entries = list_entry_blocks(document, "secretKey", secret_key)
    if len(entries) != 1:
        fail(
            f"expected exactly one data entry for {secret_key}, found {len(entries)}"
        )
    remote_key, property_name = nested_mapping(
        entries[0],
        "remoteRef",
        ("key", "property"),
        f"ExternalSecret data entry {secret_key}",
    )
    return remote_key, property_name


def deployment_secret_ref(text: str, env_name: str) -> tuple[str, str]:
    entries = list_entry_blocks(text, "name", env_name)
    if len(entries) != 1:
        fail(
            f"expected exactly one {env_name} environment entry, found {len(entries)}"
        )
    secret_name, secret_key = nested_mapping(
        entries[0],
        "secretKeyRef",
        ("name", "key"),
        f"deployment environment entry {env_name}",
    )
    return secret_name, secret_key


def main() -> None:
    secrets_text = SECRETS.read_text(encoding="utf-8")
    customer_secret = external_secret_document(secrets_text, "fiducia-backend-secrets")
    admin_secret = external_secret_document(secrets_text, "fiducia-admin-secrets")

    customer_url_remote = external_secret_ref(customer_secret, "supabase-url")
    customer_key_remote = external_secret_ref(
        customer_secret, "supabase-publishable-key"
    )
    admin_url_remote = external_secret_ref(admin_secret, "supabase-url")
    admin_key_remote = external_secret_ref(admin_secret, "supabase-publishable-key")

    require(
        customer_url_remote
        == (
            "dd/remote-dev/fiducia-backend-secrets",
            "FIDUCIA_CUSTOMER_SUPABASE_URL",
        ),
        f"customer Supabase URL has unexpected remote source {customer_url_remote}",
    )
    require(
        customer_key_remote
        == (
            "dd/remote-dev/fiducia-backend-secrets",
            "FIDUCIA_CUSTOMER_SUPABASE_PUBLISHABLE_KEY",
        ),
        f"customer Supabase key has unexpected remote source {customer_key_remote}",
    )
    require(
        admin_url_remote
        == (
            "dd/remote-dev/fiducia-admin-secrets",
            "FIDUCIA_ADMIN_SUPABASE_URL",
        ),
        f"admin Supabase URL has unexpected remote source {admin_url_remote}",
    )
    require(
        admin_key_remote
        == (
            "dd/remote-dev/fiducia-admin-secrets",
            "FIDUCIA_ADMIN_SUPABASE_PUBLISHABLE_KEY",
        ),
        f"admin Supabase key has unexpected remote source {admin_key_remote}",
    )
    require(
        customer_url_remote[0] != admin_url_remote[0],
        "customer and admin Supabase URLs must use different remote secret objects",
    )
    require(
        customer_key_remote[0] != admin_key_remote[0],
        "customer and admin Supabase keys must use different remote secret objects",
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
    require(
        customer_key_ref[0] != admin_key_ref[0],
        "customer and admin publishable keys must never consume the same Kubernetes Secret",
    )

    print("fiducia auth boundary check passed")


if __name__ == "__main__":
    main()
