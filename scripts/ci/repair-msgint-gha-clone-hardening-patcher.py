from pathlib import Path

path = Path("scripts/ci/apply-msgint-gha-clone-hardening.py")
source = path.read_text(encoding="utf-8")

needle = "combined.push('\\n');"
replacement = "combined.push('\\\\n');"
count = source.count(needle)
if count != 2:
    raise RuntimeError(f"expected two Rust newline anchors, found {count}")
source = source.replace(needle, replacement)

if "job = '''  msgint-profile-smoke:" not in source:
    raise RuntimeError("embedded Messaging Intel workflow marker was not found")
source = source.replace(
    "job = '''  msgint-profile-smoke:",
    "job = r'''  msgint-profile-smoke:",
    1,
)

warning_pattern = "/secrets\\['PROD_TOKEN'\\]/"
if warning_pattern not in source:
    raise RuntimeError("static secret-expression contract marker was not found")
source = source.replace(warning_pattern, "/PROD_TOKEN/", 1)

path.write_text(source, encoding="utf-8")
