from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github" / "workflows"
CANONICAL = WORKFLOWS / "ops-run-canonical-fleet-isolated.yml"
RETRY = WORKFLOWS / "ops-retry-expired-fleet-device-auth.yml"


class DeviceAuthorizationRetryWorkflowContracts(unittest.TestCase):
    def setUp(self) -> None:
        self.assertTrue(CANONICAL.is_file())
        self.assertTrue(RETRY.is_file())
        self.canonical = CANONICAL.read_text(encoding="utf-8")
        self.retry = RETRY.read_text(encoding="utf-8")

    def test_retry_is_bound_to_the_exact_canonical_workflow(self) -> None:
        self.assertIn("workflow_run:", self.retry)
        self.assertIn(
            "Run canonical critical organization fleet publication isolated",
            self.retry,
        )
        self.assertIn("types: [completed]", self.retry)
        self.assertNotIn("\n  push:", self.retry)
        self.assertNotIn("\n  pull_request:", self.retry)
        self.assertIn(
            "EXPECTED_WORKFLOW_PATH: .github/workflows/ops-run-canonical-fleet-isolated.yml",
            self.retry,
        )
        self.assertIn(".path == $path", self.retry)
        self.assertIn('.event == "workflow_dispatch"', self.retry)
        self.assertIn('.head_branch == "main"', self.retry)
        self.assertIn('.conclusion == "failure"', self.retry)

    def test_retry_is_capped_and_serialized_per_source_run(self) -> None:
        self.assertIn("github.event.workflow_run.run_attempt < 6", self.retry)
        self.assertIn(
            "group: retry-canonical-fleet-device-auth-${{ github.event.workflow_run.id }}",
            self.retry,
        )
        self.assertIn("cancel-in-progress: false", self.retry)
        self.assertIn("next_attempt=$((SOURCE_RUN_ATTEMPT + 1))", self.retry)
        self.assertIn("attempt ${next_attempt}/6", self.retry)

    def test_only_expired_device_sessions_are_retried(self) -> None:
        expiry_pattern = re.compile(
            r"context deadline exceeded\|expired_token\|"
            r"device authorization failed or expired\|"
            r"failed to authenticate via web browser"
        )
        self.assertRegex(self.retry, expiry_pattern)
        self.assertIn("gh run view", self.retry)
        self.assertIn("--log-failed", self.retry)
        self.assertIn(
            "Refusing to retry a run that contains publication-success evidence.",
            self.retry,
        )
        self.assertIn("Published and directly verified", self.retry)
        self.assertIn("CREATED_AND_VERIFIED", self.retry)
        self.assertIn("34/34 live repository", self.retry)

    def test_failed_jobs_are_retried_without_restarting_successful_jobs(self) -> None:
        self.assertIn("permissions:\n  actions: write", self.retry)
        self.assertIn(
            "actions/runs/${SOURCE_RUN_ID}/rerun-failed-jobs",
            self.retry,
        )
        self.assertNotIn("/rerun\"", self.retry)
        self.assertIn("The prior code is invalid.", self.retry)
        self.assertIn("Watch this PR for the fresh one-time code", self.retry)

    def test_logs_and_credentials_fail_closed(self) -> None:
        self.assertIn("[REDACTED-CODE]", self.retry)
        self.assertIn("[REDACTED-TOKEN]", self.retry)
        self.assertIn("trap cleanup EXIT", self.retry)
        self.assertIn('rm -f "$raw_log" "$sanitized_log"', self.retry)
        self.assertNotIn("${{ secrets.", self.retry)
        self.assertNotRegex(
            self.retry,
            re.compile(r"(?:cat|tee)\s+[^\n]*raw_log"),
        )

    def test_canonical_run_removes_each_stale_authorization_comment(self) -> None:
        self.assertIn("auth_comment_id=''", self.canonical)
        self.assertIn(
            "issues/comments/${auth_comment_id}",
            self.canonical,
        )
        self.assertIn("--method DELETE", self.canonical)
        self.assertIn("[REDACTED-CODE]", self.canonical)


if __name__ == "__main__":
    unittest.main()
