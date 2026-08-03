#!/usr/bin/env python3
"""Static security contract for the trusted canonical gap-bootstrap supervisor."""

from __future__ import annotations

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW_PATH = (
    ROOT / ".github" / "workflows" / "canonical-gap-bootstrap-supervisor.yml"
)


class CanonicalGapBootstrapSupervisorContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW_PATH.read_text(encoding="utf-8")

    def test_trigger_is_exact_metadata_only_pull_request_target_path(self) -> None:
        self.assertIn("pull_request_target:", self.workflow)
        self.assertIn("branches: [main]", self.workflow)
        self.assertIn(
            "- .github/canonical-gap-bootstrap-dispatch-trigger",
            self.workflow,
        )
        self.assertNotIn("workflow_run:", self.workflow)
        self.assertNotIn("workflow_call:", self.workflow)

    def test_permissions_are_bounded_and_do_not_request_secret_or_id_token_access(
        self,
    ) -> None:
        permissions = re.search(
            r"(?ms)^permissions:\n(?P<body>(?:  [^\n]+\n)+)",
            self.workflow,
        )
        self.assertIsNotNone(permissions)
        assert permissions is not None
        body = permissions.group("body")
        self.assertEqual(
            set(line.strip() for line in body.splitlines()),
            {
                "actions: write",
                "contents: read",
                "issues: write",
                "statuses: write",
            },
        )
        self.assertNotIn("id-token:", self.workflow)
        self.assertNotIn("secrets:", self.workflow)

    def test_exact_same_repository_carrier_identity_is_required(self) -> None:
        required = (
            "github.event.pull_request.number == 551",
            "github.event.pull_request.user.login == 'ORESoftware'",
            "github.event.pull_request.head.repo.full_name == github.repository",
            "github.event.pull_request.head.ref == "
            "'agent/dispatch-canonical-gap-bootstrap-20260801-v1'",
            "github.event.pull_request.title == "
            "'DO NOT MERGE: dispatch safe canonical repository-gap bootstrap'",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_carrier_shape_is_one_commit_one_added_four_line_marker(self) -> None:
        required = (
            '.commits == 1',
            '.changed_files == 1',
            '.additions == 4',
            '.deletions == 0',
            "commits?per_page=100",
            "files?per_page=100",
            '.[0].status == "added"',
            '.[0].changes == 4',
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_marker_and_trusted_inputs_are_pinned(self) -> None:
        required = (
            "WORKFLOW_BLOB_SHA: 112cb7e8249c6a2a41de9799fca870f5cabde055",
            "SCRIPT_BLOB_SHA: 9d9021a0584de8e3f7def2f32a067b4f6701e6c2",
            "FLEET_SOURCE_SHA: 5d9a0c2cb44dff607bc3953954ce4b9af08e5789",
            "target=canonical-gap-bootstrap-v1",
            "workflow=112cb7e8249c6a2a41de9799fca870f5cabde055",
            "script=9d9021a0584de8e3f7def2f32a067b4f6701e6c2",
            "fleet-source=5d9a0c2cb44dff607bc3953954ce4b9af08e5789",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_supervisor_never_checks_out_or_executes_pull_request_code(self) -> None:
        self.assertNotIn("actions/checkout@", self.workflow)
        self.assertNotIn("pull_request.head.repo.clone_url", self.workflow)
        self.assertNotIn("pull_request.head.ref }}", self.workflow)
        self.assertNotIn("pull_request.head.sha }}", self.workflow)

    def test_child_dispatch_is_registered_active_and_main_only(self) -> None:
        self.assertIn("actions/workflows/${CHILD_WORKFLOW}", self.workflow)
        self.assertIn('.state <<<"$registered"', self.workflow)
        self.assertIn("/dispatches", self.workflow)
        self.assertIn("-f ref=main", self.workflow)

    def test_child_run_discovery_excludes_preexisting_run_ids(self) -> None:
        required = (
            "before-run-ids.txt",
            "workflow_runs[].id",
            'grep -Fxq "$candidate" "$before"',
            "for _ in $(seq 1 60); do",
        )
        for snippet in required:
            with self.subTest(snippet=snippet):
                self.assertIn(snippet, self.workflow)

    def test_run_following_is_bounded_and_requires_success(self) -> None:
        self.assertIn("timeout-minutes: 125", self.workflow)
        self.assertIn("for _ in $(seq 1 460); do", self.workflow)
        self.assertIn('[[ "$status" == completed ]] && break', self.workflow)
        self.assertIn('test "$conclusion" = success', self.workflow)

    def test_status_and_evidence_are_written_to_both_tracking_prs(self) -> None:
        self.assertIn("TRIGGER_PR: '551'", self.workflow)
        self.assertIn("TRACKING_PR: '471'", self.workflow)
        self.assertGreaterEqual(
            self.workflow.count('gh pr comment "$TRIGGER_PR"'), 2
        )
        self.assertGreaterEqual(
            self.workflow.count('gh pr comment "$TRACKING_PR"'), 2
        )
        self.assertGreaterEqual(
            self.workflow.count("ops/canonical-gap-bootstrap"), 2
        )


if __name__ == "__main__":
    unittest.main()
