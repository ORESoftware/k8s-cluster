# `dd-rust-vapi-phone`

A Rust [Vapi.ai](https://vapi.ai) AI phone-tree call screener for Alex Mills.

Inbound callers reach a Vapi voice assistant that greets them, screens them,
and either **warm-transfers verified humans** to a personal forwarding number or
politely declines scammers/spammers.

There is no official Vapi Rust SDK, so the service calls the Vapi REST API
(`https://api.vapi.ai`) directly with `reqwest` — the same pattern
`dd-contract-service` uses for Solana JSON-RPC.

## The phone tree

The greeting the caller hears:

> This is the phone system for Alex Mills, a software developer based out of
> Austin, Texas. I will take your call personally. Please pick your option.
> Option 1: I am a recruiter. Option 2: I am a scammer and a spammer.

- **Option 1 (recruiter / real human):** after a short, natural human check the
  assistant calls the Vapi `transferCall` tool and forwards the caller to
  `+17372814824`.
- **Option 2 (scammer / spammer), robocalls, IVRs, or anyone who dodges the
  human check:** the assistant declines and ends the call.

The greeting, the screening system prompt, the voice, and the `transferCall`
destination all live in `build_assistant_config()` in
[`src/main.rs`](./src/main.rs). That single function is the source of truth:
`POST /setup` pushes it to Vapi, and `POST /webhook` can also return it inline
for Vapi's `assistant-request` flow.

## How it works

1. An operator calls `POST /vapi/setup` (gateway server-auth). The service:
   - creates or updates the screening **assistant** in Vapi (idempotent by
     `VAPI_ASSISTANT_ID` if set, otherwise by assistant name), and
   - attaches that assistant + this service's webhook to a Vapi **phone
     number** (using `VAPI_PHONE_NUMBER_ID` if set, reusing an existing matching
     number, or provisioning a fresh free Vapi number).
2. A person calls the Vapi number and hears the greeting.
3. Vapi runs the assistant. The model screens the caller per the system prompt.
4. On a human pass → `transferCall` → `+17372814824`. On a fail → `endCall`.
5. Vapi posts call lifecycle events to `POST /vapi/webhook`. The service
   verifies the `x-vapi-secret` header, answers `assistant-request`,
   `tool-calls`, and `transfer-destination-request` inline, records
   `end-of-call-report` metrics, and stores compact redacted call metadata in
   RDS Postgres plus low-latency caller context in Redis.

## Env vars

| Var                        | Default                              | Purpose                                                                                  |
| -------------------------- | ------------------------------------ | ---------------------------------------------------------------------------------------- |
| `HOST` / `PORT`            | `0.0.0.0` / `8113`                   | Bind address.                                                                            |
| `VAPI_API_KEY`             | _unset_                              | Vapi **private** API key. Required for `/setup` and `/status`; webhook receiving works without it. |
| `VAPI_SERVER_SECRET`       | _unset_                              | Shared secret verified on inbound `x-vapi-secret`. Required when `VAPI_WEBHOOK_URL` is configured unless `VAPI_ALLOW_UNSIGNED_WEBHOOKS=true` is explicitly set for local testing. |
| `VAPI_SERVER_CREDENTIAL_ID`| _unset_                              | Optional Vapi server credential id. When set, `/setup` sends `server.credentialId` instead of inline `server.secret`; `VAPI_SERVER_SECRET` is still required locally to verify inbound requests. |
| `SERVER_AUTH_SECRET`       | _unset_                              | `x-server-auth` value required on `/setup` and `/status`. Supplied by the gateway. Fails closed when unset. |
| `VAPI_WEBHOOK_URL`         | `https://54.91.17.58/vapi/webhook`   | Public URL Vapi posts events to. Must be https (Vapi rejects self-signed certs — use the Let's Encrypt gateway cert). |
| `VAPI_FORWARD_NUMBER`      | `+17372814824`                       | E.164 number verified humans are transferred to.                                         |
| `VAPI_OWNER_NAME`          | `Alex Mills`                         | Used in the greeting, prompt, and transfer message.                                      |
| `VAPI_OWNER_TITLE`         | `a software developer based out of Austin, Texas` | Used in the greeting and prompt.                                            |
| `VAPI_FIRST_MESSAGE`       | _(the greeting above)_               | Override the spoken greeting.                                                            |
| `VAPI_ASSISTANT_NAME`      | `Alex Mills Call Screener`           | Assistant name; also used for idempotent lookup.                                         |
| `VAPI_ASSISTANT_ID`        | _unset_                              | Update this assistant id instead of upserting by name.                                   |
| `VAPI_PHONE_NUMBER_ID`     | _unset_                              | Attach to this Vapi phone number id instead of reusing/creating one.                     |
| `VAPI_DESIRED_AREA_CODE`   | _unset_                              | 3-digit area code hint when provisioning a fresh free Vapi local number (e.g. `737`).    |
| `VAPI_NUMBER_PROVIDER`     | `vapi`                               | `vapi` allots a free US **local** number. `twilio` / `telnyx` / `vonage` import a BYO number — the only way to get a toll-free 800 number. |
| `VAPI_PHONE_NUMBER`        | _unset_                              | E.164 number to import when `VAPI_NUMBER_PROVIDER` is a BYO carrier (e.g. `+18005551234`). |
| `TWILIO_ACCOUNT_SID` / `TWILIO_AUTH_TOKEN` | _unset_              | Twilio credentials used to import a BYO Twilio number.                                   |
| `VAPI_CREDENTIAL_ID`       | _unset_                              | Pre-stored Vapi carrier credential id (alternative to inline Twilio creds; required for telnyx/vonage). |
| `VAPI_MODEL_PROVIDER`      | `openai`                             | LLM provider for the assistant.                                                          |
| `VAPI_MODEL`               | `gpt-4o`                             | LLM model.                                                                               |
| `VAPI_VOICE_PROVIDER`      | `vapi`                               | Voice provider.                                                                          |
| `VAPI_VOICE_ID`            | `Elliot`                             | Voice id.                                                                                |
| `VAPI_ENABLE_SERVER_TOOLS` | `true`                               | Adds server-side Vapi function tools for recent-caller lookup and compact screening-signal recording. |
| `VAPI_DATABASE_URL` / `AGENT_TASKS_RDS_DATABASE_URL` / `RDS_DATABASE_URL` / `DATABASE_URL` | _unset_ | Optional RDS/Postgres URL. When set, redacted call events are inserted into `vapi_phone_call_events`. |
| `VAPI_REDIS_URL` / `REDIS_URL` | `redis://dd-redis-cache.default.svc.cluster.local:6379/0` | Redis cache used by server tools for per-caller context and per-call screening signals. |
| `VAPI_REDIS_KEY_PREFIX`    | `dd:vapi-phone`                      | Redis key prefix from `remote/libs/interfaces/redis`.                                    |
| `VAPI_REDIS_CACHE_TTL_SECONDS` | `2592000`                        | TTL for caller context and per-call signal cache entries.                                |
| `VAPI_DISABLE_REDIS`       | `false`                              | Local-dev escape hatch to skip Redis client setup.                                       |
| `VAPI_API_BASE`            | `https://api.vapi.ai`                | Vapi REST base URL. Must be `https://` unless `VAPI_ALLOW_HTTP_API_BASE=true` is set for local testing. |
| `VAPI_HTTP_TIMEOUT_SECONDS`| `20`                                 | Per-request Vapi API timeout.                                                            |
| `VAPI_FLAMEGRAPH_DIR`      | `${CARGO_TARGET_DIR}/flamegraphs` or `target/flamegraphs` | Directory read by `/flamegraph` and written by the opt-in profiling helper. |
| `VAPI_ALLOW_UNAUTHENTICATED` | `false`                            | Local-dev escape hatch: skip the `x-server-auth` check on admin routes.                  |
| `VAPI_ALLOW_HTTP_WEBHOOK`  | `false`                              | Allow an `http://` webhook URL for local tunnels.                                        |
| `VAPI_ALLOW_HTTP_API_BASE` | `false`                              | Allow an `http://` Vapi API base for local test doubles.                                 |
| `VAPI_ALLOW_UNSIGNED_WEBHOOKS` | `false`                          | Local-dev escape hatch: accept webhook requests when `VAPI_SERVER_SECRET` is unset.      |

`VAPI_SERVER_SECRET`, `VAPI_API_KEY`, and optional `VAPI_SERVER_CREDENTIAL_ID`
are pulled from the `dd-agent-secrets` Kubernetes secret (AWS Secrets Manager
`dd/remote-dev/agent-secrets`). RDS URLs are pulled from
`dd-remote-rest-api-secrets`; Redis points at the in-cluster `dd-redis-cache`.
Add or rotate JSON keys in AWS Secrets Manager; do not commit them to Git. See
[`remote/readme.md`](../../readme.md) "Secrets And Key Rotation".

## HTTP API

| Method | Path           | Auth               | Purpose                                                            |
| ------ | -------------- | ------------------ | ------------------------------------------------------------------ |
| GET    | `/`            | gateway cookie     | HTML descriptor of the phone tree.                                 |
| GET    | `/healthz`     | public             | Liveness + probe-safe config booleans (no phone numbers or secrets). |
| GET    | `/metrics`     | public             | Prometheus metrics.                                                |
| GET    | `/flamegraph`  | gateway cookie     | Latest opt-in flamegraph viewer with UTC run timestamps.           |
| GET    | `/flamegraph.svg` | gateway cookie  | Latest opt-in flamegraph SVG.                                      |
| GET    | `/config`      | gateway cookie     | Secret-free view of the assistant the service will install.        |
| GET    | `/status`      | `x-server-auth`    | Live Vapi assistants + phone numbers for the configured key.       |
| POST   | `/setup`       | `x-server-auth`    | Provision/refresh the assistant + phone number.                    |
| POST   | `/webhook`     | `x-vapi-secret`    | Vapi server webhook (assistant-request, tool-calls, transfer, end-of-call). |
| GET    | `/docs/api`, `/api/docs`, `/api/docs.json` | public | Generated API docs.                            |

Behind the gateway these are served under `/vapi/...`. The webhook is the one
`/vapi/*` path that is **not** behind the operator cookie (Vapi cannot send the
cookie); it is authenticated by the `x-vapi-secret` shared secret instead.

## Build

```bash
# Local
cd remote/deployments/rust-vapi-phone-rs
VAPI_ALLOW_UNSIGNED_WEBHOOKS=true cargo run --release

# Image — repo root must be the build context so the shared client path dep
# is included
docker build -f remote/deployments/rust-vapi-phone-rs/Dockerfile -t dd-rust-vapi-phone:dev .
```

## Profiling

Flamegraph profiling is set up but not enabled in the normal deployment. Use it
only during an explicit profiling session; it samples the process and can slow
the service while it runs.

Install the profiler locally or on the host where you will run the profile:

```bash
cargo install flamegraph --locked
```

Then check host prerequisites without starting a profile:

```bash
cd remote/deployments/rust-vapi-phone-rs
bash scripts/flamegraph-vapi.sh check
```

The bounded `local` and `attach` modes also require a `timeout` command
available in `PATH` so the profiler can stop and flush the SVG. The helper has
two opt-in modes:

```bash
# Run a bounded local profile against a dev instance on 127.0.0.1:18113.
DURATION_SECONDS=60 bash scripts/flamegraph-vapi.sh local

# Attach to an already-running process by pid.
DURATION_SECONDS=60 bash scripts/flamegraph-vapi.sh attach <pid>
```

Outputs go to `VAPI_FLAMEGRAPH_DIR` (or `target/flamegraphs` locally) as
`*.svg`, plus `latest.json` with `runStartedAtUtc`, `runFinishedAtUtc`, mode,
pid, and duration. The server displays the newest run at `/vapi/flamegraph` and
serves the SVG at `/vapi/flamegraph.svg`; neither route starts profiling.

The helper sets profiling-only release debug symbols and frame pointers, and on
Linux adds the Rust 1.90/lld `--no-rosegment` linker flag needed for accurate
`perf` stacks. None of those profiling settings are part of the Kubernetes
manifest.

## Operating

```bash
# Inspect the phone tree the service will install (no secrets, no Vapi call)
curl -s https://54.91.17.58/vapi/config | jq .

# Provision the assistant + phone number (gateway injects x-server-auth)
curl -s -X POST https://54.91.17.58/vapi/setup | jq .

# What does Vapi currently have?
curl -s https://54.91.17.58/vapi/status | jq .
```

## Phone numbers: free Vapi local vs. toll-free 800

Vapi does **not** resell toll-free numbers, and there is no free 800 number.

| Option | What you get | Cost | How |
| ------ | ------------ | ---- | --- |
| **Free Vapi number** (`VAPI_NUMBER_PROVIDER=vapi`, default) | A US **local** number (you pick the area code, e.g. `737` for Austin). Up to 10 per wallet. | Free calls; covered by the $10 trial. | `POST /vapi/setup` allots it automatically. |
| **Toll-free 800/888/833** | A real toll-free number. | Buy + monthly rent at the carrier (Twilio ≈ $2/mo + usage) plus Vapi's per-minute fee. Requires carrier **toll-free verification**. | Bring Your Own carrier: buy/verify the number in Twilio, then import it (below). |

### Trial account

New Vapi accounts get **$10 in free credits** (~150–200 minutes) — enough to test
the whole flow on a free Vapi local number. There is no ongoing free tier; after
the credits, calls are pay-as-you-go (~$0.05/min Vapi + LLM + voice + telephony).

### Hooking up a toll-free 800 number (BYO Twilio)

1. Create a Twilio account and **buy a toll-free number** (Phone Numbers → Buy a
   number → Toll-free). Submit Twilio's **toll-free verification** — unverified
   toll-free numbers get filtered/blocked by carriers.
2. Add `VAPI_API_KEY`, `TWILIO_ACCOUNT_SID`, and `TWILIO_AUTH_TOKEN` to AWS
   Secrets Manager `dd/remote-dev/agent-secrets` (synced into `dd-agent-secrets`).
3. On the deployment set:
   - `VAPI_NUMBER_PROVIDER=twilio`
   - `VAPI_PHONE_NUMBER=+18005551234` (the toll-free number you bought)
4. `POST /vapi/setup`. The service imports the Twilio number into Vapi and
   attaches the screening assistant + webhook to it. `/setup` is idempotent: it
   reuses the number if it's already imported.

A `VAPI_CREDENTIAL_ID` (a carrier credential pre-stored in Vapi) can be used
instead of inline Twilio creds, and is required for Telnyx/Vonage imports.

## Notes / future work

- Vapi requires a publicly reachable **https** webhook with a trusted
  certificate. Point `VAPI_WEBHOOK_URL` at the Let's Encrypt gateway cert, not
  the self-signed bootstrap cert.
- RDS stores only compact, redacted call metadata in
  `vapi_phone_call_events`. Raw transcripts, recordings, and phone numbers stay
  out of this table; Vapi remains the system of record for full call artifacts.
- Redis caches hashed caller context and per-call screening signals using key
  formatters generated from `remote/libs/interfaces/redis`.
- A natural next step is publishing `end-of-call-report` summaries to NATS
  (`dd.remote.events`) like `dd-contract-service` does, for the telemetry plane.
