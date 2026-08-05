from __future__ import annotations

import csv
import sys
import tempfile
import unittest
from pathlib import Path

OPS_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(OPS_DIR))

from validate_github_linear_registry_relationship import (  # noqa: E402
    EXPECTED_GOVERNANCE_COUNT,
    EXPECTED_PORTFOLIO_COUNT,
    RegistryRelationshipError,
    validate_relationship,
)


class GitHubLinearRegistryRelationshipTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.governance = REPO_ROOT / "ops/portfolio/github-linear-project-registry.tsv"
        cls.portfolio = REPO_ROOT / "ops/registries/portfolio-project-links.csv"

    def validate(self, governance: Path, portfolio: Path) -> dict[str, object]:
        return validate_relationship(
            governance,
            portfolio,
            expected_governance_count=EXPECTED_GOVERNANCE_COUNT,
            expected_portfolio_count=EXPECTED_PORTFOLIO_COUNT,
        )

    def copy_registries(self, directory: Path) -> tuple[Path, Path]:
        governance = directory / "governance.tsv"
        portfolio = directory / "portfolio.csv"
        governance.write_text(self.governance.read_text(encoding="utf-8"), encoding="utf-8")
        portfolio.write_text(self.portfolio.read_text(encoding="utf-8"), encoding="utf-8")
        return governance, portfolio

    def mutate_portfolio(self, path: Path, callback) -> None:
        with path.open(newline="", encoding="utf-8") as handle:
            reader = csv.DictReader(handle)
            fieldnames = list(reader.fieldnames or [])
            rows = list(reader)
        callback(rows)
        with path.open("w", newline="", encoding="utf-8") as handle:
            writer = csv.DictWriter(handle, fieldnames=fieldnames)
            writer.writeheader()
            writer.writerows(rows)

    def test_committed_registries_are_consistent(self) -> None:
        report = self.validate(self.governance, self.portfolio)
        self.assertTrue(report["relationship_valid"])
        self.assertEqual(report["governance_organizations"], 64)
        self.assertEqual(report["active_portfolios"], 41)
        self.assertEqual(report["governance_only_organizations"], 23)

    def test_linear_url_drift_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            governance, portfolio = self.copy_registries(Path(temporary))
            self.mutate_portfolio(
                portfolio,
                lambda rows: rows[0].__setitem__(
                    "linear_project_url",
                    "https://linear.app/denman/project/different-project-0123456789ab",
                ),
            )
            with self.assertRaisesRegex(RegistryRelationshipError, "Linear URL differs"):
                self.validate(governance, portfolio)

    def test_unknown_portfolio_organization_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            governance, portfolio = self.copy_registries(Path(temporary))

            def mutate(rows):
                rows[0]["portfolio_key"] = "unknown-portfolio-org"
                rows[0]["github_org"] = "unknown-portfolio-org"
                rows[0]["github_project_title"] = "unknown-portfolio-org-project"
                rows[0]["github_project_url"] = (
                    "https://github.com/orgs/unknown-portfolio-org/projects/1"
                )

            self.mutate_portfolio(portfolio, mutate)
            with self.assertRaisesRegex(RegistryRelationshipError, "absent from"):
                self.validate(governance, portfolio)

    def test_project_number_and_url_must_move_together(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            governance, portfolio = self.copy_registries(Path(temporary))
            self.mutate_portfolio(
                portfolio,
                lambda rows: rows[0].__setitem__("github_project_number", "7"),
            )
            with self.assertRaisesRegex(RegistryRelationshipError, "expected GitHub Project"):
                self.validate(governance, portfolio)

    def test_project_title_preserves_canonical_org_casing(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            governance, portfolio = self.copy_registries(Path(temporary))
            self.mutate_portfolio(
                portfolio,
                lambda rows: rows[0].__setitem__(
                    "github_project_title",
                    rows[0]["github_project_title"].lower(),
                ),
            )
            with self.assertRaisesRegex(RegistryRelationshipError, "expected project title"):
                self.validate(governance, portfolio)

    def test_credential_shaped_material_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            governance, portfolio = self.copy_registries(Path(temporary))
            governance.write_text(
                governance.read_text(encoding="utf-8")
                + "# ghp_"
                + "a" * 30
                + "\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(RegistryRelationshipError, "credential-shaped"):
                self.validate(governance, portfolio)


if __name__ == "__main__":
    unittest.main()
