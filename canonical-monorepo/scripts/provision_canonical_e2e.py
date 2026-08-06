#!/usr/bin/env python3
"""Provision the Canonical source-owned E2E orchestrator and disposable test repositories."""

from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import re
import sys
import time
from typing import Any, Iterable
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "provisioning" / "canonical-e2e-repositories.json"
API_VERSION = "2022-11-28"
NAME_PATTERN = re.compile(r"^[A-Za-z0-9_.-]+$")
ALLOWED_KINDS = {"orchestrator", "staging-mirror", "scenario", "consumer"}
ALLOWED_LANGUAGES = {"python", "rust", "typescript", "go"}
REQUIRED_TEST_REPOSITORIES = {
    "canonical-api-server.rs",
    "api-server-contract-e2e",
    "monorepo-submodules-e2e",
    "zed-package-graph-e2e",
    "web-server-routing-e2e",
    "cli-install-e2e",
    "clients-rust-consumer",
    "clients-typescript-consumer",
    "clients-go-consumer",
    "clients-python-consumer",
    "mcp-contract-e2e",
    "legacy-mirror-guard-e2e",
}


class ProvisioningError(RuntimeError):
    """A fail-closed provisioning error."""


def load_manifest(path: Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ProvisioningError(f"cannot load {path}: {error}") from error
    if not isinstance(payload, dict):
        raise ProvisioningError("manifest root must be an object")
    return payload


def validate_manifest(manifest: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    source_org = manifest.get("source_org")
    test_org = manifest.get("test_org")
    if source_org != "canonical-cloud":
        errors.append("source_org must be canonical-cloud")
    if test_org != "canonical-cloud-test":
        errors.append("test_org must be canonical-cloud-test")
    if manifest.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    confirmation = manifest.get("confirmation")
    if confirmation != "provision canonical-cloud/canonical-e2e and canonical-cloud-test":
        errors.append("confirmation phrase is invalid")

    entries = manifest.get("repositories")
    if not isinstance(entries, list):
        return errors + ["repositories must be an array"]

    seen: set[str] = set()
    source_entries: list[dict[str, Any]] = []
    test_entries: list[dict[str, Any]] = []
    required_fields = {
        "owner",
        "name",
        "visibility",
        "description",
        "kind",
        "zed_package",
        "auto_init",
        "language",
        "topics",
        "source_repositories",
    }

    for index, raw in enumerate(entries):
        prefix = f"repositories[{index}]"
        if not isinstance(raw, dict):
            errors.append(f"{prefix} must be an object")
            continue
        missing = sorted(required_fields - raw.keys())
        if missing:
            errors.append(f"{prefix} is missing {missing}")

        owner, name = raw.get("owner"), raw.get("name")
        if owner not in {source_org, test_org}:
            errors.append(f"{prefix}.owner is invalid")
        if not isinstance(name, str) or not NAME_PATTERN.fullmatch(name):
            errors.append(f"{prefix}.name is invalid")
            continue
        key = f"{owner}/{name}"
        if key in seen:
            errors.append(f"duplicate repository: {key}")
        seen.add(key)

        if raw.get("visibility") not in {"public", "private"}:
            errors.append(f"{prefix}.visibility is invalid")
        if not isinstance(raw.get("description"), str) or not raw["description"].strip():
            errors.append(f"{prefix}.description is required")
        if raw.get("kind") not in ALLOWED_KINDS:
            errors.append(f"{prefix}.kind is invalid")
        if not isinstance(raw.get("zed_package"), bool):
            errors.append(f"{prefix}.zed_package must be boolean")
        if not isinstance(raw.get("auto_init"), bool):
            errors.append(f"{prefix}.auto_init must be boolean")
        if raw.get("language") not in ALLOWED_LANGUAGES:
            errors.append(f"{prefix}.language is invalid")

        topics = raw.get("topics")
        if (
            not isinstance(topics, list)
            or not topics
            or len(topics) > 20
            or any(
                not isinstance(topic, str)
                or not re.fullmatch(r"[a-z0-9-]{1,50}", topic)
                for topic in topics
            )
            or len(topics) != len(set(topics))
        ):
            errors.append(f"{prefix}.topics must be unique lowercase GitHub topics")

        sources = raw.get("source_repositories")
        if (
            not isinstance(sources, list)
            or not sources
            or any(
                not isinstance(source, str)
                or not NAME_PATTERN.fullmatch(source)
                or "/" in source
                for source in sources
            )
            or len(sources) != len(set(sources))
        ):
            errors.append(f"{prefix}.source_repositories must contain unique repository names")

        if owner == source_org:
            source_entries.append(raw)
        elif owner == test_org:
            test_entries.append(raw)

    if len(source_entries) != 1:
        errors.append("exactly one source-org repository must be declared")
    else:
        source = source_entries[0]
        if source.get("name") != "canonical-e2e":
            errors.append("the source-org repository must be canonical-e2e")
        if source.get("kind") != "orchestrator":
            errors.append("canonical-e2e must be an orchestrator")
        if source.get("visibility") != "private":
            errors.append("canonical-e2e must be private")
        if not source.get("zed_package") or not source.get("auto_init"):
            errors.append("canonical-e2e must be initialized as a Zed package")

    actual_tests = {entry.get("name") for entry in test_entries}
    if actual_tests != REQUIRED_TEST_REPOSITORIES:
        missing = sorted(REQUIRED_TEST_REPOSITORIES - actual_tests)
        extra = sorted(actual_tests - REQUIRED_TEST_REPOSITORIES)
        errors.append(f"test repository matrix drift: missing={missing}, extra={extra}")

    for entry in test_entries:
        name = entry.get("name")
        if entry.get("visibility") != "private":
            errors.append(f"{test_org}/{name} must be private")
        if name == "canonical-api-server.rs":
            if entry.get("kind") != "staging-mirror":
                errors.append("canonical-api-server.rs test repository must be a staging-mirror")
            if entry.get("auto_init") or entry.get("zed_package"):
                errors.append("the staging mirror must remain empty and non-Zed")
        elif (
            entry.get("kind") not in {"scenario", "consumer"}
            or not entry.get("auto_init")
            or not entry.get("zed_package")
        ):
            errors.append(f"{test_org}/{name} must be an initialized Zed test package")

    return errors


def repository_key(entry: dict[str, Any]) -> str:
    return f"{entry['owner']}/{entry['name']}"


def render_zpkg(entry: dict[str, Any]) -> str:
    q = lambda value: json.dumps(value, ensure_ascii=False)
    keywords = ", ".join(q(topic) for topic in entry["topics"])
    return (
        "[package]\n"
        f"org = {q(entry['owner'])}\n"
        f"name = {q(entry['name'])}\n"
        'version = "0.1.0"\n'
        f"description = {q(entry['description'])}\n"
        'license = "MIT"\n'
        f"keywords = [{keywords}]\n"
        f"language = {q(entry['language'])}\n\n"
        "[package.repository]\n"
        'vcs = "git"\n'
        f"url = {q('https://github.com/' + repository_key(entry))}\n\n"
        "[publish]\n"
        "include_readme = true\n"
        'tag_format = "v{version}"\n'
        'exclude = [".env", ".env.*", ".vendor/.zed/**", ".zed/**", '
        '".zed-pack/**", "tmp/**", "**/*.log"]\n\n'
        "[install]\n"
        'adapter = "none"\n'
        'dir = ".vendor/.zed"\n'
    )


def render_source_contract(entry: dict[str, Any]) -> str:
    return json.dumps(
        {
            "schema_version": 1,
            "repository": repository_key(entry),
            "kind": entry["kind"],
            "source_org": "canonical-cloud",
            "source_repositories": [
                f"canonical-cloud/{name}" for name in entry["source_repositories"]
            ],
            "revision_policy": "immutable-sha-or-digest",
            "production_source_allowed": False,
            "provisioned_from": (
                "canonical-cloud/canonical-monorepo/"
                "provisioning/canonical-e2e-repositories.json"
            ),
        },
        indent=2,
    ) + "\n"


def render_readme(entry: dict[str, Any]) -> str:
    sources = "\n".join(
        f"- `canonical-cloud/{name}`" for name in entry["source_repositories"]
    )
    package_note = (
        "\nThis reusable repository is a Zed package; `.zpkg.toml` and "
        "`.zpkg.lock` are managed contract files.\n"
        if entry["zed_package"]
        else ""
    )
    return f"""# {entry['name']}

{entry['description']}

This repository is managed by `canonical-cloud/canonical-monorepo`.
{package_note}
## Source-of-truth contract

- Production source is developed only in `canonical-cloud`.
- Tests consume immutable commit SHAs, release tags, or artifact/image digests.
- Every run records the source repository, exact revision, lock state, workflow run,
  and produced artifact or image digest.
- Test repositories must not become independent production implementations.
- Promotion is permitted only for the same SHA or digest that passed testing.

## Source repositories

{sources}

## Provisioning

The repository contract is declared in
`canonical-cloud/canonical-monorepo/provisioning/canonical-e2e-repositories.json`.
Provisioning is idempotent and refuses to overwrite drifted managed files.
"""


def render_contract_workflow(entry: dict[str, Any]) -> str:
    expected_zed = "true" if entry["zed_package"] else "false"
    return f"""name: repository-contract

on:
  pull_request:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

jobs:
  validate:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4
        with:
          persist-credentials: false

      - name: Validate immutable-source and Zed contracts
        env:
          EXPECTED_REPOSITORY: {repository_key(entry)}
          EXPECTED_ZED_PACKAGE: "{expected_zed}"
        shell: bash
        run: |
          set -euo pipefail
          python3 - <<'PY'
          import json
          import os
          from pathlib import Path
          import tomllib

          root = Path(".")
          contract = json.loads(
              (root / ".canonical-source-contract.json").read_text(encoding="utf-8")
          )
          assert contract["repository"] == os.environ["EXPECTED_REPOSITORY"]
          assert contract["source_org"] == "canonical-cloud"
          assert contract["revision_policy"] == "immutable-sha-or-digest"
          assert contract["production_source_allowed"] is False
          assert contract["source_repositories"]
          assert all(
              source.startswith("canonical-cloud/")
              for source in contract["source_repositories"]
          )
          assert "Production source is developed only in `canonical-cloud`." in (
              root / "README.md"
          ).read_text(encoding="utf-8")

          expects_zed = os.environ["EXPECTED_ZED_PACKAGE"] == "true"
          if expects_zed:
              manifest = tomllib.loads(
                  (root / ".zpkg.toml").read_text(encoding="utf-8")
              )
              owner, name = os.environ["EXPECTED_REPOSITORY"].split("/", 1)
              assert manifest["package"]["org"] == owner
              assert manifest["package"]["name"] == name
              assert manifest["install"]["dir"] == ".vendor/.zed"
              assert (root / ".zpkg.lock").read_text(encoding="utf-8") == "version = 1\\n"
          else:
              assert not (root / ".zpkg.toml").exists()
              assert not (root / ".zpkg.lock").exists()
          PY
"""


def managed_files(entry: dict[str, Any]) -> dict[str, str]:
    if not entry["auto_init"]:
        return {}
    files = {
        "README.md": render_readme(entry),
        ".canonical-source-contract.json": render_source_contract(entry),
        ".github/workflows/repository-contract.yml": render_contract_workflow(entry),
    }
    if entry["zed_package"]:
        files[".zpkg.toml"] = render_zpkg(entry)
        files[".zpkg.lock"] = "version = 1\n"
    return files


class GitHubApi:
    def __init__(self, api_url: str, tokens: dict[str, str]) -> None:
        self.api_url = api_url.rstrip("/")
        self.tokens = tokens

    def request(
        self,
        owner: str,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        *,
        allow_404: bool = False,
    ) -> Any:
        token = self.tokens.get(owner)
        if not token:
            raise ProvisioningError(f"no installation token is configured for {owner}")
        request = Request(
            f"{self.api_url}{path}",
            method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {token}",
                "Content-Type": "application/json",
                "User-Agent": "canonical-e2e-repository-provisioner/1",
                "X-GitHub-Api-Version": API_VERSION,
            },
        )
        try:
            with urlopen(request, timeout=30) as response:
                payload = response.read()
        except HTTPError as error:
            payload = error.read().decode(errors="replace")
            if allow_404 and error.code == 404:
                return None
            try:
                detail = json.loads(payload).get("message", payload)
            except json.JSONDecodeError:
                detail = payload
            raise ProvisioningError(
                f"GitHub API {method} {path} failed with {error.code}: {detail}"
            ) from error
        except URLError as error:
            raise ProvisioningError(
                f"GitHub API {method} {path} failed: {error.reason}"
            ) from error
        return None if not payload else json.loads(payload)

    def get_repository(self, owner: str, name: str) -> dict[str, Any] | None:
        return self.request(
            owner, "GET", f"/repos/{quote(owner)}/{quote(name)}", allow_404=True
        )

    def create_repository(self, entry: dict[str, Any]) -> dict[str, Any]:
        return self.request(
            entry["owner"],
            "POST",
            f"/orgs/{quote(entry['owner'])}/repos",
            {
                "name": entry["name"],
                "description": entry["description"],
                "visibility": entry["visibility"],
                "has_issues": True,
                "has_projects": False,
                "has_wiki": False,
                "auto_init": entry["auto_init"],
            },
        )

    def normalize_default_branch(
        self, entry: dict[str, Any], repository: dict[str, Any]
    ) -> dict[str, Any]:
        if not entry["auto_init"] or repository.get("default_branch") == "main":
            return repository
        current = repository.get("default_branch")
        if not isinstance(current, str) or not current:
            raise ProvisioningError(
                f"{repository_key(entry)} has no initialized default branch"
            )
        self.request(
            entry["owner"],
            "POST",
            f"/repos/{quote(entry['owner'])}/{quote(entry['name'])}/"
            f"branches/{quote(current)}/rename",
            {"new_name": "main"},
        )
        updated = self.get_repository(entry["owner"], entry["name"])
        if updated is None:
            raise ProvisioningError(f"cannot reload {repository_key(entry)}")
        return updated

    def harden_new_repository(self, entry: dict[str, Any]) -> None:
        base = f"/repos/{quote(entry['owner'])}/{quote(entry['name'])}"
        self.request(
            entry["owner"],
            "PATCH",
            base,
            {
                "allow_merge_commit": False,
                "allow_squash_merge": True,
                "allow_rebase_merge": True,
                "delete_branch_on_merge": True,
                "has_projects": False,
                "has_wiki": False,
            },
        )
        self.request(
            entry["owner"], "PUT", f"{base}/topics", {"names": entry["topics"]}
        )

    def get_file(self, entry: dict[str, Any], path: str) -> dict[str, Any] | None:
        return self.request(
            entry["owner"],
            "GET",
            f"/repos/{quote(entry['owner'])}/{quote(entry['name'])}/contents/"
            f"{quote(path, safe='/')}",
            allow_404=True,
        )

    def put_file(
        self,
        entry: dict[str, Any],
        path: str,
        content: str,
        message: str,
        sha: str | None = None,
    ) -> None:
        body: dict[str, Any] = {
            "message": message,
            "content": base64.b64encode(content.encode()).decode(),
        }
        if sha:
            body["sha"] = sha
        self.request(
            entry["owner"],
            "PUT",
            f"/repos/{quote(entry['owner'])}/{quote(entry['name'])}/contents/"
            f"{quote(path, safe='/')}",
            body,
        )


def decode_file(payload: dict[str, Any]) -> str:
    if payload.get("encoding") != "base64" or not isinstance(payload.get("content"), str):
        raise ProvisioningError("GitHub returned unsupported file content")
    try:
        return base64.b64decode(payload["content"]).decode()
    except (ValueError, UnicodeDecodeError) as error:
        raise ProvisioningError("GitHub returned invalid UTF-8 file content") from error


def validate_existing(entry: dict[str, Any], repository: dict[str, Any]) -> None:
    if repository.get("full_name") != repository_key(entry):
        raise ProvisioningError(f"GitHub resolved the wrong repository for {repository_key(entry)}")
    actual_visibility = "private" if repository.get("private") else "public"
    if actual_visibility != entry["visibility"]:
        raise ProvisioningError(
            f"{repository_key(entry)} visibility drift: "
            f"expected {entry['visibility']}, found {actual_visibility}"
        )
    if entry["auto_init"] and repository.get("default_branch") != "main":
        raise ProvisioningError(
            f"{repository_key(entry)} default branch drift: "
            f"found {repository.get('default_branch')!r}"
        )


def seed_repository(
    api: GitHubApi, entry: dict[str, Any], *, repository_was_created: bool
) -> None:
    for path, content in managed_files(entry).items():
        current = api.get_file(entry, path)
        if current is None:
            api.put_file(entry, path, content, f"chore: seed managed {path}")
            print(f"SEEDED {repository_key(entry)}:{path}")
        elif decode_file(current) == content:
            print(f"UNCHANGED {repository_key(entry)}:{path}")
        elif repository_was_created and path == "README.md":
            api.put_file(
                entry,
                path,
                content,
                "docs: replace generated README with E2E source contract",
                current.get("sha"),
            )
            print(f"SEEDED {repository_key(entry)}:{path}")
        else:
            raise ProvisioningError(
                f"managed file drift in {repository_key(entry)}:{path}; "
                "refusing to overwrite"
            )


def build_api(manifest: dict[str, Any], api_url: str) -> GitHubApi:
    source = os.environ.get("CANONICAL_SOURCE_INSTALLATION_TOKEN", "").strip()
    test = os.environ.get("CANONICAL_TEST_INSTALLATION_TOKEN", "").strip()
    if not source or not test:
        raise ProvisioningError(
            "CANONICAL_SOURCE_INSTALLATION_TOKEN and "
            "CANONICAL_TEST_INSTALLATION_TOKEN are required"
        )
    return GitHubApi(
        api_url,
        {manifest["source_org"]: source, manifest["test_org"]: test},
    )


def plan(manifest: dict[str, Any], api: GitHubApi) -> int:
    creates = 0
    for entry in manifest["repositories"]:
        repository = api.get_repository(entry["owner"], entry["name"])
        if repository is None:
            creates += 1
            print(
                f"CREATE {repository_key(entry)} "
                f"({entry['visibility']}, {entry['kind']})"
            )
        else:
            validate_existing(entry, repository)
            print(f"EXISTS {repository_key(entry)}")
    print(f"PLAN create={creates} existing={len(manifest['repositories']) - creates}")
    return 0


def apply(manifest: dict[str, Any], api: GitHubApi) -> int:
    created = 0
    for entry in manifest["repositories"]:
        repository = api.get_repository(entry["owner"], entry["name"])
        was_created = repository is None
        if was_created:
            repository = api.create_repository(entry)
            created += 1
            print(f"CREATED {repository_key(entry)}")
            if entry["auto_init"]:
                time.sleep(1)
                repository = api.normalize_default_branch(entry, repository)
            api.harden_new_repository(entry)
        else:
            print(f"EXISTS {repository_key(entry)}")
        validate_existing(entry, repository)
        seed_repository(api, entry, repository_was_created=was_created)
    print(
        f"APPLIED created={created} "
        f"existing={len(manifest['repositories']) - created}"
    )
    return 0


def parse_args(argv: Iterable[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--api-url",
        default=os.environ.get("GITHUB_API_URL", "https://api.github.com"),
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--plan", action="store_true")
    mode.add_argument("--apply", action="store_true")
    parser.add_argument("--confirm", default="")
    return parser.parse_args(argv)


def main(argv: Iterable[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        manifest = load_manifest(args.manifest)
        errors = validate_manifest(manifest)
        if errors:
            for error in errors:
                print(f"ERROR {error}", file=sys.stderr)
            return 2
        if not args.plan and not args.apply:
            print(
                f"VALID repositories={len(manifest['repositories'])} "
                f"source_org={manifest['source_org']} test_org={manifest['test_org']}"
            )
            return 0
        if args.apply and args.confirm != manifest["confirmation"]:
            raise ProvisioningError(
                "--apply requires the exact confirmation phrase from the manifest"
            )
        api = build_api(manifest, args.api_url)
        return plan(manifest, api) if args.plan else apply(manifest, api)
    except ProvisioningError as error:
        print(f"ERROR {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
