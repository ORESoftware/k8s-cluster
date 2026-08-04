from pathlib import Path

path = Path("remote/tests/general/gha-clone-msgint-config.test.ts")
source = path.read_text(encoding="utf-8")
old = "  assert.match(validation, /Exact repository admission/);"
new = """  assert.match(validation, /ensure_allowed_prefix_or_exact/);
  assert.ok(validation.includes(\"rule.strip_prefix('=')\"));"""
count = source.count(old)
if count != 1:
    raise RuntimeError(f"expected one stale exact-admission assertion, found {count}")
path.write_text(source.replace(old, new, 1), encoding="utf-8")
