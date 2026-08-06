#!/usr/bin/env python3
"""Create and verify the exact Meta Agents repository after owner device auth."""

from __future__ import annotations

import base64
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

API = "https://api.github.com"
OAUTH_CLIENT_ID = "178c6fc778ccc68e1d6a"
EXPECTED_LOGIN = "ORESoftware"
ORG = "meta-agents-demo"
NAME = "meta-agent-control-plane.rs"
TARGET = f"{ORG}/{NAME}"
DESCRIPTION = "Single-binary Rust control plane for observable, reflective AI agents."
EXPECTED_MAIN = "4d6ec3ad0ec7b688f0e777129eee7e0f0d999df1"
FEATURE_REF = "agent/den-1057-meta-agent-control-plane"
EXPECTED_FEATURE = "789d48039da232faed985d4f8de176959f117e08"
BUNDLE_SHA256 = "1ddaa03743b864348162149b7d2d2e2dce7eab585cf092ea14547c647fcec031"
MAX_BUNDLE_BYTES = 64 * 1024 * 1024
MAX_ENCODED_BUNDLE_BYTES = 90 * 1024 * 1024
MAX_API_BODY_BYTES = 1_048_576
MAX_ERROR_BODY_BYTES = 4_096
MAX_COMMAND_DIAGNOSTIC_CHARS = 3_000
ALLOWED_HTTPS_HOSTS = frozenset({"api.github.com", "github.com"})


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Turn every HTTP redirect into a bounded HTTPError."""

    def redirect_request(
        self,
        request: urllib.request.Request,
        file_pointer: Any,
        code: int,
        message: str,
        headers: Any,
        new_url: str,
    ) -> None:
        del request, file_pointer, code, message, headers, new_url
        return None


HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


def _validate_url(url: str) -> None:
    parsed = urllib.parse.urlsplit(url)
    if (
        parsed.scheme != "https"
        or parsed.hostname not in ALLOWED_HTTPS_HOSTS
        or parsed.username is not None
        or parsed.password is not None
        or parsed.fragment
    ):
        raise RuntimeError("network target is not an allowed credential-free GitHub HTTPS URL")


def _decode_json(raw: bytes, source: str) -> Any:
    try:
        return json.loads(raw.decode("utf-8")) if raw else None
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"{source} returned invalid JSON") from exc


def _read_bounded(response: Any, *, source: str) -> bytes:
    advertised = response.headers.get("Content-Length")
    if advertised is not None:
        try:
            if int(advertised) > MAX_API_BODY_BYTES:
                raise RuntimeError(f"{source} response exceeded {MAX_API_BODY_BYTES} bytes")
        except ValueError as exc:
            raise RuntimeError(f"{source} returned an invalid Content-Length") from exc
    raw = response.read(MAX_API_BODY_BYTES + 1)
    if len(raw) > MAX_API_BODY_BYTES:
        raise RuntimeError(f"{source} response exceeded {MAX_API_BODY_BYTES} bytes")
    return raw


def request_json(
    method: str,
    url: str,
    *,
    token: str | None = None,
    form: dict[str, str] | None = None,
    payload: dict[str, Any] | None = None,
    allowed_error_statuses: frozenset[int] = frozenset(),
) -> tuple[int, Any]:
    _validate_url(url)
    if form is not None and payload is not None:
        raise RuntimeError("a request cannot contain both form and JSON payloads")

    data = None
    headers = {
        "Accept": "application/json",
        "User-Agent": "meta-agent-selfhosted-publisher",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
        headers["X-GitHub-Api-Version"] = "2022-11-28"
    if form is not None:
        data = urllib.parse.urlencode(form).encode("utf-8")
        headers["Content-Type"] = "application/x-www-form-urlencoded"
    elif payload is not None:
        data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        headers["Content-Type"] = "application/json"

    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with HTTP.open(request, timeout=30) as response:
            raw = _read_bounded(response, source="GitHub")
            if raw and response.headers.get_content_type() != "application/json":
                raise RuntimeError("GitHub returned an unexpected response content type")
            return response.status, _decode_json(raw, "GitHub")
    except urllib.error.HTTPError as exc:
        # Drain only a bounded prefix, then discard it. API bodies can contain
        # tenant/repository details and do not belong in default Actions logs.
        exc.read(MAX_ERROR_BODY_BYTES)
        if exc.code in allowed_error_statuses:
            return exc.code, None
        request_id = exc.headers.get("x-github-request-id", "unknown")
        raise RuntimeError(
            f"GitHub request failed with HTTP {exc.code} (request {request_id})"
        ) from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(
            f"GitHub network request failed ({type(exc.reason).__name__})"
        ) from exc


def api_get(path: str, token: str) -> Any:
    status, payload = request_json("GET", API + path, token=token)
    if status != 200:
        raise RuntimeError(f"GitHub GET returned unexpected HTTP {status}")
    return payload


def add_comment(body: str, comment_token: str) -> None:
    repository = os.environ["GITHUB_REPOSITORY"]
    issue = os.environ["TRACKING_ISSUE"]
    status, _ = request_json(
        "POST",
        f"{API}/repos/{repository}/issues/{issue}/comments",
        token=comment_token,
        payload={"body": body},
    )
    if status != 201:
        raise RuntimeError(f"comment creation returned {status}")


def authorize(comment_token: str) -> str:
    status, device = request_json(
        "POST",
        "https://github.com/login/device/code",
        form={"client_id": OAUTH_CLIENT_ID, "scope": "repo read:org"},
    )
    if status != 200 or not isinstance(device, dict):
        raise RuntimeError("GitHub device-code request failed")

    device_code = device.get("device_code")
    user_code = device.get("user_code")
    verification_uri = device.get("verification_uri")
    expires_in = int(device.get("expires_in", 0))
    interval = int(device.get("interval", 5))
    values = (device_code, user_code, verification_uri)
    if not all(isinstance(value, str) and value for value in values):
        raise RuntimeError("GitHub device-code response is incomplete")
    if expires_in <= 0 or interval <= 0:
        raise RuntimeError("GitHub device-code timing is invalid")
    _validate_url(str(verification_uri))

    run_url = (
        f"https://github.com/{os.environ['GITHUB_REPOSITORY']}/actions/runs/"
        f"{os.environ['GITHUB_RUN_ID']}"
    )
    add_comment(
        "**Authorize exact Meta Agents repository creation now:** "
        f"open {verification_uri} and enter **`{user_code}`**. Ignore all older codes. "
        f"This self-hosted run accepts only GitHub account `{EXPECTED_LOGIN}` with active "
        f"admin access to `{ORG}`. Run: {run_url}",
        comment_token,
    )
    print(
        f"::notice title=GitHub owner authorization::Open {verification_uri} "
        f"and enter {user_code}",
        flush=True,
    )

    deadline = time.monotonic() + expires_in
    while time.monotonic() < deadline:
        time.sleep(interval)
        status, response = request_json(
            "POST",
            "https://github.com/login/oauth/access_token",
            form={
                "client_id": OAUTH_CLIENT_ID,
                "device_code": str(device_code),
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            },
        )
        if status != 200 or not isinstance(response, dict):
            raise RuntimeError("GitHub device-token request failed")
        error = response.get("error")
        if not error:
            token = response.get("access_token")
            if isinstance(token, str) and token:
                print(f"::add-mask::{token}", flush=True)
                return token
            raise RuntimeError("GitHub device-token response lacks access_token")
        if error == "authorization_pending":
            continue
        if error == "slow_down":
            interval += 5
            continue
        raise RuntimeError(f"GitHub device authorization failed: {error}")
    raise RuntimeError("GitHub device authorization expired")


def verify_owner(token: str) -> None:
    user = api_get("/user", token)
    if not isinstance(user, dict) or user.get("login") != EXPECTED_LOGIN:
        raise RuntimeError("unexpected GitHub owner identity")
    membership = api_get(f"/user/memberships/orgs/{ORG}", token)
    if not isinstance(membership, dict):
        raise RuntimeError("organization membership response is invalid")
    if (membership.get("role"), membership.get("state")) != ("admin", "active"):
        raise RuntimeError(f"{EXPECTED_LOGIN} is not an active {ORG} owner")


def _diagnostic(value: str) -> str:
    return value[-MAX_COMMAND_DIAGNOSTIC_CHARS:].replace("\x00", "�")


def run(
    command: list[str],
    *,
    cwd: pathlib.Path | None = None,
    env: dict[str, str] | None = None,
) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(command)}\n"
            f"stdout (bounded tail):\n{_diagnostic(completed.stdout)}\n"
            f"stderr (bounded tail):\n{_diagnostic(completed.stderr)}"
        )
    return completed.stdout.strip()


def git_environment(token: str, home: pathlib.Path, askpass: pathlib.Path) -> dict[str, str]:
    environment = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": str(home),
        "LANG": os.environ.get("LANG", "C.UTF-8"),
        "GITHUB_REPOSITORY_ADMIN_TOKEN": token,
        "GIT_ASKPASS": str(askpass),
        "GIT_TERMINAL_PROMPT": "0",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
    }
    for key in ("LC_ALL", "SSL_CERT_FILE", "SSL_CERT_DIR", "TMPDIR"):
        value = os.environ.get(key)
        if value:
            environment[key] = value
    return environment


def repository_metadata(token: str) -> dict[str, Any]:
    status, metadata = request_json(
        "GET",
        f"{API}/repos/{TARGET}",
        token=token,
        allowed_error_statuses=frozenset({404}),
    )
    if status == 404:
        status, metadata = request_json(
            "POST",
            f"{API}/orgs/{ORG}/repos",
            token=token,
            payload={
                "name": NAME,
                "description": DESCRIPTION,
                "private": False,
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "auto_init": False,
                "allow_squash_merge": True,
                "allow_merge_commit": True,
                "allow_rebase_merge": False,
                "delete_branch_on_merge": True,
            },
        )
        if status != 201:
            raise RuntimeError(f"repository creation returned unexpected HTTP {status}")
    elif status != 200:
        raise RuntimeError(f"repository lookup returned unexpected HTTP {status}")

    if not isinstance(metadata, dict):
        raise RuntimeError("target repository response is invalid")
    if metadata.get("full_name") != TARGET or metadata.get("visibility") != "public":
        raise RuntimeError("target repository identity or visibility mismatch")
    return metadata


def _read_bundle_parts(source: pathlib.Path) -> bytes:
    asset_root = source / "scripts/critical-org-fleet/assets"
    parts = sorted(asset_root.glob("meta.part*"))
    if not parts:
        raise RuntimeError("recovered bundle parts are missing")

    encoded = bytearray()
    for part in parts:
        if part.is_symlink() or not part.is_file():
            raise RuntimeError("recovered bundle part is not a regular file")
        if len(encoded) + part.stat().st_size > MAX_ENCODED_BUNDLE_BYTES:
            raise RuntimeError("encoded recovered bundle exceeded its size limit")
        encoded.extend(part.read_bytes())

    try:
        bundle = base64.b64decode(encoded, validate=True)
    except (ValueError, base64.binascii.Error) as exc:
        raise RuntimeError("recovered bundle is not valid base64") from exc
    if len(bundle) > MAX_BUNDLE_BYTES:
        raise RuntimeError("decoded recovered bundle exceeded its size limit")
    if hashlib.sha256(bundle).hexdigest() != BUNDLE_SHA256:
        raise RuntimeError("recovered bundle digest mismatch")
    return bundle


def publish_exact_repository(token: str) -> None:
    source = pathlib.Path(os.environ["SOURCE_ROOT"]).resolve()
    bundle_bytes = _read_bundle_parts(source)
    expected_lines = {
        f"{EXPECTED_MAIN} refs/heads/main",
        f"{EXPECTED_FEATURE} refs/heads/{FEATURE_REF}",
    }

    with tempfile.TemporaryDirectory(prefix="meta-agent-selfhosted-") as directory:
        temporary = pathlib.Path(directory)
        bundle = temporary / "meta-agent-control-plane.bundle"
        bundle.write_bytes(bundle_bytes)
        bundle.chmod(0o600)

        heads = run(["git", "bundle", "list-heads", str(bundle)])
        if set(heads.splitlines()) != expected_lines:
            raise RuntimeError("recovered bundle refs changed")

        repository_metadata(token)
        root = temporary / "repo"
        clean_home = temporary / "home"
        clean_home.mkdir(mode=0o700)
        askpass = temporary / "askpass.sh"
        askpass.write_text(
            '#!/bin/sh\ncase "$1" in *Username*) echo x-access-token;; '
            '*) echo "$GITHUB_REPOSITORY_ADMIN_TOKEN";; esac\n',
            encoding="utf-8",
        )
        askpass.chmod(0o700)
        environment = git_environment(token, clean_home, askpass)

        run(
            ["git", "-c", "protocol.file.allow=always", "clone", "--no-hardlinks", str(bundle), str(root)],
            env=environment,
        )
        main_sha = run(["git", "rev-parse", "refs/remotes/origin/main"], cwd=root, env=environment)
        feature_sha = run(["git", "rev-parse", f"refs/heads/{FEATURE_REF}"], cwd=root, env=environment)
        if (main_sha, feature_sha) != (EXPECTED_MAIN, EXPECTED_FEATURE):
            raise RuntimeError("cloned bundle refs changed")

        run(
            ["git", "remote", "set-url", "origin", f"https://github.com/{TARGET}.git"],
            cwd=root,
            env=environment,
        )
        network_git = [
            "git",
            "-c",
            "http.followRedirects=false",
            "-c",
            "credential.helper=",
        ]
        run(
            network_git
            + ["push", "origin", "refs/remotes/origin/main:refs/heads/main"],
            cwd=root,
            env=environment,
        )
        run(
            network_git + ["push", "origin", f"refs/heads/{FEATURE_REF}:refs/heads/{FEATURE_REF}"],
            cwd=root,
            env=environment,
        )

        status, _ = request_json(
            "PATCH",
            f"{API}/repos/{TARGET}",
            token=token,
            payload={
                "description": DESCRIPTION,
                "default_branch": "main",
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "allow_squash_merge": True,
                "allow_merge_commit": True,
                "allow_rebase_merge": False,
                "delete_branch_on_merge": True,
            },
        )
        if status != 200:
            raise RuntimeError(f"repository configuration returned unexpected HTTP {status}")

        remote = run(
            network_git + ["ls-remote", "origin", "refs/heads/main", f"refs/heads/{FEATURE_REF}"],
            cwd=root,
            env=environment,
        )
        observed: dict[str, str] = {}
        for line in remote.splitlines():
            sha, ref = line.split("\t", 1)
            observed[ref] = sha
        expected = {
            "refs/heads/main": EXPECTED_MAIN,
            f"refs/heads/{FEATURE_REF}": EXPECTED_FEATURE,
        }
        if observed != expected:
            raise RuntimeError("remote ref verification failed")


def verify_target(token: str) -> None:
    metadata = api_get(f"/repos/{TARGET}", token)
    if not isinstance(metadata, dict):
        raise RuntimeError("target repository response is invalid")
    if (
        metadata.get("full_name") != TARGET
        or metadata.get("visibility") != "public"
        or metadata.get("default_branch") != "main"
    ):
        raise RuntimeError("target repository metadata mismatch")
    expected = {"main": EXPECTED_MAIN, FEATURE_REF: EXPECTED_FEATURE}
    for branch, sha in expected.items():
        ref = api_get(f"/repos/{TARGET}/git/ref/heads/{branch}", token)
        observed = ((ref or {}).get("object") or {}).get("sha")
        if observed != sha:
            raise RuntimeError(f"{branch} ref mismatch")


def main() -> int:
    comment_token = os.environ["COMMENT_TOKEN"]
    try:
        owner_token = authorize(comment_token)
        verify_owner(owner_token)
        publish_exact_repository(owner_token)
        verify_target(owner_token)
        run_url = (
            f"https://github.com/{os.environ['GITHUB_REPOSITORY']}/actions/runs/"
            f"{os.environ['GITHUB_RUN_ID']}"
        )
        add_comment(
            f"Created and verified `{TARGET}`: `main` `{EXPECTED_MAIN}`; "
            f"`{FEATURE_REF}` `{EXPECTED_FEATURE}`. Run: {run_url}",
            comment_token,
        )
        return 0
    except Exception as exc:
        run_url = (
            f"https://github.com/{os.environ['GITHUB_REPOSITORY']}/actions/runs/"
            f"{os.environ['GITHUB_RUN_ID']}"
        )
        try:
            add_comment(
                "Exact Meta Agents repository creation failed before live ref verification. "
                f"Inspect: {run_url}",
                comment_token,
            )
        except Exception:
            pass
        raise SystemExit(str(exc)) from exc


if __name__ == "__main__":
    raise SystemExit(main())
