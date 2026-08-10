#!/usr/bin/env python3
"""Provision exactly shared-auth/shared-auth-web-server.js from one green canary artifact.

This is intentionally a one-repository, one-artifact workflow. It refuses target drift,
unsafe archives, unverified workflow evidence, divergent existing content, and
secret-bearing candidate files.
"""
from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import tarfile
import tempfile
from typing import Any, Iterable, NoReturn
import urllib.error
import urllib.parse
import urllib.request
import zipfile

ALLOWED_SCHEMA = "oresoftware.provision-repository/v1"
ALLOWED_OWNER = "shared-auth"
ALLOWED_REPOSITORY = "shared-auth-web-server.js"
ALLOWED_TARGET = f"{ALLOWED_OWNER}/{ALLOWED_REPOSITORY}"
ALLOWED_CANARY_REPOSITORY = "shared-auth-test/contract-conformance-tests"
ALLOWED_CANARY_PR = 6
ALLOWED_CANARY_COMMIT = "252cbda966081d902637fded7adf51d949b919cd"
ALLOWED_WORKFLOW_RUN_ID = 31442332258
ALLOWED_ARTIFACT_ID = 5866515977
ALLOWED_ARTIFACT_NAME = "shared-auth-web-server-candidate-v1"
ALLOWED_CANDIDATE_SCHEMA = "shared-auth-web-server-candidate/v1"
ALLOWED_SOURCE_SEED_COMMIT = "67a3b5138a4050a23a409094ef094b050bb162fd"
ALLOWED_SOURCE_SEED_ARCHIVE_SHA256 = (
    "095c5e0c464aae73b85f399614c0ad11be1acfb67fd2a40a4da4ee1da83cc848"
)
ALLOWED_REPAIR = "remove duplicate plain axum dependency"
MAX_ARCHIVE_ENTRIES = 200
MAX_FILE_BYTES = 16 * 1024 * 1024
MAX_TOTAL_BYTES = 64 * 1024 * 1024
REQUIRED_FILES = {
    ".env.example",
    ".github/workflows/ci.yml",
    ".sops.yaml",
    ".zpkg.toml",
    "Cargo.lock",
    "Cargo.toml",
    "Dockerfile",
    "README.md",
    "env/dec/README.md",
    "env/enc/README.md",
    "flake.nix",
    "justfile",
    "repository.json",
    "src/main.rs",
}
FORBIDDEN_SECRET = re.compile(
    rb"(?i)(ghp_[A-Za-z0-9]{20,}|lin_api_[A-Za-z0-9]{20,}|"
    rb"sk-(?:svcacct-|ant-api\d*-)?[A-Za-z0-9_-]{20,}|"
    rb"BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY|"
    rb"(?:face|fingerprint|thumbprint)[_-]?(?:image|template|embedding)\s*[:=]\s*[^<\s]{8,})"
)


class ProvisioningError(RuntimeError):
    pass


def fail(message: str) -> NoReturn:
    raise ProvisioningError(message)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def git_blob_sha(data: bytes) -> str:
    header = f"blob {len(data)}\0".encode("ascii")
    return hashlib.sha1(header + data).hexdigest()  # noqa: S324 - Git object identity


def safe_relative_path(name: str) -> PurePosixPath:
    if not name or "\x00" in name or "\\" in name:
        fail(f"unsafe archive path: {name!r}")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        fail(f"unsafe archive path: {name!r}")
    return path


class SafeGithubRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Allow HTTPS redirects while stripping GitHub credentials cross-host."""

    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: Any,
        code: int,
        msg: str,
        headers: Any,
        newurl: str,
    ) -> urllib.request.Request | None:
        parsed = urllib.parse.urlparse(newurl)
        if parsed.scheme != "https":
            fail(f"refusing non-HTTPS artifact redirect to {parsed.scheme or 'unknown'}")
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is None:
            return None
        old_host = urllib.parse.urlparse(req.full_url).hostname
        new_host = parsed.hostname
        if old_host != new_host:
            for header in ("Authorization", "X-GitHub-Api-Version"):
                redirected.remove_header(header)
                redirected.headers.pop(header, None)
                redirected.unredirected_hdrs.pop(header, None)
        return redirected


@dataclass(frozen=True)
class CandidateFile:
    path: str
    data: bytes
    mode: str
    sha256: str


@dataclass(frozen=True)
class Candidate:
    manifest: dict[str, Any]
    files: tuple[CandidateFile, ...]
    archive_sha256: str


class Github:
    def __init__(self, token: str):
        if not token or token != token.strip():
            fail("GitHub token is missing or contains surrounding whitespace")
        self._token = token
        self._opener = urllib.request.build_opener(SafeGithubRedirectHandler())

    def _request(
        self,
        method: str,
        url: str,
        data: Any | None = None,
        expected: Iterable[int] = (200,),
    ) -> tuple[int, bytes, dict[str, str]]:
        if urllib.parse.urlparse(url).scheme != "https":
            fail("GitHub API requests must use HTTPS")
        body = None if data is None else json.dumps(data, separators=(",", ":")).encode("utf-8")
        headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {self._token}",
            "User-Agent": "oresoftware-exact-repository-provisioner/1",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        if body is not None:
            headers["Content-Type"] = "application/json"
        request = urllib.request.Request(url, data=body, headers=headers, method=method)
        try:
            with self._opener.open(request, timeout=60) as response:
                status = int(response.status)
                payload = response.read()
                response_headers = dict(response.headers.items())
        except urllib.error.HTTPError as error:
            status = int(error.code)
            payload = error.read()
            response_headers = dict(error.headers.items()) if error.headers else {}
        if status not in set(expected):
            snippet = payload[:500].decode("utf-8", errors="replace")
            fail(
                f"GitHub API {method} {urllib.parse.urlparse(url).path} "
                f"returned {status}: {snippet}"
            )
        return status, payload, response_headers

    def api(
        self,
        method: str,
        path: str,
        data: Any | None = None,
        expected: Iterable[int] = (200,),
    ) -> tuple[int, Any | None]:
        if not path.startswith("/"):
            fail("GitHub API path must start with /")
        status, payload, _headers = self._request(
            method,
            f"https://api.github.com{path}",
            data=data,
            expected=expected,
        )
        if not payload:
            return status, None
        try:
            return status, json.loads(payload.decode("utf-8"))
        except json.JSONDecodeError as error:
            fail(f"GitHub API returned invalid JSON for {path}: {error}")

    def download_artifact(self, repository: str, artifact_id: int) -> bytes:
        url = f"https://api.github.com/repos/{repository}/actions/artifacts/{artifact_id}/zip"
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme != "https" or parsed.hostname != "api.github.com":
            fail("artifact download must start at api.github.com over HTTPS")
        _status, payload, _headers = self._request("GET", url, expected=(200,))
        if len(payload) > MAX_TOTAL_BYTES:
            fail("artifact ZIP exceeds the maximum compressed size")
        return payload


def load_request(path: Path) -> dict[str, Any]:
    try:
        request = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load request: {error}")
    if request.get("schema") != ALLOWED_SCHEMA:
        fail("unexpected request schema")
    target = request.get("target") or {}
    if target.get("owner") != ALLOWED_OWNER or target.get("repository") != ALLOWED_REPOSITORY:
        fail("request target is not the one allowed repository")
    if target.get("private") is not True:
        fail("the Shared Auth administrative server must be private")
    description = target.get("description")
    if not isinstance(description, str) or not 20 <= len(description) <= 350:
        fail("target description must be a bounded non-empty string")
    canary = request.get("canary") or {}
    exact_canary = {
        "repository": ALLOWED_CANARY_REPOSITORY,
        "pull_request": ALLOWED_CANARY_PR,
        "commit": ALLOWED_CANARY_COMMIT,
        "workflow_run_id": ALLOWED_WORKFLOW_RUN_ID,
        "artifact_id": ALLOWED_ARTIFACT_ID,
        "artifact_name": ALLOWED_ARTIFACT_NAME,
    }
    if canary != exact_canary:
        fail("canary evidence does not match the reviewed exact head/run/artifact")
    candidate = request.get("candidate") or {}
    exact_candidate = {
        "schema": ALLOWED_CANDIDATE_SCHEMA,
        "repository": ALLOWED_TARGET,
        "source_seed_commit": ALLOWED_SOURCE_SEED_COMMIT,
        "source_seed_archive_sha256": ALLOWED_SOURCE_SEED_ARCHIVE_SHA256,
        "repair": ALLOWED_REPAIR,
    }
    if candidate != exact_candidate:
        fail("candidate identity does not match the reviewed corrected seed")
    settings = request.get("repository_settings") or {}
    if settings.get("default_branch") != "main":
        fail("default branch must be main")
    approvals = settings.get("required_approvals")
    if not isinstance(approvals, int) or approvals < 1 or approvals > 6:
        fail("required approvals must be between 1 and 6")
    topics = settings.get("topics")
    if not isinstance(topics, list) or not topics or len(topics) > 20:
        fail("topics must be a non-empty bounded list")
    if len(set(topics)) != len(topics):
        fail("repository topics must be unique")
    if any(
        not isinstance(topic, str) or not re.fullmatch(r"[a-z0-9-]{1,50}", topic)
        for topic in topics
    ):
        fail("repository topics contain an invalid value")
    if request.get("execute") not in {True, False}:
        fail("execute must be a boolean")
    return request


def safe_extract_zip(data: bytes, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=False)
    seen: set[str] = set()
    total = 0
    try:
        archive = zipfile.ZipFile(io.BytesIO(data))
    except zipfile.BadZipFile as error:
        fail(f"invalid artifact ZIP: {error}")
    infos = archive.infolist()
    if len(infos) > MAX_ARCHIVE_ENTRIES:
        fail("artifact ZIP has too many entries")
    for info in infos:
        if info.flag_bits & 0x1:
            fail(f"encrypted ZIP entry is not allowed: {info.filename}")
        path = safe_relative_path(info.filename.rstrip("/"))
        normalized = path.as_posix()
        if normalized in seen:
            fail(f"duplicate ZIP entry: {normalized}")
        seen.add(normalized)
        mode = (info.external_attr >> 16) & 0xFFFF
        file_type = stat.S_IFMT(mode)
        if file_type not in {0, stat.S_IFREG, stat.S_IFDIR}:
            fail(f"non-regular ZIP entry is not allowed: {normalized}")
        if info.is_dir():
            (destination / normalized).mkdir(parents=True, exist_ok=True)
            continue
        if info.file_size > MAX_FILE_BYTES:
            fail(f"artifact file exceeds size limit: {normalized}")
        total += info.file_size
        if total > MAX_TOTAL_BYTES:
            fail("artifact ZIP exceeds total decompressed size limit")
        output = (destination / normalized).resolve()
        if destination.resolve() not in output.parents:
            fail(f"ZIP path escaped destination: {normalized}")
        output.parent.mkdir(parents=True, exist_ok=True)
        written = 0
        with archive.open(info, "r") as source, output.open("xb") as sink:
            while chunk := source.read(1024 * 1024):
                written += len(chunk)
                if written > info.file_size or written > MAX_FILE_BYTES:
                    fail(f"ZIP entry size mismatch: {normalized}")
                sink.write(chunk)
        if written != info.file_size:
            fail(f"ZIP entry size mismatch: {normalized}")


def extract_candidate(
    artifact_zip: bytes,
    work: Path,
    expected: dict[str, Any],
) -> Candidate:
    artifact_dir = work / "artifact"
    safe_extract_zip(artifact_zip, artifact_dir)
    manifests = list(artifact_dir.rglob("shared-auth-web-server-candidate.json"))
    archives = list(artifact_dir.rglob("shared-auth-web-server-candidate.tar.gz"))
    if len(manifests) != 1 or len(archives) != 1:
        fail("artifact must contain exactly one candidate manifest and archive")
    try:
        manifest = json.loads(manifests[0].read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"invalid candidate manifest: {error}")
    for key, value in expected.items():
        if manifest.get(key) != value:
            fail(f"candidate manifest field {key!r} does not match reviewed value")
    archive_bytes = archives[0].read_bytes()
    archive_meta = manifest.get("archive") or {}
    archive_sha = sha256_bytes(archive_bytes)
    if archive_meta.get("name") != "shared-auth-web-server-candidate.tar.gz":
        fail("unexpected candidate archive name")
    if archive_meta.get("sha256") != archive_sha or archive_meta.get("bytes") != len(archive_bytes):
        fail("candidate archive digest or byte count does not match manifest")
    declared_files = manifest.get("files")
    if not isinstance(declared_files, list) or not (
        len(REQUIRED_FILES) <= len(declared_files) <= MAX_ARCHIVE_ENTRIES
    ):
        fail("candidate manifest has an invalid file list")
    declared: dict[str, dict[str, Any]] = {}
    for item in declared_files:
        if not isinstance(item, dict):
            fail("candidate file manifest entry is not an object")
        path = safe_relative_path(str(item.get("path", ""))).as_posix()
        if path in declared:
            fail(f"duplicate candidate manifest path: {path}")
        if not re.fullmatch(r"[0-9a-f]{64}", str(item.get("sha256", ""))):
            fail(f"invalid candidate file digest: {path}")
        if item.get("mode") not in {"0644", "0755"}:
            fail(f"invalid candidate mode: {path}")
        if not isinstance(item.get("bytes"), int) or not 0 <= item["bytes"] <= MAX_FILE_BYTES:
            fail(f"invalid candidate byte count: {path}")
        declared[path] = item
    if manifest.get("file_count") != len(declared):
        fail("candidate file count does not match manifest")
    if not REQUIRED_FILES <= set(declared):
        fail(f"candidate is missing required files: {sorted(REQUIRED_FILES - set(declared))}")

    actual: dict[str, CandidateFile] = {}
    total = 0
    try:
        tar = tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:gz")
    except tarfile.TarError as error:
        fail(f"invalid candidate tar archive: {error}")
    members = tar.getmembers()
    if len(members) > MAX_ARCHIVE_ENTRIES:
        fail("candidate tar has too many entries")
    for member in members:
        path = safe_relative_path(member.name).as_posix()
        if path in actual:
            fail(f"duplicate candidate tar path: {path}")
        if not member.isreg():
            fail(f"candidate tar contains a non-regular entry: {path}")
        if member.size > MAX_FILE_BYTES:
            fail(f"candidate tar entry exceeds size limit: {path}")
        total += member.size
        if total > MAX_TOTAL_BYTES:
            fail("candidate tar exceeds total size limit")
        source = tar.extractfile(member)
        if source is None:
            fail(f"cannot read candidate tar entry: {path}")
        data = source.read(MAX_FILE_BYTES + 1)
        if len(data) != member.size:
            fail(f"candidate tar entry size mismatch: {path}")
        mode = "0755" if member.mode & 0o111 else "0644"
        actual[path] = CandidateFile(path, data, mode, sha256_bytes(data))
    if set(actual) != set(declared):
        fail("candidate archive paths do not match manifest paths")
    for path, file in actual.items():
        item = declared[path]
        if (file.sha256, len(file.data), file.mode) != (
            item["sha256"],
            item["bytes"],
            item["mode"],
        ):
            fail(f"candidate file does not match manifest: {path}")
        if FORBIDDEN_SECRET.search(file.data):
            fail(f"candidate contains a forbidden secret/private-biometric marker: {path}")
    if manifest.get("uncompressed_bytes") != total:
        fail("candidate uncompressed byte count does not match manifest")
    lock = actual["Cargo.lock"]
    if manifest.get("cargo_lock_sha256") != lock.sha256:
        fail("Cargo.lock digest does not match candidate manifest")
    cargo = actual["Cargo.toml"].data.decode("utf-8", errors="strict")
    if (
        cargo.count('axum = { version = "0.7", features = ["macros"] }') != 1
        or 'axum = "0.7"' in cargo
    ):
        fail("the reviewed duplicate-Axum repair is not present")
    return Candidate(manifest, tuple(actual[path] for path in sorted(actual)), archive_sha)


def verify_canary(github: Github, request: dict[str, Any]) -> None:
    canary = request["canary"]
    _status, pull = github.api(
        "GET",
        f"/repos/{canary['repository']}/pulls/{canary['pull_request']}",
    )
    if not isinstance(pull, dict) or not pull.get("merged_at"):
        fail("canary pull request is not merged")
    if ((pull.get("head") or {}).get("sha")) != canary["commit"]:
        fail("merged canary pull request head does not match the pinned commit")
    _status, run = github.api(
        "GET",
        f"/repos/{canary['repository']}/actions/runs/{canary['workflow_run_id']}",
    )
    if not isinstance(run, dict):
        fail("canary workflow run response is invalid")
    if (
        run.get("head_sha") != canary["commit"]
        or run.get("status") != "completed"
        or run.get("conclusion") != "success"
    ):
        fail("pinned canary workflow run is not a completed success for the exact commit")
    _status, artifacts = github.api(
        "GET",
        f"/repos/{canary['repository']}/actions/runs/{canary['workflow_run_id']}/artifacts?per_page=100",
    )
    matches = [
        item
        for item in (artifacts or {}).get("artifacts", [])
        if item.get("id") == canary["artifact_id"]
        and item.get("name") == canary["artifact_name"]
    ]
    if len(matches) != 1 or matches[0].get("expired") is not False:
        fail("pinned canary artifact is missing, duplicated, or expired")


def candidate_tree(candidate: Candidate) -> dict[str, tuple[str, str]]:
    return {
        file.path: (
            "100755" if file.mode == "0755" else "100644",
            git_blob_sha(file.data),
        )
        for file in candidate.files
    }


def remote_main_tree(github: Github) -> tuple[str | None, dict[str, tuple[str, str]]]:
    status, ref = github.api(
        "GET",
        f"/repos/{ALLOWED_TARGET}/git/ref/heads/main",
        expected=(200, 404, 409),
    )
    if status != 200:
        return None, {}
    commit_sha = ref["object"]["sha"]
    _status, commit = github.api("GET", f"/repos/{ALLOWED_TARGET}/git/commits/{commit_sha}")
    tree_sha = commit["tree"]["sha"]
    _status, tree = github.api(
        "GET",
        f"/repos/{ALLOWED_TARGET}/git/trees/{tree_sha}?recursive=1",
    )
    if tree.get("truncated"):
        fail("target repository tree response was truncated")
    entries = {
        item["path"]: (item["mode"], item["sha"])
        for item in tree.get("tree", [])
        if item.get("type") == "blob"
    }
    return commit_sha, entries


def ensure_repository(github: Github, request: dict[str, Any]) -> bool:
    status, repo = github.api("GET", f"/repos/{ALLOWED_TARGET}", expected=(200, 404))
    created = False
    if status == 404:
        target = request["target"]
        _status, repo = github.api(
            "POST",
            f"/orgs/{ALLOWED_OWNER}/repos",
            {
                "name": ALLOWED_REPOSITORY,
                "description": target["description"],
                "private": True,
                "has_issues": True,
                "has_projects": True,
                "has_wiki": False,
                "is_template": False,
                "auto_init": False,
                "delete_branch_on_merge": True,
            },
            expected=(201,),
        )
        created = True
    if not isinstance(repo, dict) or repo.get("full_name") != ALLOWED_TARGET:
        fail("GitHub returned an unexpected target repository")
    if repo.get("owner", {}).get("login") != ALLOWED_OWNER:
        fail("target repository owner does not match the allowed organization")
    if not created and repo.get("private") is not True:
        fail("an existing target repository must already be private")
    if repo.get("archived") or repo.get("disabled"):
        fail("target repository is archived or disabled")
    return created


def publish_candidate(github: Github, candidate: Candidate) -> str:
    expected = candidate_tree(candidate)
    existing_commit, existing = remote_main_tree(github)
    if existing_commit is not None:
        if existing != expected:
            fail("target repository main branch already exists with a divergent tree")
        return existing_commit
    tree_entries = []
    for file in candidate.files:
        _status, blob = github.api(
            "POST",
            f"/repos/{ALLOWED_TARGET}/git/blobs",
            {
                "content": base64.b64encode(file.data).decode("ascii"),
                "encoding": "base64",
            },
            expected=(201,),
        )
        expected_blob = git_blob_sha(file.data)
        if blob.get("sha") != expected_blob:
            fail(f"GitHub blob identity mismatch for {file.path}")
        tree_entries.append(
            {
                "path": file.path,
                "mode": "100755" if file.mode == "0755" else "100644",
                "type": "blob",
                "sha": blob["sha"],
            }
        )
    _status, tree = github.api(
        "POST",
        f"/repos/{ALLOWED_TARGET}/git/trees",
        {"tree": tree_entries},
        expected=(201,),
    )
    _status, commit = github.api(
        "POST",
        f"/repos/{ALLOWED_TARGET}/git/commits",
        {
            "message": "feat: initialize verified Shared Auth administrative web server",
            "tree": tree["sha"],
            "parents": [],
        },
        expected=(201,),
    )
    github.api(
        "POST",
        f"/repos/{ALLOWED_TARGET}/git/refs",
        {"ref": "refs/heads/main", "sha": commit["sha"]},
        expected=(201,),
    )
    _commit, actual = remote_main_tree(github)
    if actual != expected:
        fail("target repository tree differs after initial publication")
    return commit["sha"]


def configure_repository(github: Github, request: dict[str, Any]) -> None:
    settings = request["repository_settings"]
    github.api(
        "PATCH",
        f"/repos/{ALLOWED_TARGET}",
        {
            "description": request["target"]["description"],
            "private": True,
            "default_branch": "main",
            "has_issues": True,
            "has_projects": True,
            "has_wiki": False,
            "allow_merge_commit": False,
            "allow_squash_merge": True,
            "allow_rebase_merge": True,
            "delete_branch_on_merge": True,
            "web_commit_signoff_required": True,
        },
    )
    github.api("PUT", f"/repos/{ALLOWED_TARGET}/topics", {"names": settings["topics"]})
    github.api("PUT", f"/repos/{ALLOWED_TARGET}/vulnerability-alerts", expected=(204,))
    github.api(
        "PUT",
        f"/repos/{ALLOWED_TARGET}/automated-security-fixes",
        expected=(204, 422),
    )
    github.api(
        "PUT",
        f"/repos/{ALLOWED_TARGET}/branches/main/protection",
        {
            "required_status_checks": None,
            "enforce_admins": True,
            "required_pull_request_reviews": {
                "dismiss_stale_reviews": True,
                "require_code_owner_reviews": False,
                "required_approving_review_count": settings["required_approvals"],
                "require_last_push_approval": True,
            },
            "restrictions": None,
            "required_linear_history": True,
            "allow_force_pushes": False,
            "allow_deletions": False,
            "block_creations": False,
            "required_conversation_resolution": True,
            "lock_branch": False,
            "allow_fork_syncing": True,
        },
    )


def verify_final(github: Github, candidate: Candidate, request: dict[str, Any]) -> dict[str, Any]:
    _status, repo = github.api("GET", f"/repos/{ALLOWED_TARGET}")
    if repo.get("private") is not True or repo.get("default_branch") != "main":
        fail("target repository privacy/default branch verification failed")
    if repo.get("allow_merge_commit") is not False:
        fail("merge commits remain enabled on the target repository")
    if repo.get("web_commit_signoff_required") is not True:
        fail("web commit signoff is not required")
    commit, tree = remote_main_tree(github)
    if commit is None or tree != candidate_tree(candidate):
        fail("target repository final tree verification failed")
    _status, topics = github.api("GET", f"/repos/{ALLOWED_TARGET}/topics")
    if set((topics or {}).get("names", [])) != set(request["repository_settings"]["topics"]):
        fail("target repository topics do not match the request")
    _status, protection = github.api(
        "GET",
        f"/repos/{ALLOWED_TARGET}/branches/main/protection",
    )
    if not protection.get("enforce_admins", {}).get("enabled"):
        fail("main branch admin protection is not enabled")
    if not protection.get("required_linear_history", {}).get("enabled"):
        fail("main branch linear-history protection is not enabled")
    if not protection.get("required_conversation_resolution", {}).get("enabled"):
        fail("main branch conversation resolution is not required")
    reviews = protection.get("required_pull_request_reviews") or {}
    if reviews.get("required_approving_review_count", 0) < 1:
        fail("main branch does not require an approving review")
    if reviews.get("dismiss_stale_reviews") is not True:
        fail("main branch does not dismiss stale reviews")
    if reviews.get("require_last_push_approval") is not True:
        fail("main branch does not require last-push approval")
    if protection.get("allow_force_pushes", {}).get("enabled"):
        fail("main branch allows force pushes")
    if protection.get("allow_deletions", {}).get("enabled"):
        fail("main branch allows deletion")
    return {
        "repository": ALLOWED_TARGET,
        "commit": commit,
        "candidate_archive_sha256": candidate.archive_sha256,
        "cargo_lock_sha256": candidate.manifest["cargo_lock_sha256"],
        "files": len(candidate.files),
        "private": True,
        "default_branch": "main",
        "branch_protection": True,
    }


def read_bounded_file(path: Path) -> bytes:
    try:
        size = path.stat().st_size
    except OSError as error:
        fail(f"cannot inspect artifact file: {error}")
    if size > MAX_TOTAL_BYTES:
        fail("artifact ZIP exceeds the maximum compressed size")
    try:
        return path.read_bytes()
    except OSError as error:
        fail(f"cannot read artifact file: {error}")


def run(request_path: Path, validate_only: bool, artifact_path: Path | None) -> int:
    request = load_request(request_path)
    print(
        f"validated exact request for {ALLOWED_TARGET}; "
        f"execute={str(request['execute']).lower()} "
        f"canary={ALLOWED_CANARY_COMMIT[:12]} run={ALLOWED_WORKFLOW_RUN_ID}"
    )
    if artifact_path is not None:
        with tempfile.TemporaryDirectory(prefix="shared-auth-candidate-") as directory:
            candidate = extract_candidate(
                read_bounded_file(artifact_path),
                Path(directory),
                request["candidate"],
            )
            print(
                f"validated candidate artifact files={len(candidate.files)} "
                f"archive_sha256={candidate.archive_sha256}"
            )
    if validate_only or not request["execute"]:
        return 0
    token = os.environ.get("GITHUB_TOKEN", "")
    github = Github(token)
    verify_canary(github, request)
    artifact = github.download_artifact(
        request["canary"]["repository"],
        request["canary"]["artifact_id"],
    )
    with tempfile.TemporaryDirectory(prefix="shared-auth-provision-") as directory:
        candidate = extract_candidate(artifact, Path(directory), request["candidate"])
        ensure_repository(github, request)
        commit = publish_candidate(github, candidate)
        configure_repository(github, request)
        result = verify_final(github, candidate, request)
        if result["commit"] != commit:
            fail("published commit changed during final verification")
        print(json.dumps(result, sort_keys=True))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--validate-only", action="store_true")
    parser.add_argument("--artifact", type=Path)
    args = parser.parse_args()
    try:
        return run(args.request, args.validate_only, args.artifact)
    except ProvisioningError as error:
        print(f"provisioning refused: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
