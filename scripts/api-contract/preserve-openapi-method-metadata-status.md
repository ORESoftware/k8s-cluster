# Method-aware OpenAPI regeneration

Status: failed
Source commit: 36cd24889569c1084ba3afe3daf0b3ee5cc0b502
Workflow run: 30335698766

## Working tree
```text
 M remote/deployments/apostille-services-server-rs/generated/api-docs.internal.json
 M remote/deployments/apostille-services-server-rs/generated/api-docs.metadata.json
 M remote/deployments/auth-server-rs/generated/api-docs.internal.json
 M remote/deployments/auth-server-rs/generated/api-docs.metadata.json
 M remote/deployments/build-server-rs/generated/api-docs.internal.json
 M remote/deployments/build-server-rs/generated/api-docs.metadata.json
 M remote/deployments/cluster-mcp-rs/generated/api-docs.html
 M remote/deployments/cluster-mcp-rs/generated/api-docs.internal.json
 M remote/deployments/cluster-mcp-rs/generated/api-docs.metadata.json
 M remote/deployments/dataset-labeling-rs/generated/api-docs.internal.json
 M remote/deployments/dataset-labeling-rs/generated/api-docs.metadata.json
 M remote/deployments/dd-benefactor-marketing-rs/generated/api-docs.internal.json
 M remote/deployments/dd-benefactor-marketing-rs/generated/api-docs.metadata.json
 M remote/deployments/dd-compliance-rs/generated/api-docs.internal.json
 M remote/deployments/dd-compliance-rs/generated/api-docs.metadata.json
 M remote/deployments/dd-git-rs/generated/api-docs.internal.json
 M remote/deployments/dd-git-rs/generated/api-docs.metadata.json
 M remote/deployments/dev-server/generated/api-docs.internal.json
 M remote/deployments/dev-server/generated/api-docs.metadata.json
 M remote/deployments/formal-methods-server-rs/generated/api-docs.internal.json
 M remote/deployments/formal-methods-server-rs/generated/api-docs.metadata.json
 M remote/deployments/gleam-mcp-server/generated/api-docs.html
 M remote/deployments/gleam-mcp-server/generated/api-docs.internal.json
 M remote/deployments/gleam-mcp-server/generated/api-docs.metadata.json
 M remote/deployments/gleam-mcp-server/src/gleam_mcp_server/api_docs.gleam
 M remote/deployments/gleamlang-presence-server/generated/api-docs.internal.json
 M remote/deployments/gleamlang-presence-server/generated/api-docs.metadata.json
 M remote/deployments/gleamlang-presence-server/scripts/check-openapi.sh
 M remote/tools/generate-api-docs.mjs
 M scripts/api-contract/preserve-openapi-method-metadata-status.md
?? scripts/api-contract/__pycache__/
```

## Diff summary
```text
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  23 +-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  42 +++-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  23 +-
 .../cluster-mcp-rs/generated/api-docs.html         |  14 +-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  44 +++-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  23 +-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               | 232 +++++++++++++++++++--
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  23 +-
 .../dd-git-rs/generated/api-docs.internal.json     |  10 +-
 .../dd-git-rs/generated/api-docs.metadata.json     |  42 +++-
 .../dev-server/generated/api-docs.internal.json    |   2 +-
 .../dev-server/generated/api-docs.metadata.json    |  20 +-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  23 +-
 .../gleam-mcp-server/generated/api-docs.html       |  14 +-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  38 +++-
 .../src/gleam_mcp_server/api_docs.gleam            |   2 +-
 .../generated/api-docs.internal.json               |  16 +-
 .../generated/api-docs.metadata.json               |  26 ++-
 .../scripts/check-openapi.sh                       |  17 +-
 remote/tools/generate-api-docs.mjs                 | 109 +++++++---
 .../preserve-openapi-method-metadata-status.md     |  40 +++-
 30 files changed, 673 insertions(+), 128 deletions(-)
```

## Last 240 log lines
```text
Error: ambiguous OpenAPI summary for gleamlang-ws-server GET /ws: "Custom code-first route derived from the service router." versus "Upgrade to a user-scoped ws."
    at buildOpenApi (file:///home/runner/work/k8s-cluster/k8s-cluster/remote/tools/generate-api-docs.mjs:1257:19)
    at main (file:///home/runner/work/k8s-cluster/k8s-cluster/remote/tools/generate-api-docs.mjs:1719:29)
```
