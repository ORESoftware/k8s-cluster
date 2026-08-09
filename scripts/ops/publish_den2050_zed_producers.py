#!/usr/bin/env python3
"""Publish the exact DEN-2050 StreemPilot/HypeSiege producer graph.

The GitHub credential is read from a mode-0600 file supplied by the trusted
workflow. It is only exposed to git through GIT_ASKPASS and is never printed.
The Zed registry is currently in its explicitly temporary auth-disabled
bootstrap posture, so registry publication itself uses no credential.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path


# Keep auxiliary Python probes indistinguishable from the pinned Zed CLI's
# registry client. Cloudflare explicitly rejects Python urllib's default
# browser signature on registry.zpkg.net, while zed-cli sets this exact UA.
REGISTRY_USER_AGENT = "zed-cli/0.1.0"


@dataclass(frozen=True)
class Producer:
    repository: str
    commit: str
    org: str
    name: str
    version: str
    dependencies: tuple[str, ...] = ()

    @property
    def tag(self) -> str:
        return f"v{self.version}"


PRODUCERS: tuple[Producer, ...] = (
    Producer(
        "StreemPilot/sp-interfaces",
        "89d1b04a0d66ec990e6a0d95e46fafe6d99d44ce",
        "streempilot",
        "sp-interfaces",
        "0.1.0",
    ),
    Producer(
        "StreemPilot/sp-libs",
        "355d27b31c168bf81da2f737aeaf67a73381e1e9",
        "streempilot",
        "sp-libs",
        "0.1.0",
        ("streempilot/sp-interfaces",),
    ),
    Producer(
        "StreemPilot/streempilot-clients",
        "e05e4c97cf29ac6bd27dd98fbbdce176906368b4",
        "streempilot",
        "streempilot-clients",
        "0.1.0",
        ("streempilot/sp-interfaces",),
    ),
    Producer(
        "hypesiege/hypesiege-interfaces",
        "e52d76aedabbe7dc4984169486e8305f993796fb",
        "hypesiege",
        "hypesiege-interfaces",
        "0.1.0",
    ),
    Producer(
        "hypesiege/hypesiege-libs",
        "1657a2e707d11e1828e65667ba03c2b31c39a543",
        "hypesiege",
        "hypesiege-libs",
        "0.1.1",
        ("hypesiege/hypesiege-interfaces",),
    ),
    Producer(
        "hypesiege/hypesiege-clients",
        "75322d79623c2666a445e244f637af3bde3d2e46",
        "hypesiege",
        "hypesiege-clients",
        "0.1.0",
        ("hypesiege/hypesiege-interfaces", "hypesiege/hypesiege-libs"),
    ),
)


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


def http_json(url: str, *, method: str = "GET", body: dict | None = None) -> tuple[int, dict]:
    data = None
    headers = {
        "Accept": "application/json",
        "User-Agent": REGISTRY_USER_AGENT,
    }
    if body is not None:
        data = json.dumps(body, separators=(",", ":")).encode()
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            payload = response.read()
            decoded = json.loads(payload) if payload else {}
            return response.status, decoded
    except urllib.error.HTTPError as exc:
        payload = exc.read()
        try:
            decoded = json.loads(payload) if payload else {}
        except json.JSONDecodeError:
            decoded = {"body": payload.decode(errors="replace")}
        return exc.code, decoded


def claim_org(registry: str, org: str) -> None:
    status, payload = http_json(f"{registry}/v1/orgs", method="POST", body={"slug": org})
    if status in (200, 201):
        return
    if status == 409:
        return
    raise RuntimeError(f"failed to claim registry org {org}: HTTP {status} {payload}")


def validate_manifest(path: Path, producer: Producer) -> None:
    with path.open("rb") as stream:
        manifest = tomllib.load(stream)
    package = manifest["package"]
    assert package["org"] == producer.org, (producer.repository, package["org"], producer.org)
    assert package["name"] == producer.name, (producer.repository, package["name"], producer.name)
    assert package["version"] == producer.version, (
        producer.repository,
        package["version"],
        producer.version,
    )
    dependencies = manifest.get("dependencies", {})
    for dependency in producer.dependencies:
        assert dependency in dependencies, (producer.repository, dependency, dependencies)
        assert str(dependencies[dependency]).startswith("^0.1."), (
            producer.repository,
            dependency,
            dependencies[dependency],
        )


def ensure_tag(repo: Path, producer: Producer, git_env: dict[str, str]) -> None:
    result = run(
        ["git", "ls-remote", "origin", f"refs/tags/{producer.tag}"],
        cwd=repo,
        env=git_env,
        capture=True,
    )
    observed = result.stdout.strip().split()[0] if result.stdout.strip() else ""
    if observed:
        if observed != producer.commit:
            raise RuntimeError(
                f"immutable tag diverges for {producer.repository}:{producer.tag}: "
                f"{observed} != {producer.commit}"
            )
    else:
        run(["git", "tag", producer.tag, producer.commit], cwd=repo, env=git_env)
        run(
            ["git", "push", "origin", f"refs/tags/{producer.tag}:refs/tags/{producer.tag}"],
            cwd=repo,
            env=git_env,
        )
        verified = run(
            ["git", "ls-remote", "origin", f"refs/tags/{producer.tag}"],
            cwd=repo,
            env=git_env,
            capture=True,
        ).stdout.strip()
        if not verified.startswith(producer.commit):
            raise RuntimeError(f"tag verification failed for {producer.repository}:{producer.tag}")

    local = run(
        ["git", "tag", "--list", producer.tag], cwd=repo, env=git_env, capture=True
    ).stdout.strip()
    if not local:
        run(["git", "tag", producer.tag, producer.commit], cwd=repo, env=git_env)


def publish_one(
    producer: Producer,
    *,
    work: Path,
    registry: str,
    zed_bin: Path,
    git_env: dict[str, str],
) -> dict:
    repo = work / producer.repository.replace("/", "-")
    run(["git", "init", str(repo)], env=git_env)
    run(
        ["git", "-C", str(repo), "remote", "add", "origin", f"https://github.com/{producer.repository}.git"],
        env=git_env,
    )
    run(
        ["git", "-C", str(repo), "fetch", "--quiet", "--no-tags", "--depth=1", "origin", producer.commit],
        env=git_env,
    )
    run(["git", "-C", str(repo), "switch", "--quiet", "--detach", "FETCH_HEAD"], env=git_env)
    head = run(["git", "-C", str(repo), "rev-parse", "HEAD"], env=git_env, capture=True).stdout.strip()
    if head != producer.commit:
        raise RuntimeError(f"commit verification failed for {producer.repository}: {head}")

    validate_manifest(repo / ".zpkg.toml", producer)
    ensure_tag(repo, producer, git_env)
    if run(["git", "-C", str(repo), "status", "--porcelain"], env=git_env, capture=True).stdout.strip():
        raise RuntimeError(f"working tree became dirty for {producer.repository}")

    published = run(
        [str(zed_bin), "publish", "--registry", registry], cwd=repo, env=git_env, capture=True
    )
    if published.stdout.strip():
        print(published.stdout.strip())
    if published.stderr.strip():
        print(published.stderr.strip(), file=sys.stderr)

    status, metadata = http_json(
        f"{registry}/v1/packages/{producer.org}/{producer.name}/versions/{producer.version}"
    )
    if status != 200:
        raise RuntimeError(f"published metadata unavailable for {producer.org}/{producer.name}: HTTP {status}")
    assert metadata["org"] == producer.org
    assert metadata["name"] == producer.name
    assert metadata["version"] == producer.version
    assert metadata["vcs_tag"] == producer.tag
    assert metadata.get("vcs_commit") == producer.commit
    assert isinstance(metadata["sha256"], str) and len(metadata["sha256"]) == 64
    assert metadata["yanked"] is False

    return {
        "repository": producer.repository,
        "org": producer.org,
        "name": producer.name,
        "version": producer.version,
        "commit": producer.commit,
        "tag": producer.tag,
        "artifact_sha256": metadata["sha256"],
        "download_url": metadata["download_url"],
    }


def verify_install_graph(*, work: Path, registry: str, zed_bin: Path, lock_out: Path) -> None:
    consumer = work / "consumer"
    consumer.mkdir()
    (consumer / ".zpkg.toml").write_text(
        """[package]\norg = \"den2050\"\nname = \"registry-probe\"\nversion = \"0.0.0\"\n\n[dependencies]\n\"streempilot/streempilot-clients\" = \"0.1.0\"\n\"streempilot/sp-libs\" = \"0.1.0\"\n\"hypesiege/hypesiege-clients\" = \"0.1.0\"\n\n[install]\ndir = \".vendor/.zed\"\n""",
        encoding="utf-8",
    )
    run([str(zed_bin), "install", "--registry", registry], cwd=consumer)
    lock = consumer / ".zpkg.lock"
    if not lock.is_file():
        raise RuntimeError("zed install did not create .zpkg.lock")
    lock_text = lock.read_text(encoding="utf-8")
    for expected in (
        "streempilot/sp-interfaces",
        "streempilot/streempilot-clients",
        "streempilot/sp-libs",
        "hypesiege/hypesiege-interfaces",
        "hypesiege/hypesiege-libs",
        "hypesiege/hypesiege-clients",
    ):
        if expected not in lock_text:
            raise RuntimeError(f"resolved lock is missing {expected}")
    shutil.copy2(lock, lock_out)

    vendor = consumer / ".vendor"
    if vendor.exists():
        shutil.rmtree(vendor)
    run([str(zed_bin), "install", "--frozen", "--registry", registry], cwd=consumer)
    if not lock.is_file() or lock.read_text(encoding="utf-8") != lock_text:
        raise RuntimeError("frozen reinstall changed the lockfile")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--token-file", type=Path, required=True)
    parser.add_argument("--zed-bin", type=Path, required=True)
    parser.add_argument("--registry", required=True)
    parser.add_argument("--evidence-out", type=Path, required=True)
    parser.add_argument("--lock-out", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path)
    args = parser.parse_args()

    token = args.token_file.read_text(encoding="utf-8").strip()
    if len(token) < 20 or any(ch.isspace() for ch in token):
        raise RuntimeError("invalid GitHub credential material")
    if not args.zed_bin.is_file():
        raise RuntimeError(f"zed binary not found: {args.zed_bin}")

    root = args.work_dir or Path(tempfile.mkdtemp(prefix="den2050-zed-"))
    root.mkdir(parents=True, exist_ok=True)
    askpass = root / "git-askpass.sh"
    askpass.write_text(
        "#!/usr/bin/env bash\nset -euo pipefail\n"
        "case \"${1:-}\" in\n"
        "  *Username*) printf '%s\\n' 'x-access-token' ;;\n"
        "  *Password*) printf '%s\\n' \"${DEN2050_GITHUB_TOKEN:?}\" ;;\n"
        "  *) exit 1 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    askpass.chmod(stat.S_IRUSR | stat.S_IWUSR | stat.S_IXUSR)

    git_env = os.environ.copy()
    git_env.update(
        {
            "DEN2050_GITHUB_TOKEN": token,
            "GIT_ASKPASS": str(askpass),
            "GIT_TERMINAL_PROMPT": "0",
            "GIT_CONFIG_NOSYSTEM": "1",
        }
    )

    health_status, health = http_json(f"{args.registry.rstrip('/')}/healthz")
    if health_status != 200 or health.get("ok") is not True:
        raise RuntimeError(f"registry health check failed: HTTP {health_status} {health}")
    registry = args.registry.rstrip("/")
    claim_org(registry, "streempilot")
    claim_org(registry, "hypesiege")

    evidence = {"schema_version": 2, "registry": registry, "packages": []}
    for producer in PRODUCERS:
        evidence["packages"].append(
            publish_one(
                producer,
                work=root,
                registry=registry,
                zed_bin=args.zed_bin,
                git_env=git_env,
            )
        )

    verify_install_graph(work=root, registry=registry, zed_bin=args.zed_bin, lock_out=args.lock_out)
    args.evidence_out.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print("VERIFIED_DEN2050_LIVE_ZED_GRAPH 6/6")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
