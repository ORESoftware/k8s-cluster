#!/usr/bin/env python3
from __future__ import annotations

import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def require(path: str, *needles: str) -> str:
    text = (ROOT / path).read_text(encoding="utf-8")
    for needle in needles:
        assert needle in text, f"{path} missing {needle!r}"
    return text


def main() -> None:
    cargo = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    assert cargo["package"]["name"] == "honeypot-rs"
    assert cargo["package"]["edition"] == "2024"
    assert cargo["package"]["publish"] is False

    source = require(
        "src/main.rs",
        "ores.honeypot.event.v1",
        "ores_hp_v1_",
        "example.invalid",
        "cf-connecting-ip",
        "TemporaryBlock",
        "HumanReview",
    )
    assert "request.body" not in source
    assert "permanent" not in source.lower()

    workload = require(
        "deploy/k8s/workload.yaml",
        "automountServiceAccountToken: false",
        "readOnlyRootFilesystem: true",
        "allowPrivilegeEscalation: false",
        "drop:\n                - ALL",
        "egress: []",
        "cloudflare-tunnel",
        "ClusterSecretStore",
        "dd-cluster-secrets",
    )
    assert "kind: Namespace" not in workload
    assert "ClusterRole" not in workload
    assert re.search(r"sha256:0{64}\b", workload), "immutable image promotion gate missing"

    forbidden = [
        re.compile(r"ghp_[A-Za-z0-9]{20,}"),
        re.compile(r"github_pat_[A-Za-z0-9_]{20,}"),
        re.compile(r"lin_api_[A-Za-z0-9]{20,}"),
        re.compile(r"AKIA[0-9A-Z]{16}"),
        re.compile(r"sk_live_[A-Za-z0-9]{16,}"),
    ]
    for path in ROOT.rglob("*"):
        if not path.is_file() or ".git" in path.parts:
            continue
        text = path.read_text(encoding="utf-8", errors="ignore")
        for pattern in forbidden:
            assert not pattern.search(text), f"possible secret in {path.relative_to(ROOT)}"

    print("honeypot repository validation: PASS")


if __name__ == "__main__":
    main()
