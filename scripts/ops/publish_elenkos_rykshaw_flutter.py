#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

TARGET = "elenkos-systems/elenkos-rykshaw-flutter"
BRANCH = "alexanderdmills/den-3837-build-rykshaw-volunteer-qa-portal-and-non-custodial-reward"
TITLE = "feat(DEN-3837): bootstrap Rykshaw Flutter volunteer QA beta"
DESCRIPTION = (
    "Volunteer-facing Flutter QA portal with non-custodial sandbox rewards, "
    "optional approximate location, and Elenkos Rust API contracts"
)


class CommandError(RuntimeError):
    pass


def run(
    *args: str,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        list(args),
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    if check and completed.returncode != 0:
        safe = " ".join(args)
        raise CommandError(
            f"command failed ({completed.returncode}): {safe}\n"
            f"stdout:\n{completed.stdout[-4000:]}\n"
            f"stderr:\n{completed.stderr[-4000:]}"
        )
    return completed


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def gh_json(env: dict[str, str], *args: str, allow_failure: bool = False) -> Any:
    completed = run("gh", *args, env=env, check=not allow_failure)
    if completed.returncode != 0:
        return None
    text = completed.stdout.strip()
    return json.loads(text) if text else None


def ensure_private_repository(env: dict[str, str]) -> tuple[dict[str, Any], bool]:
    repo = gh_json(env, "api", f"repos/{TARGET}", allow_failure=True)
    created = False
    if repo is None:
        repo = gh_json(
            env,
            "api",
            "--method",
            "POST",
            "orgs/elenkos-systems/repos",
            "-f",
            "name=elenkos-rykshaw-flutter",
            "-f",
            f"description={DESCRIPTION}",
            "-F",
            "private=true",
            "-F",
            "auto_init=true",
            "-f",
            "license_template=mit",
        )
        created = True
        for _ in range(30):
            repo = gh_json(env, "api", f"repos/{TARGET}", allow_failure=True)
            if repo is not None:
                break
            time.sleep(2)
    if repo is None:
        raise RuntimeError("target repository was not created or could not be read")
    if repo.get("visibility") != "private" or repo.get("private") is not True:
        raise RuntimeError("refusing to publish: target repository is not private")
    if repo.get("default_branch") != "main":
        raise RuntimeError(f"unexpected default branch: {repo.get('default_branch')!r}")
    return repo, created


def copy_source(source: Path, destination: Path) -> None:
    for child in destination.iterdir():
        if child.name == ".git":
            continue
        if child.is_dir() and not child.is_symlink():
            shutil.rmtree(child)
        else:
            child.unlink()
    for child in source.iterdir():
        if child.name in {".git", ".dart_tool", "build", "coverage"}:
            continue
        target = destination / child.name
        if child.is_symlink():
            raise RuntimeError(f"source contains disallowed symlink: {child}")
        if child.is_dir():
            shutil.copytree(child, target, symlinks=False)
        else:
            shutil.copy2(child, target)


def existing_pr(env: dict[str, str]) -> dict[str, Any] | None:
    values = gh_json(
        env,
        "pr",
        "list",
        "--repo",
        TARGET,
        "--state",
        "all",
        "--head",
        BRANCH,
        "--json",
        "number,state,url,headRefOid,isDraft,title",
    )
    if not values:
        return None
    if len(values) != 1:
        raise RuntimeError(f"expected at most one PR for {BRANCH}, found {len(values)}")
    return values[0]


def publish(
    *,
    token_file: Path,
    source: Path,
    archive: Path,
    expected_archive_sha: str,
    trusted_k8s_sha: str,
    apk: Path,
    evidence_out: Path,
) -> dict[str, Any]:
    mode = stat.S_IMODE(token_file.stat().st_mode)
    if mode != 0o600:
        raise RuntimeError(f"token file mode must be 0600, got {mode:04o}")
    token = token_file.read_text().strip()
    if len(token) < 20 or any(ch.isspace() for ch in token):
        raise RuntimeError("token file is malformed")
    if not source.is_dir() or not (source / "pubspec.yaml").is_file():
        raise RuntimeError("validated Flutter source directory is missing")
    if not apk.is_file() or apk.stat().st_size < 100_000:
        raise RuntimeError("validated Android APK is missing or implausibly small")
    actual_archive_sha = sha256_file(archive)
    if actual_archive_sha != expected_archive_sha:
        raise RuntimeError(
            f"source archive digest mismatch: expected {expected_archive_sha}, got {actual_archive_sha}"
        )
    if len(trusted_k8s_sha) != 40:
        raise RuntimeError("trusted k8s-cluster SHA is malformed")

    env = os.environ.copy()
    env["GH_TOKEN"] = token
    env["GITHUB_TOKEN"] = token
    env["GIT_TERMINAL_PROMPT"] = "0"

    identity = gh_json(env, "api", "user")
    if identity.get("login") != "ORESoftware":
        raise RuntimeError(f"unexpected GitHub identity: {identity.get('login')!r}")

    repo, created = ensure_private_repository(env)
    run("gh", "auth", "setup-git", env=env)

    prior_pr = existing_pr(env)
    remote_branch = gh_json(
        env,
        "api",
        f"repos/{TARGET}/git/ref/heads/{BRANCH}",
        allow_failure=True,
    )
    if remote_branch is not None:
        head_sha = remote_branch["object"]["sha"]
        commit = gh_json(env, "api", f"repos/{TARGET}/git/commits/{head_sha}")
        message = commit.get("message", "")
        trailer = f"Source-Archive-SHA256: {actual_archive_sha}"
        if trailer not in message:
            raise RuntimeError(
                "target feature branch already exists with a different provenance; refusing to overwrite"
            )
        if prior_pr is None:
            raise RuntimeError("matching branch exists but no pull request was found")
        result = {
            "schema_version": 1,
            "status": "no-op",
            "repository": TARGET,
            "repository_url": repo["html_url"],
            "repository_id": repo["id"],
            "repository_created": created,
            "visibility": repo["visibility"],
            "branch": BRANCH,
            "commit_sha": head_sha,
            "pull_request": prior_pr,
            "source_archive_sha256": actual_archive_sha,
            "trusted_k8s_cluster_sha": trusted_k8s_sha,
            "apk_sha256": sha256_file(apk),
            "validated": True,
        }
        evidence_out.write_text(json.dumps(result, indent=2) + "\n")
        return result

    with tempfile.TemporaryDirectory(prefix="rykshaw-publish-") as temporary:
        checkout = Path(temporary) / "checkout"
        run("git", "clone", "--quiet", f"https://github.com/{TARGET}.git", str(checkout), env=env)
        run("git", "config", "user.name", "ORESoftware protected publisher", cwd=checkout)
        run(
            "git",
            "config",
            "user.email",
            "41898282+github-actions[bot]@users.noreply.github.com",
            cwd=checkout,
        )
        run("git", "fetch", "origin", "main", "--quiet", cwd=checkout, env=env)
        run("git", "switch", "-c", BRANCH, "origin/main", cwd=checkout)
        copy_source(source, checkout)

        provenance = checkout / "docs/source-provenance.json"
        provenance.parent.mkdir(parents=True, exist_ok=True)
        provenance.write_text(
            json.dumps(
                {
                    "schemaVersion": 1,
                    "trackingIssue": "DEN-3837",
                    "sourceArchiveSha256": actual_archive_sha,
                    "trustedK8sClusterSha": trusted_k8s_sha,
                    "flutterVersion": "3.47.0",
                    "validation": [
                        "secret-scan",
                        "contract-parse",
                        "dart-format",
                        "flutter-analyze",
                        "flutter-test",
                        "android-debug-apk",
                    ],
                },
                indent=2,
            )
            + "\n"
        )

        run("bash", "tool/check_no_secrets.sh", cwd=checkout)
        run("python3", "tool/static_validate.py", cwd=checkout)
        run("git", "add", "--all", cwd=checkout)
        run("git", "diff", "--cached", "--check", cwd=checkout)
        staged = run("git", "diff", "--cached", "--name-only", cwd=checkout).stdout.splitlines()
        if len(staged) < 60:
            raise RuntimeError(f"unexpectedly small publication payload: {len(staged)} files")
        message = (
            "feat(DEN-3837): bootstrap Rykshaw Flutter beta\n\n"
            "Publish the volunteer-facing QA portal with fail-closed worker-classification "
            "and non-custodial reward boundaries.\n\n"
            f"Source-Archive-SHA256: {actual_archive_sha}\n"
            f"Trusted-K8s-Cluster-SHA: {trusted_k8s_sha}"
        )
        run("git", "commit", "--quiet", "-m", message, cwd=checkout)
        head_sha = run("git", "rev-parse", "HEAD", cwd=checkout).stdout.strip()
        run("git", "push", "--quiet", "--set-upstream", "origin", BRANCH, cwd=checkout, env=env)

    body = f"""## Summary

Bootstraps the Rykshaw Flutter beta for optional Elenkos community QA.

- versioned participation acknowledgement without waiver or classification claims;
- fail-closed paid-engagement escalation for scheduled, quota-driven, productive, or reward-linked work;
- optional one-shot approximate foreground location;
- QA assignments, reports, participation notes, and sandbox reward intents;
- separate Elenkos read/write API, web, Leptos, Dioxus, and WebSocket configuration;
- declarative allow-listed remote experience manifests only;
- Flutter 3.47.0 CI plus Android/iOS beta artifact workflows.

## Validation

The protected publisher ran Dart formatting, `flutter analyze`, unit/widget tests, contract validation, credential scanning, and an Android debug APK build before pushing this exact head.

## Boundaries

No production payments, wallet custody, stored value, FX, remittance, direct P2P transfer, external tester enrollment, store-term acceptance, or spend is activated.

Linear: DEN-3837

Source archive SHA-256: `{actual_archive_sha}`
Trusted publisher SHA: `{trusted_k8s_sha}`
"""
    prior_pr = existing_pr(env)
    if prior_pr is None:
        with tempfile.NamedTemporaryFile("w", delete=False, prefix="rykshaw-pr-", suffix=".md") as handle:
            handle.write(body)
            body_path = Path(handle.name)
        try:
            url = run(
                "gh",
                "pr",
                "create",
                "--repo",
                TARGET,
                "--base",
                "main",
                "--head",
                BRANCH,
                "--title",
                TITLE,
                "--body-file",
                str(body_path),
                env=env,
            ).stdout.strip()
        finally:
            body_path.unlink(missing_ok=True)
        prior_pr = existing_pr(env)
        if prior_pr is None or prior_pr.get("url") != url:
            raise RuntimeError("pull request creation could not be verified")
    if prior_pr.get("headRefOid") != head_sha:
        raise RuntimeError("pull request head does not match the validated pushed commit")

    result = {
        "schema_version": 1,
        "status": "published",
        "repository": TARGET,
        "repository_url": repo["html_url"],
        "repository_id": repo["id"],
        "repository_created": created,
        "visibility": repo["visibility"],
        "branch": BRANCH,
        "commit_sha": head_sha,
        "pull_request": prior_pr,
        "source_archive_sha256": actual_archive_sha,
        "trusted_k8s_cluster_sha": trusted_k8s_sha,
        "apk_sha256": sha256_file(apk),
        "validated": True,
    }
    evidence_out.write_text(json.dumps(result, indent=2) + "\n")
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--token-file", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--archive-sha256", required=True)
    parser.add_argument("--trusted-k8s-cluster-sha", required=True)
    parser.add_argument("--apk", required=True, type=Path)
    parser.add_argument("--evidence-out", required=True, type=Path)
    args = parser.parse_args()
    result = publish(
        token_file=args.token_file,
        source=args.source,
        archive=args.archive,
        expected_archive_sha=args.archive_sha256,
        trusted_k8s_sha=args.trusted_k8s_cluster_sha,
        apk=args.apk,
        evidence_out=args.evidence_out,
    )
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
