#!/usr/bin/env python3
"""One-shot refinement for DEN-2926; deletes itself before committing evidence."""
from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASE_SHA = "fe434f4cb752c14b3f7db2d03031d57c299e2dad"
PATCH_COMMIT = "46c3b9d08330e26aee03a98b682adca5b44d94ea"
PATCH_BLOB = "f3e0ee581df64c78eae46d8f903361ef9f5d03f8"
PATCH_SHA256 = "cdc9799efb1e5ed9656b5aee5f19aad47a486448fa18df1115ca5918673a2af2"
TEMPORARY_PATHS = (
    ".den-2926-refine-trigger",
    ".github/workflows/den-2926-refine-run.yml",
    ".github/workflows/den-2926-refine-scanner.yml",
    "tools/den2926_refine_once.py",
)


def run(*args: str, cwd: Path = ROOT, capture: bool = False) -> str:
    completed = subprocess.run(
        list(args),
        cwd=cwd,
        check=True,
        text=True,
        capture_output=capture,
    )
    return completed.stdout if capture else ""


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old in text:
        return text.replace(old, new, 1)
    if new in text:
        return text
    raise RuntimeError(f"{label} changed unexpectedly")


def patch_source() -> None:
    path = ROOT / "tools/namespace_migration.py"
    text = path.read_text(encoding="utf-8")
    text = replace_once(
        text,
        '    ".github/workflows/namespace-migration-contract.yml",\n)',
        '    ".github/workflows/namespace-migration-contract.yml",\n'
        '    "artifacts/namespace-inventory.json",\n'
        '    "artifacts/den-2926-inventory-delta.json",\n'
        ')',
        "governance evidence exclusions",
    )
    text = replace_once(
        text,
        'r"(?:/[A-Za-z0-9._~:@%+,/-]+)*"',
        'r"(?:/[A-Za-z0-9._~:@%+,-]+)*"',
        "GitHub path segments",
    )
    text = replace_once(
        text,
        'r"(?![A-Za-z0-9._~:@%+=/-])",',
        'r"(?![A-Za-z0-9._~:@%+=-])",',
        "GitHub trailing boundary",
    )
    text = replace_once(
        text,
        'r"(?![A-Za-z0-9._~@%+=/-])"',
        'r"(?![A-Za-z0-9._~@%+=-])"',
        "host trailing boundary",
    )
    text = replace_once(
        text,
        '"generatedFrom": {"root": root.resolve().as_posix(),',
        '"generatedFrom": {"root": ".",',
        "reproducible inventory root",
    )
    path.write_text(text, encoding="utf-8")


def patch_tests() -> None:
    path = ROOT / "tools/test_namespace_migration.py"
    text = path.read_text(encoding="utf-8")

    template_test = '''    def test_scanner_preserves_owner_prefix_before_template_segments(self) -> None:
        host_line = 'path = "/home/ec2-user/codes/dd/thread-workspaces/{name}"'
        host_references = scan_line(host_line)
        self.assertEqual(1, len(host_references))
        self.assertEqual("host-path", host_references[0].system)
        self.assertEqual(
            "/home/ec2-user/codes/dd/thread-workspaces",
            host_references[0].value,
        )

        package_line = 'module = "github.com/oresoftware/dd/libs/{generated}"'
        package_references = scan_line(package_line)
        self.assertEqual(1, len(package_references))
        self.assertEqual("source-package", package_references[0].system)
        self.assertEqual(
            "github.com/oresoftware/dd/libs",
            package_references[0].value,
        )

'''
    marker = "\n\nclass RepositoryInventoryTests(unittest.TestCase):\n"
    if template_test not in text:
        if marker not in text:
            raise RuntimeError("template test insertion point changed")
        text = text.replace(marker, "\n\n" + template_test + "class RepositoryInventoryTests(unittest.TestCase):\n", 1)

    governance_test = '''    def test_generated_inventory_artifacts_are_governance(self) -> None:
        self.write(
            "artifacts/namespace-inventory.json",
            '{"reference": "dd/should-not-rescan"}\\n',
        )
        self.write(
            "artifacts/den-2926-inventory-delta.json",
            '{"reference": "/opt/dd-should-not-rescan"}\\n',
        )
        self.track_all()
        contract = load_contract(self.root)
        occurrences, diagnostics = scan_repository(self.root, contract.rules)
        self.assertEqual([], diagnostics)
        self.assertEqual([], occurrences)

'''
    inventory_marker = "    def test_inventory_separates_active_documentation_and_test_scope(self) -> None:\n"
    if governance_test not in text:
        if inventory_marker not in text:
            raise RuntimeError("governance test insertion point changed")
        text = text.replace(inventory_marker, governance_test + inventory_marker, 1)

    root_assertion = '        self.assertEqual(".", report["generatedFrom"]["root"])\n'
    assertion_marker = '        self.assertEqual(0, status)\n        occurrence = report["occurrences"][0]\n'
    if root_assertion not in text:
        if assertion_marker not in text:
            raise RuntimeError("inventory root assertion point changed")
        text = text.replace(
            assertion_marker,
            '        self.assertEqual(0, status)\n'
            + root_assertion
            + '        occurrence = report["occurrences"][0]\n',
            1,
        )

    path.write_text(text, encoding="utf-8")


def remove_temporary_paths() -> None:
    for relative in TEMPORARY_PATHS:
        path = ROOT / relative
        if path.exists():
            path.unlink()


def generate_inventory(root: Path) -> dict:
    output = run(
        "python3",
        "tools/namespace_migration.py",
        "inventory",
        "--root",
        ".",
        "--format",
        "json",
        cwd=root,
        capture=True,
    )
    return json.loads(output)


def identity(item: dict) -> tuple:
    return (
        item["path"],
        item["line"],
        item["column"],
        item["system"],
        item["reference"],
    )


def build_evidence() -> None:
    artifacts = ROOT / "artifacts"
    artifacts.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="den-2926-before-") as directory:
        before_root = Path(directory) / "repo"
        run("git", "worktree", "add", "--detach", str(before_root), BASE_SHA)
        try:
            before = generate_inventory(before_root)
        finally:
            run("git", "worktree", "remove", "--force", str(before_root))

    after = generate_inventory(ROOT)
    if after["generatedFrom"]["root"] != ".":
        raise RuntimeError("inventory root is not reproducible")
    if any(item["path"].startswith("artifacts/") for item in after["occurrences"]):
        raise RuntimeError("inventory recursively scanned committed evidence")

    before_items = {identity(item): item for item in before["occurrences"]}
    after_items = {identity(item): item for item in after["occurrences"]}
    removed = [before_items[key] for key in sorted(before_items.keys() - after_items.keys())]
    added = [after_items[key] for key in sorted(after_items.keys() - before_items.keys())]
    before_summary = before["summary"]
    after_summary = after["summary"]

    expected = {
        "total": 1134,
        "host-path": 191,
        "slash-namespace": 380,
        "source-package": 114,
        "metadata-key": 367,
        "generated-package": 82,
        "removed": 405,
        "added": 0,
    }
    actual = {
        "total": after_summary["total"],
        **after_summary["bySystem"],
        "removed": len(removed),
        "added": len(added),
    }
    for key, value in expected.items():
        if actual.get(key) != value:
            raise RuntimeError(f"unexpected refined inventory {key}: {actual.get(key)} != {value}")

    systems = sorted(set(before_summary["bySystem"]) | set(after_summary["bySystem"]))
    delta = {
        "valid": True,
        "ticket": "DEN-2926",
        "beforeSha": BASE_SHA,
        "before": before_summary,
        "after": after_summary,
        "delta": {
            "total": after_summary["total"] - before_summary["total"],
            "distinctReferences": after_summary["distinctReferences"] - before_summary["distinctReferences"],
            "unclassifiedActive": after_summary["unclassifiedActive"] - before_summary["unclassifiedActive"],
            "bySystem": {
                name: after_summary["bySystem"].get(name, 0)
                - before_summary["bySystem"].get(name, 0)
                for name in systems
            },
        },
        "removedOccurrenceCount": len(removed),
        "addedOccurrenceCount": len(added),
        "removedSamples": removed[:50],
        "addedSamples": added[:50],
        "sourcePatch": {
            "repository": "fiducia-cloud-test/infra-multicloud-e2e",
            "commit": PATCH_COMMIT,
            "gitBlob": PATCH_BLOB,
            "sha256": PATCH_SHA256,
        },
        "note": (
            "Generated after removing one-shot transport files. The committed inventory "
            "and delta are governance evidence and are excluded from scanning."
        ),
    }

    (artifacts / "namespace-inventory.json").write_text(
        json.dumps(after, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (artifacts / "den-2926-inventory-delta.json").write_text(
        json.dumps(delta, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    reproduced = generate_inventory(ROOT)
    if reproduced != after:
        raise RuntimeError("committed inventory is not reproducible before commit")
    print(json.dumps({"before": before_summary, "after": after_summary, "delta": delta["delta"]}, indent=2))


def main() -> int:
    patch_source()
    patch_tests()
    remove_temporary_paths()
    run("python3", "-m", "py_compile", "tools/namespace_migration.py", "tools/test_namespace_migration.py")
    run("python3", "tools/test_namespace_migration.py")
    run("python3", "tools/test_namespace_test_owner_contract.py")
    contract = json.loads(
        run(
            "python3",
            "tools/namespace_migration.py",
            "check",
            "--root",
            ".",
            "--format",
            "json",
            capture=True,
        )
    )
    if not contract.get("valid") or contract.get("diagnostics"):
        raise RuntimeError("namespace contract is not clean")
    build_evidence()
    run("git", "diff", "--check")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
