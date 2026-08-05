#!/usr/bin/env python3
"""Credential-free source contract for Daedalus Meshy provider and durable jobs."""

from __future__ import annotations

import json
import re
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


def validate_source_image(image: object, index: int) -> None:
    if not isinstance(image, dict):
        raise SystemExit(f"durable Meshy source image {index} must be an object")
    asset_id = image.get("asset_id")
    url = image.get("url")
    digest = image.get("sha256")
    if not isinstance(asset_id, str) or not asset_id.strip():
        raise SystemExit(f"durable Meshy source image {index} needs asset_id")
    if not isinstance(url, str) or not url.startswith("https://"):
        raise SystemExit(f"durable Meshy source image {index} must use HTTPS")
    if (
        not isinstance(digest, str)
        or len(digest) != 64
        or any(character not in "0123456789abcdef" for character in digest)
    ):
        raise SystemExit(
            f"durable Meshy source image {index} needs a lowercase SHA-256"
        )


def contains_meshy_credential(text: str) -> bool:
    """Reject likely credentials while permitting explicit documentation placeholders."""

    if re.search(r"\bmsy_[A-Za-z0-9_-]{16,}\b", text):
        return True

    placeholders = {
        "",
        "...",
        "<key>",
        "<redacted>",
        "replace-me",
        "your-key",
        "your-meshy-api-key",
        "$MESHY_API_KEY",
        "${MESHY_API_KEY}",
    }
    assignment = re.compile(
        r"(?:export\s+)?MESHY_API_KEY\s*=\s*(?:['\"]([^'\"]*)['\"]|([^\s#]+))"
    )
    for match in assignment.finditer(text):
        value = (match.group(1) if match.group(1) is not None else match.group(2)).strip()
        if value not in placeholders:
            return True
    return False


def main() -> None:
    provider_manifest = read("crates/meshy-client/Cargo.toml")
    provider_library = read("crates/meshy-client/src/lib.rs")
    provider_cli = read("crates/meshy-client/src/cli.rs")
    provider_binary = read("crates/meshy-client/src/bin/dd-meshy-adapter.rs")
    durable_manifest = read("crates/meshy-job/Cargo.toml")
    durable_library = read("crates/meshy-job/src/lib.rs")
    durable_worker = read("src/bin/meshy-job-worker.rs")
    docs = read("docs/meshy-integration.md")
    tracking = read("docs/project-tracking.md")

    require(
        provider_manifest,
        [
            'name = "dd-meshy-client"',
            'reqwest = { version = "0.12"',
            "rustls-tls",
            "publish = false",
        ],
        "crates/meshy-client/Cargo.toml",
    )
    require(
        provider_library,
        [
            'DEFAULT_BASE_URL: &str = "https://api.meshy.ai"',
            'API_KEY_ENV: &str = "MESHY_API_KEY"',
            '"image-to-3d"',
            '"multi-image-to-3d"',
            "authorization.set_sensitive(true)",
            '"Bearer <redacted>"',
            "machine_ready: false",
            'release_state: "draft"',
            "TargetFormat::ThreeMf",
            '"dd.fabrication.external-geometry-candidate.v1"',
            "create_image_task_uses_bearer_auth_and_expected_endpoint",
            "succeeded_provider_task_is_still_review_blocked",
        ],
        "crates/meshy-client/src/lib.rs",
    )
    require(
        provider_cli,
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
    require(
        provider_binary,
        ["run_from_env", "exit_on_error"],
        "crates/meshy-client/src/bin/dd-meshy-adapter.rs",
    )

    require(
        durable_manifest,
        [
            'name = "dd-meshy-job"',
            'dd-meshy-client = { path = "../meshy-client" }',
            'reqwest = { version = "0.12"',
            'sha2 = "0.10"',
            "publish = false",
        ],
        "crates/meshy-job/Cargo.toml",
    )
    require(
        durable_library,
        [
            'JOB_KIND: &str = "meshy_image_to_3d"',
            "SubmissionCheckpoint::Prepared",
            "provider_submission_unreconciled",
            "provider_submission_requires_reconciliation",
            "existing_provider_task",
            'env::var("MESHY_ARTIFACT_ALLOWED_HOSTS")',
            "redirect(reqwest::redirect::Policy::none())",
            "artifact_ip_literal_rejected",
            "artifact_host_not_allowed",
            "archive_object_conflict",
            "sha256",
            "ArtifactArchive",
            "DirectoryArchive",
            'review_state: "needs_review".to_string()',
            'release_state: "blocked".to_string()',
            "machine_ready: false",
            "persisted_intent_cannot_be_resubmitted_after_process_loss",
            "ambiguous_create_is_checkpointed_and_never_retried_automatically",
            "successful_job_archives_only_requested_outputs_and_stays_release_blocked",
        ],
        "crates/meshy-job/src/lib.rs",
    )
    require(
        durable_worker,
        [
            "ClaimedJobLease::acquire",
            "store.ensure_schema()",
            'persist_checkpoint(lease, "meshy_submission_prepared"',
            "lease.heartbeat()",
            "lease.complete(&value)",
            "lease.fail(&code, &message, retryable)",
            "do not resubmit automatically",
            "MESHY_ARTIFACT_ALLOWED_HOSTS",
            "FIDUCIA_AUTH_TOKEN",
        ],
        "src/bin/meshy-job-worker.rs",
    )
    require(
        docs,
        [
            "Linear `DEN-2465`",
            "Linear `DEN-2506`",
            "SubmissionCheckpoint::Prepared",
            "provider_submission_unreconciled",
            "GLB/STL/3MF",
            "durable directory or Kubernetes PVC",
            "MESHY_ARTIFACT_ALLOWED_HOSTS",
            '"release_state": "blocked"',
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
    durable = json.loads(read("examples/meshy/durable-job-request.json"))
    expected_formats = {"glb", "stl", "3mf"}
    if set(single.get("target_formats", [])) != expected_formats:
        raise SystemExit("single-image example must request glb, stl, and 3mf")
    if set(multi.get("target_formats", [])) != expected_formats:
        raise SystemExit("multi-image example must request glb, stl, and 3mf")
    if not 1 <= len(multi.get("image_urls", [])) <= 4:
        raise SystemExit("multi-image example must contain between one and four views")

    if durable.get("schema_version") != "daedalus.meshy-job.v1":
        raise SystemExit("durable Meshy example must use daedalus.meshy-job.v1")
    if set(durable.get("target_formats", [])) != expected_formats:
        raise SystemExit("durable Meshy example must request glb, stl, and 3mf")
    images = durable.get("images", [])
    if not isinstance(images, list) or not 1 <= len(images) <= 4:
        raise SystemExit("durable Meshy example must contain between one and four images")
    for index, image in enumerate(images):
        validate_source_image(image, index)
    if not isinstance(durable.get("max_artifact_bytes"), int):
        raise SystemExit("durable Meshy example must include max_artifact_bytes")

    forbidden = []
    for path in ROOT.rglob("*"):
        if not path.is_file() or path.name == "check_meshy_integration.py":
            continue
        text = path.read_text(encoding="utf-8", errors="ignore")
        if contains_meshy_credential(text):
            forbidden.append(path)
    if forbidden:
        paths = "\n".join(f"  - {path.relative_to(ROOT)}" for path in forbidden)
        raise SystemExit(f"possible Meshy API key material found in tracked files:\n{paths}")

    print("Meshy provider and durable job integration contract is complete")


if __name__ == "__main__":
    main()
