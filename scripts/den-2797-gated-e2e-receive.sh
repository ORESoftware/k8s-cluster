#!/usr/bin/env bash
set -Eeuo pipefail

work=/tmp/den-2797-gated-e2e
private_key="$work/owner.private.pem"
public_key="$work/owner.public.pem"
ciphertext_text="$work/owner.ciphertext.b64"
ciphertext_binary="$work/owner.ciphertext.bin"
token_file="$work/owner.token"

mkdir -p "$work"
umask 077

for command_name in openssl python3 base64; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "required command is missing: $command_name" >&2
    exit 127
  }
done

openssl genpkey \
  -algorithm RSA \
  -pkeyopt rsa_keygen_bits:4096 \
  -out "$private_key" \
  >/dev/null 2>&1
openssl pkey -in "$private_key" -pubout -out "$public_key" >/dev/null 2>&1
chmod 600 "$private_key" "$public_key"

CONTROL_FILE_PATH="$PUBLIC_KEY_PATH" \
CONTROL_FILE_SOURCE="$public_key" \
CONTROL_FILE_MESSAGE="chore(DEN-2797): publish gated E2E handoff key $HANDSHAKE_NONCE" \
python3 - <<'PY'
import base64
import json
import os
import urllib.error
import urllib.request

repository = os.environ["CONTROL_REPOSITORY"]
branch = os.environ["CONTROL_BRANCH"]
path = os.environ["CONTROL_FILE_PATH"]
source = os.environ["CONTROL_FILE_SOURCE"]
token = os.environ["CONTROL_TOKEN"]
message = os.environ["CONTROL_FILE_MESSAGE"]
base = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
url = f"{base}/repos/{repository}/contents/{path}"
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "Content-Type": "application/json",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "den-2797-gated-e2e-publisher",
}

check = urllib.request.Request(url, headers=headers)
try:
    with urllib.request.urlopen(check) as response:
        if response.status == 200:
            raise SystemExit(f"handoff path already exists: {path}")
except urllib.error.HTTPError as error:
    if error.code != 404:
        raise

with open(source, "rb") as handle:
    encoded = base64.b64encode(handle.read()).decode("ascii")
payload = json.dumps({
    "message": message,
    "content": encoded,
    "branch": branch,
}).encode("utf-8")
request = urllib.request.Request(url, data=payload, method="PUT", headers=headers)
with urllib.request.urlopen(request) as response:
    if response.status not in (200, 201):
        raise SystemExit(f"public-key publication returned HTTP {response.status}")
PY

echo "source validation passed; nonce-bound encrypted credential handoff is open"

CIPHERTEXT_OUTPUT="$ciphertext_text" python3 - <<'PY'
import base64
import json
import os
import time
import urllib.error
import urllib.request

repository = os.environ["CONTROL_REPOSITORY"]
path = os.environ["CIPHERTEXT_PATH"]
token = os.environ["CONTROL_TOKEN"]
output = os.environ["CIPHERTEXT_OUTPUT"]
base = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
url = f"{base}/repos/{repository}/contents/{path}?ref={os.environ['CONTROL_BRANCH']}"
headers = {
    "Accept": "application/vnd.github+json",
    "Authorization": f"Bearer {token}",
    "X-GitHub-Api-Version": "2022-11-28",
    "User-Agent": "den-2797-gated-e2e-publisher",
}

for _ in range(240):
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request) as response:
            payload = json.load(response)
            content = base64.b64decode(payload["content"]).decode("utf-8").strip()
            if not content:
                raise SystemExit("ciphertext file is empty")
            with open(output, "w", encoding="utf-8") as handle:
                handle.write(content)
                handle.write("\n")
            break
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
    time.sleep(5)
else:
    raise SystemExit("timed out waiting for nonce-bound ciphertext")
PY

base64 --decode "$ciphertext_text" >"$ciphertext_binary"
openssl pkeyutl -decrypt \
  -inkey "$private_key" \
  -in "$ciphertext_binary" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  -out "$token_file"
chmod 600 "$token_file"

python3 - "$token_file" <<'PY'
from pathlib import Path
import sys

token = Path(sys.argv[1]).read_bytes()
if len(token) < 30 or len(token) > 512:
    raise SystemExit("decrypted credential length is invalid")
if b"\n" in token or b"\r" in token or b"\x00" in token:
    raise SystemExit("decrypted credential contains forbidden bytes")
PY

OWNER_TOKEN_FILE="$token_file" python3 - <<'PY'
import json
import os
import urllib.request

with open(os.environ["OWNER_TOKEN_FILE"], "r", encoding="utf-8") as handle:
    token = handle.read()
base = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
request = urllib.request.Request(
    f"{base}/user",
    headers={
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "den-2797-gated-e2e-publisher",
    },
)
with urllib.request.urlopen(request) as response:
    if response.status != 200:
        raise SystemExit(f"credential verification returned HTTP {response.status}")
    payload = json.load(response)
    if not payload.get("login"):
        raise SystemExit("credential verification returned no login")
PY

echo "encrypted owner credential received and verified"
