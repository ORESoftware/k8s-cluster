# One-shot exact repair for the duplicated pull/push workflow path block.
# Retained only until the full Rust integration and deployment contracts pass.
from pathlib import Path

path = Path("scripts/ops/apply_msgint_gha_clone.py")
text = path.read_text(encoding="utf-8")
old = '''def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)
'''
new = '''def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    expected = 2 if label == "pull request continuity patch trigger" else 1
    if count != expected:
        raise RuntimeError(
            f"{label}: expected {expected} match(es), found {count}"
        )
    return text.replace(old, new, 1)
'''
if text.count(old) != 1:
    raise RuntimeError(f"patcher helper: expected one match, found {text.count(old)}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
