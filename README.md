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

## Kubernetes

`k8s/` contains:

- master Deployment + Service in namespace `ai-ml`
- slave Deployment in namespace `ai-ml`
- KEDA `ScaledObject` watching JetStream stream `DD_REMOTE_MIP_SOLVER` and durable consumer `dd-in-house-mip-solver-node-workers`

KEDA scales slave pods from NATS JetStream consumer lag. New pods boot with `MIP_SOLVER_NODE_ROLE=slave`, attach to the same durable pull consumer, and start draining pending subproblems.

GPU resources are intentionally not requested in the base manifest. Add an overlay with `nvidia.com/gpu` limits for GPU-backed node pools; the server reports GPU availability when devices are present.
