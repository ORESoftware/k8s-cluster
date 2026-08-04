from pathlib import Path

IMMUTABLE_SHA = "0123456789abcdef0123456789abcdef01234567"
path = Path("remote/deployments/gha-clone-server-rs/tests/planner_adversarial.rs")
source = path.read_text(encoding="utf-8")

lower = "actions/setup-node@abc"
lower_count = source.count(lower)
if lower_count < 2:
    raise RuntimeError(f"expected at least two lowercase mutable setup refs, found {lower_count}")
source = source.replace(lower, f"actions/setup-node@{IMMUTABLE_SHA}")

for old, new in (
    ("ACTIONS/CHECKOUT@abc", f"ACTIONS/CHECKOUT@{IMMUTABLE_SHA}"),
    ("Actions/Setup-Node@abc", f"Actions/Setup-Node@{IMMUTABLE_SHA}"),
):
    count = source.count(old)
    if count != 1:
        raise RuntimeError(f"expected one {old!r}, found {count}")
    source = source.replace(old, new, 1)

old = '"secret-bearing env/with values are unsupported"'
if source.count(old) != 1:
    raise RuntimeError("stale combined secret-reason fixture was not found exactly once")
source = source.replace(old, '"secret-bearing"', 1)

old = 'reason.contains("secret-bearing env/with")'
if source.count(old) != 1:
    raise RuntimeError("stale secret reason assertion was not found exactly once")
source = source.replace(old, 'reason.contains("secret-bearing")', 1)

path.write_text(source, encoding="utf-8")
