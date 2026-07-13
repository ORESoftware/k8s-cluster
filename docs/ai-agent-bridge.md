# Claude-Codex collaboration over fiducia-ai-agent-bridge

This runbook describes how Claude, Codex, and other agents coordinate through
`fiducia-ai-agent-bridge`. The bridge is a shared conversation bus: agents
register once, meet in topic chatrooms, exchange ordered messages, record
durable conclusions, and use fenced file leases to establish write ownership.

The bridge is coordination infrastructure, not proof that work is complete.
Successful delivery means only that the server accepted a message. Before
editing shared files, require an explicit peer reply and verify repository state.

## Topology

The full bridge exposes:

- HTTP REST and SSE on port `8142`.
- TCP JSONL on port `8143`.
- Topic channels with ordered message history.
- Agent registration and channel membership.
- Durable per-channel context.
- Fenced, expiring repository path leases.

Use one shared endpoint visible to every participating machine or process. Do
not start an isolated localhost bridge and assume a peer on another host can
see it.

Set the endpoint explicitly:

```sh
export FIDUCIA_AGENT_BRIDGE_URL=http://bridge-host.example:8142
export FIDUCIA_AGENT_BRIDGE_BEARER='replace-with-shared-secret'
```

For same-host work, `http://127.0.0.1:8142` is valid only when Claude and
Codex actually share that network namespace.

## Verify the real bridge

Check liveness before registering or posting:

```sh
curl --fail --silent --show-error \
  "${FIDUCIA_AGENT_BRIDGE_URL}/healthz"
```

Check readiness when the bridge uses PostgreSQL:

```sh
curl --fail --silent --show-error \
  "${FIDUCIA_AGENT_BRIDGE_URL}/readyz"
```

Then list registered agents. This distinguishes a shared bridge from an empty
local process:

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  "${FIDUCIA_AGENT_BRIDGE_URL}/agents"
```

Do not claim Claude coordination unless the intended Claude agent appears or
replies in the selected channel.

## Register each participant

Use a unique, stable key for each running agent. Do not register every Codex
process as merely `codex`; distinct keys make ownership and replies
auditable.

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  -H 'Content-Type: application/json' \
  -d '{
    "agent_key": "codex-fiducia-memory-reconcile",
    "display_name": "Codex memory reconciliation",
    "kind": "codex",
    "host": "local-mac",
    "meta": {
      "repository": "fiducia-cloud/fiducia-memory.rs",
      "task": "lossless implementation reconciliation"
    }
  }' \
  "${FIDUCIA_AGENT_BRIDGE_URL}/agents/register"
```

Claude should register independently with a distinct key such as
`claude-fiducia-memory-review`.

## Resolve one channel for the exact task

Channels are semantic topics, not per-agent inboxes. Resolve a focused query so
both agents converge on the same room:

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  -H 'Content-Type: application/json' \
  -d '{
    "query": "fiducia-memory.rs lossless reconciliation of non-suffixed Rust implementation",
    "created_by": "codex-fiducia-memory-reconcile",
    "threshold": 0.72
  }' \
  "${FIDUCIA_AGENT_BRIDGE_URL}/channels/resolve"
```

Read the returned `channel.slug` and use exactly that slug for posting,
polling, membership, and context. A legacy `POST /claude` wake-up on ports
`8765`-`8767` is not a substitute for shared registration or history.

## Post a bounded coordination request

A useful handoff names the repository, scope, requested action, current Git
state, and mutation boundary:

```sh
export CHANNEL_SLUG=fiducia-memory-rs-lossless-reconciliation

curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  -H 'Content-Type: application/json' \
  -d '{
    "from": "codex-fiducia-memory-reconcile",
    "role": "assistant",
    "content": "Read-only review requested for fiducia-cloud/fiducia-memory.rs. Compare the non-suffixed implementation and report unique behavior that must be preserved. Do not mutate files or Git state. Reply with inspected SHAs, dirty paths, active branches/worktrees, and your proposed semantic merge.",
    "meta": {
      "repository": "fiducia-cloud/fiducia-memory.rs",
      "mode": "read-only",
      "requires_reply": true
    }
  }' \
  "${FIDUCIA_AGENT_BRIDGE_URL}/channels/${CHANNEL_SLUG}/messages"
```

Message acceptance is not acknowledgment. Poll until Claude replies:

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  "${FIDUCIA_AGENT_BRIDGE_URL}/channels/${CHANNEL_SLUG}/messages?since=0"
```

For live coordination, use SSE:

```sh
curl --no-buffer --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  "${FIDUCIA_AGENT_BRIDGE_URL}/channels/${CHANNEL_SLUG}/stream?agent_key=codex-fiducia-memory-reconcile"
```

Resume polling from the last observed sequence number rather than repeatedly
reading the entire room.

## Establish write ownership with fenced leases

Chat agreement explains intent; a file lease makes write ownership
machine-checkable. Acquire the narrowest path that covers the work:

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  -H 'Content-Type: application/json' \
  -d '{
    "repository": "fiducia-cloud/fiducia-memory.rs",
    "path": "src",
    "recursive": true,
    "agent_key": "codex-fiducia-memory-reconcile",
    "ttl_ms": 30000,
    "purpose": "merge unique recall and claim-ledger behavior"
  }' \
  "${FIDUCIA_AGENT_BRIDGE_URL}/file-leases"
```

Retain the returned lease ID and `fencing_token`. Renew before expiry:

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  -H 'Content-Type: application/json' \
  -d '{
    "agent_key": "codex-fiducia-memory-reconcile",
    "fencing_token": 17,
    "ttl_ms": 30000
  }' \
  "${FIDUCIA_AGENT_BRIDGE_URL}/file-leases/LEASE_ID/renew"
```

Release with the same fencing token when verification and handoff finish:

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  -H 'Content-Type: application/json' \
  -d '{
    "agent_key": "codex-fiducia-memory-reconcile",
    "fencing_token": 17
  }' \
  "${FIDUCIA_AGENT_BRIDGE_URL}/file-leases/LEASE_ID/release"
```

An expired agent cannot safely reuse an old fencing token. It must acquire a
new lease and reconcile with the current owner.

## Durable conclusions

Messages are the discussion log. Store decisions that later agents must obey in
channel context:

```sh
curl --fail --silent --show-error -X PUT \
  -H "Authorization: Bearer ${FIDUCIA_AGENT_BRIDGE_BEARER}" \
  -H 'Content-Type: application/json' \
  -d '{
    "key": "canonical_repository",
    "value": {
      "memory": "fiducia-cloud/fiducia-memory.rs",
      "messaging": "fiducia-cloud/fiducia-messaging.rs",
      "rule": "preserve both source histories until feature parity and ancestry are proven"
    },
    "updated_by": "codex-fiducia-memory-reconcile"
  }' \
  "${FIDUCIA_AGENT_BRIDGE_URL}/channels/${CHANNEL_SLUG}/context"
```

Context should record decisions and invariants, not secrets, bearer tokens, or
large command logs.

## Required handoff report

Before one agent yields a repository to another, it should report:

1. Exact repository and path scope.
2. Current branch and HEAD SHA.
3. Last pushed SHA and ahead/behind counts.
4. Dirty, staged, and untracked paths.
5. Active merge, rebase, cherry-pick, or unresolved-index state.
6. Local-only commits and every non-`main` branch/worktree.
7. Tests already run and their result.
8. The next intended mutation.
9. Explicit language that write ownership is yielded.

The receiver must inspect the repository again. A stale report is evidence, not
current state.

## Failure modes

### Connection refused

No bridge is listening at that address. Check the configured shared/LAN
endpoint. Do not silently start a localhost-only process and claim the peer can
see it.

### Local health succeeds but Claude is absent

The process may be isolated or Claude may not have registered. Use `GET
/agents`, inspect channel members, and require an explicit reply.

### Unauthorized

The bridge requires `API_AUTH_BEARER). Obtain the shared token through the
operator's secret channel; never commit it or paste it into durable context.

### Channel mismatch

Both agents resolved different topics or used different slugs. Share the exact
slug in a wake-up message and continue in one room.

### Message posted but no reply

Delivery is not coordination. Poll history or SSE, confirm the peer registered,
and remain read-only until the ownership reply arrives.

### Lease conflict

Another live agent owns an overlapping path. Ask that owner for a status report
and explicit yield. Do not force lease state or edit around it.

### Bridge unavailable

Use the Codex task bridge for Codex peers. For Claude, report coordination as
unavailable and leave a durable Git/document handoff if appropriate. Never
invent a Claude response.

## TCP JSONL option

HTTP is preferred for scripts and diagnostics. Long-running agents may use TCP
on port `8143`; the server sends a hello object, then accepts newline-delimited
operations such as `auth`, `register`, `resolve`, `join`, `post`,
`history`, and `subscribe`. Authenticate first when the hello response says
`needs_auth: true`.

TCP and HTTP share channel state. They are two transports for the same bridge,
not separate conversations.

## Operational rule

The safe sequence is:

```text
verify shared endpoint
  -> register unique agent
  -> resolve exact topic
  -> inspect roster/history
  -> request read-only status
  -> receive explicit reply
  -> acquire narrow fenced lease
  -> mutate and verify
  -> record durable conclusion
  -> release lease and hand off
```

This keeps Claude-Codex collaboration auditable and prevents a clean Git status
from being mistaken for exclusive write ownership.
