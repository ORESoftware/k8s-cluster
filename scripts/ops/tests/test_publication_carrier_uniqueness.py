#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).parents[1] / "validate_publication_carrier_uniqueness.py"
SPEC = importlib.util.spec_from_file_location("publication_carrier_uniqueness", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)

REPOSITORY = "ORESoftware/k8s-cluster"


def pull(
    number: int,
    title: str,
    body: str,
    *,
    draft: bool = True,
    author: str = "ORESoftware",
    base_ref: str = "main",
    base_repo: str = REPOSITORY,
    head_repo: str = REPOSITORY,
    head_ref: str | None = None,
) -> dict:
    return {
        "number": number,
        "state": "open",
        "draft": draft,
        "title": title,
        "body": body,
        "user": {"login": author},
        "base": {"ref": base_ref, "repo": {"full_name": base_repo}},
        "head": {
            "ref": head_ref or f"agent/publication-carrier-{number}",
            "repo": {"full_name": head_repo},
        },
    }


class PublicationCarrierUniquenessTests(unittest.TestCase):
    def test_distinct_intents_are_accepted(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    515,
                    "DO NOT MERGE: create exact Meta Agents repository",
                    "Create meta-agents-demo/meta-agent-control-plane.rs.",
                    head_ref="agent/create-meta-agent-repo-owner-auth-20260801",
                ),
                pull(
                    471,
                    "DO NOT MERGE: publish exact 34-repository organization fleet",
                    "Publish the critical organization fleet.",
                    head_ref="agent/critical-org-fleet-device-auth",
                ),
            ]
        )
        self.assertEqual(report["status"], "accepted")
        self.assertEqual(report["violations"], [])
        self.assertEqual(
            report["intents"],
            {"critical-org-fleet": [471], "meta-agent-control-plane": [515]},
        )

    def test_title_variants_for_same_meta_agent_intent_are_duplicates(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    435,
                    "DO NOT MERGE: publish exact Meta Agents repository",
                    "Target meta-agents-demo/meta-agent-control-plane.rs.",
                ),
                pull(
                    515,
                    "DO NOT MERGE: create exact Meta Agents repository",
                    "Target meta-agents-demo/meta-agent-control-plane.rs.",
                ),
            ]
        )
        self.assertEqual(report["status"], "rejected")
        self.assertEqual(report["violations"][0]["code"], "duplicate-active-intent")
        self.assertEqual(report["violations"][0]["pull_requests"], [435, 515])

    def test_current_pr_only_enforces_its_own_intent(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    515,
                    "DO NOT MERGE: create exact Meta Agents repository",
                    "Target meta-agents-demo/meta-agent-control-plane.rs.",
                ),
                pull(
                    470,
                    "DO NOT MERGE: publish exact 34-repository organization fleet",
                    "critical-org-fleet",
                ),
                pull(
                    471,
                    "DO NOT MERGE: publish exact 34-repository organization fleet",
                    "critical-org-fleet",
                ),
            ],
            current_pr=515,
        )
        self.assertEqual(report["status"], "accepted")
        self.assertEqual(report["current_intent"], "meta-agent-control-plane")

    def test_ordinary_hardening_pr_is_ignored(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    516,
                    "harden(DEN-319): pin isolated publisher and add Playwright fleet canary",
                    "Mentions meta-agents-demo/meta-agent-control-plane.rs but is not a carrier.",
                    draft=False,
                )
            ],
            current_pr=516,
        )
        self.assertEqual(report["status"], "ignored-non-carrier")
        self.assertEqual(report["violations"], [])

    def test_non_draft_carrier_is_rejected(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    515,
                    "DO NOT MERGE: create exact Meta Agents repository",
                    "Target meta-agents-demo/meta-agent-control-plane.rs.",
                    draft=False,
                )
            ],
            current_pr=515,
        )
        self.assertEqual(report["status"], "rejected")
        self.assertEqual(report["violations"][0]["code"], "not-draft")

    def test_cross_repository_carrier_is_rejected(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    515,
                    "DO NOT MERGE: create exact Meta Agents repository",
                    "Target meta-agents-demo/meta-agent-control-plane.rs.",
                    head_repo="example-org/example-repo",
                )
            ],
            current_pr=515,
        )
        self.assertEqual(report["status"], "rejected")
        self.assertEqual(report["violations"][0]["code"], "cross-repository-head")

    def test_wrong_author_and_base_are_rejected(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    515,
                    "DO NOT MERGE: create exact Meta Agents repository",
                    "Target meta-agents-demo/meta-agent-control-plane.rs.",
                    author="example-user",
                    base_ref="dev",
                )
            ],
            current_pr=515,
        )
        codes = {violation["code"] for violation in report["violations"]}
        self.assertEqual(codes, {"unexpected-author", "unexpected-base"})

    def test_unknown_do_not_merge_intent_is_ignored(self) -> None:
        report = MODULE.audit(
            [
                pull(
                    999,
                    "DO NOT MERGE: perform unrelated migration",
                    "No protected repository-publication intent.",
                )
            ],
            current_pr=999,
        )
        self.assertEqual(report["status"], "ignored-non-carrier")
        self.assertEqual(report["carrier_count"], 0)


if __name__ == "__main__":
    unittest.main()
