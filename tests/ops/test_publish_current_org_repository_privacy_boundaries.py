from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
OPS = ROOT / "scripts" / "ops"
MODULE_PATH = OPS / "publish_current_org_repository_relationships.py"

if str(OPS) not in sys.path:
    sys.path.insert(0, str(OPS))

spec = importlib.util.spec_from_file_location(
    "publish_current_org_repository_relationships_privacy_boundaries",
    MODULE_PATH,
)
module = importlib.util.module_from_spec(spec)
assert spec.loader is not None
assert spec.name is not None
sys.modules[spec.name] = module
spec.loader.exec_module(module)
publisher = module.publisher


class RepositoryPrivacyBoundaryTests(unittest.TestCase):
    @staticmethod
    def repository(
        organization: str,
        name: str,
        *,
        private: bool = False,
    ) -> dict[str, object]:
        return {
            "name": name,
            "full_name": f"{organization}/{name}",
            "private": private,
            "visibility": "private" if private else "public",
            "description": None,
            "archived": False,
            "fork": False,
            "default_branch": "main",
        }

    def test_private_name_prefix_of_public_repository_is_not_a_leak(self) -> None:
        organization = "example-internal"
        private_name = organization
        public_name = f"{private_name}.github.io"
        inventory = [
            self.repository(organization, ".github"),
            self.repository(organization, public_name),
            self.repository(organization, private_name, private=True),
        ]

        with mock.patch.object(publisher.base, "fetch_file", return_value=None):
            branch, files, references, result = (
                module.build_plan_with_exact_private_references(
                    object(),
                    organization,
                    {"default_branch": "main"},
                    inventory,
                )
            )

        public_url = f"https://github.com/{organization}/{public_name}"
        private_url = f"https://github.com/{organization}/{private_name}"
        rendered = files[publisher.JSON_PATH][0]
        self.assertEqual("main", branch)
        self.assertEqual(1, result["private_repository_count"])
        self.assertFalse(
            module.contains_private_reference(public_url, references)
        )
        self.assertTrue(
            module.contains_private_reference(private_url, references)
        )
        self.assertIn(public_name, rendered)
        self.assertNotIn(f'"name": "{private_name}"', rendered)

    def test_post_write_verification_uses_reference_boundaries(self) -> None:
        organization = "example-internal"
        private_name = organization
        public_name = f"{private_name}.github.io"
        desired = f"https://github.com/{organization}/{public_name}\n"
        existing = mock.Mock(content=desired)
        references = module.private_references(
            organization,
            {private_name},
        )
        result = {
            "changed_files": [],
            "unchanged_files": [],
            "verified": False,
        }
        plan = (
            organization,
            "main",
            {"README.md": (desired, existing)},
            references,
            result,
        )

        with (
            mock.patch.object(publisher.base, "fetch_file") as fetch_file,
            mock.patch.object(publisher, "write_file") as write_file,
        ):
            module.run_plan_with_exact_private_references(
                object(),
                plan,
                True,
            )

        write_file.assert_not_called()
        fetch_file.assert_not_called()
        self.assertTrue(result["verified"])
        self.assertEqual(["README.md"], result["unchanged_files"])

    def test_changed_file_is_re_read_after_write(self) -> None:
        organization = "example-internal"
        desired = "managed relationship content\n"
        observed = mock.Mock(content=desired)
        references = module.private_references(organization, set())
        result = {
            "changed_files": [],
            "unchanged_files": [],
            "verified": False,
        }
        plan = (
            organization,
            "main",
            {"README.md": (desired, None)},
            references,
            result,
        )

        with (
            mock.patch.object(
                publisher.base,
                "fetch_file",
                return_value=observed,
            ) as fetch_file,
            mock.patch.object(publisher, "write_file") as write_file,
        ):
            module.run_plan_with_exact_private_references(
                object(),
                plan,
                True,
            )

        write_file.assert_called_once()
        fetch_file.assert_called_once()
        self.assertTrue(result["verified"])
        self.assertEqual(["README.md"], result["changed_files"])

    def test_terminal_private_reference_still_fails(self) -> None:
        organization = "example-internal"
        private_name = organization
        references = module.private_references(
            organization,
            {private_name},
        )
        private_url = f"https://github.com/{organization}/{private_name}"

        for content in (
            private_url,
            f"See {private_url}.",
            f"git@github.com:{organization}/{private_name}.git",
            f'{{"full_name": "{organization}/{private_name}"}}',
        ):
            with self.subTest(content=content):
                self.assertTrue(
                    module.contains_private_reference(content, references)
                )


if __name__ == "__main__":
    unittest.main()
