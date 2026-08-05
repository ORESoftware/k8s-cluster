#!/usr/bin/env python3
"""Credential-free source contract for the Daedalus Meshy provider adapter."""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def read(path: str) -> str:
    target = ROOT / path
    if not target.is_file():
        raise SystemExit(f"missing required Meshy integration file: {path}")
    return target.read_text(encoding="utf-8")


def require(text: str, tokens: list[str], path: str) -> None:
    missing = [token for token in tokens if token not in text]
    if missing:
        rendered = "\n".join(f"  - {token}" for token in missing)
        raise SystemExit(f"{path} is missing required Meshy contracts:\n{rendered}")


def main() -> None:
    manifest = read("crates/meshy-client/Cargo.toml")
    library = read("crates/meshy-client/src/lib.rs")
    cli = read("crates/meshy-client/src/cli.rs")
    binary = read("crates/meshy-client/src/bin/dd-meshy-adapter.rs")
    docs = read("docs/meshy-integration.md")
    tracking = read("docs/project-tracking.md")

    require(
        manifest,
        [
            'name = "dd-meshy-client"',
            'reqwest = { version = "0.12"',
            'rustls-tls',
            'publish = false',
        ],
        "crates/meshy-client/Cargo.toml",
    )
    require(
        library,
        [
            'DEFAULT_BASE_URL: &str = "https://api.meshy.ai"',
            'API_KEY_ENV: &str = "MESHY_API_KEY"',
            '"image-to-3d"',
            '"multi-image-to-3d"',
            'authorization.set_sensitive(true)',
            '"Bearer <redacted>"',
            'machine_ready: false',
            'release_state: "draft"',
            'TargetFormat::ThreeMf',
            '"dd.fabrication.external-geometry-candidate.v1"',
            'create_image_task_uses_bearer_auth_and_expected_endpoint',
            'succeeded_provider_task_is_still_review_blocked',
        ],
        "crates/meshy-client/src/lib.rs",
    )
    require(
        cli,
        [
            '"create-image"',
            '"create-multi-image"',
            '"wait-image"',
            '"wait-multi-image"',
            '"delete-image"',
            '"delete-multi-image"',
        ],
        "crates/meshy-client/src/cli.rs",
    )
    require(binary, ["run_from_env", "exit_on_error"], "dd-meshy-adapter.rs")
    require(
        docs,
        [
            "Linear `DEN-2465`",
            "machineReady: false",
            "GLB/STL/3MF",
            "Daedalus-controlled storage",
            "webhook",
        ],
        "docs/meshy-integration.md",
    )
    require(
        tracking,
        ["github.com/daedalus-fab", "DEN-2465", "daedalus-fab-project"],
        "docs/project-tracking.md",
    )

    single = json.loads(read("examples/meshy/image-to-3d.json"))
    multi = json.loads(read("examples/meshy/multi-image-to-3d.json"))
    expected_formats = {"glb", "stl", "3mf"}
    if set(single.get("target_formats", [])) != expected_formats:
        raise SystemExit("single-image example must request glb, stl, and 3mf")
    if set(multi.get("target_formats", [])) != expected_formats:
        raise SystemExit("multi-image example must request glb, stl, and 3mf")
    if not 1 <= len(multi.get("image_urls", [])) <= 4:
        raise SystemExit("multi-image example must contain between one and four views")

    forbidden = [
        path
        for path in ROOT.rglob("*")
        if path.is_file()
        and path.name not in {"check_meshy_integration.py"}
        and ("msy_" in path.read_text(encoding="utf-8", errors="ignore"))
    ]
    if forbidden:
        paths = "\n".join(f"  - {path.relative_to(ROOT)}" for path in forbidden)
        raise SystemExit(f"possible Meshy API key material found in tracked files:\n{paths}")

    print("Meshy provider integration contract is complete")


if __name__ == "__main__":
    main()
