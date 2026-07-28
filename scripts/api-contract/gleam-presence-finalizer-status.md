# Gleam presence OpenAPI finalizer

Status: failed
Source commit: b3f492f7e2bc0fc22d7648b8488745e5f9660e6a
Workflow run: 30334342591

## Working tree
```text
 M remote/api-contracts/manifest.json
 M remote/api-sdks/README.md
 M remote/api-sdks/contracts/internal.json
 M remote/api-sdks/contracts/public.json
 M remote/api-sdks/dart/internal/bin/smoke.dart
 M remote/api-sdks/dart/internal/lib/dd_api_sdk.dart
 M remote/api-sdks/dart/internal/sdk-manifest.json
 M remote/api-sdks/dart/public/bin/smoke.dart
 M remote/api-sdks/dart/public/lib/dd_api_sdk.dart
 M remote/api-sdks/dart/public/sdk-manifest.json
 M remote/api-sdks/gleam/internal/sdk-manifest.json
 M remote/api-sdks/gleam/internal/src/dd_api_sdk.gleam
 M remote/api-sdks/gleam/internal/test/oresoftware_k8s_api_sdk_internal_test.gleam
 M remote/api-sdks/gleam/public/sdk-manifest.json
 M remote/api-sdks/gleam/public/src/dd_api_sdk.gleam
 M remote/api-sdks/gleam/public/test/oresoftware_k8s_api_sdk_public_test.gleam
 M remote/api-sdks/rust/internal/sdk-manifest.json
 M remote/api-sdks/rust/internal/src/lib.rs
 M remote/api-sdks/rust/public/sdk-manifest.json
 M remote/api-sdks/rust/public/src/lib.rs
 M remote/api-sdks/sdk-lock.json
 M remote/api-sdks/typescript/internal/sdk-manifest.json
 M remote/api-sdks/typescript/internal/src/index.ts
 M remote/api-sdks/typescript/internal/test/smoke.test.mjs
 M remote/api-sdks/typescript/public/sdk-manifest.json
 M remote/api-sdks/typescript/public/src/index.ts
 M remote/api-sdks/typescript/public/test/smoke.test.mjs
 M remote/config/api-contracts.json
 M remote/deployments/generated-api-docs-index.html
 M remote/deployments/generated-api-docs-index.json
 M remote/deployments/gleamlang-presence-server/generated/api-docs.html
 M remote/deployments/gleamlang-presence-server/generated/api-docs.internal.json
 M remote/deployments/gleamlang-presence-server/generated/api-docs.json
 M remote/deployments/gleamlang-presence-server/generated/api-docs.metadata.json
 M remote/deployments/gleamlang-presence-server/src/gleamlang_presence_server/api_docs.gleam
 M remote/deployments/gleamlang-presence-server/src/gleamlang_presence_server/route_contract.gleam
 M scripts/api-contract/gleam-presence-finalizer-status.md
?? .tmp/
?? remote/deployments/gleamlang-presence-server/generated/openapi.json
?? remote/deployments/gleamlang-presence-server/scripts/export-openapi.sh
```

## Diff summary
```text
 remote/api-contracts/manifest.json                 |  34 +++
 remote/api-sdks/README.md                          |   2 +-
 remote/api-sdks/contracts/internal.json            | 119 ++++-----
 remote/api-sdks/contracts/public.json              |  28 +--
 remote/api-sdks/dart/internal/bin/smoke.dart       |   4 +-
 remote/api-sdks/dart/internal/lib/dd_api_sdk.dart  |  43 ++--
 remote/api-sdks/dart/internal/sdk-manifest.json    |  10 +-
 remote/api-sdks/dart/public/bin/smoke.dart         |   2 +-
 remote/api-sdks/dart/public/lib/dd_api_sdk.dart    |  16 +-
 remote/api-sdks/dart/public/sdk-manifest.json      |   8 +-
 remote/api-sdks/gleam/internal/sdk-manifest.json   |  10 +-
 .../api-sdks/gleam/internal/src/dd_api_sdk.gleam   |  43 ++--
 .../oresoftware_k8s_api_sdk_internal_test.gleam    |   4 +-
 remote/api-sdks/gleam/public/sdk-manifest.json     |   8 +-
 remote/api-sdks/gleam/public/src/dd_api_sdk.gleam  |  16 +-
 .../test/oresoftware_k8s_api_sdk_public_test.gleam |   2 +-
 remote/api-sdks/rust/internal/sdk-manifest.json    |   8 +-
 remote/api-sdks/rust/internal/src/lib.rs           |  47 ++--
 remote/api-sdks/rust/public/sdk-manifest.json      |   6 +-
 remote/api-sdks/rust/public/src/lib.rs             |  18 +-
 remote/api-sdks/sdk-lock.json                      |  58 ++---
 .../api-sdks/typescript/internal/sdk-manifest.json |  10 +-
 remote/api-sdks/typescript/internal/src/index.ts   |  46 ++--
 .../typescript/internal/test/smoke.test.mjs        |   4 +-
 .../api-sdks/typescript/public/sdk-manifest.json   |   8 +-
 remote/api-sdks/typescript/public/src/index.ts     |  16 +-
 .../api-sdks/typescript/public/test/smoke.test.mjs |   2 +-
 remote/config/api-contracts.json                   |   1 -
 remote/deployments/generated-api-docs-index.html   |   4 +-
 remote/deployments/generated-api-docs-index.json   |   6 +-
 .../generated/api-docs.html                        |  12 +-
 .../generated/api-docs.internal.json               | 265 ++++++++++++---------
 .../generated/api-docs.json                        |  24 +-
 .../generated/api-docs.metadata.json               | 229 ++++++++++--------
 .../src/gleamlang_presence_server/api_docs.gleam   |  24 +-
 .../gleamlang_presence_server/route_contract.gleam |  71 +++---
 .../gleam-presence-finalizer-status.md             |  49 +++-
 37 files changed, 718 insertions(+), 539 deletions(-)
```

## Last 220 log lines
```text
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/apis/DocumentationApi.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/docs/DocumentationApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/apis/OperationsApi.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/docs/OperationsApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/apis/PresenceApi.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/docs/PresenceApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/apis/RuntimeConfigApi.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/docs/RuntimeConfigApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/apis/ServiceApi.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/docs/ServiceApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/index.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/runtime.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/README.md
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/package.json
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/tsconfig.json
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/tsconfig.esm.json
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/.npmignore
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/.gitignore
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/models/index.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/src/apis/index.ts
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/.openapi-generator-ignore
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/.openapi-generator/VERSION
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/typescript/.openapi-generator/FILES
############################################################################################
# Thanks for using OpenAPI Generator.                                                      #
# We appreciate your support! Please consider donation to help us maintain this project.   #
# https://opencollective.com/openapi_generator/donate                                      #
############################################################################################
[main] WARN  o.o.codegen.DefaultCodegen - OpenAPI 3.1 support is still in beta. To report an issue related to 3.1 spec, please kindly open an issue in the Github repo: https://github.com/openAPITools/openapi-generator.
[main] INFO  o.o.codegen.DefaultGenerator - Generating with dryRun=false
[main] INFO  o.o.c.ignore.CodegenIgnoreProcessor - Output directory (/local/.tmp/gleam-presence-sdk/dart) does not exist, or is inaccessible. No file (.openapi-generator-ignore) will be evaluated.
[main] INFO  o.o.codegen.DefaultGenerator - OpenAPI Generator: dart (client)
[main] INFO  o.o.codegen.DefaultGenerator - Generator 'dart' is considered stable.
[main] INFO  o.o.c.languages.AbstractDartCodegen - Environment variable DART_POST_PROCESS_FILE not defined so the Dart code may not be properly formatted. To define it, try `export DART_POST_PROCESS_FILE="/usr/local/bin/dartfmt -w"` (Linux/Mac)
[main] INFO  o.o.c.languages.AbstractDartCodegen - NOTE: To enable file post-processing, 'enablePostProcessFile' must be set to `true` (--enable-post-process-file for CLI).
[main] INFO  o.o.c.languages.DartClientCodegen - Using serialization library native_serialization
[main] INFO  o.o.codegen.InlineModelResolver - Inline schema created as getPresenceHealth_200_response. To have complete control of the model name, set the `title` field or use the modelNameMapping option (e.g. --model-name-mappings getPresenceHealth_200_response=NewModel,ModelA=NewModelA in CLI) or inlineSchemaNameMapping option (--inline-schema-name-mappings getPresenceHealth_200_response=NewModel,ModelA=NewModelA in CLI).
[main] WARN  o.o.codegen.DefaultCodegen - OpenAPI 3.1 support is still in beta. To report an issue related to 3.1 spec, please kindly open an issue in the Github repo: https://github.com/openAPITools/openapi-generator.
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/model/get_presence_health200_response.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/doc/GetPresenceHealth200Response.md
[main] WARN  o.o.codegen.utils.ExamplesUtils - No application/json content media type found in response. Response examples can currently only be generated for application/json media type.
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api/documentation_api.dart
[main] INFO  o.o.codegen.TemplateManager - Skipped /local/.tmp/gleam-presence-sdk/dart/test/documentation_api_test.dart (Skipped by apiTests options supplied by user.)
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/doc/DocumentationApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api/operations_api.dart
[main] INFO  o.o.codegen.TemplateManager - Skipped /local/.tmp/gleam-presence-sdk/dart/test/operations_api_test.dart (Skipped by apiTests options supplied by user.)
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/doc/OperationsApi.md
[main] WARN  o.o.c.languages.AbstractDartCodegen - The media-type 'text/plain' for operation '/conv/{conv_id}/broadcast' is not support in the Dart generators by default.
[main] WARN  o.o.c.languages.AbstractDartCodegen - The media-type 'text/plain' for operation '/user/{user_id}/broadcast' is not support in the Dart generators by default.
[main] WARN  o.o.c.languages.AbstractDartCodegen - The media-type 'text/plain' for operation '/user/{user_id}/devices/{device_id}/logout' is not support in the Dart generators by default.
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api/presence_api.dart
[main] INFO  o.o.codegen.TemplateManager - Skipped /local/.tmp/gleam-presence-sdk/dart/test/presence_api_test.dart (Skipped by apiTests options supplied by user.)
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/doc/PresenceApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api/runtime_config_api.dart
[main] INFO  o.o.codegen.TemplateManager - Skipped /local/.tmp/gleam-presence-sdk/dart/test/runtime_config_api_test.dart (Skipped by apiTests options supplied by user.)
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/doc/RuntimeConfigApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api/service_api.dart
[main] INFO  o.o.codegen.TemplateManager - Skipped /local/.tmp/gleam-presence-sdk/dart/test/service_api_test.dart (Skipped by apiTests options supplied by user.)
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/doc/ServiceApi.md
[main] INFO  o.o.codegen.utils.URLPathUtils - 'host' (OAS 2.0) or 'servers' (OAS 3.0) not defined in the spec. Default to [http://localhost] for server URL [http://localhost/]
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/pubspec.yaml
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/analysis_options.yaml
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api_client.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api_exception.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api_helper.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/api.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/auth/authentication.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/auth/http_basic_auth.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/auth/http_bearer_auth.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/auth/api_key_auth.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/lib/auth/oauth.dart
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/git_push.sh
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/.gitignore
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/README.md
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/.travis.yml
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/.openapi-generator-ignore
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/.openapi-generator/VERSION
[main] INFO  o.o.codegen.TemplateManager - writing file /local/.tmp/gleam-presence-sdk/dart/.openapi-generator/FILES
############################################################################################
# Thanks for using OpenAPI Generator.                                                      #
# We appreciate your support! Please consider donation to help us maintain this project.   #
# https://opencollective.com/openapi_generator/donate                                      #
############################################################################################
generated gleamlang-presence-server SDK smoke trees at .tmp/gleam-presence-sdk
[1m[32m    Updating[0m crates.io index
[1m[32m     Locking[0m 176 packages to latest compatible versions
[1m[36m      Adding[0m icu_collections v2.2.0 [1m[31m(requires Rust 1.86)[0m
[1m[36m      Adding[0m icu_locale_core v2.2.0 [1m[31m(requires Rust 1.86)[0m
[1m[36m      Adding[0m icu_normalizer v2.2.0 [1m[31m(requires Rust 1.86)[0m
[1m[36m      Adding[0m icu_normalizer_data v2.2.0 [1m[31m(requires Rust 1.86)[0m
[1m[36m      Adding[0m icu_properties v2.2.0 [1m[31m(requires Rust 1.86)[0m
[1m[36m      Adding[0m icu_properties_data v2.2.0 [1m[31m(requires Rust 1.86)[0m
[1m[36m      Adding[0m icu_provider v2.2.0 [1m[31m(requires Rust 1.86)[0m
[1m[36m      Adding[0m idna_adapter v1.2.2 [1m[31m(requires Rust 1.86)[0m
[1m[32m Downloading[0m crates ...
[1m[32m  Downloaded[0m cfg-if v1.0.4
[1m[32m  Downloaded[0m atomic-waker v1.1.2
[1m[32m  Downloaded[0m displaydoc v0.2.6
[1m[32m  Downloaded[0m serde_urlencoded v0.7.1
[1m[32m  Downloaded[0m try-lock v0.2.5
[1m[32m  Downloaded[0m sync_wrapper v1.0.2
[1m[32m  Downloaded[0m stable_deref_trait v1.2.1
[1m[32m  Downloaded[0m find-msvc-tools v0.1.9
[1m[32m  Downloaded[0m native-tls v0.2.18
[1m[32m  Downloaded[0m bytes v1.12.1
[1m[32m  Downloaded[0m yoke v0.8.3
[1m[32m  Downloaded[0m icu_provider v2.2.0
[1m[32m  Downloaded[0m mime v0.3.17
[1m[32m  Downloaded[0m utf8_iter v1.0.4
[1m[32m  Downloaded[0m hyper-tls v0.6.0
[1m[32m  Downloaded[0m openssl-probe v0.2.1
[1m[32m  Downloaded[0m pin-project-lite v0.2.17
[1m[32m  Downloaded[0m want v0.3.1
[1m[32m  Downloaded[0m icu_collections v2.2.0
[1m[32m  Downloaded[0m zerofrom-derive v0.1.7
[1m[32m  Downloaded[0m yoke-derive v0.8.2
[1m[32m  Downloaded[0m futures-task v0.3.33
[1m[32m  Downloaded[0m http-body v1.1.0
[1m[32m  Downloaded[0m zerofrom v0.1.8
[1m[32m  Downloaded[0m serde_derive v1.0.229
[1m[32m  Downloaded[0m zmij v1.0.23
[1m[32m  Downloaded[0m zerovec-derive v0.11.3
[1m[32m  Downloaded[0m slab v0.4.12
[1m[32m  Downloaded[0m icu_properties v2.2.0
[1m[32m  Downloaded[0m unicode-ident v1.0.24
[1m[32m  Downloaded[0m zerovec v0.11.6
[1m[32m  Downloaded[0m zerotrie v0.2.4
[1m[32m  Downloaded[0m tower-http v0.6.11
[1m[32m  Downloaded[0m reqwest v0.13.4
[1m[32m  Downloaded[0m idna v1.1.0
[1m[32m  Downloaded[0m serde_json v1.0.151
[1m[32m  Downloaded[0m hyper v1.11.0
[1m[32m  Downloaded[0m icu_properties_data v2.2.0
[1m[32m  Downloaded[0m vcpkg v0.2.15
[1m[32m  Downloaded[0m hyper-util v0.1.20
[1m[32m  Downloaded[0m tower v0.5.3
[1m[32m  Downloaded[0m mio v1.2.2
[1m[32m  Downloaded[0m syn v3.0.3
[1m[32m  Downloaded[0m syn v2.0.119
[1m[32m  Downloaded[0m openssl v0.10.81
[1m[32m  Downloaded[0m memchr v2.8.3
[1m[32m  Downloaded[0m icu_normalizer_data v2.2.0
[1m[32m  Downloaded[0m futures-util v0.3.33
[1m[32m  Downloaded[0m url v2.5.8
[1m[32m  Downloaded[0m serde v1.0.229
[1m[32m  Downloaded[0m http v1.4.2
[1m[32m  Downloaded[0m tracing v0.1.44
[1m[32m  Downloaded[0m tracing-core v0.1.36
[1m[32m  Downloaded[0m smallvec v1.15.2
[1m[32m  Downloaded[0m icu_locale_core v2.2.0
[1m[32m  Downloaded[0m icu_normalizer v2.2.0
[1m[32m  Downloaded[0m rustls-pki-types v1.15.1
[1m[32m  Downloaded[0m log v0.4.33
[1m[32m  Downloaded[0m futures-channel v0.3.33
[1m[32m  Downloaded[0m zeroize v1.9.0
[1m[32m  Downloaded[0m unicase v2.9.0
[1m[32m  Downloaded[0m tokio-native-tls v0.3.1
[1m[32m  Downloaded[0m tinystr v0.8.3
[1m[32m  Downloaded[0m synstructure v0.13.2
[1m[32m  Downloaded[0m socket2 v0.6.5
[1m[32m  Downloaded[0m serde_core v1.0.229
[1m[32m  Downloaded[0m httparse v1.10.1
[1m[32m  Downloaded[0m http-body-util v0.1.4
[1m[32m  Downloaded[0m form_urlencoded v1.2.2
[1m[32m  Downloaded[0m libc v0.2.189
[1m[32m  Downloaded[0m writeable v0.6.3
[1m[32m  Downloaded[0m shlex v2.0.1
[1m[32m  Downloaded[0m quote v1.0.47
[1m[32m  Downloaded[0m proc-macro2 v1.0.107
[1m[32m  Downloaded[0m tokio v1.53.1
[1m[32m  Downloaded[0m once_cell v1.21.4
[1m[32m  Downloaded[0m pkg-config v0.3.33
[1m[32m  Downloaded[0m mime_guess v2.0.5
[1m[32m  Downloaded[0m futures-core v0.3.33
[1m[32m  Downloaded[0m foreign-types-shared v0.1.1
[1m[32m  Downloaded[0m tower-service v0.3.3
[1m[32m  Downloaded[0m serde_repr v0.1.21
[1m[32m  Downloaded[0m openssl-macros v0.1.1
[1m[32m  Downloaded[0m itoa v1.0.18
[1m[32m  Downloaded[0m cc v1.4.0
[1m[32m  Downloaded[0m percent-encoding v2.3.2
[1m[32m  Downloaded[0m openssl-sys v0.9.117
[1m[32m  Downloaded[0m foreign-types v0.3.2
[1m[32m  Downloaded[0m tower-layer v0.3.3
[1m[32m  Downloaded[0m ryu v1.0.23
[1m[32m  Downloaded[0m bitflags v2.13.1
[1m[32m  Downloaded[0m potential_utf v0.1.5
[1m[32m  Downloaded[0m litemap v0.8.2
[1m[32m  Downloaded[0m ipnet v2.12.0
[1m[32m  Downloaded[0m idna_adapter v1.2.2
[1m[32m  Downloaded[0m base64 v0.22.1
[1m[31merror[0m[1m:[0m rustc 1.85.1 is not supported by the following packages:
  icu_collections@2.2.0 requires rustc 1.86
  icu_locale_core@2.2.0 requires rustc 1.86
  icu_normalizer@2.2.0 requires rustc 1.86
  icu_normalizer_data@2.2.0 requires rustc 1.86
  icu_normalizer_data@2.2.0 requires rustc 1.86
  icu_normalizer_data@2.2.0 requires rustc 1.86
  icu_properties@2.2.0 requires rustc 1.86
  icu_properties_data@2.2.0 requires rustc 1.86
  icu_properties_data@2.2.0 requires rustc 1.86
  icu_properties_data@2.2.0 requires rustc 1.86
  icu_provider@2.2.0 requires rustc 1.86
  idna_adapter@1.2.2 requires rustc 1.86
Either upgrade rustc or select compatible dependency versions with
`cargo update <name>@<current-ver> --precise <compatible-ver>`
where `<compatible-ver>` is the latest version supporting rustc 1.85.1

```
