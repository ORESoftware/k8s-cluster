import unittest

from repository_catalog import diff_catalogs, render_markdown, validate_catalog


class RepositoryCatalogTests(unittest.TestCase):
    def record(self, name: str, **overrides):
        value = {
            "name": name,
            "visibility": "public",
            "default_branch": "main",
            "lifecycle": "active",
            "conformance_profile": "service",
            "canonical_location": name,
            "linear_project": "Shared Platform & Portfolio Architecture",
            "security_class": "public",
            "review_date": "2026-07-28",
            "dependencies": [],
        }
        value.update(overrides)
        return value

    def catalog(self, *records):
        return {"schema_version": 1, "repositories": list(records)}

    def test_valid_catalog(self):
        errors = validate_catalog(self.catalog(self.record("ORESoftware/k8s-cluster")))
        self.assertEqual([], errors)

    def test_duplicate_and_verified_dependency_without_pin(self):
        record = self.record(
            "ORESoftware/k8s-cluster",
            dependencies=[
                {
                    "target": "fiducia-cloud/fiducia-node",
                    "kind": "gitlink",
                    "evidence": ".gitmodules",
                    "verified": True,
                }
            ],
        )
        errors = validate_catalog(self.catalog(record, record.copy()))
        rendered = "\n".join(error.render() for error in errors)
        self.assertIn("duplicate repository", rendered)
        self.assertIn("verified dependencies require an exact pin", rendered)

    def test_diff_reports_add_remove_and_tracked_changes(self):
        baseline = self.catalog(
            self.record("acme/removed"),
            self.record("acme/changed", default_branch="master"),
        )
        current = self.catalog(
            self.record("acme/added"),
            self.record("acme/changed", default_branch="main"),
        )
        report = diff_catalogs(baseline, current)
        self.assertEqual(["acme/added"], report["added"])
        self.assertEqual(["acme/removed"], report["removed"])
        self.assertEqual("master", report["changed"][0]["fields"]["default_branch"]["before"])
        self.assertIn("acme/changed", render_markdown(report))


if __name__ == "__main__":
    unittest.main()
