#!/usr/bin/env python3
from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
CRONJOB = ROOT / "remote/argocd/benefactor-contact-pipeline/cronjob.yaml"
KUSTOMIZATION = ROOT / "remote/argocd/benefactor-contact-pipeline/kustomization.yaml"
APPLICATION = ROOT / "remote/argocd/apps/benefactor-contact-pipeline.application.yaml"
README = ROOT / "remote/argocd/benefactor-contact-pipeline/README.md"
DIGEST = "sha256:17ba7b0dd64aad31e9ab9486d73897481d861c72123f87ad3f42f7ae0a8f34eb"
SOURCE_COMMIT = "5212868d77616d0cc3661dc27b9e5ada6ab48a26"


class BenefactorContactCronJobTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.cronjob = CRONJOB.read_text(encoding="utf-8")
        cls.kustomization = KUSTOMIZATION.read_text(encoding="utf-8")
        cls.application = APPLICATION.read_text(encoding="utf-8")
        cls.readme = README.read_text(encoding="utf-8")

    def test_schedule_is_exact_6am_central_and_active_dry_run(self) -> None:
        self.assertIn('schedule: "0 6 * * *"', self.cronjob)
        self.assertIn("timeZone: America/Chicago", self.cronjob)
        self.assertIn("suspend: false", self.cronjob)
        self.assertNotIn("suspend: true", self.cronjob)
        self.assertIn("concurrencyPolicy: Forbid", self.cronjob)
        self.assertIn("startingDeadlineSeconds: 900", self.cronjob)
        self.assertIn("benefactor.cc/activation-mode: scheduled-dry-run", self.cronjob)
        self.assertIn(
            "benefactor.cc/activation-evidence: github-actions-run-31397763253-attempt-2",
            self.cronjob,
        )

    def test_image_is_immutable_and_matches_build_evidence(self) -> None:
        image = re.search(r"^\s*image:\s*(\S+)$", self.cronjob, re.MULTILINE)
        self.assertIsNotNone(image)
        value = image.group(1)
        self.assertEqual(
            f"ghcr.io/oresoftware/benefactor-contact-orchestrator@{DIGEST}",
            value,
        )
        self.assertNotIn(":main", value)
        self.assertNotIn(":latest", value)
        self.assertIn("imagePullPolicy: IfNotPresent", self.cronjob)
        self.assertIn(SOURCE_COMMIT, self.readme)
        self.assertIn(DIGEST, self.readme)

    def test_contact_bounds_and_dry_run_are_fail_closed(self) -> None:
        expected_pairs = {
            "BATCH_DRY_RUN": "true",
            "BATCH_TARGET_CONTACTS": "250",
            "BATCH_MINIMUM_CONTACTS": "200",
            "BATCH_MAXIMUM_CONTACTS": "300",
            "BATCH_MAX_CONTACTS_PER_CATEGORY": "50",
            "HUBSPOT_SYNC_AFTER_DISCOVERY": "true",
            "HUBSPOT_DRY_RUN": "true",
            "REQUIRE_ROLE_EMAIL": "true",
        }
        for name, value in expected_pairs.items():
            pattern = rf"- name: {re.escape(name)}\n\s+value: \"{re.escape(value)}\""
            self.assertRegex(self.cronjob, pattern)
        self.assertNotIn("BATCH_PERSIST_CONFIRM", self.cronjob)

    def test_discovery_job_has_no_outreach_credentials_or_commands(self) -> None:
        upper = self.cronjob.upper()
        forbidden = (
            "SENDGRID",
            "GMAIL",
            "LIVE_SEND",
            "OUTREACH_CONFIRM",
            "CAMPAIGN_APPROVAL",
            "HELLO@MAIL.BENEFACTOR.CC",
        )
        for marker in forbidden:
            self.assertNotIn(marker, upper)
        self.assertIn("benefactor.cc/outreach-capability: disabled", self.cronjob)

    def test_security_and_runtime_bounds_are_present(self) -> None:
        for marker in (
            "automountServiceAccountToken: false",
            "enableServiceLinks: false",
            "backoffLimit: 0",
            "activeDeadlineSeconds: 3600",
            "runAsNonRoot: true",
            "readOnlyRootFilesystem: true",
            "allowPrivilegeEscalation: false",
            "PG_SSL_CA_FILE",
            "RDS_CA_PEM",
        ):
            self.assertIn(marker, self.cronjob)
        self.assertRegex(self.cronjob, r"drop:\s*\n\s+- ALL")

    def test_private_scraper_and_provider_secrets_are_bounded(self) -> None:
        self.assertIn(
            "http://dd-web-scraper.default.svc.cluster.local:8097",
            self.cronjob,
        )
        self.assertIn("SCRAPER_ALLOWED_HOSTS", self.cronjob)
        self.assertIn("SCRAPER_AUTH", self.cronjob)
        self.assertIn("BRAVE_SEARCH_API_KEY", self.cronjob)
        self.assertIn("SERPER_API_KEY", self.cronjob)
        self.assertNotIn("ALLOW_DIRECT_FALLBACK", self.cronjob)

    def test_argo_and_kustomize_wiring_are_exact(self) -> None:
        self.assertIn("- cronjob.yaml", self.kustomization)
        self.assertIn("name: benefactor-contact-pipeline", self.application)
        self.assertIn("targetRevision: dev", self.application)
        self.assertIn("path: remote/argocd/benefactor-contact-pipeline", self.application)
        self.assertIn("namespace: default", self.application)
        self.assertIn("selfHeal: true", self.application)

    def test_runbook_preserves_separate_outreach_approval(self) -> None:
        self.assertIn("active scheduled dry run", self.readme)
        self.assertIn("safeToMutate: false", self.readme)
        self.assertIn("zero contacts", self.readme)
        self.assertIn("DEN-833", self.readme)
        self.assertIn("intentionally absent from this CronJob", self.readme)
        self.assertIn("Changing this manifest never grants marketing consent", self.readme)


if __name__ == "__main__":
    unittest.main()
