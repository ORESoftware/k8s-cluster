#!/usr/bin/env python3
"""Run the immutable GHA test-fallback activator inside the AWS cluster."""

from __future__ import annotations

import hashlib
import ipaddress
import os
import ssl
import sys
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit


TRUSTED_SHA = "04cfc4658f715d6ac77b9c16445add31ad0a761a"
ACTIVATOR_SHA256 = "36d0e65154c03132fa6bd9491834629aa8883c28a1333e36ebdec054a22d9589"
ACTIVATOR_PATH = "scripts/ops/activate_gha_test_org_fallback.py"
ACTIVATOR_URL = (
    "https://raw.githubusercontent.com/ORESoftware/k8s-cluster/"
    + TRUSTED_SHA
    + "/"
    + ACTIVATOR_PATH
)
KUBECTL_URL = "https://dl.k8s.io/release/v1.31.14/bin/linux/amd64/kubectl"
KUBECTL_SHA256 = "8791ec7c8966b61420d55103a5fb948de9f0ca3d7306d789734975ad9704bdb0"
PUBLIC_IP_URL = "https://checkip.amazonaws.com/"
MAX_ACTIVATOR_BYTES = 2 * 1024 * 1024
MAX_KUBECTL_BYTES = 64 * 1024 * 1024


def fail(message: str) -> None:
    raise SystemExit(f"gha-test-fallback in-cluster bootstrap failed: {message}")


def fetch(url: str, *, maximum: int, label: str) -> bytes:
    request = urllib.request.Request(
        url,
        headers={"User-Agent": "gha-test-fallback-in-cluster-bootstrap/1"},
    )
    try:
        with urllib.request.urlopen(
            request,
            timeout=90,
            context=ssl.create_default_context(),
        ) as response:
            final = urlsplit(response.geturl())
            if final.scheme != "https" or not final.hostname:
                fail(f"{label} crossed a non-HTTPS redirect boundary")
            value = response.read(maximum + 1)
    except urllib.error.HTTPError as exc:
        fail(f"{label} returned HTTP {exc.code}")
    except (urllib.error.URLError, TimeoutError, OSError):
        fail(f"{label} could not be fetched")
    if len(value) > maximum:
        fail(f"{label} exceeded its byte boundary")
    return value


def verify_digest(value: bytes, expected: str, *, label: str) -> None:
    if hashlib.sha256(value).hexdigest() != expected:
        fail(f"{label} did not match its immutable SHA-256 digest")


def resolve_public_ip() -> str:
    raw = fetch(PUBLIC_IP_URL, maximum=128, label="AWS public-IP probe")
    try:
        candidate = raw.decode("ascii").strip()
        address = ipaddress.ip_address(candidate)
    except (UnicodeDecodeError, ValueError):
        fail("AWS public-IP probe returned an invalid address")
    if address.version != 4 or not address.is_global:
        fail("AWS public-IP probe did not return a global IPv4 address")
    return str(address)


def install_kubectl() -> None:
    binary = fetch(KUBECTL_URL, maximum=MAX_KUBECTL_BYTES, label="kubectl")
    verify_digest(binary, KUBECTL_SHA256, label="kubectl")
    target = Path("/tools/kubectl")
    try:
        with target.open("xb") as stream:
            stream.write(binary)
        target.chmod(0o555)
    except OSError:
        fail("kubectl could not be installed in the bounded tools volume")


def main() -> int:
    public_ip = resolve_public_ip()
    install_kubectl()
    source = fetch(ACTIVATOR_URL, maximum=MAX_ACTIVATOR_BYTES, label="activator")
    verify_digest(source, ACTIVATOR_SHA256, label="activator")
    try:
        source_text = source.decode("utf-8")
    except UnicodeDecodeError:
        fail("activator is not valid UTF-8")

    os.environ["PATH"] = "/tools:/usr/local/bin:/usr/bin:/bin"
    os.environ["HOME"] = "/tmp"
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    sys.argv = [
        ACTIVATOR_PATH,
        "--callback-url",
        f"https://{public_ip}/gha-webhooks/github",
        "--source-revision",
        TRUSTED_SHA,
        "--namespace",
        "default",
        "--poll-seconds",
        "3",
        "--timeout-seconds",
        "1800",
        "--reconcile-timeout-seconds",
        "900",
    ]
    namespace = {"__name__": "__main__", "__file__": ACTIVATOR_PATH}
    exec(compile(source_text, ACTIVATOR_PATH, "exec"), namespace, namespace)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
