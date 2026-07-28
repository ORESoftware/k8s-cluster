# Method-aware OpenAPI regeneration

Status: failed
Source commit: 111461c803f1ab7f0431a289996e16d01bf30a4b
Workflow run: 30335889000

## Working tree
```text
 M remote/api-sdks/contracts/internal.json
 M remote/api-sdks/dart/internal/bin/smoke.dart
 M remote/api-sdks/dart/internal/lib/dd_api_sdk.dart
 M remote/api-sdks/dart/internal/sdk-manifest.json
 M remote/api-sdks/gleam/internal/sdk-manifest.json
 M remote/api-sdks/gleam/internal/src/dd_api_sdk.gleam
 M remote/api-sdks/gleam/internal/test/oresoftware_k8s_api_sdk_internal_test.gleam
 M remote/api-sdks/rust/internal/sdk-manifest.json
 M remote/api-sdks/rust/internal/src/lib.rs
 M remote/api-sdks/sdk-lock.json
 M remote/api-sdks/typescript/internal/sdk-manifest.json
 M remote/api-sdks/typescript/internal/src/index.ts
 M remote/api-sdks/typescript/internal/test/smoke.test.mjs
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
 M remote/deployments/generated-api-docs-index.html
 M remote/deployments/generated-api-docs-index.json
 M remote/deployments/gleam-mcp-server/generated/api-docs.html
 M remote/deployments/gleam-mcp-server/generated/api-docs.internal.json
 M remote/deployments/gleam-mcp-server/generated/api-docs.metadata.json
 M remote/deployments/gleam-mcp-server/src/gleam_mcp_server/api_docs.gleam
 M remote/deployments/gleamlang-presence-server/generated/api-docs.internal.json
 M remote/deployments/gleamlang-presence-server/generated/api-docs.metadata.json
 M remote/deployments/gleamlang-presence-server/scripts/check-openapi.sh
 M remote/deployments/gleamlang-ws-server/generated/api-docs.internal.json
 M remote/deployments/gleamlang-ws-server/generated/api-docs.metadata.json
 M remote/deployments/rest-api-rs/generated/api-docs.internal.json
 M remote/deployments/rest-api-rs/generated/api-docs.metadata.json
 M remote/deployments/runtime-config-rs/generated/api-docs.internal.json
 M remote/deployments/runtime-config-rs/generated/api-docs.metadata.json
 M remote/deployments/spark-pipeline-server/generated/api-docs.internal.json
 M remote/deployments/spark-pipeline-server/generated/api-docs.metadata.json
 M remote/deployments/web-scraper-service/generated/api-docs.internal.json
 M remote/deployments/web-scraper-service/generated/api-docs.metadata.json
 M remote/tools/generate-api-docs.mjs
 M scripts/api-contract/preserve-openapi-method-metadata-status.md
?? scripts/api-contract/__pycache__/
```

## Diff summary
```text
 remote/api-sdks/contracts/internal.json            |  54 ++---
 remote/api-sdks/dart/internal/bin/smoke.dart       |   2 +-
 remote/api-sdks/dart/internal/lib/dd_api_sdk.dart  |  14 +-
 remote/api-sdks/dart/internal/sdk-manifest.json    |   8 +-
 remote/api-sdks/gleam/internal/sdk-manifest.json   |   8 +-
 .../api-sdks/gleam/internal/src/dd_api_sdk.gleam   |  14 +-
 .../oresoftware_k8s_api_sdk_internal_test.gleam    |   2 +-
 remote/api-sdks/rust/internal/sdk-manifest.json    |   6 +-
 remote/api-sdks/rust/internal/src/lib.rs           |  16 +-
 remote/api-sdks/sdk-lock.json                      |  56 ++---
 .../api-sdks/typescript/internal/sdk-manifest.json |   8 +-
 remote/api-sdks/typescript/internal/src/index.ts   |  14 +-
 .../typescript/internal/test/smoke.test.mjs        |   2 +-
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
 remote/deployments/generated-api-docs-index.html   |  36 ++--
 remote/deployments/generated-api-docs-index.json   |  74 +++----
 .../gleam-mcp-server/generated/api-docs.html       |  14 +-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  38 +++-
 .../src/gleam_mcp_server/api_docs.gleam            |   2 +-
 .../generated/api-docs.internal.json               |  16 +-
 .../generated/api-docs.metadata.json               |  26 ++-
 .../scripts/check-openapi.sh                       |  17 +-
 .../generated/api-docs.internal.json               |  37 +++-
 .../generated/api-docs.metadata.json               |  20 +-
 .../rest-api-rs/generated/api-docs.internal.json   |  26 +--
 .../rest-api-rs/generated/api-docs.metadata.json   | 219 +++++++++++++++++--
 .../generated/api-docs.internal.json               |  10 +-
 .../generated/api-docs.metadata.json               |  42 +++-
 .../generated/api-docs.internal.json               |   4 +-
 .../generated/api-docs.metadata.json               |  24 ++-
 .../generated/api-docs.internal.json               |   2 +-
 .../generated/api-docs.metadata.json               |  20 +-
 remote/tools/generate-api-docs.mjs                 | 155 +++++++++++---
 .../preserve-openapi-method-metadata-status.md     |  65 +++++-
 55 files changed, 1245 insertions(+), 345 deletions(-)
```

## Last 240 log lines
```text
updated central API docs JSON index while preserving HTML route details for 5 uninitialized gitlink service(s): billing-server-rs, dart-server, fiducia-customer.rs, gleam-lambda-runner, usacc-rest-api-backend-rs
generated API docs for 44 service(s)
generated 38 SDK file(s)
Traceback (most recent call last):
  File "/home/runner/work/k8s-cluster/k8s-cluster/scripts/api-contract/assert-gleam-method-metadata.py", line 39, in <module>
    main()
  File "/home/runner/work/k8s-cluster/k8s-cluster/scripts/api-contract/assert-gleam-method-metadata.py", line 25, in main
    assert native_operation["operationId"] == operation_id
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError
```
