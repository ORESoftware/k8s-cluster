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

Postgres is the intended durable journal for solve starts, model revisions, subproblem submissions/completions/splits, and solve finishes. The Rust binary imports table names from `remote/libs/pg-defs/generated/rust` and advertises the current contract from `/` under `persistence.postgres`; when `MIP_SOLVER_DATABASE_URL`, `AGENT_TASKS_RDS_DATABASE_URL`, `RDS_DATABASE_URL`, `DATABASE_URL`, or `PG_DATABASE_URL` is configured, the node reports the Postgres journal path as enabled.

Redis is the hot cache and coordination layer for solve snapshots, frontier snapshots, live session model snapshots, and revision locks. Configure `MIP_SOLVER_REDIS_URL` or `REDIS_URL`; key names are under `MIP_SOLVER_REDIS_KEY_PREFIX` (default `dd:mip-solver`) and `/` reports the concrete key templates. Network-wide ownership locks can use Redis `SET NX PX` or the in-cluster live-mutex service if ownership migration is added later.

## Kubernetes

`k8s/` contains:

- master Deployment + Service in namespace `ai-ml`
- slave Deployment + metrics Service in namespace `ai-ml`
- KEDA `ScaledObject` watching JetStream stream `DD_REMOTE_MIP_SOLVER` and durable consumer `dd-in-house-mip-solver-node-workers`

KEDA scales slave pods from NATS JetStream consumer lag. New pods boot with `MIP_SOLVER_NODE_ROLE=slave`, attach to the same durable pull consumer, and start draining pending subproblems.

The Kubernetes startup command runs Cargo from `remote/deployments/mip-solver-node.rs` inside a full `k8s-cluster` source tree, with `CARGO_TARGET_DIR` pointed at `/tmp`. Keep that relative layout intact: the normal manifest depends on generated NATS definitions under `remote/libs/nats/...` and the in-house DES solver under `remote/submodules/discrete-event-system.rs`.

`/healthz` reports process liveness. `/readyz` requires a live NATS connection so Kubernetes does not route traffic to masters or count slaves as ready while they cannot publish or consume distributed work.

Masters subscribe to the generated MIP solver control subject and track live slave control frames (`worker-ready`, `request-work`, `worker-completed`) in memory. `GET /mip-solver-cluster/workers` reports the workers a master has observed (`/workers` is kept as a short compatibility alias).

Masters also keep an in-memory solve registry. `GET /mip-solver-cluster/solves` reports tracked solves, expected/published/completed/re-delegated/split job counts, and per-attempt job status. `/metrics` exposes worker-control, solve-registry, active-solve, split, and re-delegation counters for operational debugging.

The server handles both Ctrl-C and Kubernetes SIGTERM for graceful HTTP shutdown during rolling updates and KEDA scale-down.

GPU resources are intentionally not requested in the base manifest. Add an overlay with `nvidia.com/gpu` limits for GPU-backed node pools; the server reports GPU availability when devices are present.
