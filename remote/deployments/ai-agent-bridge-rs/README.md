# `ai-agent-bridge-rs`

Token-protected LAN inbox so a **peer AI agent** (e.g. the Codex MacBook) can push
messages to **this Claude session**. Claude polls `inbox.jsonl` via a watcher,
processes each message, and replies by POSTing to the peer's own bridge. Symmetric
to the peer HTTP bridge (the Codex side listens on `:8765`; this side on `:8766`).

Rust port of the retired `~/bin/claude_inbox_bridge.py`. Kept **wire-compatible** so
existing senders and the Claude-side watcher keep working unchanged. Dependency-light
on purpose: a threaded `std::net` HTTP/1.1 server (mirrors Python's
`ThreadingTCPServer`) plus `serde_json` — no async runtime for two routes.

## HTTP surface

| method | path      | auth        | effect |
|--------|-----------|-------------|--------|
| `GET`  | `/health` | none        | `{ok, service, port, inbox_messages, auth}` |
| `POST` | `/claude` | Bearer token | append one JSON line to `inbox.jsonl`, return `{queued, id, note}` |

`POST /claude` body: `{"prompt": "...", "from": "codex", "topic": "..."}`. Only
`prompt` is required. Each queued line is:

```json
{"id": 1783542256013, "ts": "2026-07-08T20:24:16Z", "from": "codex", "topic": "plateau", "prompt": "..."}
```

- `id` — epoch milliseconds. `ts` — UTC `YYYY-MM-DDTHH:MM:SSZ`.
- `from` is truncated to 64 chars, `topic` to 128 (parity with the Python bridge).
- `prompt` is preserved verbatim (quotes, newlines, unicode round-trip exactly).

## Config (env)

New `AI_AGENT_BRIDGE_*` names, falling back to the legacy `CLAUDE_INBOX_*` names so
this is a drop-in replacement:

| var | default | meaning |
|-----|---------|---------|
| `AI_AGENT_BRIDGE_PORT` / `CLAUDE_INBOX_PORT` | `8766` | listen port |
| `AI_AGENT_BRIDGE_TOKEN` / `CLAUDE_INBOX_TOKEN` | *(empty)* | Bearer token for `POST /claude`; empty ⇒ **open** (dev only) |
| `AI_AGENT_BRIDGE_DIR` / `CLAUDE_INBOX_DIR` | `/tmp/claude_bridge` | dir holding `inbox.jsonl` |

## Run locally

```sh
cargo build --release
AI_AGENT_BRIDGE_TOKEN=<token> ./target/release/ai-agent-bridge
# health
curl -s localhost:8766/health
# a peer pushes a message
curl -s -X POST localhost:8766/claude \
  -H "Authorization: Bearer <token>" \
  -d '{"from":"codex","topic":"plateau","prompt":"round 17: ..."}'
```

The Claude-side watcher tails `${AI_AGENT_BRIDGE_DIR}/inbox.jsonl` and replies via the
peer bridge; that half is unchanged by this port.

## Runtime model / deployment note

This is a **LAN-local daemon**, not a cluster workload: the peer Codex Mac reaches it
over the LAN and the local Claude watcher reads `inbox.jsonl` on the **same host**.
It therefore ships as a buildable crate here (repo hygiene, alongside the other `*-rs`
services) and runs on the Mac — not (yet) as an in-cluster Deployment. The `Dockerfile`
builds a static musl binary if we ever want to containerize it; k8s manifests are
intentionally **not** added until there's a cluster-side use for a shared inbox.
