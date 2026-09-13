from pathlib import Path

path = Path("remote/tests/general/gha-clone-msgint-config.test.ts")
source = path.read_text(encoding="utf-8")

replacements = (
    (
        "  assert.match(validation, /Exact repository admission/);",
        """  assert.match(validation, /ensure_allowed_prefix_or_exact/);
  assert.ok(validation.includes(\"rule.strip_prefix('=')\"));""",
        "generic exact-admission implementation",
    ),
    (
        "  assert.match(validation, /messaging-intel\\/msgint-connectors/);",
        """  assert.match(buildPatch, /BUILD_SERVER_ALLOWED_REPOSITORY_RULES_JSON/);
  assert.match(
    buildPatch,
    /=https:\\/\\/github\\.com\\/messaging-intel\\/msgint-connectors\\.git/,
  );""",
        "exact Messaging Intel deployment rule",
    ),
    (
        "  assert.match(validation, /msgint-connectors\\.git-evil/);",
        "  assert.match(validation, /repo\\.git-suffix/);",
        "generic suffix-lookalike rejection",
    ),
    (
        "  assert.match(validation, /msgint-connectors-extra\\.git/);",
        "  assert.match(validation, /disallow_near_match_for_exact_rule/);",
        "generic exact-rule near-match regression",
    ),
    (
        "  assert.match(readme, /This hermetic proof needs neither the private Messaging Intel repository nor a Kubernetes context/);",
        "  assert.match(readme, /This hermetic proof needs neither the private Messaging Intel repository nor a\\s+Kubernetes context/);",
        "hermetic access-boundary documentation",
    ),
)

for old, new, label in replacements:
    count = source.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one stale assertion, found {count}")
    source = source.replace(old, new, 1)

path.write_text(source, encoding="utf-8")

process_test = Path(
    "remote/deployments/gha-clone-server-rs/tests/"
    "gha_clone_server_messaging_intel_process.test.ts"
)
if process_test.exists():
    print("unexpected process test exists immediately after clean materialization")
    print(process_test.read_text(encoding="utf-8"))
    raise RuntimeError("unexpected process test provenance must be resolved before compilation")
print("unexpected process test is absent immediately after clean materialization")
