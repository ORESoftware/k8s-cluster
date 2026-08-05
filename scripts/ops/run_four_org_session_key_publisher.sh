#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

base_script="${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}/scripts/ops/run_four_org_device_auth_publisher.sh"
patched_script="${RUNNER_TEMP:?RUNNER_TEMP is required}/run_four_org_device_auth_publisher.session-key.sh"

python3 - "$base_script" "$patched_script" <<'PY'
from __future__ import annotations

import re
import sys
from pathlib import Path

source_path = Path(sys.argv[1])
target_path = Path(sys.argv[2])
source = source_path.read_text(encoding="utf-8")

public_key = """-----BEGIN PUBLIC KEY-----
MIIBojANBgkqhkiG9w0BAQEFAAOCAY8AMIIBigKCAYEAtJisU8rOdO9CMXbDi/XH
aW3+aJwcxMymZm+qjt2WBU44UlszpQzbXqhp6N186w6jMQifVBalnigtSmASw7EV
+R6CYZYl8hry9jvFmJ8Qd/VJdV+6UXXD1h/xswS5Icup44bB5J3+Uiu5Eh2bexbT
pe73ipN29KsiG/u+pODH8tJhEq2L+2xeZ+YDmIQDDbM7fDfAqZsWY0fYz5eHkoYr
A/ww0Xa4zOZoxQWhpTCAZJgxy/TFNhOFYHuwP2IW/chLX/6wIECU/nefSbkxHzza
jyvAqbwC3VWQT3czAJ0FIO5rRUwO3h5foikaTv1jMJVmUMzQi5dMlFgC+cnrRFjG
Oyy3ekZfni2VR+HBNgG6ygf2uJMUoy09DoNWGbaAJccc1gW9OF99a9mbbowa6q5F
zAy6JGpSOjs6Czi8YBkU8E1RrRCZkLooJGapY6Tf7DrH/dFmZfAbf4WuMj2MnkNZ
yWp3Z19nopBaN2SMAjwBzFK/7DbeYZCcZqJVrlCE2gdlAgMBAAE=
-----END PUBLIC KEY-----"""
fingerprint = "c62b2beed529242a4e2db359750ea6d1d470779b213ceca7f28b03a76c9fdcd8"

pattern = re.compile(
    r"cat > \"\$public_key\" <<'PUBLIC_KEY'\n"
    r".*?\n"
    r"PUBLIC_KEY\n"
    r"expected_fingerprint='[0-9a-f]{64}'",
    re.DOTALL,
)
replacement = (
    "cat > \"$public_key\" <<'PUBLIC_KEY'\n"
    + public_key
    + "\nPUBLIC_KEY\nexpected_fingerprint='"
    + fingerprint
    + "'"
)
patched, replacements = pattern.subn(lambda _: replacement, source, count=1)
if replacements != 1:
    raise SystemExit(f"expected one device-recipient block, replaced {replacements}")
if fingerprint not in patched or public_key not in patched:
    raise SystemExit("session recipient was not embedded")
if "0910b9a6f418e5e898957138ba98c641e721cb3da0a36d9e6da529d2a7d1db06" in patched:
    raise SystemExit("legacy recipient fingerprint remains")

target_path.write_text(patched, encoding="utf-8")
PY

chmod 700 "$patched_script"
exec bash "$patched_script"
