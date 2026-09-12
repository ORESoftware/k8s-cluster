#!/usr/bin/env python3
"""Publish one exact ores-rate-limit core package through the DEN-2050 registry path.

The publisher is intentionally single-purpose and fail-closed. It accepts only a
short-lived Zed registry token file, verifies the exact public source commit and
immutable tag, publishes only when the version is absent, then proves a clean
copy-mode install and frozen reinstall before retaining non-secret evidence.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any

REGISTRY_USER_AGENT = "zed-cli/0.2.3"
REPOSITORY = "ores-rate-limit/ores-rl-lib-core"
COMMIT = "cfc81aef5d1de60ff6c46798745a6b3f970bc39d"
ORG = "ores-rate-limit"
NAME = "ores-rl-lib-core"
VERSION = "0.1.0"
TAG = "v0.1.0"
COORDINATE = f"{ORG}/{NAME}"


@dataclass(frozen=True)
class TagEvidence:
    direct_object: str
    peeled_commit: str
    kind: str


def run(
    argv: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    capture: bool = False,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        cwd=cwd,
        env=env,
        text=True,
        check=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def http_json(
    url: str,
    *,
    token: str | None = None,
    method: str = "GET",
    body: dict[str, Any] | None = None,
) -> tuple[int, dict[str, Any]]:
    data = None
    headers = {"Accept": "application/json", "User-Agent": REGISTRY_USER_AGENT}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if body is not None:
        data = json.dumps(body, separators=(",", ":")).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            payload = response.read()
            return response.status, json.loads(payload) if payload else {}
    except urllib.error.HTTPError as exc:
        payload = exc.read()
        try:
            decoded = json.loads(payload) if payload else {}
        except json.JSONDecodeError:
            decoded = {"body": payload.decode(errors="replace")}
        return exc.code, decoded


def claim_org(registry: str, token: str) -> None:
    status, payload = http_json(
        f"{registry}/v1/orgs",
        token=token,
        method="POST",
        body={"slug": ORG},
    )
    if status not in (200, 201, 409):
        raise RuntimeError(f"failed to claim registry org {ORG}: HTTP {status} {payload}")


def parse_remote_tag(output: str) -> TagEvidence:
    direct_ref = f"refs/tags/{TAG}"
    peeled_ref = f"{direct_ref}^{{}}"
    refs: dict[str, str] = {}
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        fields = line.split()
        if len(fields) != 2 or not re.fullmatch(r"[0-9a-f]{40}", fields[0]):
            raise ValueError(f"malformed git ls-remote tag line: {raw_line!r}")
        if fields[1] in refs:
            raise ValueError(f"duplicate remote tag ref: {fields[1]}")
        refs[fields[1]] = fields[0]

    direct = refs.get(direct_ref)
    peeled = refs.get(peeled_ref)
    if direct is None:
        raise ValueError(f"required immutable tag is missing: {TAG}")
    if any(ref not in (direct_ref, peeled_ref) for ref in refs):
        raise ValueError(f"unexpected tag refs returned for {TAG}: {sorted(refs)}")

    if peeled is None:
        if direct != COMMIT:
            raise ValueError(
                f"lightweight tag {TAG} diverges: expected {COMMIT}, observed {direct}"
            )
        return TagEvidence(direct_object=direct, peeled_commit=direct, kind="lightweight")

    if peeled != COMMIT:
        raise ValueError(
            f"annotated tag {TAG} peels to the wrong commit: expected {COMMIT}, observed {peeled}"
        )
    if direct == peeled:
        raise ValueError("annotated-tag response did not retain a distinct tag object")
    return TagEvidence(direct_object=direct, peeled_commit=peeled, kind="annotated")


def verify_remote_tag(repo: Path) -> TagEvidence:
    result = run(
        [
            "git",
            "ls-remote",
            "origin",
            f"refs/tags/{TAG}",
            f"refs/tags/{TAG}^{{}}",
        ],
        cwd=repo,
        capture=True,
    )
    return parse_remote_tag(result.stdout)


def validate_manifest(path: Path) -> None:
    with path.open("rb") as stream:
        manifest = tomllib.load(stream)
    package = manifest.get("package")
    if not isinstance(package, dict):
        raise ValueError("Zed manifest lacks [package]")
    expected = {"org": ORG, "name": NAME, "version": VERSION}
    observed = {key: package.get(key) for key in expected}
    if observed != expected:
        raise ValueError(f"unexpected package identity: expected {expected}, observed {observed}")
    repository = package.get("repository")
    if not isinstance(repository, dict):
        raise ValueError("Zed manifest lacks [package.repository]")
    if repository.get("vcs") != "git" or repository.get("url") != f"https://github.com/{REPOSITORY}":
        raise ValueError(f"unexpected package repository metadata: {repository}")
    publish = manifest.get("publish")
    if not isinstance(publish, dict) or publish.get("tag_format") != "v{version}":
        raise ValueError("Zed manifest must bind publication to v{version}")


def verify_metadata(metadata: dict[str, Any]) -> dict[str, Any]:
    expected = {"org": ORG, "name": NAME, "version": VERSION}
    observed = {key: metadata.get(key) for key in expected}
    if observed != expected:
        raise ValueError(f"registry package identity diverged: expected {expected}, observed {observed}")
    if metadata.get("vcs_tag") != TAG:
        raise ValueError(f"registry tag diverged: {metadata.get('vcs_tag')!r}")
    if metadata.get("vcs_commit") != COMMIT:
        raise ValueError(f"registry commit diverged: {metadata.get('vcs_commit')!r}")
    digest = metadata.get("sha256")
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise ValueError("registry metadata lacks a lowercase SHA-256 digest")
    if metadata.get("yanked") is not False:
        raise ValueError("registry version is yanked or has no explicit yanked=false marker")
    download_url = metadata.get("download_url")
    if not isinstance(download_url, str) or not download_url.startswith("https://"):
        raise ValueError("registry metadata lacks an HTTPS download URL")
    return {
        "org": ORG,
        "name": NAME,
        "version": VERSION,
        "vcs_tag": TAG,
        "vcs_commit": COMMIT,
        "sha256": digest,
        "download_url": download_url,
        "yanked": False,
    }


def validate_lock(lock_path: Path) -> dict[str, str]:
    if not lock_path.is_file():
        raise ValueError("zed install did not create .zpkg.lock")
    text = lock_path.read_text(encoding="utf-8")
    for required in (COORDINATE, VERSION, TAG, COMMIT):
        if required not in text:
            raise ValueError(f"resolved lock is missing immutable package evidence: {required}")
    digests = sorted(set(re.findall(r"(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])", text)))
    if not digests:
        raise ValueError("resolved lock lacks a SHA-256-shaped artifact digest")
    return {"sha256": sha256(lock_path), "artifact_sha256": digests[0]}


def checkout_exact_source(work: Path) -> Path:
    repo = work / "ores-rl-lib-core"
    run(["git", "init", str(repo)])
    run(["git", "-C", str(repo), "remote", "add", "origin", f"https://github.com/{REPOSITORY}.git"])
    run(["git", "-C", str(repo), "fetch", "--quiet", "--no-tags", "--depth=1", "origin", COMMIT])
    run(["git", "-C", str(repo), "switch", "--quiet", "--detach", "FETCH_HEAD"])
    head = run(["git", "-C", str(repo), "rev-parse", "HEAD"], capture=True).stdout.strip()
    if head != COMMIT:
        raise RuntimeError(f"source checkout diverged: expected {COMMIT}, observed {head}")
    if run(["git", "-C", str(repo), "status", "--porcelain"], capture=True).stdout.strip():
        raise RuntimeError("source checkout is unexpectedly dirty")
    validate_manifest(repo / ".zpkg.toml")
    return repo


def publish_or_verify(
    *,
    repo: Path,
    registry: str,
    registry_token: str,
    zed_bin: Path,
) -> dict[str, Any]:
    version_url = f"{registry}/v1/packages/{ORG}/{NAME}/versions/{VERSION}"
    status, metadata = http_json(version_url, token=registry_token)
    published = False
    if status == 404:
        zed_env = os.environ.copy()
        zed_env["ZED_PKG_TOKEN"] = registry_token
        run([str(zed_bin), "release", "plan"], cwd=repo, env=zed_env)
        run([str(zed_bin), "release", "preflight"], cwd=repo, env=zed_env)
        run([str(zed_bin), "pack"], cwd=repo, env=zed_env)
        run([str(zed_bin), "publish", "--registry", registry], cwd=repo, env=zed_env)
        published = True
        status, metadata = http_json(version_url, token=registry_token)
    if status != 200:
        raise RuntimeError(f"registry metadata unavailable after publication: HTTP {status} {metadata}")
    result = verify_metadata(metadata)
    result["published"] = published
    return result


def verify_copy_mode_install(
    *,
    work: Path,
    registry: str,
    registry_token: str,
    zed_bin: Path,
    lock_out: Path,
) -> dict[str, str]:
    consumer = work / "consumer"
    consumer.mkdir()
    (consumer / ".zpkg.toml").write_text(
        f'''[package]\norg = "den2050"\nname = "ores-rl-registry-probe"\nversion = "0.0.0"\n\n[dependencies]\n"{COORDINATE}" = "{VERSION}"\n\n[install]\ndir = ".vendor/.zed"\n''',
        encoding="utf-8",
    )
    zed_env = os.environ.copy()
    zed_env["ZED_PKG_TOKEN"] = registry_token
    command = [
        str(zed_bin),
        "install",
        "--registry",
        registry,
        "--install-mode",
        "copy",
    ]
    run(command, cwd=consumer, env=zed_env)
    lock = consumer / ".zpkg.lock"
    lock_evidence = validate_lock(lock)
    installed = consumer / ".vendor" / ".zed" / ORG / NAME
    if not installed.is_dir() or installed.is_symlink():
        raise RuntimeError(f"copy-mode installed package is missing or linked: {installed}")
    shutil.copy2(lock, lock_out)
    before = lock.read_bytes()
    shutil.rmtree(consumer / ".vendor")
    run([*command, "--frozen"], cwd=consumer, env=zed_env)
    if lock.read_bytes() != before:
        raise RuntimeError("frozen reinstall changed .zpkg.lock")
    if not installed.is_dir() or installed.is_symlink():
        raise RuntimeError("frozen copy-mode reinstall did not restore a real package directory")
    lock_evidence["installed_path"] = str(installed.relative_to(consumer))
    return lock_evidence


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry-token-file", type=Path, required=True)
    parser.add_argument("--zed-bin", type=Path, required=True)
    parser.add_argument("--registry", required=True)
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--lock-out", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path)
    args = parser.parse_args()

    token = args.registry_token_file.read_text(encoding="utf-8").strip()
    if not token.startswith("zpkg_") or len(token) < 20 or any(ch.isspace() for ch in token):
        raise RuntimeError("invalid Zed registry credential material")
    if not args.zed_bin.is_file() or not os.access(args.zed_bin, os.X_OK):
        raise RuntimeError(f"zed binary is missing or not executable: {args.zed_bin}")

    registry = args.registry.rstrip("/")
    status, health = http_json(f"{registry}/healthz", token=token)
    if status != 200 or health.get("ok") is not True:
        raise RuntimeError(f"registry health check failed: HTTP {status} {health}")
    claim_org(registry, token)

    work = args.work_dir or Path(tempfile.mkdtemp(prefix="den2050-ores-rl-zed-"))
    work.mkdir(parents=True, exist_ok=True)
    repo = checkout_exact_source(work)
    tag = verify_remote_tag(repo)
    package = publish_or_verify(
        repo=repo,
        registry=registry,
        registry_token=token,
        zed_bin=args.zed_bin,
    )
    lock = verify_copy_mode_install(
        work=work,
        registry=registry,
        registry_token=token,
        zed_bin=args.zed_bin,
        lock_out=args.lock_out,
    )
    evidence = {
        "schema_version": 1,
        "registry": registry,
        "source": {
            "repository": REPOSITORY,
            "commit": COMMIT,
            "tag": TAG,
            "tag_kind": tag.kind,
            "tag_object": tag.direct_object,
            "peeled_commit": tag.peeled_commit,
        },
        "package": package,
        "lock": lock,
        "copy_mode": True,
        "frozen_reinstall": True,
    }
    args.evidence_out.write_text(
        json.dumps(evidence, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(f"VERIFIED_DEN2050_ORES_RL_ZED_PACKAGE {COORDINATE}@{VERSION}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
