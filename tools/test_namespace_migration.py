#!/usr/bin/env python3
"""Unit and adversarial tests for the DEN-2786 namespace contract."""

from __future__ import annotations

import contextlib
import copy
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from namespace_migration import (
    DEFAULT_REGISTRY,
    DEFAULT_RULES,
    Reference,
    added_lines_from_diff,
    build_check_report,
    classify_reference,
    discovered_owner_diagnostics,
    inventory_report,
    load_contract,
    main,
    ratchet_report,
    scan_line,
    scan_repository,
    validate_registry,
    validate_rules,
)

SOURCE_ROOT = Path(__file__).resolve().parents[1]


def read_json(relative: str) -> dict:
    return json.loads((SOURCE_ROOT / relative).read_text(encoding="utf-8"))


def git(root: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip()


class ContractValidationTests(unittest.TestCase):
    def test_checked_in_contract_is_valid(self) -> None:
        contract = load_contract(SOURCE_ROOT)
        errors = [item for item in contract.diagnostics if item.severity == "error"]
        self.assertEqual([], errors)
        self.assertGreaterEqual(len(contract.owners), 20)
        self.assertGreaterEqual(len(contract.rules), 20)

    def test_json_schemas_are_present_and_parseable(self) -> None:
        registry_schema = read_json("catalog/namespaces/owner-registry.schema.json")
        rules_schema = read_json("catalog/namespaces/migration-rules.schema.json")
        self.assertEqual("https://json-schema.org/draft/2020-12/schema", registry_schema["$schema"])
        self.assertEqual("https://json-schema.org/draft/2020-12/schema", rules_schema["$schema"])
        self.assertFalse(registry_schema["additionalProperties"])
        self.assertFalse(rules_schema["additionalProperties"])

    def test_alias_cannot_collide_with_later_namespace_id(self) -> None:
        registry = read_json(DEFAULT_REGISTRY)
        registry["spec"]["owners"][0]["aliases"].append("future-owner")
        registry["spec"]["owners"].append(
            {
                "namespaceId": "future-owner",
                "githubOwner": "future-owner",
                "kind": "product",
                "aliases": [],
                "description": "Test owner used to prove alias collision detection.",
            }
        )
        _, diagnostics = validate_registry(registry, path=DEFAULT_REGISTRY)
        self.assertIn("registry.alias-collision", {item.rule_id for item in diagnostics})

    def test_duplicate_github_owner_is_rejected_case_insensitively(self) -> None:
        registry = read_json(DEFAULT_REGISTRY)
        registry["spec"]["owners"].append(
            {
                "namespaceId": "another-ores",
                "githubOwner": "oresoftware",
                "kind": "product",
                "aliases": [],
                "description": "Invalid duplicate GitHub owner.",
            }
        )
        _, diagnostics = validate_registry(registry, path=DEFAULT_REGISTRY)
        self.assertIn(
            "registry.duplicate-github-owner",
            {item.rule_id for item in diagnostics},
        )

    def test_product_rule_cannot_launder_target_into_ores(self) -> None:
        registry = read_json(DEFAULT_REGISTRY)
        owners, registry_diagnostics = validate_registry(registry, path=DEFAULT_REGISTRY)
        self.assertFalse([item for item in registry_diagnostics if item.severity == "error"])
        rules = read_json(DEFAULT_RULES)
        candidate = next(
            item for item in rules["spec"]["rules"] if item["id"] == "remote-dev.sonus-auris"
        )
        candidate["targetTemplate"] = "ores/dev/sonus/{suffix}"
        _, diagnostics = validate_rules(
            rules,
            path=DEFAULT_RULES,
            owner_index={item.namespace_id: item for item in owners},
        )
        self.assertIn("rules.owner-root", {item.rule_id for item in diagnostics})

    def test_unclassified_rule_cannot_have_target(self) -> None:
        registry = read_json(DEFAULT_REGISTRY)
        owners, _ = validate_registry(registry, path=DEFAULT_REGISTRY)
        rules = read_json(DEFAULT_RULES)
        fallback = next(
            item for item in rules["spec"]["rules"] if item["id"] == "fallback.slash-namespace"
        )
        fallback["targetTemplate"] = "ores/shared/unknown/{suffix}"
        _, diagnostics = validate_rules(
            rules,
            path=DEFAULT_RULES,
            owner_index={item.namespace_id: item for item in owners},
        )
        self.assertIn("rules.unclassified-target", {item.rule_id for item in diagnostics})

    def test_unknown_cross_owner_consumer_is_rejected(self) -> None:
        registry = read_json(DEFAULT_REGISTRY)
        owners, _ = validate_registry(registry, path=DEFAULT_REGISTRY)
        rules = read_json(DEFAULT_RULES)
        rules["spec"]["rules"][0]["consumers"] = ["not-a-registered-owner"]
        _, diagnostics = validate_rules(
            rules,
            path=DEFAULT_RULES,
            owner_index={item.namespace_id: item for item in owners},
        )
        self.assertIn("rules.unknown-consumer", {item.rule_id for item in diagnostics})

    def test_exact_target_collision_is_rejected(self) -> None:
        registry = read_json(DEFAULT_REGISTRY)
        owners, _ = validate_registry(registry, path=DEFAULT_REGISTRY)
        rules = read_json(DEFAULT_RULES)
        duplicate = copy.deepcopy(
            next(item for item in rules["spec"]["rules"] if item["id"] == "metadata.thread-id")
        )
        duplicate["id"] = "metadata.other-thread-id"
        duplicate["match"]["value"] = "dd/otherThreadId"
        rules["spec"]["rules"].append(duplicate)
        _, diagnostics = validate_rules(
            rules,
            path=DEFAULT_RULES,
            owner_index={item.namespace_id: item for item in owners},
        )
        self.assertIn("rules.target-collision", {item.rule_id for item in diagnostics})


class ClassificationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.contract = load_contract(SOURCE_ROOT)
        if not cls.contract.valid:
            raise AssertionError(cls.contract.diagnostics)

    def test_metadata_key_wins_over_generic_slash_pattern(self) -> None:
        references = scan_line('labels: {"dd/threadId": "abc"}')
        self.assertEqual(1, len(references))
        self.assertEqual("metadata-key", references[0].system)
        self.assertEqual("dd/threadId", references[0].value)

    def test_specific_fiducia_rule_beats_remote_dev_fallback(self) -> None:
        rule, target = classify_reference(
            Reference("slash-namespace", "dd/remote-dev/fiducia-signing", 1),
            self.contract.rules,
        )
        self.assertIsNotNone(rule)
        assert rule is not None
        self.assertEqual("remote-dev.fiducia", rule.rule_id)
        self.assertEqual("fiducia-cloud", rule.owner)
        self.assertEqual("fiducia-cloud/dev/signing", target)

    def test_unknown_remote_dev_reference_remains_unclassified(self) -> None:
        rule, target = classify_reference(
            Reference("slash-namespace", "dd/remote-dev/unknown-service", 1),
            self.contract.rules,
        )
        self.assertIsNotNone(rule)
        assert rule is not None
        self.assertEqual("fallback.remote-dev", rule.rule_id)
        self.assertEqual("unclassified", rule.owner)
        self.assertIsNone(target)

    def test_known_shared_auth_rule_keeps_environment_placeholder_visible(self) -> None:
        rule, target = classify_reference(
            Reference("slash-namespace", "dd/shared-auth/signing", 1),
            self.contract.rules,
        )
        self.assertIsNotNone(rule)
        assert rule is not None
        self.assertEqual("shared-auth", rule.owner)
        self.assertEqual(
            "shared-auth/{environment}/shared-auth-server/signing",
            target,
        )

    def test_scanner_trims_yaml_and_markdown_punctuation(self) -> None:
        references = scan_line("- secret: `dd/remote-dev/gha-clone-server-secrets`,")
        self.assertEqual("dd/remote-dev/gha-clone-server-secrets", references[0].value)

    def test_scanner_does_not_truncate_hyphenated_host_siblings(self) -> None:
        for line in (
            "WORKDIR /opt/dd-akka-ws-server",
            "cache=/var/lib/dd-cache",
            "checkout=/srv/dd-next",
            "checkout=/home/ec2-user/codes/dd-next-1",
        ):
            with self.subTest(line=line):
                self.assertEqual([], scan_line(line))

    def test_scanner_does_not_truncate_repository_owner_prefix(self) -> None:
        self.assertEqual(
            [],
            scan_line("module github.com/oresoftware/dd-next-1/remote/service"),
        )
        references = scan_line(
            "require github.com/oresoftware/dd/libs/telemetry-go v0.0.0"
        )
        self.assertEqual(1, len(references))
        self.assertEqual(
            "github.com/oresoftware/dd/libs/telemetry-go",
            references[0].value,
        )

    def test_scanner_does_not_truncate_reverse_dns_package_prefix(self) -> None:
        self.assertEqual([], scan_line("package com.oresoftware.ddnext.service"))
        references = scan_line("package com.oresoftware.dd.runtime")
        self.assertEqual(1, len(references))
        self.assertEqual("com.oresoftware.dd.runtime", references[0].value)

    def test_scanner_does_not_misclassify_longer_metadata_name(self) -> None:
        references = scan_line('labels: {"dd/threadIdentifier": "abc"}')
        self.assertEqual(1, len(references))
        self.assertEqual("slash-namespace", references[0].system)
        self.assertEqual("dd/threadIdentifier", references[0].value)

    def test_scanner_does_not_truncate_dd_dev_metadata_name(self) -> None:
        self.assertEqual([], scan_line('annotations: {"dd.dev/fiducia-key/child": "1"}'))
        references = scan_line('annotations: {"dd.dev/fiducia-key": "1"}')
        self.assertEqual(1, len(references))
        self.assertEqual("dd.dev/fiducia-key", references[0].value)

    def test_scanner_keeps_real_host_subpaths(self) -> None:
        references = scan_line(
            "install=/opt/dd/bin/bootstrap-cluster.sh "
            "state=/var/lib/dd/nats "
            "repo=/home/ec2-user/codes/dd/dd-next-1"
        )
        self.assertEqual(
            [
                "/opt/dd/bin/bootstrap-cluster.sh",
                "/var/lib/dd/nats",
                "/home/ec2-user/codes/dd/dd-next-1",
            ],
            [item.value for item in references],
        )


class RepositoryInventoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        git(self.root, "init", "-q")
        git(self.root, "config", "user.email", "namespace-contract@example.invalid")
        git(self.root, "config", "user.name", "Namespace Contract Test")
        for relative in (
            DEFAULT_REGISTRY,
            DEFAULT_RULES,
        ):
            destination = self.root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text((SOURCE_ROOT / relative).read_text(encoding="utf-8"), encoding="utf-8")

    def tearDown(self) -> None:
        self.directory.cleanup()

    def write(self, relative: str, content: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def track_all(self) -> None:
        git(self.root, "add", ".")

    def test_inventory_separates_active_documentation_and_test_scope(self) -> None:
        self.write("remote/app.yaml", "secret: dd/remote-dev/gha-clone-server-secrets\n")
        self.write("docs/example.md", "Historical: dd/remote-dev/unknown-doc\n")
        self.write("tests/example_test.py", 'VALUE = "dd/shared-auth/signing"\n')
        self.track_all()
        contract = load_contract(self.root)
        occurrences, diagnostics = scan_repository(self.root, contract.rules)
        self.assertEqual([], diagnostics)
        self.assertEqual(
            {"active", "documentation", "test"},
            {item.scope for item in occurrences},
        )

    def test_governance_catalog_is_excluded_from_default_inventory(self) -> None:
        self.track_all()
        contract = load_contract(self.root)
        occurrences, _ = scan_repository(self.root, contract.rules)
        self.assertEqual([], occurrences)
        included, _ = scan_repository(self.root, contract.rules, include_governance=True)
        self.assertGreater(len(included), 0)
        self.assertTrue(all(item.scope == "governance" for item in included))

    def test_unregistered_gitops_owner_is_reported_as_warning(self) -> None:
        self.write(
            "catalog/gitops/apps/example.json",
            json.dumps({"spec": {"owner": "brand-new-org"}}) + "\n",
        )
        self.track_all()
        contract = load_contract(self.root)
        diagnostics = discovered_owner_diagnostics(self.root, contract.owners)
        self.assertEqual(1, len(diagnostics))
        self.assertEqual("warning", diagnostics[0].severity)
        self.assertIn("brand-new-org", diagnostics[0].message)

    def test_strict_mode_fails_active_unclassified_references(self) -> None:
        self.write("remote/unknown.yaml", "secret: dd/remote-dev/not-classified\n")
        self.track_all()
        report, status = build_check_report(
            self.root,
            registry_path=DEFAULT_REGISTRY,
            rules_path=DEFAULT_RULES,
            strict_unclassified=True,
        )
        self.assertEqual(2, status)
        self.assertFalse(report["valid"])
        self.assertIn(
            "inventory.unclassified-active",
            {item["rule_id"] for item in report["diagnostics"]},
        )

    def test_inventory_json_contains_classification_and_target_preview(self) -> None:
        self.write("remote/known.yaml", "secret: dd/remote-dev/gha-clone-server-secrets\n")
        self.track_all()
        report, status = inventory_report(
            self.root,
            registry_path=DEFAULT_REGISTRY,
            rules_path=DEFAULT_RULES,
            include_governance=False,
        )
        self.assertEqual(0, status)
        occurrence = report["occurrences"][0]
        self.assertEqual("ores", occurrence["owner"])
        self.assertEqual("ores/dev/ci/clone-server-secrets", occurrence["target_preview"])


class RatchetTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        git(self.root, "init", "-q")
        git(self.root, "config", "user.email", "namespace-contract@example.invalid")
        git(self.root, "config", "user.name", "Namespace Contract Test")

    def tearDown(self) -> None:
        self.directory.cleanup()

    def commit(self, message: str) -> str:
        git(self.root, "add", ".")
        git(self.root, "commit", "-q", "-m", message)
        return git(self.root, "rev-parse", "HEAD")

    def test_added_line_parser_tracks_new_file_line_numbers(self) -> None:
        diff = """diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,1 +1,3 @@
 old
+first
+second
"""
        self.assertEqual(
            [("a.txt", 2, "first"), ("a.txt", 3, "second")],
            list(added_lines_from_diff(diff)),
        )

    def test_ratchet_ignores_preexisting_debt_and_blocks_only_new_reference(self) -> None:
        (self.root / "deploy.yaml").write_text("old: dd/existing/debt\n", encoding="utf-8")
        base = self.commit("base")
        (self.root / "deploy.yaml").write_text(
            "old: dd/existing/debt\nnew: dd/new/debt\n",
            encoding="utf-8",
        )
        head = self.commit("head")
        report, status = ratchet_report(self.root, base, head)
        self.assertEqual(2, status)
        self.assertEqual(1, len(report["violations"]))
        self.assertEqual("dd/new/debt", report["violations"][0]["reference"])

    def test_ratchet_allows_governance_contract_to_describe_legacy_names(self) -> None:
        (self.root / "README.md").write_text("base\n", encoding="utf-8")
        base = self.commit("base")
        path = self.root / "docs/namespace-migration.md"
        path.parent.mkdir(parents=True)
        path.write_text("legacy example: dd/remote-dev/example\n", encoding="utf-8")
        head = self.commit("head")
        report, status = ratchet_report(self.root, base, head)
        self.assertEqual(0, status)
        self.assertTrue(report["valid"])

    def test_ratchet_accepts_reviewed_single_line_exception_marker(self) -> None:
        (self.root / "README.md").write_text("base\n", encoding="utf-8")
        base = self.commit("base")
        (self.root / "deploy.yaml").write_text(
            "secret: dd/temporary/path # namespace-migration: allow-legacy\n",
            encoding="utf-8",
        )
        head = self.commit("head")
        report, status = ratchet_report(self.root, base, head)
        self.assertEqual(0, status)
        self.assertTrue(report["valid"])

    def test_ratchet_passes_when_new_code_uses_registered_target(self) -> None:
        (self.root / "README.md").write_text("base\n", encoding="utf-8")
        base = self.commit("base")
        (self.root / "deploy.yaml").write_text(
            "secret: fiducia-cloud/prod/fiducia-node/runtime\n",
            encoding="utf-8",
        )
        head = self.commit("head")
        report, status = ratchet_report(self.root, base, head)
        self.assertEqual(0, status)
        self.assertTrue(report["valid"])


class CliTests(unittest.TestCase):
    def test_check_command_emits_json(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = main(["check", "--root", str(SOURCE_ROOT), "--format", "json"])
        self.assertEqual(0, status)
        payload = json.loads(output.getvalue())
        self.assertTrue(payload["valid"])
        self.assertEqual("NamespaceOwnerRegistry", payload["contract"]["registryKind"])


if __name__ == "__main__":
    unittest.main()
