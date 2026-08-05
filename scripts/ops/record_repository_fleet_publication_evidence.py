#!/usr/bin/env python3
"""Validate and record evidence from the sealed repository-fleet publisher."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
import subprocess
from typing import Any, Callable

from repository_fleet_aliases import load_repository_aliases


FLEET_SOURCE_REPOSITORY = "ORESoftware/ai-agent-coordinator.rs"
FLEET_SOURCE_SHA = "5d9a0c2cb44dff607bc3953954ce4b9af08e5789"
_CREATED_RE = re.compile(r"^VERIFIED_CREATED_PRIVATE (\S+) ([0-9a-f]{40})$")
_PRESERVED_RE = re.compile(r"^VERIFIED_PRESERVED_PRIVATE (\S+) ([0-9a-f]{40})$")
_RENAMED_RE = re.compile(
    r"^VERIFIED_PRESERVED_RENAMED (\S+) (\S+) ([1-9][0-9]*) ([0-9a-f]{40})$"
)
_SUMMARY_RE = re.compile(
    r"^VERIFIED private canonical fleet remote state "
    r"created=([0-9]+) preserved=([0-9]+) total=([0-9]+)$"
)
_FULL_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")


class PublicationEvidenceError(RuntimeError):
    pass


@dataclass(frozen=True)
class PublisherLog:
    created: dict[str, str]
    preserved: dict[str, str]
    renamed: dict[str, tuple[str, int, str]]
    created_count: int
    preserved_count: int
    total_count: int


RepositoryLookup = Callable[[str], tuple[int, dict[str, Any] | None]]
MainRefLookup = Callable[[str], str | None]


def _insert_unique(target: dict[str, Any], full_name: str, value: Any, label: str) -> None:
    key = full_name.casefold()
    if any(existing.casefold() == key for existing in target):
        raise PublicationEvidenceError(f"duplicate {label} repository: {full_name}")
    target[full_name] = value


def parse_publisher_log(text: str) -> PublisherLog:
    created: dict[str, str] = {}
    preserved: dict[str, str] = {}
    renamed: dict[str, tuple[str, int, str]] = {}
    summary: tuple[int, int, int] | None = None

    for raw_line in text.splitlines():
        line = raw_line.strip()
        if match := _CREATED_RE.fullmatch(line):
            _insert_unique(created, match.group(1), match.group(2), "created")
        elif match := _PRESERVED_RE.fullmatch(line):
            _insert_unique(preserved, match.group(1), match.group(2), "preserved")
        elif match := _RENAMED_RE.fullmatch(line):
            _insert_unique(
                renamed,
                match.group(1),
                (match.group(2), int(match.group(3)), match.group(4)),
                "renamed",
            )
        elif match := _SUMMARY_RE.fullmatch(line):
            candidate = tuple(int(match.group(index)) for index in (1, 2, 3))
            if summary is not None:
                raise PublicationEvidenceError("publisher log contains multiple final summaries")
            summary = candidate

    if summary is None:
        raise PublicationEvidenceError("publisher log lacks the final remote-state summary")
    created_count, preserved_count, total_count = summary
    if total_count != 32:
        raise PublicationEvidenceError(f"publisher total changed: {total_count} != 32")
    if created_count + preserved_count != total_count:
        raise PublicationEvidenceError("publisher summary counts do not add to the total")
    if len(created) != created_count:
        raise PublicationEvidenceError(
            f"created evidence count mismatch: {len(created)} != {created_count}"
        )
    if len(preserved) != preserved_count:
        raise PublicationEvidenceError(
            f"preserved evidence count mismatch: {len(preserved)} != {preserved_count}"
        )

    created_keys = {name.casefold() for name in created}
    preserved_keys = {name.casefold() for name in preserved}
    if created_keys & preserved_keys:
        raise PublicationEvidenceError("a repository is both created and preserved")
    if len(created_keys | preserved_keys) != total_count:
        raise PublicationEvidenceError("publisher evidence does not cover 32 unique identities")
    if any(name.casefold() not in preserved_keys for name in renamed):
        raise PublicationEvidenceError("renamed evidence is not a subset of preserved identities")

    return PublisherLog(
        created=created,
        preserved=preserved,
        renamed=renamed,
        created_count=created_count,
        preserved_count=preserved_count,
        total_count=total_count,
    )


def _validate_private_repository(
    requested_full_name: str,
    payload: dict[str, Any] | None,
) -> tuple[dict[str, Any], str]:
    if not isinstance(payload, dict):
        raise PublicationEvidenceError(
            f"invalid GitHub repository response for {requested_full_name}"
        )
    remote_full_name = payload.get("full_name")
    if not isinstance(remote_full_name, str) or _FULL_NAME_RE.fullmatch(remote_full_name) is None:
        raise PublicationEvidenceError(
            f"GitHub returned an invalid repository identity for {requested_full_name}"
        )
    if payload.get("private") is not True or payload.get("visibility") != "private":
        raise PublicationEvidenceError(f"repository is not private: {requested_full_name}")
    if payload.get("default_branch") != "main":
        raise PublicationEvidenceError(
            f"repository does not default to main: {requested_full_name}"
        )
    if payload.get("archived") is True or payload.get("disabled") is True:
        raise PublicationEvidenceError(
            f"repository is archived or disabled: {requested_full_name}"
        )
    return payload, remote_full_name


def collect_publication_evidence(
    parsed: PublisherLog,
    *,
    alias_ledger_path: Path,
    repository_lookup: RepositoryLookup,
    main_ref_lookup: MainRefLookup,
    required_repositories: list[str],
) -> dict[str, Any]:
    sealed_full_names = [*parsed.created, *parsed.preserved]
    aliases = load_repository_aliases(
        alias_ledger_path,
        sealed_full_names=sealed_full_names,
        expected_source_repository=FLEET_SOURCE_REPOSITORY,
        expected_source_sha=FLEET_SOURCE_SHA,
    )
    if len(aliases) != 6:
        raise PublicationEvidenceError(f"reviewed alias count changed: {len(aliases)} != 6")
    if {name.casefold() for name in parsed.renamed} != set(aliases):
        raise PublicationEvidenceError(
            "publisher renamed evidence does not exactly match the reviewed alias ledger"
        )

    created_evidence: list[dict[str, Any]] = []
    for full_name, expected_head in sorted(
        parsed.created.items(), key=lambda item: item[0].casefold()
    ):
        status, raw = repository_lookup(full_name)
        if status != 200:
            raise PublicationEvidenceError(
                f"created repository lookup failed for {full_name}: HTTP {status}"
            )
        repository, remote_full_name = _validate_private_repository(full_name, raw)
        if remote_full_name.casefold() != full_name.casefold():
            raise PublicationEvidenceError(
                f"created repository resolved through an unexpected redirect: {full_name} -> {remote_full_name}"
            )
        actual_head = main_ref_lookup(remote_full_name)
        if actual_head != expected_head:
            raise PublicationEvidenceError(
                f"created repository head mismatch for {full_name}: {actual_head!r} != {expected_head}"
            )
        created_evidence.append(
            {
                "sealed_full_name": full_name,
                "remote_full_name": remote_full_name,
                "repository_id": repository.get("id"),
                "visibility": repository.get("visibility"),
                "default_branch": repository.get("default_branch"),
                "main_sha": actual_head,
                "html_url": repository.get("html_url"),
            }
        )

    alias_evidence: list[dict[str, Any]] = []
    for sealed_key, alias in sorted(aliases.items()):
        logged_remote, logged_id, logged_head = parsed.renamed[alias.sealed_full_name]
        if logged_remote != alias.remote_full_name or logged_id != alias.repository_id:
            raise PublicationEvidenceError(
                f"publisher alias evidence changed for {alias.sealed_full_name}"
            )
        if parsed.preserved[alias.sealed_full_name] != logged_head:
            raise PublicationEvidenceError(
                f"preserved head differs from renamed head for {alias.sealed_full_name}"
            )
        status, raw = repository_lookup(alias.sealed_full_name)
        if status != 200:
            raise PublicationEvidenceError(
                f"alias source no longer resolves: {alias.sealed_full_name}"
            )
        repository, remote_full_name = _validate_private_repository(
            alias.sealed_full_name, raw
        )
        if remote_full_name != alias.remote_full_name:
            raise PublicationEvidenceError(
                f"alias target changed for {alias.sealed_full_name}: {remote_full_name}"
            )
        if repository.get("id") != alias.repository_id:
            raise PublicationEvidenceError(
                f"alias repository ID changed for {alias.sealed_full_name}"
            )
        actual_head = main_ref_lookup(remote_full_name)
        if actual_head != logged_head:
            raise PublicationEvidenceError(
                f"alias head changed for {alias.sealed_full_name}: {actual_head!r} != {logged_head}"
            )
        alias_evidence.append(
            {
                "sealed_full_name": alias.sealed_full_name,
                "remote_full_name": remote_full_name,
                "repository_id": repository.get("id"),
                "visibility": repository.get("visibility"),
                "default_branch": repository.get("default_branch"),
                "main_sha": actual_head,
                "html_url": repository.get("html_url"),
            }
        )

    required_evidence: list[dict[str, Any]] = []
    all_sealed_keys = {name.casefold() for name in sealed_full_names}
    for full_name in required_repositories:
        if full_name.casefold() not in all_sealed_keys:
            raise PublicationEvidenceError(
                f"required repository is not in the sealed fleet: {full_name}"
            )
        status, raw = repository_lookup(full_name)
        if status != 200:
            raise PublicationEvidenceError(
                f"required repository is absent after publication: {full_name}"
            )
        repository, remote_full_name = _validate_private_repository(full_name, raw)
        if remote_full_name.casefold() != full_name.casefold():
            raise PublicationEvidenceError(
                f"required gap resolved through a redirect: {full_name} -> {remote_full_name}"
            )
        head = main_ref_lookup(remote_full_name)
        if not isinstance(head, str) or re.fullmatch(r"[0-9a-f]{40}", head) is None:
            raise PublicationEvidenceError(f"required repository lacks a valid main SHA: {full_name}")
        required_evidence.append(
            {
                "full_name": remote_full_name,
                "repository_id": repository.get("id"),
                "visibility": repository.get("visibility"),
                "default_branch": repository.get("default_branch"),
                "main_sha": head,
                "html_url": repository.get("html_url"),
            }
        )

    return {
        "summary": {
            "created": parsed.created_count,
            "preserved": parsed.preserved_count,
            "renamed": len(alias_evidence),
            "total": parsed.total_count,
        },
        "created_repositories": created_evidence,
        "preserved_aliases": alias_evidence,
        "required_repositories": required_evidence,
    }


def _gh_json(args: list[str]) -> Any:
    completed = subprocess.run(
        ["gh", *args], text=True, capture_output=True, check=False
    )
    if completed.returncode != 0:
        raise PublicationEvidenceError(
            f"GitHub CLI failed ({completed.returncode}): {' '.join(args)}\n{completed.stderr}"
        )
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise PublicationEvidenceError(
            f"GitHub CLI returned non-JSON output for {' '.join(args)}"
        ) from error


def _repository_lookup(full_name: str) -> tuple[int, dict[str, Any] | None]:
    completed = subprocess.run(
        ["gh", "api", f"repos/{full_name}", "--include"],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode == 0:
        lines = completed.stdout.splitlines()
        body_index = next(
            (index for index, line in enumerate(lines) if line.strip() == ""),
            None,
        )
        body = "\n".join(lines[(body_index + 1 if body_index is not None else 0) :])
        return 200, json.loads(body)
    if "HTTP 404" in completed.stderr or '"status":"404"' in completed.stderr:
        return 404, None
    raise PublicationEvidenceError(
        f"repository lookup failed for {full_name}: {completed.stderr.strip()}"
    )


def _main_ref_lookup(full_name: str) -> str | None:
    payload = _gh_json(["api", f"repos/{full_name}/git/ref/heads/main"])
    value = payload.get("object", {}).get("sha") if isinstance(payload, dict) else None
    return value if isinstance(value, str) else None


def write_evidence(output_dir: Path, evidence: dict[str, Any]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    files = {
        "summary.json": evidence["summary"],
        "created-repositories.json": evidence["created_repositories"],
        "preserved-aliases.json": evidence["preserved_aliases"],
        "requested-gaps.json": evidence["required_repositories"],
    }
    for name, value in files.items():
        (output_dir / name).write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--publisher-log", type=Path, required=True)
    parser.add_argument("--alias-ledger", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--required-repository", action="append", default=[])
    args = parser.parse_args()

    parsed = parse_publisher_log(args.publisher_log.read_text(encoding="utf-8"))
    evidence = collect_publication_evidence(
        parsed,
        alias_ledger_path=args.alias_ledger,
        repository_lookup=_repository_lookup,
        main_ref_lookup=_main_ref_lookup,
        required_repositories=args.required_repository,
    )
    write_evidence(args.output_dir, evidence)
    summary = evidence["summary"]
    print(
        "VERIFIED_PUBLICATION_EVIDENCE "
        f"created={summary['created']} preserved={summary['preserved']} "
        f"renamed={summary['renamed']} total={summary['total']}"
    )
    print(
        "VERIFIED_REQUESTED_GAPS "
        f"{len(evidence['required_repositories'])}/{len(args.required_repository)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
