from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ops-org-dotgithub-ephemeral-owner-publish.yml"

EXPECTED_ORGANIZATIONS = (
    "channelsiege",
    "OmniBlitz",
    "streamkore",
    "hypeblitz",
    "3FA-app",
    "messaging-intel",
    "akrion-sim",
    "athlet-o",
    "benefactor-cc",
    "canonical-cloud",
    "claritas-viz",
    "cliptown",
    "daedalus-fab",
    "declarative-migrations",
    "fiducia-cloud",
    "anticaptrad",
    "opto-sync",
    "quaestor-ledger",
    "sagitta-stack",
    "shared-auth",
    "scintilla-run",
    "rust-ssr-demos",
    "sonus-auris",
    "usa-acc",
    "voxletra",
    "zed-pkg",
    "zed-pkg-test",
    "memebank",
    "meta-agents-demo",
    "networking-components",
    "StreemPilot",
    "unreal-unity-poc",
    "file-tunnel",
    "hypesiege",
    "discrete-event-systems",
    "drone-mngr",
)


class EphemeralOwnerWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.text = WORKFLOW.read_text(encoding="utf-8")

    def test_exact_owner_issue_trigger_is_bounded(self) -> None:
        text = self.text
        self.assertIn("github.repository == 'ORESoftware/k8s-cluster'", text)
        self.assertIn("github.event.issue.number == 615", text)
        self.assertIn("github.event.comment.user.login == 'ORESoftware'", text)
        self.assertIn("github.actor == 'ORESoftware'", text)
        self.assertIn(
            "github.event.comment.body == 'ops-bootstrap-org-dotgithub-ephemeral:615:20260804-v1'",
            text,
        )

    def test_fixed_36_organization_allowlist_is_exact(self) -> None:
        match = re.search(
            r"OWNER_ORGANIZATIONS: \|-\n(?P<body>(?:        .+\n)+?)\n    steps:",
            self.text,
        )
        self.assertIsNotNone(match)
        observed = tuple(
            line.strip() for line in match.group("body").splitlines() if line.strip()
        )
        self.assertEqual(EXPECTED_ORGANIZATIONS, observed)
        self.assertEqual(36, len({item.lower() for item in observed}))

    def test_plaintext_credential_never_enters_github_storage(self) -> None:
        text = self.text
        self.assertIn("openssl genpkey", text)
        self.assertIn("rsa_oaep_md:sha256", text)
        self.assertIn('select(.user.login == "ORESoftware")', text)
        self.assertIn('owner_login" = ORESoftware', text)
        self.assertIn('test "$membership" = admin:active', text)
        self.assertNotIn("GITHUB_ENV", text)
        self.assertNotIn("upload-artifact", text)
        self.assertNotIn("ciphertext-base64=${owner_token}", text)

    def test_workflow_does_not_use_blacklisted_destructive_commands(self) -> None:
        lowered = self.text.lower()
        forbidden = (
            "git stash",
            "git reset",
            "git clean",
            "git filter-repo",
            "git filter-branch",
            "git push --force",
            "git push --force-with-lease",
            "rm -rf",
            "find -delete",
            "terraform destroy",
            "pulumi destroy",
            "kubectl delete",
            "helm uninstall",
            "--no-verify",
        )
        for command in forbidden:
            self.assertNotIn(command, lowered)

    def test_publication_uses_reviewed_bounded_publisher_and_verifies_report(self) -> None:
        text = self.text
        self.assertIn("bootstrap_org_dotgithub_repositories_hardened.py", text)
        self.assertIn("--execute", text)
        self.assertIn('test "$membership_count" -eq 36', text)
        self.assertIn('if len(organizations) != 36:', text)
        self.assertIn('item.get("verified") is not True', text)
        self.assertIn('repository != f"{organization}/.github"', text)

    def test_source_is_current_trusted_main_not_comment_supplied_code(self) -> None:
        text = self.text
        self.assertIn('"repos/${REPOSITORY}/git/ref/heads/main"', text)
        self.assertIn('fetch \\\n            --quiet --depth=1 origin "$trusted_sha"', text)
        self.assertIn('test "$(git -C "$source_root" rev-parse HEAD)" = "$trusted_sha"', text)
        self.assertNotIn("pull_request_target", text)


if __name__ == "__main__":
    unittest.main()
