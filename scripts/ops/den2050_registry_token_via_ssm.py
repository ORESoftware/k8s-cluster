#!/usr/bin/env python3
"""Mint/revoke a short-lived Zed registry token through a protected SSM host.

The plaintext token never traverses SSM. Context selection is bound to the
expected public registry identity, and every minted token is validated against
the exact live pod over a local kubectl port-forward before it is encrypted for
the Actions runner.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import re
import shlex
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path

TERMINAL = {"Success", "Failed", "Cancelled", "TimedOut", "Cancelling"}
TOKEN_NAME_RE = re.compile(r"^[A-Za-z0-9._-]{1,120}$")
PUBLIC_URL_RE = re.compile(r"^https://[A-Za-z0-9.-]+(?::[0-9]+)?/?$")
SECRET_RE = re.compile(r"zpkg_[A-Za-z0-9]+")

# The protected host has several kubeconfigs. Its ambient context is not
# authoritative, so choose only a context whose live API pod advertises the
# expected PUBLIC_BASE_URL and whose server is database-backed/auth-enabled.
KUBE_CONTEXT_SHELL = r'''
select_zed_context() {
  candidates_file=$(mktemp /tmp/den2050-kubeconfigs.XXXXXX)
  {
    printf '%s\n' \
      /etc/kubernetes/admin.conf \
      /etc/rancher/k3s/k3s.yaml \
      /root/.kube/config \
      /home/ec2-user/.kube/config \
      /home/ubuntu/.kube/config
    for root in /etc/kubernetes /etc/rancher /root/.kube /home/ec2-user/.kube /home/ubuntu/.kube; do
      if [ -d "$root" ]; then
        find "$root" -type f -size -1048576c \
          \( -name '*.conf' -o -name 'config' -o -name '*.yaml' -o -name '*.yml' \) \
          -print 2>/dev/null || true
      fi
    done
  } | awk 'NF && !seen[$0]++' > "$candidates_file"

  try_kubeconfig() {
    config="$1"
    [ -r "$config" ] || return 1
    contexts_file=$(mktemp /tmp/den2050-contexts.XXXXXX)
    if ! kubectl --kubeconfig "$config" config get-contexts -o name > "$contexts_file" 2>/dev/null; then
      rm -f "$contexts_file"
      return 1
    fi
    while IFS= read -r context; do
      [ -n "$context" ] || continue
      kubectl --kubeconfig "$config" --context "$context" \
        get namespace zed --request-timeout=8s >/dev/null 2>&1 || continue
      pod=$(kubectl --kubeconfig "$config" --context "$context" -n zed \
        get pods -l app=dd-zed-api-server \
        -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)
      [ -n "$pod" ] || continue
      public_base=$(kubectl --kubeconfig "$config" --context "$context" -n zed \
        exec "$pod" -c zed-api-server -- /bin/sh -c \
        'printf "%s" "${PUBLIC_BASE_URL:-}"' 2>/dev/null || true)
      public_base=${public_base%/}
      expected_base=${EXPECTED_PUBLIC_BASE_URL%/}
      [ "$public_base" = "$expected_base" ] || continue
      kubectl --kubeconfig "$config" --context "$context" -n zed \
        exec "$pod" -c zed-api-server -- /bin/sh -c \
        'test -n "${DATABASE_URL:-}" && [ "${ZED_AUTH_DISABLED:-0}" != "1" ]' \
        >/dev/null 2>&1 || continue
      ZED_KUBECONFIG="$config"
      ZED_KUBE_CONTEXT="$context"
      ZED_API_POD="$pod"
      export ZED_KUBECONFIG ZED_KUBE_CONTEXT ZED_API_POD
      rm -f "$contexts_file" "$candidates_file"
      return 0
    done < "$contexts_file"
    rm -f "$contexts_file"
    return 1
  }

  while IFS= read -r config; do
    try_kubeconfig "$config" && return 0
  done < "$candidates_file"
  rm -f "$candidates_file"
  return 1
}

zkubectl() {
  kubectl --kubeconfig "$ZED_KUBECONFIG" --context "$ZED_KUBE_CONTEXT" "$@"
}
'''


def redact(value: str) -> str:
    return SECRET_RE.sub("[REDACTED_ZED_TOKEN]", value)


def validate_inputs(token_name: str, expected_public_base_url: str) -> None:
    if not TOKEN_NAME_RE.fullmatch(token_name):
        raise RuntimeError("invalid bounded token name")
    if not PUBLIC_URL_RE.fullmatch(expected_public_base_url):
        raise RuntimeError("invalid expected public registry URL")


def run(argv: list[str], *, capture: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        argv,
        text=True,
        check=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def get_invocation(instance_id: str, region: str, command_id: str) -> dict:
    return json.loads(
        run(
            [
                "aws", "ssm", "get-command-invocation",
                "--region", region,
                "--command-id", command_id,
                "--instance-id", instance_id,
                "--query", "{Status:Status,Stdout:StandardOutputContent,Stderr:StandardErrorContent}",
                "--output", "json",
            ]
        ).stdout
    )


def send_ssm(instance_id: str, region: str, command: str) -> dict:
    with tempfile.TemporaryDirectory(prefix="den2050-ssm-") as tmp:
        params = Path(tmp) / "parameters.json"
        params.write_text(json.dumps({"commands": [command]}), encoding="utf-8")
        command_id = run(
            [
                "aws", "ssm", "send-command",
                "--region", region,
                "--instance-ids", instance_id,
                "--document-name", "AWS-RunShellScript",
                "--comment", "DEN-2050 bounded Zed registry token operation",
                "--parameters", f"file://{params}",
                "--query", "Command.CommandId",
                "--output", "text",
            ]
        ).stdout.strip()
        if not command_id:
            raise RuntimeError("SSM did not return a command id")

        status = "Pending"
        for _ in range(120):
            result = subprocess.run(
                [
                    "aws", "ssm", "get-command-invocation",
                    "--region", region,
                    "--command-id", command_id,
                    "--instance-id", instance_id,
                    "--query", "Status",
                    "--output", "text",
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
            status = result.stdout.strip() if result.returncode == 0 else "Pending"
            if status in TERMINAL:
                break
            time.sleep(2)

        invocation = get_invocation(instance_id, region, command_id)
        observed = invocation.get("Status") or status
        if observed != "Success":
            stdout = redact(invocation.get("Stdout") or "")[-4000:]
            stderr = redact(invocation.get("Stderr") or "")[-4000:]
            raise RuntimeError(
                f"SSM registry-token command failed status={observed}; "
                f"stdout={stdout!r}; stderr={stderr!r}"
            )
        return invocation


def revoke(instance_id: str, region: str, token_name: str, expected_public_base_url: str) -> None:
    validate_inputs(token_name, expected_public_base_url)
    q_name = shlex.quote(token_name)
    q_expected = shlex.quote(expected_public_base_url.rstrip("/"))
    remote = f"""set -euo pipefail
EXPECTED_PUBLIC_BASE_URL={q_expected}
export EXPECTED_PUBLIC_BASE_URL
{KUBE_CONTEXT_SHELL}
work=$(mktemp -d /tmp/den2050-zed-revoke.XXXXXX)
trap 'rm -rf "$work"' EXIT
printf 'stage=context_selection\n'
select_zed_context
printf 'stage=revoke_token\n'
if ! zkubectl -n zed exec "$ZED_API_POD" -c zed-api-server -- /usr/local/bin/zed-api-server revoke-token --name {q_name} >"$work/out" 2>"$work/err"; then
  if grep -qF 'no token matched' "$work/err" "$work/out"; then
    printf 'DEN2050_ZED_TOKEN_ALREADY_ABSENT name=%s\n' {q_name}
    exit 0
  fi
  sed -E 's/zpkg_[A-Za-z0-9]+/[REDACTED_ZED_TOKEN]/g' "$work/err" >&2
  exit 1
fi
printf 'DEN2050_ZED_TOKEN_REVOKED name=%s\n' {q_name}
"""
    send_ssm(instance_id, region, remote)
    print(f"DEN2050_ZED_TOKEN_REVOKED name={token_name}")


def mint(
    instance_id: str,
    region: str,
    token_name: str,
    token_file: Path,
    expected_public_base_url: str,
) -> None:
    validate_inputs(token_name, expected_public_base_url)
    with tempfile.TemporaryDirectory(prefix="den2050-zed-token-") as tmp:
        root = Path(tmp)
        private_key = root / "private.pem"
        public_key = root / "public.pem"
        encrypted = root / "token.bin"
        run(
            [
                "openssl", "genpkey", "-algorithm", "RSA",
                "-pkeyopt", "rsa_keygen_bits:4096", "-out", str(private_key),
            ],
            capture=False,
        )
        run(["openssl", "pkey", "-in", str(private_key), "-pubout", "-out", str(public_key)], capture=False)
        public_b64 = base64.b64encode(public_key.read_bytes()).decode("ascii")
        q_name = shlex.quote(token_name)
        q_key = shlex.quote(public_b64)
        q_expected = shlex.quote(expected_public_base_url.rstrip("/"))
        remote = f"""set -euo pipefail
EXPECTED_PUBLIC_BASE_URL={q_expected}
export EXPECTED_PUBLIC_BASE_URL
{KUBE_CONTEXT_SHELL}
umask 077
work=$(mktemp -d /tmp/den2050-zed-token.XXXXXX)
pf_pid=''
cleanup() {{
  if [ -n "$pf_pid" ]; then kill "$pf_pid" >/dev/null 2>&1 || true; fi
  rm -rf "$work"
}}
trap cleanup EXIT
printf 'stage=prerequisites\n'
for command in kubectl openssl base64 tail grep find awk curl; do command -v "$command" >/dev/null; done
printf '%s' {q_key} | base64 -d > "$work/pub.pem"
printf 'stage=context_selection\n'
select_zed_context
printf 'stage=container_healthcheck\n'
zkubectl -n zed exec "$ZED_API_POD" -c zed-api-server -- /usr/local/bin/zed-api-server healthcheck >/dev/null
printf 'stage=create_token\n'
created=$(zkubectl -n zed exec "$ZED_API_POD" -c zed-api-server -- /usr/local/bin/zed-api-server create-token --name {q_name} --expires-in-days 1)
token=$(printf '%s\n' "$created" | tail -n 1)
printf '%s' "$token" | grep -Eq '^zpkg_[A-Za-z0-9]+$'
printf '%s' "$token" > "$work/token"
printf 'stage=validate_token_local\n'
printf 'header = "Authorization: Bearer %s"\n' "$token" > "$work/curl.conf"
zkubectl -n zed port-forward "pod/$ZED_API_POD" 18080:8080 >"$work/port-forward.log" 2>&1 &
pf_pid=$!
ready=0
for _ in $(seq 1 20); do
  if curl --silent --fail --max-time 2 http://127.0.0.1:18080/healthz >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
[ "$ready" = 1 ]
status=$(curl --config "$work/curl.conf" --silent --max-time 5 \
  --output "$work/probe.body" --write-out '%{{http_code}}' \
  'http://127.0.0.1:18080/v1/orgs/den2050-token-probe/audit?limit=1')
[ "$status" = 404 ] || {{
  echo "local minted-token probe returned HTTP $status" >&2
  exit 1
}}
kill "$pf_pid" >/dev/null 2>&1 || true
pf_pid=''
printf 'stage=encrypt_token\n'
openssl pkeyutl -encrypt -pubin -inkey "$work/pub.pem" -in "$work/token" \
  -pkeyopt rsa_padding_mode:oaep -pkeyopt rsa_oaep_md:sha256 -pkeyopt rsa_mgf1_md:sha256 \
  -out "$work/token.bin"
printf 'DEN2050_ZED_TOKEN_CIPHERTEXT=%s\n' "$(base64 -w0 "$work/token.bin")"
unset token created
"""
        try:
            invocation = send_ssm(instance_id, region, remote)
        except Exception:
            try:
                revoke(instance_id, region, token_name, expected_public_base_url)
            except Exception as cleanup_error:
                print(
                    f"DEN2050_ZED_TOKEN_FAILED_MINT_CLEANUP name={token_name} "
                    f"error={redact(str(cleanup_error))}",
                    file=sys.stderr,
                )
            raise

        stdout = invocation.get("Stdout") or ""
        matches = [
            line.split("=", 1)[1]
            for line in stdout.splitlines()
            if line.startswith("DEN2050_ZED_TOKEN_CIPHERTEXT=")
        ]
        if len(matches) != 1:
            try:
                revoke(instance_id, region, token_name, expected_public_base_url)
            finally:
                raise RuntimeError(f"expected exactly one encrypted Zed token, got {len(matches)}")
        encrypted.write_bytes(base64.b64decode(matches[0], validate=True))
        decrypted = run(
            [
                "openssl", "pkeyutl", "-decrypt", "-inkey", str(private_key), "-in", str(encrypted),
                "-pkeyopt", "rsa_padding_mode:oaep", "-pkeyopt", "rsa_oaep_md:sha256", "-pkeyopt", "rsa_mgf1_md:sha256",
            ]
        ).stdout.strip()
        if not re.fullmatch(r"zpkg_[A-Za-z0-9]+", decrypted):
            try:
                revoke(instance_id, region, token_name, expected_public_base_url)
            finally:
                raise RuntimeError("decrypted Zed registry token has an invalid shape")
        token_file.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        try:
            os.write(fd, decrypted.encode("utf-8"))
        finally:
            os.close(fd)
        token_file.chmod(stat.S_IRUSR | stat.S_IWUSR)
    print(f"DEN2050_ZED_TOKEN_MINTED name={token_name}")


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    mint_parser = sub.add_parser("mint")
    revoke_parser = sub.add_parser("revoke")
    for target in (mint_parser, revoke_parser):
        target.add_argument("--instance-id", required=True)
        target.add_argument("--region", required=True)
        target.add_argument("--token-name", required=True)
        target.add_argument("--expected-public-base-url", required=True)
    mint_parser.add_argument("--token-file", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "mint":
        mint(
            args.instance_id,
            args.region,
            args.token_name,
            args.token_file,
            args.expected_public_base_url,
        )
    else:
        revoke(args.instance_id, args.region, args.token_name, args.expected_public_base_url)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
