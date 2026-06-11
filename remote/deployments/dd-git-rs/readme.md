# dd-git-rs

A Rust multi-VCS operations server. It registers repositories in Postgres,
mirrors them to a local storage volume, and inspects them through each VCS's
native CLI. Supported version-control systems:

| Kind     | Binary   | Mirror layout            | Structured refs/log |
| -------- | -------- | ------------------------ | ------------------- |
| `git`    | `git`    | bare mirror directory    | yes                 |
| `hg`     | `hg`     | bare clone directory     | yes                 |
| `svn`    | `svn`    | working-copy checkout    | text (best-effort)  |
| `fossil` | `fossil` | single `.fossil` file    | text (best-effort)  |

The Postgres contract lives in `remote/libs/pg-defs/schema/schema.sql`
(`vcs_repositories`, `vcs_refs`, `vcs_operations`). This service only runs DML
against those tables; schema changes go through pg-defs, not application code.

## HTTP API

Service surface (public via the gateway): `/`, `/healthz`, `/readyz`,
`/metrics`, `/docs/api`, `/api/docs`, `/api/docs.json`.

| Method + path                         | Auth   | Purpose                                          |
| ------------------------------------- | ------ | ------------------------------------------------ |
| `GET  /api/v1/vcs/kinds`              | public | Supported VCS kinds + binary availability.       |
| `GET  /api/v1/repos`                  | public | List repositories (`?kind=&limit=`).             |
| `POST /api/v1/repos`                  | auth   | Register a repository.                            |
| `GET  /api/v1/repos/:id`              | public | Repository detail.                               |
| `DELETE /api/v1/repos/:id`            | auth   | Soft-delete + disable a repository.              |
| `POST /api/v1/repos/:id/sync`         | auth   | Mirror (first run) or re-fetch from origin.      |
| `GET  /api/v1/repos/:id/refs`         | public | Branches/tags/bookmarks (Redis-cached).          |
| `GET  /api/v1/repos/:id/log`          | public | Commit log (`?rev=&limit=`).                      |
| `GET  /api/v1/repos/:id/show/:rev`    | public | Show one commit/revision with diff.              |
| `GET  /api/v1/repos/:id/operations`   | public | Operation audit trail.                           |

Privileged routes require the `X-Server-Auth` (or `Auth`) header to match
`GIT_RS_SERVER_AUTH_SECRET`. Mirror/sync runs an outbound network clone, so it
is always behind auth; concurrent syncs of the same repo are serialized with a
Redis lock. Read routes additionally honor each repo's `visibility`: `public`
repos are open, while `private`/`internal` repos require the same auth header
(the Redis refs cache is gated too — it can't leak a private snapshot).

### Register and sync

```sh
curl -sS -X POST http://dd-git-rs:8137/api/v1/repos \
  -H 'X-Server-Auth: <secret>' -H 'content-type: application/json' \
  -d '{"slug":"k8s-cluster","vcsKind":"git","remoteUrl":"https://github.com/ORESoftware/k8s-cluster.git","defaultBranch":"dev"}'

curl -sS -X POST http://dd-git-rs:8137/api/v1/repos/<id>/sync -H 'X-Server-Auth: <secret>'
curl -sS http://dd-git-rs:8137/api/v1/repos/<id>/refs
curl -sS 'http://dd-git-rs:8137/api/v1/repos/<id>/log?limit=20'
```

## Safety model

- **No shell.** Every VCS command runs via an explicit argument vector
  (`tokio::process`). Slugs, revisions, branches, and remote URLs are validated
  before they reach a command line; revisions may not start with `-` and `--`
  terminators are used where the CLI supports them.
- **No path escape.** On-disk mirror paths are derived solely from the
  validated slug + kind, so they cannot escape `GIT_RS_STORAGE_ROOT`.
- **SSRF / file-disclosure guard.** `file://` remotes are rejected unless
  `GIT_RS_ALLOW_FILE_URLS=true`; remotes that resolve to loopback, link-local,
  the unspecified address, or the cloud metadata endpoint (`169.254.169.254`)
  are always blocked, and private networks too when
  `GIT_RS_BLOCK_PRIVATE_REMOTES=true`.
- **Bounded resource use.** A global semaphore (`GIT_RS_MAX_CONCURRENT_OPS`)
  caps concurrent VCS subprocesses and sheds excess load with `503`; commands
  run under timeouts and capture at most `GIT_RS_MAX_OUTPUT_BYTES` per stream;
  request bodies are capped at `GIT_RS_MAX_BODY_BYTES`.
- **No secret/path leakage.** URL credentials (`user:pass@`) are stripped from
  API responses, audit rows, logs, and NATS events; the storage root is scrubbed
  from VCS error text before it is returned or stored; the absolute `mirror_path`
  is write-only and never surfaced in responses.
- **No prompts.** Credential prompts are disabled (`GIT_TERMINAL_PROMPT=0`,
  `GIT_ASKPASS=/bin/true`, `GIT_CONFIG_GLOBAL=/dev/null`, SSH `BatchMode`,
  `HGPLAIN`).

## Configuration

| Env var                          | Default                                   | Notes                                  |
| -------------------------------- | ----------------------------------------- | -------------------------------------- |
| `GIT_RS_HOST` / `HOST`           | `0.0.0.0`                                  | Bind host.                             |
| `GIT_RS_PORT` / `PORT`           | `8137`                                     | Bind port.                             |
| `DD_GIT_RDS_DATABASE_URL`        | (falls back to `RDS_DATABASE_URL`, …)      | Postgres connection string.            |
| `GIT_RS_SERVER_AUTH_SECRET`      | (falls back to `SERVER_AUTH_SECRET`)       | Shared secret for privileged routes.   |
| `GIT_RS_ALLOW_UNAUTHENTICATED`   | `false`                                    | Disable auth (dev only).               |
| `GIT_RS_STORAGE_ROOT`            | `/var/lib/dd-git-rs/repos`                 | Local mirror volume.                   |
| `GIT_RS_REDIS_URL` / `REDIS_URL` | in-cluster `dd-redis-cache`                | Ref cache + sync lock.                 |
| `GIT_RS_REDIS_PREFIX`            | `dd:git`                                   | Redis key prefix.                      |
| `GIT_RS_MIRROR_TIMEOUT_SECONDS`  | `600`                                      | Clone/fetch timeout.                   |
| `GIT_RS_READ_TIMEOUT_SECONDS`    | `120`                                      | refs/log/show timeout.                 |
| `GIT_RS_REFS_CACHE_TTL_SECONDS`  | `60`                                       | Redis refs TTL.                        |
| `GIT_RS_MAX_OUTPUT_BYTES`        | `4194304`                                  | Per-stream output cap.                 |
| `GIT_RS_MAX_CONCURRENT_OPS`      | `4`                                        | Concurrent VCS subprocess cap (503 over). |
| `GIT_RS_MAX_BODY_BYTES`          | `65536`                                    | Max request body size.                 |
| `GIT_RS_ALLOW_FILE_URLS`         | `false`                                    | Permit `file://` remotes (disclosure risk). |
| `GIT_RS_BLOCK_PRIVATE_REMOTES`   | `false`                                    | Also reject private-network remotes.   |
| `GIT_RS_LOG_FORMAT`              | text                                       | `json` for structured logs.            |
| `NATS_URL`                       | unset                                      | Publishes sync failures to the         |
|                                  |                                            | `dd.remote.events.critical` subject.   |

## Build / run locally

```sh
cargo run --release --locked
```

Requires `git`, `hg`, `svn`, and `fossil` on `PATH` for full functionality; the
service starts and reports per-kind availability at `/api/v1/vcs/kinds` even if
some binaries are missing. Readiness only requires `git`.
