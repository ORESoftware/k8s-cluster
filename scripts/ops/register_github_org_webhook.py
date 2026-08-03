#!/usr/bin/env python3
"""Idempotently create or update the ORESoftware workflow-run webhook.

Secrets are accepted only through environment variables. The script never prints
request bodies, response bodies, the bearer token, or the webhook secret.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any, Iterable

API_ROOT = "https://api.github.com"
API_VERSION = "2026-03-10"
USER_AGENT = "oresoftware-k8s-cluster-webhook-registrar/1"
ORG_PATTERN = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$")


class RegistrationError(RuntimeError):
    """Bounded registration failure safe to show in CI logs."""


@dataclass(frozen=True)
class Settings:
    org: str
    webhook_url: str
    webhook_secret: str
    token: str


def required_env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise RegistrationError(f"{name} is required")
    return value


def load_settings() -> Settings:
    token = (os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN") or "").strip()
    if not token:
        raise RegistrationError("GH_TOKEN or GITHUB_TOKEN is required")
    org = os.environ.get("GITHUB_ORG", "ORESoftware").strip()
    if not ORG_PATTERN.fullmatch(org):
        raise RegistrationError("GITHUB_ORG is invalid")
    webhook_url = required_env("GITHUB_WEBHOOK_URL")
    parsed = urllib.parse.urlparse(webhook_url)
    if parsed.scheme != "https" or not parsed.netloc or parsed.username or parsed.password:
        raise RegistrationError("GITHUB_WEBHOOK_URL must be an HTTPS URL without userinfo")
    if parsed.fragment:
        raise RegistrationError("GITHUB_WEBHOOK_URL must not contain a fragment")
    webhook_secret = required_env("GITHUB_WEBHOOK_SECRET")
    if len(webhook_secret.encode("utf-8")) < 32:
        raise RegistrationError("GITHUB_WEBHOOK_SECRET must be at least 32 bytes")
    return Settings(org=org, webhook_url=webhook_url, webhook_secret=webhook_secret, token=token)


def desired_hook_payload(webhook_url: str, webhook_secret: str) -> dict[str, Any]:
    return {
        "name": "web",
        "active": True,
        "events": ["workflow_run"],
        "config": {
            "url": webhook_url,
            "content_type": "json",
            "insecure_ssl": "0",
            "secret": webhook_secret,
        },
    }


def find_hook(hooks: Iterable[dict[str, Any]], webhook_url: str) -> dict[str, Any] | None:
    for hook in hooks:
        config = hook.get("config")
        if isinstance(config, dict) and config.get("url") == webhook_url:
            return hook
    return None


def api_request(
    settings: Settings,
    method: str,
    path: str,
    payload: dict[str, Any] | None = None,
) -> tuple[Any, dict[str, str]]:
    url = f"{API_ROOT}{path}"
    body = None if payload is None else json.dumps(payload, separators=(",", ":")).encode("utf-8")
    request = urllib.request.Request(url=url, data=body, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("Authorization", f"Bearer {settings.token}")
    request.add_header("X-GitHub-Api-Version", API_VERSION)
    request.add_header("User-Agent", USER_AGENT)
    if body is not None:
        request.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            raw = response.read()
            parsed: Any = None if not raw else json.loads(raw.decode("utf-8"))
            return parsed, {key.lower(): value for key, value in response.headers.items()}
    except urllib.error.HTTPError as error:
        raise RegistrationError(f"GitHub API {method} {path} returned HTTP {error.code}") from None
    except urllib.error.URLError as error:
        reason = getattr(error, "reason", "network error")
        raise RegistrationError(f"GitHub API {method} {path} failed: {type(reason).__name__}") from None
    except (TimeoutError, json.JSONDecodeError):
        raise RegistrationError(f"GitHub API {method} {path} returned an unusable response") from None


def list_hooks(settings: Settings) -> list[dict[str, Any]]:
    hooks: list[dict[str, Any]] = []
    for page in range(1, 101):
        value, _headers = api_request(
            settings,
            "GET",
            f"/orgs/{settings.org}/hooks?per_page=100&page={page}",
        )
        if not isinstance(value, list):
            raise RegistrationError("GitHub organization hooks response was not a list")
        page_hooks = [item for item in value if isinstance(item, dict)]
        hooks.extend(page_hooks)
        if len(page_hooks) < 100:
            return hooks
    raise RegistrationError("GitHub organization hooks pagination exceeded 100 pages")


def register(settings: Settings, *, dry_run: bool) -> str:
    existing = find_hook(list_hooks(settings), settings.webhook_url)
    payload = desired_hook_payload(settings.webhook_url, settings.webhook_secret)
    if existing is None:
        if dry_run:
            return "would-create"
        result, _headers = api_request(settings, "POST", f"/orgs/{settings.org}/hooks", payload)
        if not isinstance(result, dict) or not isinstance(result.get("id"), int):
            raise RegistrationError("GitHub did not return the created hook identifier")
        return f"created:{result['id']}"

    hook_id = existing.get("id")
    if not isinstance(hook_id, int):
        raise RegistrationError("matching GitHub hook did not contain a numeric id")
    if dry_run:
        return f"would-update:{hook_id}"
    result, _headers = api_request(
        settings,
        "PATCH",
        f"/orgs/{settings.org}/hooks/{hook_id}",
        payload,
    )
    if not isinstance(result, dict) or result.get("id") != hook_id:
        raise RegistrationError("GitHub did not confirm the updated hook identifier")
    return f"updated:{hook_id}"


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="read existing hooks and report create/update without writing",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        outcome = register(load_settings(), dry_run=args.dry_run)
    except RegistrationError as error:
        print(f"webhook registration failed: {error}", file=sys.stderr)
        return 2
    print(f"organization webhook {outcome}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
