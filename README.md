# mip-solver-node.rs

Rust server deployment for `dd-in-house-mip-solver-node`: a distributed in-house LP/MIP/IP solver node using NATS JetStream for branch-and-bound work distribution.

The same binary can run as either a master or a slave. The role is deterministic at boot via `MIP_SOLVER_NODE_ROLE`; pods do not elect or switch roles dynamically.

## NATS source of truth

When this repo is mounted as `k8s-cluster/remote/deployments/mip-solver-node.rs`, `Cargo.toml` imports generated constants from:

`../../libs/nats/subject-defs/generated/rust`

The service uses those generated definitions for the MIP solver jobs, results, events, stream name, and worker queue group, avoiding local string drift in Rust code.

## Local checkout

This repo was created for `~/codes/ores/mip-solver-node.rs`. For direct local checks from that location, use the local manifest because the shared path dependencies live in the sibling k8s-cluster checkout:

`cargo check --manifest-path local/Cargo.toml`

For the k8s-cluster submodule layout, use the normal manifest:

`cargo check --locked`

## Runtime solve defaults

Request `options` override environment defaults. When an option is omitted, the service reads:

- `MIP_SOLVER_MAX_NODES`
- `MIP_SOLVER_MAX_TICKS`
- `MIP_SOLVER_LP_MAX_ITERS`
- `MIP_SOLVER_INT_TOL`
- `MIP_SOLVER_SPLIT_DEPTH`
- `MIP_SOLVER_MAX_SUBPROBLEMS`
- `MIP_SOLVER_MAX_JOB_RETRIES`
- `MIP_SOLVER_TIMEOUT_MS`
- `MIP_SOLVER_EMIT_TRACE`

`MIP_SOLVER_MAX_SUBPROBLEMS` caps the master's pre-split branch-and-bound frontier. When the cap is reached, remaining fractional nodes are delegated as subtree jobs instead of being split further by the master.

Delegated subtree jobs can split again on a slave when their LP relaxation is fractional and their depth is still below `splitDepth`. The slave returns a `split` result with child jobs; the master records the parent as non-terminal, publishes the children to JetStream, and aggregates only completed leaf results.

`MIP_SOLVER_MAX_JOB_RETRIES` caps master-side re-delegation for errored subproblem attempts. Failed attempts are re-published as retry jobs while the solve still counts completion by original subproblem.

## Persistence model

Active solves are kept in memory on the master for low-latency frontier mutation, worker tracking, and aggregation. NATS JetStream carries persisted job/result messages between pods.

Postgres is the durable journal for solve starts, model revisions, subproblem submissions/completions/splits, and solve finishes. The Rust binary imports MIP-specific table names from `remote/libs/pg-defs/generated/rust` (`mip_solver_sessions`, `mip_solver_solves`, `mip_solver_jobs`, `mip_solver_events`) and writes best-effort upserts/events when `MIP_SOLVER_DATABASE_URL`, `AGENT_TASKS_RDS_DATABASE_URL`, `RDS_DATABASE_URL`, `DATABASE_URL`, or `PG_DATABASE_URL` is configured.

Redis is the hot cache and coordination layer for solve snapshots, frontier snapshots, live session model snapshots, and revision locks. Configure `MIP_SOLVER_REDIS_URL` or `REDIS_URL`; key names are under `MIP_SOLVER_REDIS_KEY_PREFIX` (default `dd:mip-solver`) and `/` reports the concrete key templates. Network-wide ownership locks can use Redis `SET NX PX` or the in-cluster live-mutex service if ownership migration is added later.

Cross-pod coordination is optional and controlled by `MIP_SOLVER_COORDINATION_BACKENDS` (`auto`, `redis`, `live-mutex`, `both`, or `none`). In `auto`, Redis locks are used when Redis is connected, and live-mutex locks are used when `MIP_SOLVER_LIVE_MUTEX_URL`, `LIVE_MUTEX_URL`, or `LMX_HTTP_URL` is configured. Lock TTL/wait defaults are `MIP_SOLVER_COORDINATION_LOCK_TTL_MS=30000` and `MIP_SOLVER_COORDINATION_WAIT_MS=5000`; solve request locks add `MIP_SOLVER_COORDINATION_SOLVE_LOCK_MARGIN_MS` beyond the solve timeout. Redis locks use `SET NX PX` with token-checked release; live-mutex uses `POST /v1/lock` and `POST /v1/unlock` over HTTP, with optional `MIP_SOLVER_LIVE_MUTEX_AUTH_TOKEN`, `MIP_SOLVER_LIVE_MUTEX_REQUEST_TIMEOUT_MS`, and `MIP_SOLVER_LIVE_MUTEX_MAX_RESPONSE_BYTES`.

Session edits and session solves acquire the session revision lock and load the latest Postgres snapshot before applying commands, falling back to the Redis session snapshot when Postgres is unavailable. Solve submissions acquire a request-level lock keyed by `problemId`, so duplicate requests across overlapping masters do not race the in-memory registry.

## Kubernetes

`k8s/` contains:

- master Deployment + Service in namespace `ai-ml`
- slave Deployment + metrics Service in namespace `ai-ml`
- KEDA `ScaledObject` watching JetStream stream `DD_REMOTE_MIP_SOLVER` and durable consumer `dd-in-house-mip-solver-node-workers`

KEDA scales slave pods from NATS JetStream consumer lag. New pods boot with `MIP_SOLVER_NODE_ROLE=slave`, attach to the same durable pull consumer, and start draining pending subproblems.

Argo CD should source this repository directly (`path: k8s`). The Kubernetes startup command still builds inside a full `k8s-cluster` source tree because the normal Cargo manifest depends on generated NATS/Postgres/Redis crates under `remote/libs/...` and the in-house DES solver under `remote/submodules/discrete-event-system.rs`. By default the pod clones `k8s-cluster` from `MIP_SOLVER_CLUSTER_GIT_URL`/`MIP_SOLVER_CLUSTER_GIT_REF`, initializes the DES submodule, then clones this solver repo from `MIP_SOLVER_NODE_GIT_URL`/`MIP_SOLVER_NODE_GIT_REF` into `remote/deployments/mip-solver-node.rs`. The pod does not mount the host checkout; source lives in an isolated `/tmp` checkout.

That split is intentional: C owns cluster/shared dependency layout, while A owns the service source and rendered Kubernetes bundle. It also prevents a stale C submodule pointer from making pods build older A code than Argo rendered.

`/home` serves the human-readable service home page. `/docs/api` and `/api/docs` serve the API docs page; `/api/docs.json` and `/api-docs.json` serve the machine-readable route inventory. `/version` and `/version.json` report the package version plus git commit/ref/build metadata captured by `build.rs`.

`/healthz` reports process liveness. `/readyz` requires a live NATS connection so Kubernetes does not route traffic to masters or count slaves as ready while they cannot publish or consume distributed work.

Masters subscribe to the generated MIP solver control subject and track live slave control frames (`worker-ready`, `request-work`, `worker-completed`) in memory. `GET /workers` reports the workers a master has observed.

`GET /mip-solver-cluster/nats` reports the active NATS connection flag, generated JetStream stream, jobs/results/control/events subjects, durable worker consumer, and master-observed slave frames. Masters also keep an in-memory solve registry. `POST /solve` and `POST /sessions/:session_id/solve` accept an optional `problemId` UUID; when omitted, the server attaches a UUID to the running problem and includes it in the solve response. Subproblem payloads carry `jobUuid` and `problemId`. `GET /solves` and `GET /mip-solver-cluster/solves` report tracked solves, expected/published/completed/re-delegated/split job counts, and per-attempt job status. Use `GET /solves?problem=<uuid>` to filter the registry to a single problem UUID. `GET /tasks` and `GET /tasks/:id` report this node's recent runtime task map and resolve by task id, problem UUID, solve id, request id, job id, or job UUID. `/metrics` exposes worker-control, solve-registry, active-solve, split, and re-delegation counters for operational debugging.

Cancel requests are pushed through the generated control subject and recorded in memory. Problem and worker tasks check that in-memory cancel map while waiting; `MIP_SOLVER_CANCEL_POLL_SECONDS` controls the periodic check interval and defaults to 10 seconds.

The server handles both Ctrl-C and Kubernetes SIGTERM for graceful HTTP shutdown during rolling updates and KEDA scale-down.

GPU resources are intentionally not requested in the base manifest. Add an overlay with `nvidia.com/gpu` limits for GPU-backed node pools; the server reports GPU availability when devices are present.
