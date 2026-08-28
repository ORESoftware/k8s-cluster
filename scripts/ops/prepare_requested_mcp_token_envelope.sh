#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

key_root="/var/lib/oresoftware/requested-mcp-token-envelope"
private_key="$key_root/private.pem"
public_key="$key_root/public.pem"
probe="$key_root/probe.bin"
probe_ciphertext="$key_root/probe.bin.enc"
probe_plaintext="$key_root/probe.bin.dec"

die() {
  printf 'MCP_TOKEN_ENVELOPE_ERROR stage=%s code=%d\n' "$1" "${2:-1}"
  exit "${2:-1}"
}

command -v openssl >/dev/null 2>&1 || die prerequisites 64
command -v base64 >/dev/null 2>&1 || die prerequisites 64
command -v sha256sum >/dev/null 2>&1 || die prerequisites 64

install -d -m 0700 "$key_root"
rm -f "$private_key" "$public_key" "$probe" "$probe_ciphertext" "$probe_plaintext"

openssl genpkey \
  -algorithm RSA \
  -pkeyopt rsa_keygen_bits:4096 \
  -out "$private_key" \
  >/dev/null 2>&1 || die key-generation 65
chmod 0600 "$private_key"

openssl pkey \
  -in "$private_key" \
  -pubout \
  -out "$public_key" \
  >/dev/null 2>&1 || die public-key-derivation 66
chmod 0644 "$public_key"

# Prove the exact OAEP-SHA256 contract before publishing the public key.
openssl rand 32 > "$probe" || die key-self-test 67
openssl pkeyutl \
  -encrypt \
  -pubin \
  -inkey "$public_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  -in "$probe" \
  -out "$probe_ciphertext" \
  >/dev/null 2>&1 || die key-self-test 67
openssl pkeyutl \
  -decrypt \
  -inkey "$private_key" \
  -pkeyopt rsa_padding_mode:oaep \
  -pkeyopt rsa_oaep_md:sha256 \
  -pkeyopt rsa_mgf1_md:sha256 \
  -in "$probe_ciphertext" \
  -out "$probe_plaintext" \
  >/dev/null 2>&1 || die key-self-test 67
cmp -s "$probe" "$probe_plaintext" || die key-self-test 67
rm -f "$probe" "$probe_ciphertext" "$probe_plaintext"

fingerprint="$(
  openssl pkey -pubin -in "$public_key" -outform DER 2>/dev/null \
    | sha256sum \
    | awk '{print $1}'
)"
[[ "$fingerprint" =~ ^[0-9a-f]{64}$ ]] || die fingerprint 68
public_key_b64="$(base64 --wrap=0 "$public_key")"
[[ "$public_key_b64" =~ ^[A-Za-z0-9+/=]+$ ]] || die encoding 69

printf 'MCP_TOKEN_ENVELOPE_PUBLIC_KEY fingerprint=%s key=%s\n' \
  "$fingerprint" "$public_key_b64"
