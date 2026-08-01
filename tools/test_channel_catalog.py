import copy
import json
import tempfile
import unittest
from pathlib import Path

from channel_catalog import (
    GOVERNING_ISSUE,
    SCHEMA_REFERENCE,
    SCHEMA_VERSION,
    eponymous_channel,
    find_public_boundary_violations,
    find_unregistered_owners,
    render_summary,
    validate_catalog,
)


class ChannelCatalogTests(unittest.TestCase):
    def channel(self, owner: str, **overrides):
        value = {
            "owner": owner,
            "slack_channel": eponymous_channel(owner),
            "linear_project": f"github.com/{owner}",
            "binding_state": "unbound",
            "channel_inventoried": True,
            "linear_notifications": {
                "new_issue": True,
                "issue_comments": True,
                "issue_statuses": True,
            },
        }
        value.update(overrides)
        return value

    def catalog(self, *channels):
        return {
            "$schema": SCHEMA_REFERENCE,
            "schema_version": SCHEMA_VERSION,
            "governing_issue": GOVERNING_ISSUE,
            "captured_at": "2026-08-01T00:00:00Z",
            "channels": list(channels) or [self.channel("cliptown")],
        }

    def test_valid_catalog_has_no_errors(self):
        catalog = self.catalog(
            self.channel("cliptown"),
            self.channel("3FA-app", linear_project="github.com/3FA-app"),
        )
        self.assertEqual(validate_catalog(catalog), [])

    def test_mixed_case_owner_maps_to_lowercase_channel(self):
        self.assertEqual(eponymous_channel("ORESoftware"), "#oresoftware")
        self.assertEqual(eponymous_channel("3FA-app"), "#3fa-app")

    def test_non_eponymous_channel_is_rejected(self):
        catalog = self.catalog(self.channel("cliptown", slack_channel="#clip-town"))
        errors = validate_catalog(catalog)
        self.assertTrue(any("not eponymous" in error for error in errors), errors)

    def test_duplicate_channel_is_rejected(self):
        catalog = self.catalog(self.channel("cliptown"), self.channel("cliptown"))
        errors = validate_catalog(catalog)
        self.assertIn("duplicate slack_channel: #cliptown", errors)
        self.assertIn("duplicate owner: cliptown", errors)

    def test_bound_channel_must_be_inventoried(self):
        catalog = self.catalog(
            self.channel(
                "cliptown",
                binding_state="bound",
                channel_inventoried=False,
            )
        )
        errors = validate_catalog(catalog)
        self.assertTrue(
            any("requires channel_inventoried true" in error for error in errors),
            errors,
        )

    def test_unknown_binding_state_is_rejected(self):
        catalog = self.catalog(self.channel("cliptown", binding_state="pending"))
        errors = validate_catalog(catalog)
        self.assertTrue(any("binding_state" in error for error in errors), errors)

    def test_missing_notification_toggle_is_rejected(self):
        entry = self.channel("cliptown")
        del entry["linear_notifications"]["issue_statuses"]
        errors = validate_catalog(self.catalog(entry))
        self.assertTrue(any("issue_statuses" in error for error in errors), errors)

    def test_wrong_governing_issue_is_rejected(self):
        catalog = self.catalog()
        catalog["governing_issue"] = "DEN-1"
        self.assertIn(
            f"governing_issue must be {GOVERNING_ISSUE}", validate_catalog(catalog)
        )

    def test_slack_channel_id_is_a_public_boundary_violation(self):
        raw = json.dumps({"slack_channel_id": "C0BLYPGGFH6"})
        violations = find_public_boundary_violations(raw)
        self.assertTrue(any("C0BLYPGGFH6" in item for item in violations), violations)

    def test_linear_uuid_is_a_public_boundary_violation(self):
        raw = json.dumps({"id": "83b03121-db08-4e34-a69f-99fd1c873ced"})
        violations = find_public_boundary_violations(raw)
        self.assertTrue(any("Linear UUID" in item for item in violations), violations)

    def test_clean_registry_has_no_boundary_violations(self):
        raw = json.dumps(self.catalog())
        self.assertEqual(find_public_boundary_violations(raw), [])

    def test_unregistered_owner_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "catalog").mkdir()
            (root / "catalog" / "owners.json").write_text(
                json.dumps({"owners": [{"owner": "cliptown"}]}),
                encoding="utf-8",
            )
            catalog = self.catalog(self.channel("cliptown"), self.channel("memebank"))
            problems = find_unregistered_owners(catalog, root)
            self.assertEqual(problems, ["memebank: not present in catalog/owners.json"])

    def test_render_summary_lists_every_channel(self):
        catalog = self.catalog(
            self.channel("cliptown"),
            self.channel("zed-pkg", binding_state="bound"),
        )
        text = render_summary(catalog)
        self.assertIn("tracked channels: 2", text)
        self.assertIn("bound: 1", text)
        self.assertIn("unbound: 1", text)
        self.assertIn("`#cliptown`", text)

    def test_committed_registry_is_valid_and_public_safe(self):
        root = Path(__file__).resolve().parent.parent
        path = root / "catalog" / "channels.json"
        catalog = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(validate_catalog(catalog), [])
        self.assertEqual(
            find_public_boundary_violations(path.read_text(encoding="utf-8")), []
        )
        # Deep-copying and revalidating guards against shared-state mutation.
        self.assertEqual(validate_catalog(copy.deepcopy(catalog)), [])


if __name__ == "__main__":
    unittest.main()
