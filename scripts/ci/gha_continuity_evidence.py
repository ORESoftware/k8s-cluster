#!/usr/bin/env python3
"""Emit and compare normalized GitHub Actions continuity evidence.

This tool deliberately compares only stable, non-secret facts. It never accepts
commands, runner labels, environment variables, logs, tokens, or arbitrary
metadata from a workflow caller. Native GitHub Actions parity remains the job of
GitHub's runner protocol through ARC; the independent lane contributes bounded
planner/build-profile evidence under the same immutable source identity.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping, Sequence

SCHEMA_VERSION = "gha-continuity-evidence.v1"
REPORT_SCHEMA_VERSION = "gha-continuity-parity-report.v1"
LANES = frozenset({"hosted", "arc-aws", "arc-hetzner", "independent"})
TERMINAL_STATUSES = frozenset({"succeeded", "failed"})
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
ARTIFACT_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]{1,100}$")
WORKFLOW_PATH_RE = re.compile(r"^\.github/workflows/[A-Za-z0-9_./-]+\.ya?ml$")
ALLOWED_KEYS = frozenset(
    {
        "schemaVersion",
        "lane",
        "repository",
        "revision",
        "workflowPath",
        "planId",
        "status",
        "artifacts",
    }
)
FORBIDDEN_KEY_PARTS = (
    "token",
    "secret",
    "password",
    "credential",
    "privatekey",
    "authorization",
    "command",
    "runnerlabel",
    "environment",
    "log",
)


class EvidenceError(ValueError):
    """Raised when evidence is malformed or comparison is unsafe."""


@dataclass(frozen=True)
class Evidence:
    lane: str
    repository: str
    revision: str
    workflow_path: str
    plan_id: str
    status: str
    artifacts: tuple[tuple[str, str], ...]

    def identity(self) -> tuple[str, str, str, str]:
        return (self.repository, self.revision, self.workflow_path, self.plan_id)

    def artifact_map(self) -> dict[str, str]:
        return dict(self.artifacts)

    def to_json(self) -> dict[str, object]:
        return {
            "schemaVersion": SCHEMA_VERSION,
            "lane": self.lane,
            "repository": self.repository,
            "revision": self.revision,
            "workflowPath": self.workflow_path,
            "planId": self.plan_id,
            "status": self.status,
            "artifacts": {name: digest for name, digest in self.artifacts},
        }


def _require_string(value: object, field: str) -> str:
    if not isinstance(value, str) or not value:
        raise EvidenceError(f"{field} must be a non-empty string")
    if any(character.is_control() for character in value):
        raise EvidenceError(f"{field} must not contain control characters")
    return value


def _reject_forbidden_keys(value: object, path: str = "$") -> None:
    if isinstance(value, Mapping):
        for key, child in value.items():
            if not isinstance(key, str):
                raise EvidenceError(f"{path}: every object key must be a string")
            normalized = re.sub(r"[^a-z]", "", key.lower())
            if any(part in normalized for part in FORBIDDEN_KEY_PARTS):
                raise EvidenceError(f"{path}.{key}: secret or execution-bearing keys are forbidden")
            _reject_forbidden_keys(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            _reject_forbidden_keys(child, f"{path}[{index}]")


def parse_evidence(value: object) -> Evidence:
    _reject_forbidden_keys(value)
    if not isinstance(value, Mapping):
        raise EvidenceError("evidence must be a JSON object")
    keys = frozenset(value.keys())
    unknown = sorted(keys - ALLOWED_KEYS)
    missing = sorted(ALLOWED_KEYS - keys)
    if unknown:
        raise EvidenceError(f"unexpected evidence fields: {', '.join(unknown)}")
    if missing:
        raise EvidenceError(f"missing evidence fields: {', '.join(missing)}")
    if value["schemaVersion"] != SCHEMA_VERSION:
        raise EvidenceError(f"schemaVersion must be {SCHEMA_VERSION}")

    lane = _require_string(value["lane"], "lane")
    if lane not in LANES:
        raise EvidenceError(f"lane must be one of: {', '.join(sorted(LANES))}")
    repository = _require_string(value["repository"], "repository")
    if not REPOSITORY_RE.fullmatch(repository):
        raise EvidenceError("repository must be an owner/name identifier")
    revision = _require_string(value["revision"], "revision")
    if not SHA_RE.fullmatch(revision):
        raise EvidenceError("revision must be an exact lowercase 40-hex commit SHA")
    workflow_path = _require_string(value["workflowPath"], "workflowPath")
    if not WORKFLOW_PATH_RE.fullmatch(workflow_path) or ".." in workflow_path.split("/"):
        raise EvidenceError("workflowPath must stay under .github/workflows and end in .yml/.yaml")
    plan_id = _require_string(value["planId"], "planId")
    if not DIGEST_RE.fullmatch(plan_id):
        raise EvidenceError("planId must be a sha256: digest")
    status = _require_string(value["status"], "status")
    if status not in TERMINAL_STATUSES:
        raise EvidenceError("status must be succeeded or failed")

    raw_artifacts = value["artifacts"]
    if not isinstance(raw_artifacts, Mapping) or not raw_artifacts:
        raise EvidenceError("artifacts must be a non-empty object")
    if len(raw_artifacts) > 32:
        raise EvidenceError("artifacts may contain at most 32 entries")
    artifacts: list[tuple[str, str]] = []
    for raw_name, raw_digest in raw_artifacts.items():
        name = _require_string(raw_name, "artifact name")
        digest = _require_string(raw_digest, f"artifacts.{name}")
        if not ARTIFACT_NAME_RE.fullmatch(name):
            raise EvidenceError(f"invalid artifact name: {name!r}")
        if not DIGEST_RE.fullmatch(digest):
            raise EvidenceError(f"artifacts.{name} must be a sha256: digest")
        artifacts.append((name, digest))
    artifacts.sort()

    return Evidence(
        lane=lane,
        repository=repository,
        revision=revision,
        workflow_path=workflow_path,
        plan_id=plan_id,
        status=status,
        artifacts=tuple(artifacts),
    )


def load_evidence(path: Path) -> Evidence:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise EvidenceError(f"failed to read {path}: {error}") from error
    return parse_evidence(value)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return f"sha256:{digest.hexdigest()}"


def parse_artifact(value: str) -> tuple[str, str]:
    name, separator, raw_path = value.partition("=")
    if not separator or not name or not raw_path:
        raise EvidenceError("--artifact must use NAME=PATH")
    if not ARTIFACT_NAME_RE.fullmatch(name):
        raise EvidenceError(f"invalid artifact name: {name!r}")
    path = Path(raw_path)
    if not path.is_file():
        raise EvidenceError(f"artifact path is not a file: {path}")
    return (name, sha256_file(path))


def write_json(path: Path, value: object) -> None:
    payload = json.dumps(value, indent=2, sort_keys=True) + "\n"
    if str(path) == "-":
        sys.stdout.write(payload)
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(payload, encoding="utf-8")


def compare(evidence: Sequence[Evidence], required_lanes: Iterable[str]) -> dict[str, object]:
    if len(evidence) < 2:
        raise EvidenceError("at least two lane evidence files are required")
    by_lane: dict[str, Evidence] = {}
    for item in evidence:
        if item.lane in by_lane:
            raise EvidenceError(f"duplicate evidence for lane {item.lane}")
        by_lane[item.lane] = item

    required = frozenset(required_lanes)
    unknown_required = sorted(required - LANES)
    if unknown_required:
        raise EvidenceError(f"unknown required lanes: {', '.join(unknown_required)}")
    missing = sorted(required - by_lane.keys())
    if missing:
        raise EvidenceError(f"missing required lanes: {', '.join(missing)}")

    reference = evidence[0]
    mismatches: list[dict[str, object]] = []
    for item in evidence[1:]:
        if item.identity() != reference.identity():
            mismatches.append(
                {
                    "lane": item.lane,
                    "kind": "identity",
                    "expected": {
                        "repository": reference.repository,
                        "revision": reference.revision,
                        "workflowPath": reference.workflow_path,
                        "planId": reference.plan_id,
                    },
                    "actual": {
                        "repository": item.repository,
                        "revision": item.revision,
                        "workflowPath": item.workflow_path,
                        "planId": item.plan_id,
                    },
                }
            )
        if item.status != reference.status:
            mismatches.append(
                {
                    "lane": item.lane,
                    "kind": "status",
                    "expected": reference.status,
                    "actual": item.status,
                }
            )
        if item.artifacts != reference.artifacts:
            mismatches.append(
                {
                    "lane": item.lane,
                    "kind": "artifacts",
                    "expected": reference.artifact_map(),
                    "actual": item.artifact_map(),
                }
            )

    return {
        "schemaVersion": REPORT_SCHEMA_VERSION,
        "parity": not mismatches,
        "referenceLane": reference.lane,
        "lanes": sorted(by_lane),
        "requiredLanes": sorted(required),
        "identity": {
            "repository": reference.repository,
            "revision": reference.revision,
            "workflowPath": reference.workflow_path,
            "planId": reference.plan_id,
        },
        "status": reference.status,
        "artifacts": reference.artifact_map(),
        "mismatches": mismatches,
    }


def command_emit(args: argparse.Namespace) -> int:
    artifacts = [parse_artifact(value) for value in args.artifact]
    raw = {
        "schemaVersion": SCHEMA_VERSION,
        "lane": args.lane,
        "repository": args.repository,
        "revision": args.revision,
        "workflowPath": args.workflow_path,
        "planId": args.plan_id,
        "status": args.status,
        "artifacts": dict(artifacts),
    }
    evidence = parse_evidence(raw)
    write_json(args.output, evidence.to_json())
    return 0


def command_compare(args: argparse.Namespace) -> int:
    evidence = [load_evidence(path) for path in args.evidence]
    report = compare(evidence, args.require_lane)
    write_json(args.output, report)
    return 0 if report["parity"] else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    emit = subparsers.add_parser("emit", help="create normalized lane evidence")
    emit.add_argument("--lane", required=True, choices=sorted(LANES))
    emit.add_argument("--repository", required=True)
    emit.add_argument("--revision", required=True)
    emit.add_argument("--workflow-path", required=True)
    emit.add_argument("--plan-id", required=True)
    emit.add_argument("--status", required=True, choices=sorted(TERMINAL_STATUSES))
    emit.add_argument("--artifact", action="append", required=True, metavar="NAME=PATH")
    emit.add_argument("--output", type=Path, required=True)
    emit.set_defaults(handler=command_emit)

    compare_parser = subparsers.add_parser("compare", help="compare lane evidence")
    compare_parser.add_argument("evidence", nargs="+", type=Path)
    compare_parser.add_argument("--require-lane", action="append", default=[])
    compare_parser.add_argument("--output", type=Path, required=True)
    compare_parser.set_defaults(handler=command_compare)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        return int(args.handler(args))
    except EvidenceError as error:
        parser.error(str(error))
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
