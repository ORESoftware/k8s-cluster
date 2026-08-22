# honeypot.rs

`honeypot.rs` is a low-interaction Rust deception service for the ORESoftware Kubernetes fleet. It renders a believable control-plane login with Leptos server-side rendering, exposes synthetic honeytokens through a small set of impossible paths, detects exact reuse, and emits privacy-minimized signed security events.

It is a sensor, not a retaliation system. It must never contain production credentials, execute attacker input, provide a general-purpose shell, or redirect volumetric denial-of-service traffic into the cluster.

## Initial lures

- `/`
- `/admin/login`
- `/.env`
- `/.git/config`
- `/backup/config.json`
- `/api/v1/auth`
- `/api/v1/backup`
- `/robots.txt`

Tokens use the vendor-neutral format `ores_hp_v1_<lure>_<generation>_<hmac-prefix>`. They authenticate nowhere, and every embedded host uses the reserved `.invalid` top-level domain.

## Privacy boundary

Events contain HMAC-pseudonymized source and user-agent identifiers, method, path without query, request size, lure, signal, evidence count, a reversible policy recommendation, optional trusted Cloudflare metadata, and an HMAC-SHA256 event signature.

The service never logs raw IP addresses, raw user agents, request bodies, usernames, passwords, authorization values, cookies, query strings, or honeytoken values.

## Required runtime secrets

Each value must contain at least 32 bytes:

- `HONEYTOKEN_HMAC_KEY`
- `PSEUDONYM_HMAC_KEY`
- `EVENT_HMAC_KEY`

Optional settings include `BIND_ADDR`, `PUBLIC_ORIGIN`, `LURE_GENERATION`, `TRUST_CLOUDFLARE_HEADERS`, `TRUSTED_PROXY_CIDRS`, `MAX_REQUEST_BYTES`, `MAX_CONCURRENT_REQUESTS`, and `REQUEST_TIMEOUT_SECONDS`.

Run `python3 scripts/validate_repo.py`, `cargo fmt --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, and `cargo test --locked --all-targets --all-features` before promotion.

Cloudflare must absorb DDoS traffic at the edge. Only bounded, exact impossible-path traffic may reach this ClusterIP service through Cloudflare Tunnel.
