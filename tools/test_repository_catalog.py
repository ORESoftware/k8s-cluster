import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from subprocess import CompletedProcess
from unittest import mock

from repository_catalog import (
    DEN369_ISSUE,
    DEN637_ISSUE,
    _private_output_is_safe,
    build_dashboard,
    collect_catalog,
    diff_catalogs,
    merge_den369,
    render_drift_markdown,
    validate_catalog,
)


class RepositoryCatalogTests(unittest.TestCase):
    def record(self, name: str, **overrides):
        value = {
            "name": name,
            "repository_id": 123,
            "visibility": "public",
            "fork": False,
            "archived": False,
            "empty": False,
            "default_branch": "main",
            "canonical_location": name,
            "classification": {
                "lifecycle": "active",
                "profile": "infrastructure/GitOps repository",
                "evidence_state": "verified",
                "source": "fixture",
            },
            "ownership": {
                "linear_project": "Portfolio Repository Conformance",
                "linear_issue": "DEN-599",
                "linear_issue_url": "https://linear.app/denman/issue/DEN-599",
            },
            "release": {
                "state": "continuous-delivery",
                "deployment_state": "active",
            },
            "consumers": [],
            "dependencies": [],
            "security": {
                "security_class": "public",
                "data_class": "configuration-no-production-secrets",
            },
            "exemptions": [],
            "review": {
                "status": "reviewed",
                "issue": "DEN-599",
                "review_date": "2026-07-28",
                "reason": "fixture reviewed",
            },
            "conformance": {
                "state": "conformant",
                "issue": "DEN-599",
                "evidence_state": "verified",
            },
            "nix_oci": {
                "issue": DEN369_ISSUE,
                "classification": "full flake",
                "evidence_state": "verified",
                "reason": "fixture",
                "nix": {},
                "container": {},
                "source_artifact_sha256": "not-imported",
            },
            "zed": {
                "applicable": False,
                "state": "not-applicable",
                "issue": "",
            },
        }
        value.update(overrides)
        return value

    def catalog(self, *records, scope="fixture", den369=None):
        return {
            "schema_version": 2,
            "snapshot": {
                "id": "fixture",
                "captured_at": "2026-07-28T00:00:00Z",
                "record_scope": scope,
                "source": "unit-test fixture",
                "governing_issue": "DEN-627",
            },
            "inventory": {
                "repository_count": len(records),
                "public_count": sum(item["visibility"] == "public" for item in records),
                "private_count": sum(
                    item["visibility"] != "public" for item in records
                ),
                "fork_count": sum(item["fork"] for item in records),
                "archived_count": sum(item["archived"] for item in records),
                "empty_count": sum(item["empty"] for item in records),
                "canonical_active_count": len(records),
                "owner_count": len({item["name"].split("/", 1)[0] for item in records}),
            },
            "imports": {
                "den369": den369
                or {
                    "issue": DEN369_ISSUE,
                    "contract": "nix-fleet-audit/report.json@v1",
                    "source_path": "not-imported",
                    "artifact_sha256": "not-imported",
                }
            },
            "repositories": list(records),
        }

    def test_valid_catalog(self):
        errors = validate_catalog(self.catalog(self.record("ORESoftware/k8s-cluster")))
        self.assertEqual([], errors)

    def test_public_safe_catalog_rejects_private_names(self):
        private = self.record(
            "private-org/private-repository",
            visibility="private",
            security={"security_class": "confidential", "data_class": "confidential"},
        )
        errors = validate_catalog(self.catalog(private), public_safe=True)
        self.assertIn(
            "public-safe catalogs may only name public repositories",
            "\n".join(error.render() for error in errors),
        )

    def test_duplicate_and_verified_dependency_without_pin(self):
        record = self.record(
            "ORESoftware/k8s-cluster",
            dependencies=[
                {
                    "target": "fiducia-cloud/fiducia-node",
                    "kind": "gitlink",
                    "state": "verified",
                    "source_evidence": {
                        "repository": "ORESoftware/k8s-cluster",
                        "path": ".gitmodules",
                        "immutable_ref": "0123456789abcdef",
                    },
                }
            ],
        )
        errors = validate_catalog(self.catalog(record, copy.deepcopy(record)))
        rendered = "\n".join(error.render() for error in errors)
        self.assertIn("duplicate repository", rendered)
        self.assertIn("verified dependencies require an exact pin", rendered)

    def test_den369_artifact_hash_is_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "report.json"
            artifact.write_text("[]\n", encoding="utf-8")
            den369 = {
                "issue": DEN369_ISSUE,
                "contract": "nix-fleet-audit/report.json@v1",
                "source_path": "report.json",
                "artifact_sha256": "0" * 64,
            }
            errors = validate_catalog(
                self.catalog(self.record("ORESoftware/k8s-cluster"), den369=den369),
                repo_root=root,
            )
            self.assertIn(
                "does not match source artifact",
                "\n".join(error.render() for error in errors),
            )

    def test_diff_reports_all_governed_drift_categories(self):
        baseline_record = self.record(
            "acme/changed",
            default_branch="master",
            dependencies=[
                {
                    "target": "acme/contract",
                    "kind": "deployment-pin",
                    "state": "verified",
                    "pin": "sha-old",
                    "source_evidence": {
                        "repository": "acme/changed",
                        "path": "deploy.yaml",
                        "immutable_ref": "sha-old",
                    },
                }
            ],
        )
        current_record = copy.deepcopy(baseline_record)
        current_record["default_branch"] = "main"
        current_record["canonical_location"] = "acme-renamed/changed"
        current_record["ownership"]["linear_issue"] = "DEN-600"
        current_record["dependencies"][0]["pin"] = "sha-new"
        current_record["conformance"]["state"] = "gap-owned"
        current_record["classification"]["profile"] = "shared library/SDK/tool"
        current_record["zed"] = {
            "applicable": True,
            "state": "gap-owned",
            "issue": DEN637_ISSUE,
            "manifest": "missing",
            "lock": "missing",
            "source_pin": "missing",
            "ci_gate": "missing",
        }
        baseline = self.catalog(self.record("acme/removed"), baseline_record)
        current = self.catalog(self.record("acme/added"), current_record)
        current["inventory"]["empty_count"] = 1
        report = diff_catalogs(baseline, current)
        self.assertEqual(["acme/added"], report["added"])
        self.assertEqual(["acme/removed"], report["removed"])
        for key in (
            "ownership_moves",
            "default_branch_changes",
            "pin_drift",
            "conformance_regressions",
            "classification_changes",
            "zed_drift",
            "inventory_changes",
        ):
            self.assertEqual(1, report["summary"][key], key)
        self.assertIn("acme/changed", render_drift_markdown(report))

    def test_dashboard_routes_repository_and_zed_gaps(self):
        record = self.record("zed-pkg/example-client")
        record["review"] = {
            "status": "needs-review",
            "issue": "DEN-612",
            "review_date": "2026-07-28",
            "reason": "owner review required",
        }
        record["conformance"]["state"] = "gap-owned"
        record["zed"] = {
            "applicable": True,
            "state": "gap-owned",
            "issue": DEN637_ISSUE,
            "manifest": "missing",
            "lock": "missing",
            "source_pin": "missing",
            "ci_gate": "missing",
        }
        dashboard = build_dashboard(self.catalog(record))
        self.assertEqual(1, dashboard["owners"][0]["needs_review"])
        self.assertEqual(1, dashboard["owners"][0]["zed_gaps"])
        self.assertEqual(
            {"DEN-599", DEN637_ISSUE}, {item["issue"] for item in dashboard["actions"]}
        )

    def test_approved_exemption_is_not_a_conformance_regression(self):
        baseline_record = self.record("acme/exempted")
        current_record = copy.deepcopy(baseline_record)
        current_record["conformance"]["state"] = "exempt"
        report = diff_catalogs(
            self.catalog(baseline_record),
            self.catalog(current_record),
        )
        self.assertEqual([], report["conformance_regressions"])

    def test_merge_den369_records_artifact_provenance(self):
        artifact = [
            {
                "repository": "ORESoftware/k8s-cluster",
                "classification": "full flake",
                "reason": "flake.nix and flake.lock",
                "nix": {"flake": True},
                "container": {"containerfile": True},
            }
        ]
        digest = hashlib.sha256(json.dumps(artifact).encode()).hexdigest()
        merged = merge_den369(
            self.catalog(self.record("ORESoftware/k8s-cluster")),
            artifact,
            source_path="catalog/fixtures/den369-report.json",
            artifact_sha256=digest,
        )
        self.assertEqual(digest, merged["imports"]["den369"]["artifact_sha256"])
        self.assertEqual(
            "full flake", merged["repositories"][0]["nix_oci"]["classification"]
        )

    @mock.patch("repository_catalog.subprocess.run")
    def test_collection_filters_to_contract_and_redacts_private_records(self, run):
        response = [
            [
                {
                    "id": 1,
                    "full_name": "ORESoftware/public",
                    "name": "public",
                    "owner": {"login": "ORESoftware"},
                    "visibility": "public",
                    "fork": False,
                    "archived": False,
                    "size": 10,
                    "default_branch": "main",
                },
                {
                    "id": 2,
                    "full_name": "ORESoftware/private",
                    "name": "private",
                    "owner": {"login": "ORESoftware"},
                    "visibility": "private",
                    "fork": False,
                    "archived": False,
                    "size": 10,
                    "default_branch": "main",
                },
                {
                    "id": 3,
                    "full_name": "outside/ignored",
                    "name": "ignored",
                    "owner": {"login": "outside"},
                    "visibility": "public",
                    "fork": False,
                    "archived": False,
                    "size": 10,
                    "default_branch": "main",
                },
            ]
        ]
        run.return_value = CompletedProcess([], 0, json.dumps(response), "")
        owners = {
            "baseline": {
                "repository_count": 2,
                "public_count": 1,
                "private_count": 1,
                "fork_count": 0,
                "archived_count": 0,
                "empty_count": 0,
                "canonical_active_count": 2,
            },
            "owners": [
                {
                    "owner": "ORESoftware",
                    "linear_project": "Portfolio Repository Conformance",
                    "linear_issue": "DEN-599",
                }
            ],
        }
        catalog = collect_catalog(
            owners,
            captured_at="2026-07-28T00:00:00Z",
            visibility_mode="public",
        )
        self.assertEqual(
            ["ORESoftware/public"], [item["name"] for item in catalog["repositories"]]
        )
        self.assertEqual(2, catalog["inventory"]["repository_count"])
        self.assertEqual(1, catalog["inventory"]["private_count"])
        self.assertEqual(1, catalog["inventory"]["owner_count"])
        self.assertEqual(0, catalog["inventory"]["baseline_deltas"]["repository_count"])
        self.assertEqual([], validate_catalog(catalog, public_safe=True))

    def test_full_output_must_be_outside_checkout(self):
        root = Path("/workspace/repository")
        self.assertFalse(_private_output_is_safe(root / "catalog/full.json", root))
        self.assertTrue(_private_output_is_safe(Path("/secure/full.json"), root))


if __name__ == "__main__":
    unittest.main()
