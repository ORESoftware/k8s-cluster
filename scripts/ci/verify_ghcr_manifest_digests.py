#!/usr/bin/env python3
"""Fail closed when an exact GHCR image digest is not retrievable.

The verifier reads Kubernetes manifests, extracts exact public GHCR image
references, obtains an anonymous repository-scoped pull token, and performs a
manifest HEAD request. It never accepts mutable tags, follows redirects, reads
or logs manifest bodies, or serializes bearer credentials.
"""

from __future__ import annotations

import argparse
import json
import re
import ssl
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Protocol, Sequence

IMAGE_RE = re.compile(
    r"^\s*image:\s*['\"]?(ghcr\.io/([a-z0-9._-]+/[a-z0-9._/-]+)@sha256:([0-9a-f]{64}))['\"]?\s*(?:#.*)?$"
)
MAX_TOKEN_RESPONSE_BYTES = 64 * 1024
TOKEN_RE = re.compile(r"^[A-Za-z0-9._~+/=-]{16,8192}$")
ACCEPT = ", ".join(
    (
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    )
)


class VerificationError(RuntimeError):
    """A content-free, reviewable verification failure."""


@dataclass(frozen=True, order=True)
class ImageReference:
    full: str
    repository: str
    digest: str


@dataclass(frozen=True)
class Response:
    status: int
    headers: Mapping[str, str]
    body: bytes = b""


class Transport(Protocol):
    def request(
        self,
        *,
        method: str,
        url: str,
        headers: Mapping[str, str],
        max_body_bytes: int,
    ) -> Response: ...


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


class UrllibTransport:
    def __init__(self, *, timeout_seconds: float = 10.0) -> None:
        if timeout_seconds <= 0 or timeout_seconds > 60:
            raise ValueError("timeout must be in (0, 60]")
        context = ssl.create_default_context()
        self._opener = urllib.request.build_opener(
            NoRedirect(), urllib.request.HTTPSHandler(context=context)
        )
        self._timeout_seconds = timeout_seconds

    def request(
        self,
        *,
        method: str,
        url: str,
        headers: Mapping[str, str],
        max_body_bytes: int,
    ) -> Response:
        request = urllib.request.Request(url=url, method=method, headers=dict(headers))
        try:
            with self._opener.open(request, timeout=self._timeout_seconds) as response:
                status = int(response.status)
                body = response.read(max_body_bytes + 1) if max_body_bytes else b""
                if len(body) > max_body_bytes:
                    raise VerificationError("registry response exceeded the bounded body limit")
                return Response(status=status, headers=dict(response.headers.items()), body=body)
        except urllib.error.HTTPError as exc:
            # Do not read or log provider response bodies. Status and headers are
            # sufficient to classify an unavailable or redirected manifest.
            return Response(status=int(exc.code), headers=dict(exc.headers.items()), body=b"")
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise VerificationError(f"registry request failed: {type(exc).__name__}") from None


def parse_manifest(path: Path) -> list[ImageReference]:
    if not path.is_file():
        raise VerificationError(f"manifest is missing: {path}")
    if path.stat().st_size > 4 * 1024 * 1024:
        raise VerificationError(f"manifest exceeds size limit: {path}")
    references: list[ImageReference] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        stripped = line.strip()
        if not stripped.startswith("image:"):
            continue
        match = IMAGE_RE.match(line)
        if not match:
            if "ghcr.io/" in line:
                raise VerificationError(
                    f"GHCR image is not an exact lowercase sha256 reference: {path}:{line_number}"
                )
            continue
        full, repository, digest_hex = match.groups()
        references.append(
            ImageReference(full=full, repository=repository, digest=f"sha256:{digest_hex}")
        )
    return references


def collect_references(paths: Sequence[Path]) -> list[ImageReference]:
    unique: dict[str, ImageReference] = {}
    for path in paths:
        for reference in parse_manifest(path):
            unique[reference.full] = reference
    if not unique:
        raise VerificationError("no exact GHCR image references were found")
    return sorted(unique.values())


def normalized_headers(headers: Mapping[str, str]) -> dict[str, str]:
    return {key.lower(): value.strip() for key, value in headers.items()}


def request_token(reference: ImageReference, transport: Transport) -> str:
    query = urllib.parse.urlencode(
        {
            "service": "ghcr.io",
            "scope": f"repository:{reference.repository}:pull",
        }
    )
    response = transport.request(
        method="GET",
        url=f"https://ghcr.io/token?{query}",
        headers={
            "Accept": "application/json",
            "User-Agent": "oresoftware-ghcr-manifest-preflight/1",
        },
        max_body_bytes=MAX_TOKEN_RESPONSE_BYTES,
    )
    if response.status != 200:
        raise VerificationError(
            f"anonymous GHCR token request returned HTTP {response.status} for {reference.repository}"
        )
    try:
        payload = json.loads(response.body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise VerificationError("anonymous GHCR token response was not valid JSON") from None
    token = payload.get("token") or payload.get("access_token")
    if not isinstance(token, str) or not TOKEN_RE.fullmatch(token):
        raise VerificationError("anonymous GHCR token response did not contain a bounded bearer")
    return token


def verify_reference(reference: ImageReference, transport: Transport) -> dict[str, object]:
    token = request_token(reference, transport)
    response = transport.request(
        method="HEAD",
        url=f"https://ghcr.io/v2/{reference.repository}/manifests/{reference.digest}",
        headers={
            "Accept": ACCEPT,
            "Authorization": f"Bearer {token}",
            "User-Agent": "oresoftware-ghcr-manifest-preflight/1",
        },
        max_body_bytes=0,
    )
    if response.status != 200:
        raise VerificationError(
            f"GHCR manifest HEAD returned HTTP {response.status} for {reference.full}"
        )
    headers = normalized_headers(response.headers)
    observed_digest = headers.get("docker-content-digest")
    if observed_digest != reference.digest:
        state = "missing" if observed_digest is None else "mismatched"
        raise VerificationError(
            f"GHCR manifest digest header was {state} for {reference.full}"
        )
    return {
        "image": reference.full,
        "repository": reference.repository,
        "digest": reference.digest,
        "http_status": response.status,
        "digest_header_match": True,
        "anonymous_pull_scope": True,
    }


def verify_with_retries(
    reference: ImageReference,
    transport: Transport,
    *,
    attempts: int,
) -> dict[str, object]:
    if attempts < 1 or attempts > 3:
        raise ValueError("attempts must be in [1, 3]")
    last_error: VerificationError | None = None
    for attempt in range(1, attempts + 1):
        try:
            result = verify_reference(reference, transport)
            result["attempt"] = attempt
            return result
        except VerificationError as exc:
            last_error = exc
            # The retry is intentionally bounded so a transient network reset
            # does not look like a missing release; persistent 4xx, redirect,
            # malformed-response, and digest mismatch conditions still fail.
            if attempt < attempts:
                time.sleep(0.25 * attempt)
    assert last_error is not None
    raise last_error


def write_audit(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.chmod(0o600)
    temporary.replace(path)


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", action="append", required=True, type=Path)
    parser.add_argument("--audit", required=True, type=Path)
    parser.add_argument("--timeout-seconds", type=float, default=10.0)
    parser.add_argument("--attempts", type=int, default=2, choices=(1, 2, 3))
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    audit: dict[str, object] = {
        "schema_version": "ore.ghcr-manifest-preflight.v1",
        "passed": False,
        "references": [],
        "credential_source": "anonymous_repository_scoped_token",
        "response_bodies_recorded": False,
    }
    try:
        references = collect_references(args.manifest)
        transport = UrllibTransport(timeout_seconds=args.timeout_seconds)
        results = [
            verify_with_retries(reference, transport, attempts=args.attempts)
            for reference in references
        ]
        audit["references"] = results
        audit["reference_count"] = len(results)
        audit["passed"] = True
        write_audit(args.audit, audit)
        print(json.dumps({"passed": True, "reference_count": len(results)}))
        return 0
    except Exception as exc:
        audit["error_type"] = type(exc).__name__
        audit["error"] = str(exc)
        write_audit(args.audit, audit)
        print(json.dumps({"passed": False, "error_type": type(exc).__name__}))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
