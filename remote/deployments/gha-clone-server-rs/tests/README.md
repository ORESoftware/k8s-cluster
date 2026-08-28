# GHA continuity integration tests

These tests exercise the independent Rust continuity lane as an untrusted-input and remote-control boundary.

- `planner_adversarial.rs` checks bounded YAML parsing, static DAG validation, immutable revisions, fixed-profile classification, ARC lane selection, and explicit rejection of unsupported GitHub Actions semantics.
- `http_api.rs` starts the compiled server on an ephemeral loopback port and checks authentication, allowlists, body limits, readiness, planning, direct execution, build-server request shape, polling, and failure persistence.
- `startup_config.rs` runs the binary as a child process and proves malformed booleans, limits, repository rules, retention bounds, and GitHub API origins fail before a listener is opened.
- `webhook_e2e.rs` uses loopback GitHub and build-server doubles to exercise the signed `workflow_run` failure path through exact-SHA fetch, planning, dispatch, retry, and duplicate suppression.
- `meta_self_test.rs` submits the bounded meta workflow through the real HTTP server and verifies the outgoing fixed-profile request.

The production deployment remains at zero replicas with API and webhook execution disabled. These suites do not require production credentials and never execute caller-selected commands.
