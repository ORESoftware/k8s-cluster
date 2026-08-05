# Meta Agents bootstrap reconcile

This marker intentionally changes an existing `remote-k8s-maintenance` watched path.
When merged to `dev`, the trusted workflow runs its established `reconcile-runtime`
operation with `TARGET_REVISION=dev`, forcing Argo CD to reconcile the already-reviewed
`ai-agent-coordinator.rs@d25e04e50be4a9fad039cfcfa6c321e9c99a1e02` workload revision.

No workflow logic, Kubernetes manifest, credential source, target repository, or
publication payload is changed by this marker. Completion still requires a direct
GitHub read proving `meta-agents-demo/meta-agent-control-plane.rs` exists publicly.

Refs DEN-1057, DEN-1058, DEN-319.
