#!/usr/bin/env python3
"""Static security contract for the ephemeral Google Chat relay workflows."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GET_WORKFLOW = ROOT / ".github/workflows/ephemeral-google-chat-relay-get.yml"
POST_WORKFLOW = ROOT / ".github/workflows/ephemeral-google-chat-relay.yml"


def require(text: str, needle: str, description: str) -> None:
    if needle not in text:
        raise AssertionError(f"missing {description}: {needle!r}")


def forbid(text: str, needle: str, description: str) -> None:
    if needle in text:
        raise AssertionError(f"forbidden {description}: {needle!r}")


def main() -> None:
    get_workflow = GET_WORKFLOW.read_text(encoding="utf-8")
    post_workflow = POST_WORKFLOW.read_text(encoding="utf-8")

    require(
        get_workflow,
        "- .github/chat-relay-trigger/**",
        "explicit audit-trigger path",
    )
    forbid(
        get_workflow,
        "- .github/workflows/ephemeral-google-chat-relay-get.yml",
        "live relay self-trigger on workflow edits",
    )
    forbid(
        get_workflow,
        "- tools/google-chat-space-export/**",
        "live relay trigger on documentation or test edits",
    )
    forbid(get_workflow, "tail -n1", "last-writer-wins ciphertext selection")
    require(
        get_workflow,
        "reason=duplicate_ciphertexts",
        "duplicate-ciphertext rejection evidence",
    )
    require(
        get_workflow,
        '(.run_id == $run_id)',
        "decrypted payload binding to the one-time run ID",
    )
    require(
        get_workflow,
        "EXPECTED_DISPLAY_NAME: alex-alex-me",
        "fixed display-name assertion",
    )
    require(
        get_workflow,
        "find \"$out\" -type f ! -name SHA256SUMS",
        "non-self-referential plaintext checksum generation",
    )
    require(
        get_workflow,
        "relayCiphertextSha256",
        "ciphertext hash in the export manifest",
    )
    require(
        get_workflow,
        "ciphertext_sha256=${{ steps.payload.outputs.ciphertext_sha256 }}",
        "ciphertext hash in completion metadata",
    )
    require(
        get_workflow,
        "archive_sha256=${{ steps.encrypt.outputs.archive_sha256 }}",
        "encrypted archive hash in completion metadata",
    )
    require(
        get_workflow,
        "retention-days: 1",
        "one-day artifact retention",
    )

    require(post_workflow, "workflow_dispatch:", "manual-only retired workflow")
    require(post_workflow, "POST relay retired", "retirement notice")
    forbid(post_workflow, "pull_request:", "automatic POST relay trigger")
    forbid(post_workflow, "CHAT_EXEC_URL", "live bridge URL in retired POST workflow")
    forbid(post_workflow, "curl ", "network call in retired POST workflow")

    print("Google Chat relay workflow contract: PASS")


if __name__ == "__main__":
    main()
