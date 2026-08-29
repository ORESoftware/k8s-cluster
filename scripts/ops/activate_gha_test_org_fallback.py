#!/usr/bin/env python3
"""Activate and prove the exact gha-indie-worker-test fallback lane.

This program is intended to run on the protected Kubernetes node through AWS
SSM. It keeps Kubernetes and GitHub credentials in process memory, verifies the
live fixed-profile boundary before changing hooks, upserts exactly one
workflow_run hook per reviewed repository, requires a real GitHub ping delivery,
and then runs one synthetic signed exact-SHA terminal canary per repository.

The synthetic canaries prove application execution, not that GitHub reported an
Actions billing outage. Hook ping receipts and synthetic execution evidence are
reported separately for that reason.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import ipaddress
import json
import os
import re
import ssl
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass
from typing import Any
from urllib.parse import quote, urlsplit


API_ROOT = "https://api.github.com"
API_VERSION = "2022-11-28"
MAX_RESPONSE_BYTES = 1024 * 1024
EXPECTED_CLONE_IMAGE = (
    "ghcr.io/oresoftware/gha-clone-server@"
    "sha256:719a50b3d8cf105cd8c78bb66ce9d10dca072e4de28f6f7ba4fa79db446a2be8"
)
EXPECTED_ROUTER_IMAGE = (
    "ghcr.io/oresoftware/gha-executor-router@"
    "sha256:e87bee0e28911fbdc096d2fec0c1a65811b7d2173594d81c377dc437ac658e8f"
)
EXPECTED_GATEWAY_REVISION = "2026-08-19-gha-webhook-no-retry"
NAMESPACE_RE = re.compile(r"^[a-z0-9](?:[-a-z0-9]*[a-z0-9])?$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
EVIDENCE_REPOSITORY = "ORESoftware/k8s-cluster"
EVIDENCE_ISSUE = 1093


@dataclass(frozen=True)
class Pilot:
    repository: str
    revision: str
    workflow_path: str
    workflow_name: str


PILOTS = (
    Pilot(
        repository="gha-indie-worker-test/gha-indie-worker.rs",
        revision="129723d26294933b7b4ccff2d30323acd2235679",
        workflow_path=".github/workflows/gha-indie-worker-custom.yml",
        workflow_name="GHA indie worker independent fixed-profile declaration",
    ),
    Pilot(
        repository="gha-indie-worker-test/gha-clone-server.rs",
        revision="7fb5aed82cea31771e26d3bd908456017a286533",
        workflow_path=".github/workflows/gha-clone-server-meta.yml",
        workflow_name="GHA continuity server meta self-test",
    ),
)


class ActivationError(RuntimeError):
    """A redacted fail-closed activation error."""


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(  # type: ignore[override]
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> None:
        return None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--callback-url", required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--namespace", default="default")
    parser.add_argument("--poll-seconds", type=float, default=3.0)
    parser.add_argument("--timeout-seconds", type=float, default=1800.0)
    parser.add_argument("--reconcile-timeout-seconds", type=float, default=900.0)
    args = parser.parse_args()
    if not NAMESPACE_RE.fullmatch(args.namespace):
        parser.error("--namespace must be a valid lowercase Kubernetes namespace")
    if not 0.5 <= args.poll_seconds <= 30:
        parser.error("--poll-seconds must be between 0.5 and 30")
    if not 300 <= args.timeout_seconds <= 7200:
        parser.error("--timeout-seconds must be between 300 and 7200")
    if not 0 <= args.reconcile_timeout_seconds <= 1800:
        parser.error("--reconcile-timeout-seconds must be between 0 and 1800")
    if not SHA_RE.fullmatch(args.source_revision):
        parser.error("--source-revision must be immutable lowercase 40-hex")
    validate_callback_url(args.callback_url)
    return args


def validate_callback_url(raw: str) -> None:
    parsed = urlsplit(raw)
    if (
        parsed.scheme != "https"
        or parsed.username is not None
        or parsed.password is not None
        or parsed.port is not None
        or parsed.query
        or parsed.fragment
        or parsed.path != "/gha-webhooks/github"
        or not parsed.hostname
    ):
        raise ActivationError(
            "callback URL must be an exact credential-free HTTPS /gha-webhooks/github URL"
        )
    try:
        address = ipaddress.ip_address(parsed.hostname)
    except ValueError as exc:
        raise ActivationError("callback URL must use the currently resolved public node IP") from exc
    if address.version != 4 or not address.is_global:
        raise ActivationError("callback URL must use a global public IPv4 address")


def run_json(command: list[str], *, label: str) -> Any:
    completed = subprocess.run(
        command,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        timeout=60,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip().splitlines()
        suffix = detail[-1][:300] if detail else f"exit {completed.returncode}"
        raise ActivationError(f"{label} failed: {suffix}")
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise ActivationError(f"{label} returned invalid JSON") from exc


def kubectl_json(namespace: str, kind: str, name: str) -> dict[str, Any]:
    value = run_json(
        ["kubectl", "-n", namespace, "get", kind, name, "-o", "json"],
        label=f"kubectl get {kind}/{name}",
    )
    if not isinstance(value, dict):
        raise ActivationError(f"kubectl get {kind}/{name} returned a non-object")
    return value


def decode_secret(secret: dict[str, Any], key: str, *, name: str) -> bytes:
    encoded = secret.get("data", {}).get(key)
    if not isinstance(encoded, str) or not encoded:
        raise ActivationError(f"Secret/{name} is missing non-empty key {key}")
    try:
        value = base64.b64decode(encoded, validate=True)
    except (ValueError, TypeError) as exc:
        raise ActivationError(f"Secret/{name} key {key} is not valid base64") from exc
    if not value:
        raise ActivationError(f"Secret/{name} key {key} decoded empty")
    return value


def visible_secret(value: bytes, *, label: str, minimum: int = 32) -> bytes:
    if not minimum <= len(value) <= 4096:
        raise ActivationError(f"{label} length is outside the reviewed boundary")
    if any(byte < 0x21 or byte > 0x7E for byte in value):
        raise ActivationError(f"{label} must be a single visible-ASCII value")
    return value


def resolve_admin_token(agent_secret: dict[str, Any]) -> str:
    try:
        value = decode_secret(agent_secret, "GH_PAT", name="dd-agent-secrets")
    except ActivationError:
        value = b""
    if not value:
        completed = subprocess.run(
            [
                "aws",
                "secretsmanager",
                "get-secret-value",
                "--region",
                "us-east-1",
                "--secret-id",
                "dd/remote-dev/agent-secrets",
                "--query",
                "SecretString",
                "--output",
                "text",
            ],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            timeout=60,
        )
        try:
            decoded = json.loads(completed.stdout)
            candidate = decoded.get("GH_PAT") if isinstance(decoded, dict) else None
        except json.JSONDecodeError:
            candidate = None
        if isinstance(candidate, str):
            try:
                value = candidate.encode("ascii", errors="strict")
            except UnicodeEncodeError:
                value = b""
    return visible_secret(
        value,
        label="protected GitHub operator token",
        minimum=20,
    ).decode("ascii")


def external_secret_ready(value: dict[str, Any], *, name: str) -> None:
    conditions = value.get("status", {}).get("conditions", [])
    if not any(
        isinstance(condition, dict)
        and condition.get("type") == "Ready"
        and condition.get("status") == "True"
        for condition in conditions
    ):
        raise ActivationError(f"ExternalSecret/{name} is not Ready=True")


def named_container(workload: dict[str, Any], name: str) -> dict[str, Any]:
    containers = (
        workload.get("spec", {})
        .get("template", {})
        .get("spec", {})
        .get("containers", [])
    )
    matches = [item for item in containers if isinstance(item, dict) and item.get("name") == name]
    if len(matches) != 1:
        raise ActivationError(f"workload must contain exactly one container named {name}")
    return matches[0]


def env_literals(container: dict[str, Any]) -> dict[str, str]:
    result: dict[str, str] = {}
    for item in container.get("env", []):
        if isinstance(item, dict) and isinstance(item.get("name"), str):
            if isinstance(item.get("value"), str):
                result[item["name"]] = item["value"]
    return result


def require_deployment(
    value: dict[str, Any],
    *,
    name: str,
    container_name: str,
    image: str | None = None,
    hardened_container: bool = True,
) -> dict[str, Any]:
    if value.get("spec", {}).get("replicas") != 1:
        raise ActivationError(f"Deployment/{name} must have exactly one desired replica")
    if int(value.get("status", {}).get("availableReplicas") or 0) < 1:
        raise ActivationError(f"Deployment/{name} has no available replica")
    pod_spec = value.get("spec", {}).get("template", {}).get("spec", {})
    if pod_spec.get("automountServiceAccountToken") is not False:
        raise ActivationError(f"Deployment/{name} must disable service-account token mounting")
    container = named_container(value, container_name)
    if image is not None and container.get("image") != image:
        raise ActivationError(f"Deployment/{name} is not running the reviewed image digest")
    if hardened_container:
        security = container.get("securityContext", {})
        if security.get("readOnlyRootFilesystem") is not True:
            raise ActivationError(f"Deployment/{name} must use a read-only root filesystem")
        if security.get("allowPrivilegeEscalation") is not False:
            raise ActivationError(f"Deployment/{name} must disable privilege escalation")
    return container


def service_origin(namespace: str, name: str, port: int) -> str:
    service = kubectl_json(namespace, "service", name)
    cluster_ip = service.get("spec", {}).get("clusterIP")
    if not isinstance(cluster_ip, str):
        raise ActivationError(f"Service/{name} has no ClusterIP")
    try:
        address = ipaddress.ip_address(cluster_ip)
    except ValueError as exc:
        raise ActivationError(f"Service/{name} has an invalid ClusterIP") from exc
    if address.version != 4 or not address.is_private:
        raise ActivationError(f"Service/{name} ClusterIP is outside the private IPv4 boundary")
    ports = service.get("spec", {}).get("ports", [])
    if not any(isinstance(item, dict) and item.get("port") == port for item in ports):
        raise ActivationError(f"Service/{name} does not expose reviewed port {port}")
    return f"http://{cluster_ip}:{port}"


def read_http_response(response: Any) -> bytes:
    data = response.read(MAX_RESPONSE_BYTES + 1)
    if len(data) > MAX_RESPONSE_BYTES:
        raise ActivationError("HTTP response exceeded the 1 MiB boundary")
    return data


def http_json(
    method: str,
    url: str,
    *,
    body: bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = 20,
) -> tuple[int, Any]:
    request = urllib.request.Request(url, data=body, method=method)
    request.add_header("Accept", "application/json")
    for key, value in (headers or {}).items():
        request.add_header(key, value)
    opener = urllib.request.build_opener(
        NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context()),
    )
    try:
        with opener.open(request, timeout=timeout) as response:
            status = response.status
            raw = read_http_response(response)
    except urllib.error.HTTPError as exc:
        status = exc.code
        raw = read_http_response(exc)
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        reason = getattr(exc, "reason", exc)
        raise ActivationError(f"request to {url} failed: {reason}") from exc
    if 300 <= status < 400:
        raise ActivationError(f"request to {url} returned forbidden redirect HTTP {status}")
    if not raw:
        return status, None
    try:
        return status, json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ActivationError(f"HTTP {status} from {url} returned invalid JSON") from exc


def github_request(
    token: str,
    method: str,
    path: str,
    *,
    payload: dict[str, Any] | None = None,
    expected: tuple[int, ...] = (200,),
) -> Any:
    body = None
    if payload is not None:
        body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    status, response = http_json(
        method,
        API_ROOT + path,
        body=body,
        headers={
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": API_VERSION,
            "User-Agent": "gha-test-fallback-activator/1",
            "Content-Type": "application/json",
        },
        timeout=30,
    )
    if status not in expected:
        message = response.get("message") if isinstance(response, dict) else None
        if not isinstance(message, str):
            message = "no GitHub error message"
        raise ActivationError(f"GitHub {method} {path} returned HTTP {status}: {message[:200]}")
    return response


def validate_live_cluster(
    namespace: str,
) -> tuple[dict[str, Any], bytes, str, str, str]:
    for name in ("dd-gha-clone-server-secrets", "dd-gha-executor-router-secrets"):
        external_secret_ready(
            kubectl_json(namespace, "externalsecret", name),
            name=name,
        )

    clone_secret = kubectl_json(namespace, "secret", "dd-gha-clone-server-secrets")
    router_secret = kubectl_json(namespace, "secret", "dd-gha-executor-router-secrets")
    agent_secret = kubectl_json(namespace, "secret", "dd-agent-secrets")
    webhook_secret = visible_secret(
        decode_secret(
            clone_secret,
            "github_webhook_secret",
            name="dd-gha-clone-server-secrets",
        ),
        label="GitHub webhook HMAC",
    )
    clone_auth = visible_secret(
        decode_secret(clone_secret, "auth_secret", name="dd-gha-clone-server-secrets"),
        label="clone API authority",
    ).decode("ascii")
    runtime_token = visible_secret(
        decode_secret(clone_secret, "github_token", name="dd-gha-clone-server-secrets"),
        label="runtime GitHub token",
        minimum=20,
    ).decode("ascii")
    decode_secret(router_secret, "inbound_auth", name="dd-gha-executor-router-secrets")
    decode_secret(agent_secret, "SERVER_AUTH_SECRET", name="dd-agent-secrets")
    admin_token = resolve_admin_token(agent_secret)

    clone = kubectl_json(namespace, "deployment", "dd-gha-clone-server")
    clone_container = require_deployment(
        clone,
        name="dd-gha-clone-server",
        container_name="gha-clone-server",
        image=EXPECTED_CLONE_IMAGE,
    )
    clone_env = env_literals(clone_container)
    for variable in ("GHA_CLONE_EXECUTION_ENABLED", "GHA_CLONE_WEBHOOK_EXECUTION_ENABLED"):
        if clone_env.get(variable) != "true":
            raise ActivationError(f"Deployment/dd-gha-clone-server requires {variable}=true")
    if clone_env.get("GHA_CLONE_WEBHOOK_FAILURE_CONCLUSIONS") != "action_required":
        raise ActivationError("clone server failure policy must be exactly action_required")

    router = kubectl_json(namespace, "deployment", "dd-gha-executor-router")
    router_container = require_deployment(
        router,
        name="dd-gha-executor-router",
        container_name="gha-executor-router",
        image=EXPECTED_ROUTER_IMAGE,
    )
    if env_literals(router_container).get("GHA_EXECUTOR_ROUTER_EXECUTION_ENABLED") != "true":
        raise ActivationError("executor router must have execution enabled")

    profile_runner = kubectl_json(namespace, "deployment", "dd-ci-profile-runner")
    profile_runner_container = require_deployment(
        profile_runner,
        name="dd-ci-profile-runner",
        container_name="ci-profile-runner",
        hardened_container=False,
    )
    try:
        profile_runner_rules = json.loads(
            env_literals(profile_runner_container).get("CI_PROFILE_RUNNER_RULES_JSON", "")
        )
    except json.JSONDecodeError as exc:
        raise ActivationError("live CI profile-runner rules are invalid JSON") from exc
    if not isinstance(profile_runner_rules, dict):
        raise ActivationError("live CI profile-runner rules must be an object")
    for pilot in PILOTS:
        if profile_runner_rules.get(pilot.repository) != "rust-verify":
            raise ActivationError(f"live CI profile-runner rule drifted for {pilot.repository}")
    if any(
        key == "gha-indie-worker-test" or key.startswith("gha-indie-worker-test/*")
        for key in profile_runner_rules
    ):
        raise ActivationError("CI profile runner must not admit the test organization by wildcard")

    gateway = kubectl_json(namespace, "daemonset", "dd-remote-gateway")
    desired = int(gateway.get("status", {}).get("desiredNumberScheduled") or 0)
    available = int(gateway.get("status", {}).get("numberAvailable") or 0)
    if desired < 1 or available != desired:
        raise ActivationError("DaemonSet/dd-remote-gateway is not fully available")
    annotations = gateway.get("spec", {}).get("template", {}).get("metadata", {}).get("annotations", {})
    if annotations.get("dd.dev/gateway-config-revision") != EXPECTED_GATEWAY_REVISION:
        raise ActivationError("DaemonSet/dd-remote-gateway is not on the no-retry webhook revision")

    clone_config = kubectl_json(namespace, "configmap", "dd-gha-clone-server")
    data = clone_config.get("data", {})
    allowed = {
        item.strip()
        for item in str(data.get("GHA_CLONE_ALLOWED_REPOSITORIES", "")).split(",")
        if item.strip()
    }
    try:
        workflow_rules = json.loads(data.get("GHA_CLONE_WORKFLOW_RULES_JSON", ""))
    except (json.JSONDecodeError, TypeError) as exc:
        raise ActivationError("live clone workflow rules are invalid JSON") from exc
    for pilot in PILOTS:
        if pilot.repository not in allowed:
            raise ActivationError(f"live clone allowlist is missing {pilot.repository}")
        if workflow_rules.get(pilot.repository) != [pilot.workflow_path]:
            raise ActivationError(f"live clone workflow rule drifted for {pilot.repository}")

    build = kubectl_json(namespace, "deployment", "dd-build-server")
    build_container = named_container(build, "build-server")
    build_env = env_literals(build_container)
    try:
        repository_rules = json.loads(build_env.get("BUILD_SERVER_PROFILE_REPOSITORY_RULES_JSON", ""))
    except json.JSONDecodeError as exc:
        raise ActivationError("live build-server repository rules are invalid JSON") from exc
    if not isinstance(repository_rules, list):
        raise ActivationError("live build-server repository rules must be an array")
    indexed = {
        item.get("repository"): item.get("profiles")
        for item in repository_rules
        if isinstance(item, dict)
    }
    for pilot in PILOTS:
        repository_url = f"https://github.com/{pilot.repository}.git"
        if indexed.get(repository_url) != ["rust-verify"]:
            raise ActivationError(f"live fixed-profile binding drifted for {pilot.repository}")
    prefixes = build_env.get("BUILD_SERVER_ALLOWED_PROFILE_REPO_PREFIXES", "")
    if "gha-indie-worker-test" in prefixes:
        raise ActivationError("test organization must not be admitted by repository prefix")

    clone_origin = service_origin(namespace, "dd-gha-clone-server", 8125)
    service_origin(namespace, "dd-gha-executor-router", 8126)
    for endpoint in ("healthz", "readyz"):
        status, response = http_json("GET", f"{clone_origin}/{endpoint}")
        if status != 200 or not isinstance(response, dict) or response.get("ok") is not True:
            raise ActivationError(f"clone server {endpoint} is not ready")

    argo: dict[str, Any] = {"available": False}
    try:
        application = kubectl_json("argocd", "application", "dd-next-runtime")
    except ActivationError:
        pass
    else:
        argo = {
            "available": True,
            "sync": application.get("status", {}).get("sync", {}).get("status"),
            "health": application.get("status", {}).get("health", {}).get("status"),
            "revision": application.get("status", {}).get("sync", {}).get("revision"),
        }

    summary = {
        "cloneImage": EXPECTED_CLONE_IMAGE,
        "routerImage": EXPECTED_ROUTER_IMAGE,
        "cloneAvailable": int(clone.get("status", {}).get("availableReplicas") or 0),
        "routerAvailable": int(router.get("status", {}).get("availableReplicas") or 0),
        "profileRunnerAvailable": int(profile_runner.get("status", {}).get("availableReplicas") or 0),
        "gatewayAvailable": available,
        "argo": argo,
    }
    return summary, webhook_secret, clone_auth, admin_token, runtime_token


def wait_for_live_cluster(
    namespace: str,
    *,
    poll_seconds: float,
    timeout_seconds: float,
) -> tuple[dict[str, Any], bytes, str, str, str]:
    deadline = time.monotonic() + timeout_seconds
    last_error: ActivationError | None = None
    while True:
        try:
            return validate_live_cluster(namespace)
        except ActivationError as exc:
            last_error = exc
        if time.monotonic() >= deadline:
            assert last_error is not None
            raise ActivationError(
                f"live GitOps state did not reconcile before activation: {last_error}"
            ) from last_error
        time.sleep(max(poll_seconds, 5.0))


def validate_github_authority(admin_token: str, runtime_token: str) -> None:
    identity = github_request(admin_token, "GET", "/user")
    if not isinstance(identity, dict) or identity.get("login") != "ORESoftware":
        observed = identity.get("login") if isinstance(identity, dict) else None
        raise ActivationError(f"protected GitHub identity is not ORESoftware: {observed!r}")
    membership = github_request(
        admin_token,
        "GET",
        "/user/memberships/orgs/gha-indie-worker-test",
    )
    if (
        not isinstance(membership, dict)
        or membership.get("state") != "active"
        or membership.get("role") != "admin"
    ):
        raise ActivationError("protected GitHub identity is not an active test-org admin")

    for pilot in PILOTS:
        ref = github_request(
            runtime_token,
            "GET",
            f"/repos/{pilot.repository}/git/ref/heads/dev",
        )
        observed = ref.get("object", {}).get("sha") if isinstance(ref, dict) else None
        if observed != pilot.revision:
            raise ActivationError(
                f"reviewed dev head drifted for {pilot.repository}: {observed!r}"
            )
        encoded_path = quote(pilot.workflow_path, safe="/")
        github_request(
            runtime_token,
            "GET",
            f"/repos/{pilot.repository}/contents/{encoded_path}?ref={pilot.revision}",
        )


def hook_contract(value: dict[str, Any], callback_url: str) -> bool:
    config = value.get("config", {})
    return (
        value.get("active") is True
        and value.get("events") == ["workflow_run"]
        and config.get("url") == callback_url
        and config.get("content_type") == "json"
        and str(config.get("insecure_ssl")) == "0"
    )


def upsert_hook(
    admin_token: str,
    pilot: Pilot,
    callback_url: str,
    webhook_secret: bytes,
    *,
    poll_seconds: float,
) -> dict[str, Any]:
    hooks = github_request(admin_token, "GET", f"/repos/{pilot.repository}/hooks?per_page=100")
    if not isinstance(hooks, list):
        raise ActivationError(f"GitHub hook inventory is not an array for {pilot.repository}")
    workflow_hooks = [
        item
        for item in hooks
        if isinstance(item, dict)
        and item.get("active") is True
        and isinstance(item.get("events"), list)
        and "workflow_run" in item["events"]
    ]
    exact = [item for item in hooks if isinstance(item, dict) and item.get("config", {}).get("url") == callback_url]
    foreign = [item for item in workflow_hooks if item not in exact]
    if foreign:
        raise ActivationError(f"{pilot.repository} already has a different active workflow_run hook")
    if len(exact) > 1:
        raise ActivationError(f"{pilot.repository} has ambiguous duplicate callback hooks")

    payload = {
        "name": "web",
        "active": True,
        "events": ["workflow_run"],
        "config": {
            "url": callback_url,
            "content_type": "json",
            "secret": webhook_secret.decode("ascii"),
            "insecure_ssl": "0",
        },
    }
    if exact:
        hook_id = exact[0].get("id")
        if not isinstance(hook_id, int):
            raise ActivationError(f"{pilot.repository} returned a non-numeric hook id")
        result = github_request(
            admin_token,
            "PATCH",
            f"/repos/{pilot.repository}/hooks/{hook_id}",
            payload=payload,
        )
        action = "updated"
    else:
        result = github_request(
            admin_token,
            "POST",
            f"/repos/{pilot.repository}/hooks",
            payload=payload,
            expected=(201,),
        )
        action = "created"
        hook_id = result.get("id") if isinstance(result, dict) else None
    if not isinstance(hook_id, int):
        raise ActivationError(f"GitHub returned an invalid hook id for {pilot.repository}")
    try:
        if not isinstance(result, dict) or not hook_contract(result, callback_url):
            raise ActivationError(f"GitHub returned a drifted hook contract for {pilot.repository}")

        reconciled = github_request(
            admin_token,
            "GET",
            f"/repos/{pilot.repository}/hooks?per_page=100",
        )
        if not isinstance(reconciled, list):
            raise ActivationError(f"GitHub hook inventory is not an array for {pilot.repository}")
        active_workflow_hooks = [
            item
            for item in reconciled
            if isinstance(item, dict)
            and item.get("active") is True
            and isinstance(item.get("events"), list)
            and "workflow_run" in item["events"]
        ]
        if len(active_workflow_hooks) != 1 or not hook_contract(
            active_workflow_hooks[0], callback_url
        ):
            raise ActivationError(
                f"{pilot.repository} does not have exactly one active reviewed hook"
            )

        deliveries_path = f"/repos/{pilot.repository}/hooks/{hook_id}/deliveries?per_page=20"
        before = github_request(admin_token, "GET", deliveries_path)
        if not isinstance(before, list):
            raise ActivationError(f"GitHub delivery inventory is not an array for {pilot.repository}")
        before_ids = {item.get("id") for item in before if isinstance(item, dict)}
        github_request(
            admin_token,
            "POST",
            f"/repos/{pilot.repository}/hooks/{hook_id}/pings",
            expected=(204,),
        )
        deadline = time.monotonic() + 120
        delivery: dict[str, Any] | None = None
        while time.monotonic() < deadline:
            observed = github_request(admin_token, "GET", deliveries_path)
            if not isinstance(observed, list):
                raise ActivationError(
                    f"GitHub delivery inventory is not an array for {pilot.repository}"
                )
            candidates = [
                item
                for item in observed
                if isinstance(item, dict)
                and item.get("id") not in before_ids
                and item.get("event") == "ping"
            ]
            if candidates:
                delivery = candidates[0]
                break
            time.sleep(poll_seconds)
        if delivery is None:
            raise ActivationError(f"GitHub did not record a fresh ping for {pilot.repository}")
        if delivery.get("status_code") != 202:
            raise ActivationError(
                f"GitHub ping for {pilot.repository} returned "
                f"{delivery.get('status_code')!r}, expected 202"
            )
        return {
            "repository": pilot.repository,
            "hookId": hook_id,
            "action": action,
            "pingDeliveryId": delivery.get("id"),
            "pingGuid": delivery.get("guid"),
            "pingStatus": delivery.get("status_code"),
        }
    except Exception:
        try:
            github_request(
                admin_token,
                "PATCH",
                f"/repos/{pilot.repository}/hooks/{hook_id}",
                payload={"active": False},
            )
        except ActivationError:
            print(
                f"activation rollback could not deactivate {pilot.repository}",
                file=sys.stderr,
            )
        raise


def deactivate_hooks(admin_token: str, hooks: list[dict[str, Any]]) -> None:
    failures: list[str] = []
    for hook in hooks:
        repository = hook.get("repository")
        hook_id = hook.get("hookId")
        if not isinstance(repository, str) or not isinstance(hook_id, int):
            continue
        try:
            github_request(
                admin_token,
                "PATCH",
                f"/repos/{repository}/hooks/{hook_id}",
                payload={"active": False},
            )
        except ActivationError:
            failures.append(repository)
    if failures:
        print(
            "activation rollback could not deactivate: " + ",".join(sorted(failures)),
            file=sys.stderr,
        )
    elif hooks:
        print(f"activation rollback deactivated {len(hooks)} test hooks", file=sys.stderr)


def publish_activation_evidence(
    admin_token: str,
    source_revision: str,
    evidence: dict[str, Any],
) -> dict[str, Any]:
    marker = f"<!-- gha-test-fallback-activation:{source_revision} -->"
    issue_path = f"/repos/{EVIDENCE_REPOSITORY}/issues/{EVIDENCE_ISSUE}"
    issue = github_request(admin_token, "GET", issue_path)
    comment_count = issue.get("comments") if isinstance(issue, dict) else None
    if not isinstance(comment_count, int) or comment_count < 0:
        raise ActivationError("activation evidence issue returned an invalid comment count")
    last_page = max(1, (comment_count + 99) // 100)
    comments = github_request(
        admin_token,
        "GET",
        f"{issue_path}/comments?per_page=100&page={last_page}",
    )
    if not isinstance(comments, list):
        raise ActivationError("activation evidence issue comments are not an array")
    for comment in comments:
        body = comment.get("body") if isinstance(comment, dict) else None
        if isinstance(body, str) and marker in body:
            comment_id = comment.get("id")
            comment_url = comment.get("html_url")
            if not isinstance(comment_id, int) or not isinstance(comment_url, str):
                raise ActivationError("existing activation evidence receipt is malformed")
            return {"action": "existing", "id": comment_id, "url": comment_url}

    body = (
        marker
        + "\n### GHA test-org fallback activation receipt\n\n"
        + "The AWS-only in-cluster activator completed both exact-head synthetic canaries "
        + "after fresh GitHub-originated hook pings. Sanitized evidence:\n\n```json\n"
        + json.dumps(evidence, indent=2, sort_keys=True)
        + "\n```\n\n"
        + "Boundary: this proves hook reachability and synthetic fixed-profile execution. "
        + "It does not prove a GitHub-originated `workflow_run` execution delivery or "
        + "authoritative billing exhaustion."
    )
    created = github_request(
        admin_token,
        "POST",
        f"{issue_path}/comments",
        payload={"body": body},
        expected=(201,),
    )
    comment_id = created.get("id") if isinstance(created, dict) else None
    comment_url = created.get("html_url") if isinstance(created, dict) else None
    if not isinstance(comment_id, int) or not isinstance(comment_url, str):
        raise ActivationError("GitHub returned an invalid activation evidence receipt")
    return {"action": "created", "id": comment_id, "url": comment_url}


def post_webhook(
    callback_url: str,
    body: bytes,
    *,
    delivery: str,
    signature: str,
) -> tuple[int, Any]:
    return http_json(
        "POST",
        callback_url,
        body=body,
        headers={
            "Content-Type": "application/json",
            "X-GitHub-Event": "workflow_run",
            "X-GitHub-Delivery": delivery,
            "X-Hub-Signature-256": signature,
            "User-Agent": "gha-test-fallback-activator/1",
        },
        timeout=30,
    )


def run_canary(
    pilot: Pilot,
    callback_url: str,
    status_origin: str,
    webhook_secret: bytes,
    clone_auth: str,
    *,
    poll_seconds: float,
    timeout_seconds: float,
) -> dict[str, Any]:
    payload = {
        "action": "completed",
        "repository": {"full_name": pilot.repository},
        "workflow_run": {
            "name": pilot.workflow_name,
            "path": pilot.workflow_path,
            "head_sha": pilot.revision,
            "conclusion": "action_required",
        },
    }
    body = json.dumps(payload, separators=(",", ":"), sort_keys=True).encode("utf-8")
    invalid_status, _ = post_webhook(
        callback_url,
        body,
        delivery=str(uuid.uuid4()),
        signature="sha256=" + "0" * 64,
    )
    if invalid_status != 401:
        raise ActivationError(
            f"public edge did not reject an invalid HMAC for {pilot.repository}: HTTP {invalid_status}"
        )

    delivery = str(uuid.uuid4())
    signature = "sha256=" + hmac.new(webhook_secret, body, hashlib.sha256).hexdigest()
    status, accepted = post_webhook(
        callback_url,
        body,
        delivery=delivery,
        signature=signature,
    )
    if status != 202 or not isinstance(accepted, dict) or accepted.get("accepted") is not True:
        raise ActivationError(f"signed canary was not accepted for {pilot.repository}: HTTP {status}")
    if (
        accepted.get("delivery") != delivery
        or accepted.get("repository") != pilot.repository
        or accepted.get("revision") != pilot.revision
    ):
        raise ActivationError(f"signed canary lost immutable authority for {pilot.repository}")
    run_ids = accepted.get("runIds")
    if not isinstance(run_ids, list) or len(run_ids) != 1 or not isinstance(run_ids[0], str):
        raise ActivationError(f"signed canary did not create exactly one run for {pilot.repository}")

    replay_status, replay = post_webhook(
        callback_url,
        body,
        delivery=delivery,
        signature=signature,
    )
    if (
        replay_status != 202
        or not isinstance(replay, dict)
        or replay.get("accepted") is not False
        or "duplicate" not in str(replay.get("reason", "")).lower()
        or "runIds" in replay
    ):
        raise ActivationError(f"delivery replay was not suppressed for {pilot.repository}")

    pending = set(run_ids)
    terminal: dict[str, str] = {}
    deadline = time.monotonic() + timeout_seconds
    while pending and time.monotonic() < deadline:
        for run_id in list(pending):
            run_status, run = http_json(
                "GET",
                f"{status_origin}/v1/runs/{run_id}",
                headers={"X-Server-Auth": clone_auth},
                timeout=20,
            )
            if run_status != 200 or not isinstance(run, dict):
                raise ActivationError(f"run-state lookup failed for {pilot.repository}/{run_id}")
            if (
                run.get("repository") != pilot.repository
                or run.get("revision") != pilot.revision
                or run.get("workflowPath") != pilot.workflow_path
            ):
                raise ActivationError(f"run-state authority drifted for {pilot.repository}/{run_id}")
            state = run.get("status")
            if state in {"succeeded", "failed"}:
                terminal[run_id] = state
                pending.remove(run_id)
            elif state not in {"queued", "running"}:
                raise ActivationError(
                    f"unexpected run state {state!r} for {pilot.repository}/{run_id}"
                )
        if pending:
            time.sleep(poll_seconds)
    if pending:
        raise ActivationError(f"canary timed out for {pilot.repository}: {sorted(pending)}")
    if set(terminal.values()) != {"succeeded"}:
        raise ActivationError(f"canary execution failed for {pilot.repository}: {terminal}")
    return {
        "synthetic": True,
        "repository": pilot.repository,
        "revision": pilot.revision,
        "workflowPath": pilot.workflow_path,
        "delivery": delivery,
        "runIds": run_ids,
        "statuses": terminal,
        "invalidSignatureRejected": True,
        "duplicateDeliverySuppressed": True,
    }


def main() -> int:
    args = parse_args()
    cluster, webhook_secret, clone_auth, admin_token, runtime_token = wait_for_live_cluster(
        args.namespace,
        poll_seconds=args.poll_seconds,
        timeout_seconds=args.reconcile_timeout_seconds,
    )
    validate_github_authority(admin_token, runtime_token)
    status_origin = service_origin(args.namespace, "dd-gha-clone-server", 8125)

    hooks: list[dict[str, Any]] = []
    try:
        for pilot in PILOTS:
            hooks.append(
                upsert_hook(
                    admin_token,
                    pilot,
                    args.callback_url,
                    webhook_secret,
                    poll_seconds=args.poll_seconds,
                )
            )
        canaries = [
            run_canary(
                pilot,
                args.callback_url,
                status_origin,
                webhook_secret,
                clone_auth,
                poll_seconds=args.poll_seconds,
                timeout_seconds=args.timeout_seconds,
            )
            for pilot in PILOTS
        ]
        evidence = {
            "ok": True,
            "sourceRevision": args.source_revision,
            "callbackUrl": args.callback_url,
            "cluster": cluster,
            "hooks": hooks,
            "canaries": canaries,
            "githubWorkflowRunDeliveryProven": False,
            "githubPingDeliveryProven": True,
            "billingExhaustionProven": False,
        }
        evidence["githubIssueReceipt"] = publish_activation_evidence(
            admin_token,
            args.source_revision,
            evidence,
        )
    except Exception:
        deactivate_hooks(admin_token, hooks)
        raise
    print(json.dumps(evidence, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ActivationError as exc:
        print(f"gha-test-fallback activation failed: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
