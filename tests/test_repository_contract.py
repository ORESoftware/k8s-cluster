import configparser
import pathlib
import re
import subprocess
import unittest
from urllib.parse import urlparse


ROOT = pathlib.Path(__file__).resolve().parents[1]
APPROVED_OWNERS = {"sonus-auris", "ORESoftware"}
CONTRACTOR_HANDBOOK = ROOT / "docs" / "contractor-work-intelligence"
CONTRACTOR_HANDBOOK_FILES = {
    "README.md",
    "PRODUCT.md",
    "ARCHITECTURE.md",
    "OFFLINE_SYNC_PROTOCOL.md",
    "DOMAIN_MODEL.md",
    "USER_EXPERIENCE.md",
    "PRIVACY_AND_TRUST.md",
    "REPORTS_AND_BILLING.md",
    "OPERATIONS_AND_QUALITY.md",
    "ROADMAP.md",
    "GLOSSARY.md",
    "adrs/0001-separate-sister-product.md",
    "adrs/0002-evidence-is-not-accounting.md",
    "adrs/0003-local-first-selective-sync.md",
}
MARKDOWN_LINK = re.compile(r"\[[^\]]+\]\(([^)]+)\)")


def submodules():
    config = configparser.ConfigParser()
    config.optionxform = str
    with (ROOT / ".gitmodules").open(encoding="utf-8") as source:
        config.read_file(source)
    return [
        {
            "name": section.removeprefix('submodule "').removesuffix('"'),
            **dict(config[section]),
        }
        for section in config.sections()
    ]


def target_is_inside_declared_submodule(target: pathlib.Path) -> bool:
    try:
        relative = target.relative_to(ROOT)
    except ValueError:
        return False
    declared = [pathlib.PurePosixPath(entry["path"]) for entry in submodules()]
    relative_posix = pathlib.PurePosixPath(relative.as_posix())
    return any(
        relative_posix == path or path in relative_posix.parents
        for path in declared
    )


class RepositoryContractTests(unittest.TestCase):
    def test_submodule_names_and_paths_are_unique_and_track_main(self):
        entries = submodules()
        names = [entry["name"] for entry in entries]
        paths = [entry["path"] for entry in entries]
        self.assertEqual(len(names), len(set(names)))
        self.assertEqual(len(paths), len(set(paths)))
        self.assertTrue(paths)
        for entry in entries:
            self.assertEqual(entry.get("branch"), "main", entry["path"])
            self.assertFalse(pathlib.PurePosixPath(entry["path"]).is_absolute())
            self.assertNotIn("..", pathlib.PurePosixPath(entry["path"]).parts)

    def test_submodule_urls_use_github_and_approved_owners(self):
        for entry in submodules():
            url = entry["url"]
            ssh = re.fullmatch(r"git@github\.com:([^/]+)/[^/]+\.git", url)
            if ssh:
                owner = ssh.group(1)
            else:
                parsed = urlparse(url)
                self.assertEqual(parsed.scheme, "https", entry["path"])
                self.assertEqual(parsed.hostname, "github.com", entry["path"])
                self.assertIsNone(parsed.username, entry["path"])
                self.assertFalse(parsed.query or parsed.fragment, entry["path"])
                parts = parsed.path.strip("/").split("/")
                self.assertGreaterEqual(len(parts), 2, entry["path"])
                owner = parts[0]
            self.assertIn(owner, APPROVED_OWNERS, entry["path"])

    def test_declared_paths_exactly_match_pinned_gitlinks(self):
        output = subprocess.check_output(
            ["git", "ls-tree", "-r", "HEAD"],
            cwd=ROOT,
            text=True,
        )
        gitlinks = {
            line.split("\t", 1)[1]
            for line in output.splitlines()
            if line.startswith("160000 commit ")
        }
        declared = {entry["path"] for entry in submodules()}
        self.assertEqual(gitlinks, declared)

    def test_contractor_handbook_has_the_required_product_and_engineering_set(self):
        actual = {
            path.relative_to(CONTRACTOR_HANDBOOK).as_posix()
            for path in CONTRACTOR_HANDBOOK.rglob("*.md")
        }
        self.assertTrue(CONTRACTOR_HANDBOOK_FILES.issubset(actual))

        for relative in sorted(CONTRACTOR_HANDBOOK_FILES):
            path = CONTRACTOR_HANDBOOK / relative
            content = path.read_text(encoding="utf-8")
            self.assertTrue(content.startswith("# "), relative)
            self.assertGreater(len(content), 900, relative)
            self.assertEqual(content.count("```") % 2, 0, relative)

    def test_contractor_handbook_keeps_the_product_name_provisional(self):
        index = (CONTRACTOR_HANDBOOK / "README.md").read_text(encoding="utf-8")
        product = (CONTRACTOR_HANDBOOK / "PRODUCT.md").read_text(encoding="utf-8")
        combined = f"{index}\n{product}"
        self.assertIn("not a final product name", combined)
        self.assertIn("DEN-990", combined)
        self.assertIn("Sensor observations are evidence", combined)
        self.assertNotIn("Sonus Operis", combined)

    def test_contractor_handbook_relative_links_resolve_or_target_a_gitlink(self):
        root = ROOT.resolve()
        markdown_files = [ROOT / "README.md", *CONTRACTOR_HANDBOOK.rglob("*.md")]
        for source in markdown_files:
            content = source.read_text(encoding="utf-8")
            for raw_target in MARKDOWN_LINK.findall(content):
                target = raw_target.strip().split(maxsplit=1)[0]
                if target.startswith(("https://", "http://", "mailto:", "#")):
                    continue
                path_part = target.split("#", 1)[0]
                if not path_part:
                    continue
                resolved = (source.parent / path_part).resolve()
                self.assertTrue(
                    resolved == root or root in resolved.parents,
                    f"{source.relative_to(ROOT)} links outside the repository: {target}",
                )
                self.assertTrue(
                    resolved.exists() or target_is_inside_declared_submodule(resolved),
                    f"{source.relative_to(ROOT)} has an unresolved link: {target}",
                )

    def test_contractor_adrs_expose_decision_status_and_consequences(self):
        for path in sorted((CONTRACTOR_HANDBOOK / "adrs").glob("*.md")):
            content = path.read_text(encoding="utf-8")
            self.assertIn("**Status:**", content, path.name)
            self.assertIn("## Context", content, path.name)
            self.assertIn("## Decision", content, path.name)
            self.assertIn("## Consequences", content, path.name)

    def test_offline_sync_protocol_preserves_replay_and_trust_invariants(self):
        protocol = (
            CONTRACTOR_HANDBOOK / "OFFLINE_SYNC_PROTOCOL.md"
        ).read_text(encoding="utf-8")
        for required in (
            "Array order is never authority",
            "new",
            "duplicate",
            "conflict",
            "last-write-wins",
            "cryptographicallyEraseEvidence",
            "sync-batch.schema.json",
            "sync-semantics.mjs",
            "production mobile sync worker",
        ):
            self.assertIn(required, protocol)
        normalized = " ".join(protocol.split())
        self.assertIn(
            "never resolves an identity conflict with last-write-wins",
            normalized,
        )
        self.assertIn("Raw evidence bytes are carried separately", protocol)
        self.assertNotIn("status: live", protocol.lower())


if __name__ == "__main__":
    unittest.main()
