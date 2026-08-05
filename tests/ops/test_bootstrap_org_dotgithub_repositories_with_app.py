#!/usr/bin/env python3
from __future__ import annotations

import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
OPS = ROOT / "scripts" / "ops"
if str(OPS) not in sys.path:
    sys.path.insert(0, str(OPS))

import bootstrap_org_dotgithub_repositories as base  # noqa: E402
import bootstrap_org_dotgithub_repositories_all as fleet  # noqa: E402
import bootstrap_org_dotgithub_repositories_with_app as module  # noqa: E402


class DummyApi:
    def __init__(self, token: str) -> None:
        self.token = token


class AppOrganizationGovernancePublisherTests(unittest.TestCase):
    def test_credentials_are_app_only_and_removed_from_the_environment(self) -> None:
        begin = "-----BEGIN " + "PRIVATE KEY-----"
        end = "-----END " + "PRIVATE KEY-----"
        private_key = f"{begin}\nfixture-not-used-for-signing\n{end}\n"
        environment = {
            module.APP_ID_ENV: "12345",
            module.APP_PRIVATE_KEY_ENV: private_key,
            "GH_TOKEN": "ambient-user-token",
            "GITHUB_TOKEN": "ambient-workflow-token",
            "GITHUB_REPOSITORY_ADMIN_TOKEN": "ambient-admin-token",
        }
        with patch.dict(os.environ, environment, clear=True):
            app_id, observed_key = module.read_app_credentials()
            self.assertEqual("12345", app_id)
            self.assertEqual(private_key, observed_key)
            for variable in environment:
                self.assertNotIn(variable, os.environ)

    def test_installation_token_requires_all_repositories_and_exact_permissions(self) -> None:
        calls: list[tuple[str, str]] = []

        def request(method: str, path: str, bearer: str, body=None):
            calls.append((method, path))
            if method == "GET":
                return 200, {
                    "id": 77,
                    "app_slug": "fleet-admin-app",
                    "repository_selection": "all",
                    "account": {"login": "fiducia-cloud"},
                }
            if method == "POST":
                self.assertEqual({"permissions": module.REQUIRED_PERMISSIONS}, body)
                return 201, {
                    "token": "installation-token-77",
                    "permissions": dict(module.REQUIRED_PERMISSIONS),
                }
            if method == "DELETE":
                return 204, None
            self.fail(f"unexpected request: {method} {path}")

        app_slug, token = module.validated_installation_token(
            "fiducia-cloud",
            "app-jwt",
            request_fn=request,
        )
        self.assertEqual("fleet-admin-app", app_slug)
        self.assertEqual("installation-token-77", token)
        self.assertEqual(
            [
                ("GET", "/orgs/fiducia-cloud/installation"),
                ("POST", "/app/installations/77/access_tokens"),
            ],
            calls,
        )

    def test_complete_61_org_preflight_happens_before_first_reconcile(self) -> None:
        events: list[str] = []
        installation_ids = {
            organization.lower(): index
            for index, organization in enumerate(fleet.TARGET_ORGANIZATIONS, start=1)
        }

        def request(method: str, path: str, bearer: str, body=None):
            if method == "GET" and path.endswith("/installation"):
                organization = path.split("/")[2]
                events.append(f"installation:{organization}")
                return 200, {
                    "id": installation_ids[organization.lower()],
                    "app_slug": "fleet-admin-app",
                    "repository_selection": "all",
                    "account": {"login": organization},
                }
            if method == "POST" and path.endswith("/access_tokens"):
                installation_id = int(path.split("/")[3])
                events.append(f"token:{installation_id}")
                return 201, {
                    "token": f"installation-token-{installation_id}",
                    "permissions": dict(module.REQUIRED_PERMISSIONS),
                }
            if method == "DELETE" and path == "/installation/token":
                events.append("revoke")
                return 204, None
            self.fail(f"unexpected request: {method} {path}")

        def repository_getter(api: DummyApi, organization: str):
            events.append(f"repository:{organization}")
            return None

        prepared, app_slug = module.preflight_installations(
            "app-jwt",
            request_fn=request,
            api_factory=DummyApi,
            repository_getter=repository_getter,
            repository_validator=lambda repository, organization: None,
        )
        self.assertEqual("fleet-admin-app", app_slug)
        self.assertEqual(61, len(prepared))
        self.assertEqual(
            {name.lower() for name in fleet.TARGET_ORGANIZATIONS},
            {item.organization.lower() for item in prepared},
        )
        self.assertFalse(any(event.startswith("reconcile:") for event in events))

        def reconcile(api, organization, repository, *, execute):
            events.append(f"reconcile:{organization}")
            return base.OrganizationResult(
                organization=organization,
                repository=f"{organization}/.github",
                verified=execute,
            )

        results = module.reconcile_prepared_installations(
            prepared,
            execute=True,
            reconcile=reconcile,
        )
        first_reconcile = next(
            index for index, event in enumerate(events) if event.startswith("reconcile:")
        )
        last_preflight = max(
            index for index, event in enumerate(events) if event.startswith("repository:")
        )
        self.assertGreater(first_reconcile, last_preflight)
        self.assertEqual(61, len(results))
        module.verify_result_set(results, execute=True)

        for item in prepared:
            module.revoke_installation_token(item.token, request_fn=request)

    def test_failed_last_installation_revokes_every_prepared_token_before_mutation(self) -> None:
        events: list[str] = []
        final_organization = fleet.TARGET_ORGANIZATIONS[-1]
        installation_ids = {
            organization.lower(): index
            for index, organization in enumerate(fleet.TARGET_ORGANIZATIONS, start=1)
        }

        def request(method: str, path: str, bearer: str, body=None):
            if method == "GET" and path.endswith("/installation"):
                organization = path.split("/")[2]
                if organization == final_organization:
                    return 404, None
                return 200, {
                    "id": installation_ids[organization.lower()],
                    "app_slug": "fleet-admin-app",
                    "repository_selection": "all",
                    "account": {"login": organization},
                }
            if method == "POST":
                installation_id = int(path.split("/")[3])
                return 201, {
                    "token": f"installation-token-{installation_id}",
                    "permissions": dict(module.REQUIRED_PERMISSIONS),
                }
            if method == "DELETE":
                events.append("revoke")
                return 204, None
            self.fail(f"unexpected request: {method} {path}")

        with self.assertRaises(module.AppPublisherError):
            module.preflight_installations(
                "app-jwt",
                request_fn=request,
                api_factory=DummyApi,
                repository_getter=lambda api, organization: None,
                repository_validator=lambda repository, organization: None,
            )

        self.assertEqual(60, events.count("revoke"))
        self.assertFalse(any(event.startswith("reconcile:") for event in events))

    def test_repository_inspection_failure_revokes_current_and_prepared_tokens(self) -> None:
        revoked: list[str] = []
        organizations = fleet.TARGET_ORGANIZATIONS[:2]
        installation_ids = {
            organization.lower(): index
            for index, organization in enumerate(organizations, start=1)
        }

        def request(method: str, path: str, bearer: str, body=None):
            if method == "GET":
                organization = path.split("/")[2]
                return 200, {
                    "id": installation_ids[organization.lower()],
                    "app_slug": "fleet-admin-app",
                    "repository_selection": "all",
                    "account": {"login": organization},
                }
            if method == "POST":
                installation_id = int(path.split("/")[3])
                return 201, {
                    "token": f"installation-token-{installation_id}",
                    "permissions": dict(module.REQUIRED_PERMISSIONS),
                }
            if method == "DELETE":
                revoked.append(bearer)
                return 204, None
            self.fail(f"unexpected request: {method} {path}")

        def repository_getter(api: DummyApi, organization: str):
            if organization == organizations[1]:
                raise module.AppPublisherError("simulated repository inspection failure")
            return None

        original_targets = fleet.TARGET_ORGANIZATIONS
        try:
            fleet.TARGET_ORGANIZATIONS = organizations
            with self.assertRaises(module.AppPublisherError):
                module.preflight_installations(
                    "app-jwt",
                    request_fn=request,
                    api_factory=DummyApi,
                    repository_getter=repository_getter,
                    repository_validator=lambda repository, organization: None,
                )
        finally:
            fleet.TARGET_ORGANIZATIONS = original_targets

        self.assertCountEqual(
            ["installation-token-1", "installation-token-2"],
            revoked,
        )

    def test_result_set_rejects_missing_or_unverified_organizations(self) -> None:
        complete = [
            base.OrganizationResult(
                organization=organization,
                repository=f"{organization}/.github",
                verified=True,
            )
            for organization in fleet.TARGET_ORGANIZATIONS
        ]
        module.verify_result_set(complete, execute=True)
        with self.assertRaises(module.AppPublisherError):
            module.verify_result_set(complete[:-1], execute=True)
        complete[-1].verified = False
        with self.assertRaises(module.AppPublisherError):
            module.verify_result_set(complete, execute=True)


if __name__ == "__main__":
    unittest.main()
