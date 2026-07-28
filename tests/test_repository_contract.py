import configparser
import pathlib
import re
import subprocess
import unittest
from urllib.parse import urlparse


ROOT = pathlib.Path(__file__).resolve().parents[1]
APPROVED_OWNERS = {"sonus-auris", "ORESoftware"}


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


if __name__ == "__main__":
    unittest.main()
