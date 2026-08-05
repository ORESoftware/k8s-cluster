from __future__ import annotations

import hashlib
import importlib.util
import json
import tempfile
import unittest
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts/ops/publish_zed_pkg_marketing_sites.py"
SPEC = importlib.util.spec_from_file_location("marketing_publisher", MODULE_PATH)
assert SPEC and SPEC.loader
publisher = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = publisher
SPEC.loader.exec_module(publisher)


class MarketingPublisherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.sites = publisher.load_specs()

    def test_exact_non_test_target_set(self) -> None:
        self.assertEqual(tuple(site["slug"] for site in self.sites), publisher.EXPECTED_SLUGS)
        self.assertEqual(len(self.sites), 14)
        self.assertEqual(len({site["repository_full_name"].lower() for site in self.sites}), 14)
        for site in self.sites:
            self.assertFalse(site["slug"].endswith("-test"))
            self.assertEqual(site["repo"], f"{site['slug']}.github.io")
            self.assertEqual(site["site_url"], f"https://{site['slug']}.github.io/")

    def test_every_site_has_real_source_evidence_and_language_choices(self) -> None:
        client_backed = {
            "agent-pontifex",
            "file-tunnel",
            "daedalus-fab",
            "opto-sync",
            "voxletra",
            "shared-auth",
            "streempilot",
            "hypesiege",
        }
        for site in self.sites:
            self.assertTrue(site["commit_url"].startswith("https://github.com/"))
            self.assertGreaterEqual(len(site["examples"]), 3)
            self.assertEqual(
                len({example["language"].casefold() for example in site["examples"]}),
                len(site["examples"]),
            )
            if site["slug"] in client_backed:
                self.assertTrue(
                    any(example["source_kind"] == "client" for example in site["examples"]),
                    site["slug"],
                )
            for example in site["examples"]:
                self.assertTrue(example["source_url"].startswith("https://github.com/"))
                self.assertNotIn("TODO", example["code"])
                self.assertNotIn("example-package", example["code"])

    def test_renderer_is_deterministic_and_complete(self) -> None:
        with tempfile.TemporaryDirectory() as first, tempfile.TemporaryDirectory() as second:
            publisher.render_all(Path(first), self.sites)
            publisher.render_all(Path(second), self.sites)
            first_files = {
                path.relative_to(first): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in Path(first).rglob("*")
                if path.is_file()
            }
            second_files = {
                path.relative_to(second): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in Path(second).rglob("*")
                if path.is_file()
            }
            self.assertEqual(first_files, second_files)
            self.assertEqual(len([path for path in first_files if path.name == "site.json"]), 14)
            manifest = json.loads((Path(first) / "manifest.json").read_text())
            self.assertEqual(manifest["marker"], publisher.MARKER)
            self.assertEqual(len(manifest["sites"]), 14)

    def test_generated_sites_are_accessible_and_deployable(self) -> None:
        for site in self.sites:
            files = publisher.render_site(site, self.sites)
            self.assertIn("<select id={explorerId} data-code-select>", files["src/components/CodeExplorer.astro"])
            self.assertIn('aria-live="polite"', files["src/components/CodeExplorer.astro"])
            self.assertIn("prefers-reduced-motion", files["src/styles/global.css"])
            self.assertIn('rel="canonical"', files["src/layouts/Layout.astro"])
            self.assertIn('set:html={JSON.stringify(schema)}></script>', files["src/layouts/Layout.astro"])
            self.assertIn("Explore the integration", files["src/pages/index.astro"])
            self.assertIn("actions/deploy-pages@cd2ce8f", files[".github/workflows/pages.yml"])
            self.assertNotIn("uses: actions/checkout@v", files[".github/workflows/ci.yml"])
            self.assertNotIn("package-lock.json", files)

    def test_branch_upsert_uses_the_correct_git_ref_endpoints(self) -> None:
        class Client:
            def __init__(self, existing: bool) -> None:
                self.existing = existing
                self.optional_calls = []
                self.request_calls = []

            def optional(self, method, path, payload=None):
                self.optional_calls.append((method, path, payload))
                return {"object": {"sha": "b" * 40}} if self.existing else None

            def request(self, method, path, payload=None, **kwargs):
                self.request_calls.append((method, path, payload, kwargs))
                return {}

        new_client = Client(existing=False)
        publisher.upsert_branch(new_client, "owner/site", "a" * 40)
        self.assertEqual(
            new_client.optional_calls[0][1],
            "/repos/owner/site/git/ref/heads/feat/astro-marketing-refresh-20260805",
        )
        self.assertEqual(new_client.request_calls[0][0:2], ("POST", "/repos/owner/site/git/refs"))

        existing_client = Client(existing=True)
        publisher.upsert_branch(existing_client, "owner/site", "a" * 40)
        self.assertEqual(existing_client.request_calls[0][0], "PATCH")
        self.assertEqual(
            existing_client.request_calls[0][1],
            "/repos/owner/site/git/refs/heads/feat/astro-marketing-refresh-20260805",
        )

    def test_non_main_repository_is_normalized_without_overwriting_main(self) -> None:
        class Client:
            def __init__(self) -> None:
                self.calls = []

            def optional(self, method, path, payload=None):
                self.calls.append(("optional", method, path, payload))
                return None

            def request(self, method, path, payload=None, **kwargs):
                self.calls.append(("request", method, path, payload))
                if path.endswith("/git/ref/heads/master"):
                    return {"object": {"sha": "c" * 40}}
                if method == "PATCH" and path == "/repos/owner/site":
                    return {"default_branch": "main"}
                return {}

        client = Client()
        repo = publisher.ensure_main_default(client, "owner/site", {"default_branch": "master"})
        self.assertEqual(repo["default_branch"], "main")
        self.assertIn(
            ("request", "POST", "/repos/owner/site/git/refs", {"ref": "refs/heads/main", "sha": "c" * 40}),
            client.calls,
        )
        self.assertIn(
            ("request", "PATCH", "/repos/owner/site", {"default_branch": "main"}),
            client.calls,
        )

    def test_report_requires_every_site(self) -> None:
        results = [
            publisher.SiteResult(
                slug=site["slug"],
                repository=site["repository_full_name"],
                site_url=site["site_url"],
                created_repository=False,
                pull_request_url=f"https://github.com/{site['repository_full_name']}/pull/1",
                pull_request_number=1,
                merge_sha="a" * 40,
                source_verified=True,
                pages_enabled=True,
                pages_url=site["site_url"],
                pages_workflow="success",
                pages_run_url=None,
                verified=True,
            )
            for site in self.sites
        ]
        report = publisher.markdown_report(results, "b" * 40)
        self.assertIn("14/14", report)
        self.assertIn(f"<!-- {publisher.MARKER}-report-complete -->", report)
        for site in self.sites:
            self.assertIn(site["site_url"], report)


if __name__ == "__main__":
    unittest.main()
