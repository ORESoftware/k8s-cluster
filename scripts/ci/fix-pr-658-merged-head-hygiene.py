from pathlib import Path

path = Path("remote/tests/general/gha-clone-server-config.test.ts")
source = path.read_text(encoding="utf-8")
old = "  assert.doesNotMatch(workflow, /rm -rf|ghp_|github_pat_/);"
new = """  assert.doesNotMatch(workflow, /rm -rf/);
  assert.doesNotMatch(
    workflow,
    /ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}/,
  );"""
count = source.count(old)
if count != 1:
    raise RuntimeError(f"expected one stale credential assertion, found {count}")
path.write_text(source.replace(old, new, 1), encoding="utf-8")
