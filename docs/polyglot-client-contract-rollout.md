# Polyglot client contract policy and ten-organization evidence ledger

Status: proposed canonical format; rollout audit in progress

Decision date: 2026-08-23

Tracking: [ORESoftware/k8s-cluster#1403](https://github.com/ORESoftware/k8s-cluster/issues/1403), [DEN-3958](https://linear.app/denman/issue/DEN-3958/enforce-json-schema-interface-contracts-across-clients-sdk-languages), [DEN-3959](https://linear.app/denman/issue/DEN-3959/evaluate-alternatives-to-json-schema-for-cross-language-client)

## Outcome

Keep JSON Schema Draft 2020-12 as the canonical description and validator for
JSON-shaped data. Pair it with a small, separately versioned, language-neutral API
manifest for exported symbols and behavior. Do not claim that JSON Schema alone
describes a TypeScript module, Rust crate, Dart package, Java library, streaming
lifecycle, cancellation rule, or error model.

OpenAPI remains authoritative when a client is generated from an HTTP service
contract. AsyncAPI or a product-owned event schema remains authoritative for an
asynchronous wire contract. Protobuf or Smithy may be used where the product already
chooses that IDL and wire model, but neither is imposed on existing JSON/HTTP,
WebSocket, local, or mixed-transport client fleets solely to make this audit uniform.

The portfolio contract therefore has four independently versioned layers:

1. **Data schemas** describe serialized request, response, event, configuration, and
   error payloads with immutable schema IDs and digests.
2. **API manifest** describes public symbols, methods/functions, parameter and return
   type references, error vocabulary, sync/async/streaming behavior, cancellation and
   backpressure semantics, visibility, stability, and required capabilities.
3. **Runtime bindings** map stable manifest IDs to the actual public name and native
   type/execution convention in each language package.
4. **Evidence manifest** binds the three declarations to generated source/package
   digests, the generator/checker version, source contract commit, target matrix, and
   test receipts.

Changing one layer does not silently change another layer's version. A compatible
data-schema addition, for example, does not justify changing an API method from a
future/promise to a stream without an API-manifest version and compatibility review.

## Alternatives evaluated

| Technology | Strength | Why it is not the universal portfolio contract |
| --- | --- | --- |
| JSON Schema Draft 2020-12 | Language-neutral validation for JSON values, reusable schemas, references, composition, and explicit vocabularies | It validates instance data, not native package exports, overloads, async behavior, cancellation, stream ownership, or language-specific error idioms |
| OpenAPI 3.1 | Standard language-agnostic HTTP interface description; drives HTTP docs, validation, and client generation while using the JSON Schema vocabulary | It is transport-specific and cannot by itself describe offline helpers, local storage APIs, WebSocket ownership, package-only symbols, or non-HTTP capabilities |
| Protocol Buffers | Strong schema evolution, compact wire format, generated bindings, and broad cross-language message support | Adoption changes the wire and generator model; the official compiler does not directly cover the entire portfolio matrix, and message/service parity is not the same as preserving existing public package APIs |
| Smithy 2.0 | Rich service, operation, input/output, error, behavior, auth, streaming, and protocol traits with code-generation support | It is a strong option for a newly Smithy-owned service, but migrating established hand-written and mixed-transport SDKs creates a second service model unless Smithy becomes that product's source of truth |
| Product-native OpenAPI/AsyncAPI/Protobuf/Smithy plus the portfolio manifest | Preserves the strongest existing wire authority while normalizing package-level evidence | Chosen: the manifest references the authoritative wire/data artifacts rather than duplicating them |

No evaluated alternative removes the need for a package-level binding and capability
record across C, C++, Dart, Elixir, Erlang, Gleam, Go, Java, Kotlin, PHP, Python, Ruby,
Rust, Swift, several TypeScript runtimes, WebAssembly, Zig, and product-specific
extensions. The small manifest is deliberately not a new serialization protocol.

Primary specification evidence, retrieved 2026-08-23:

- [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12)
- [OpenAPI Specification 3.1.1](https://spec.openapis.org/oas/v3.1.1.html)
- [Protocol Buffers overview](https://protobuf.dev/overview/)
- [Smithy 2.0 IDL](https://smithy.io/2.0/spec/idl.html)

## Required contract shape

The exact JSON property names may evolve through a versioned schema, but every
repository must represent these concepts:

```json
{
  "apiManifestVersion": 1,
  "package": "example/example-clients",
  "dataSchemas": {
    "version": "2026-08-23",
    "artifacts": [
      {
        "id": "urn:example:Widget:v1",
        "uri": "../example-interfaces/schemas/widget.v1.schema.json",
        "sha256": "synthetic-review-placeholder"
      }
    ]
  },
  "symbols": [
    {
      "id": "client.widgets.list",
      "kind": "method",
      "parameters": [
        {"name": "cursor", "schema": "urn:example:Cursor:v1", "optional": true}
      ],
      "returns": "urn:example:WidgetPage:v1",
      "errors": ["unauthenticated", "forbidden", "invalid_input", "unavailable"],
      "execution": "async",
      "cancellation": "caller_scoped",
      "streaming": "none",
      "capabilities": ["widgets.read"]
    }
  ],
  "runtimes": [
    {
      "id": "dart",
      "status": "supported",
      "bindings": [
        {"symbol": "client.widgets.list", "publicName": "WidgetClient.listWidgets"}
      ]
    },
    {
      "id": "objective-c",
      "status": "unsupported",
      "reason": "No product consumer or maintained generator; Swift is the Apple target.",
      "owner": "example-sdk-owners",
      "reviewAfter": "2027-02-23"
    }
  ]
}
```

The digest above is illustrative and must never be copied as real provenance.
Production manifests use a real SHA-256 digest calculated from canonical bytes.

### Data-schema rules

- Use Draft 2020-12 with stable `$id` values and explicit schema versions.
- The sibling `*-interfaces` repository owns wire/data schemas when it exists. The
  `*-clients` API manifest pins the exact interface artifact version and digest.
- OpenAPI/AsyncAPI/Protobuf/Smithy-derived schemas are generated from their product
  source of truth; they are not hand-maintained copies.
- Validators exercise positive examples and one negative fixture for every meaningful
  constraint, including bounds, enums, required fields, and additional properties.
- Breaking and compatible changes follow the product's explicit schema evolution
  rules. A digest change without a version/evolution decision fails closed.

### API-manifest rules

Every public callable records a stable ID, kind, visibility, parameters in order,
optional/default semantics, return type, declared errors, sync/async/streaming mode,
cancellation behavior, stream cardinality/backpressure/lifecycle when applicable,
authorization/capability requirements, stability, and deprecation replacement.

Types refer to the data-schema layer where they cross a serialization boundary. Purely
native helpers may use a portable type algebra, but the manifest must say when a type is
language-local. Unknown, `any`, untyped map, and raw JSON escape hatches require an
explicit reason and review date.

The normalized contract describes semantic parity, not spelling identity. For example,
a TypeScript `Promise<Result>`, Dart `Future<Result>`, Rust future returning
`Result<T, E>`, and Java `CompletionStage<Result>` may all implement one `async`
operation when their runtime bindings preserve the same completion, cancellation, and
error contract. The checker must not demand that their source signatures are textually
identical.

### Runtime support rules

Every immediate language/runtime directory below `clients/` has exactly one matrix
entry. Multiple runtime targets may share one implementation directory only when the
matrix records the alias and the pre-publish job tests each target environment.

A runtime entry is one of:

- `supported`: generated/implemented, compiled, package-consumer tested, and mapped to
  every required public symbol;
- `partial`: temporarily missing named optional capabilities with an owner and expiry;
  it does not satisfy full fleet parity; or
- `unsupported`: no package is published, with a concrete product/toolchain reason,
  owner, and review date.

A missing directory is not automatically “unsupported,” and an empty scaffold is not
“supported.” The final fleet matrix records both cases explicitly.

## Checker and generator contract

Repository CI and pre-publish use the same deterministic checker version. The checker:

1. validates every data schema, API manifest, runtime binding, and evidence manifest
   against its declared schema/dialect;
2. discovers every client language/runtime directory and fails on an uncovered,
   duplicated, escaped, empty, or stale target;
3. obtains the public package surface through a package/compiler-native exporter or a
   deterministic generated binding receipt;
4. normalizes that observed surface through the runtime binding and compares symbol
   IDs, public names, ordered parameters, types, optional/default semantics, returns,
   errors, async/streaming/cancellation semantics, and capabilities;
5. regenerates artifacts in a clean directory and fails if committed generated files,
   package metadata, or provenance digests differ;
6. compiles and runs at least one consumer contract plus shared success/error/edge
   fixtures for every supported target; and
7. emits a bounded JSON receipt containing commit, tool version, contract digests,
   target, result, and artifact digest without source payloads, tokens, or user data.

A source-tree digest and per-runtime marker are useful stale-artifact controls, but
they are not semantic API validation by themselves. A repository that only proves
“these files have not changed since the marker was generated” remains in the
`landed-evidence-gap` state until it also proves the declared exports and behavior.

Both ordinary test CI and every publish/release job run the checker before compilation
or upload. Publish fails when a supported target is skipped, a generated diff exists,
a contract/schema digest is unknown, a required consumer test is absent, or a package
would be built from a different commit than the receipt.

## Ten-organization rollout ledger

Evidence was queried from GitHub on 2026-08-23. For every row, the recorded merge
commit is an ancestor of the repository's current `main` branch: GitHub comparison
reported `behind_by: 0` and `main` ahead of the merge. “PR rollup” is the current
GitHub check rollup on that PR head, not a reconstructed claim about historical runner
conditions.

| Organization / repository | Contract evidence PR | Merge commit on current `main` | PR rollup | Certification state |
| --- | --- | --- | --- | --- |
| `3FA-app/3fa-clients` | [#53](https://github.com/3FA-app/3fa-clients/pull/53) | `a10234630a9e` | 0 green, 20 failed | `landed-evidence-gap`: API schema/manifest and target markers landed, but the linked rollup is red |
| `canonical-cloud/canonical-clients` | [#26](https://github.com/canonical-cloud/canonical-clients/pull/26) | `792e5b297005` | 0 green, 22 failed | `landed-evidence-gap`: contract landed; green default-branch and semantic-export receipts still required |
| `messaging-intel/msgint-clients` | [#40](https://github.com/messaging-intel/msgint-clients/pull/40) | `b431cacab39c` | 0 green, 10 failed | `landed-evidence-gap`: contract landed; linked checks do not certify it |
| `opto-sync/opto-sync-clients` | [#81](https://github.com/opto-sync/opto-sync-clients/pull/81) | `767d7706e592` | 34 green, 0 failed | `green-reference`: separate SDK/API schema and cross-language contract gates are green |
| `ores-otel/ores-interfaces` | [#3](https://github.com/ores-otel/ores-interfaces/pull/3) | `24fef3a063b5` | 8 green, 0 failed | `green-reference`: canonical schema plus four language binding checks are green |
| `zed-pkg/zed-clients` | [#42](https://github.com/zed-pkg/zed-clients/pull/42) | `23879c32e753` | 18 green, 0 failed | `green-reference`: canonical hardener tests are green; runtime export receipts remain the next strengthening step |
| `benefactor-cc/benefactor-clients` | [#4](https://github.com/benefactor-cc/benefactor-clients/pull/4) | `cd30a7d7c956` | 3 green, 0 failed | `green-pilot`: JSON Schema, manifest, markers, matrix, and implementation changes landed |
| `file-tunnel/ftnl-clients` | [#12](https://github.com/file-tunnel/ftnl-clients/pull/12) | `ec98f8b5f0c1` | 9 green, 0 failed | `green-pilot`: JSON Schema, manifest, target markers, and language bindings landed |
| `quaestor-ledger/quaestor-clients` | [#50](https://github.com/quaestor-ledger/quaestor-clients/pull/50) | `4f0ce5078680` | 0 green, 24 failed | `landed-evidence-gap`: strict gates and contract artifacts landed, but the linked rollup is red |
| `daedalus-fab/daedalus-clients` | [#19](https://github.com/daedalus-fab/daedalus-clients/pull/19) | `7ed854dfc57d` | 0 green, 15 failed | `landed-evidence-gap`: schema/manifest/markers landed; linked checks do not certify them |

This is a ten-organization implementation set with linked, default-branch evidence. It
is **not** ten certified organizations yet. Five rows have green linked evidence and
five remain explicit gaps. Merging despite a red rollup is not treated as a pass.

The six PRs named in #1403 are all reconciled above. Benefactor, File Tunnel,
Quaestor Ledger, and Daedalus are the initial four additional high-value fleets because
they own material product/client boundaries and already expose a broad `clients/`
matrix. Fiducia, Shared Auth, Sonus Auris, Athlet-O, StreemPilot, and Scintilla are
follow-up candidates, but package-layout or nightly-hardening work is not counted as a
JSON Schema/API-manifest certification until its artifacts and checks meet this policy.

## Evidence gaps and next PRs

The rollout remains open until all of these are true:

- 3FA, Canonical, Messaging Intel, Quaestor Ledger, and Daedalus obtain green checks
  on current `main` or a linked follow-up PR at the exact evidence commit.
- Every fleet distinguishes its data-schema version from its API-manifest version.
- Hash-only implementations add a deterministic observed-export/binding comparison
  covering names, signatures, types, error vocabulary, async/streaming/cancellation,
  and capabilities.
- Every supported runtime has a package-consumer test and a pre-publish receipt from
  the same commit; every generated tree fails on a regeneration diff.
- Every language absent from a repository's supported target set is recorded as
  `unsupported` or `partial` with a product/toolchain reason, owner, and review date.
- The final issue comment links each repository-native command, green run, default
  commit, contract digest, and published-package prevention test.

Repository-specific fixes stay in their source repositories. This central ledger does
not copy client source into `k8s-cluster`, weaken failed checks, or certify a package
based only on file presence.

## Rollout sequence

1. Treat the five green rows as reference/pilot evidence, not automatic permission to
   copy their generated files blindly.
2. Diagnose the five red rollups and open narrow follow-up PRs for real contract or CI
   failures. Infrastructure/billing failures are recorded separately and rerun; they
   are never relabeled green.
3. Add the normalized runtime binding/export receipt and unsupported-language record
   to one green reference, validate its compatibility, then roll the checker version
   through the remaining nine repositories.
4. Wire the exact checker into test and pre-publish for every supported target, with a
   negative fixture proving stale generated output blocks publication.
5. Update this ledger only from immutable PR, commit, run, artifact, and digest links.
6. Close #1403 and DEN-3958 only after at least ten rows are green and DEN-3959 links
   this alternatives decision; keep later organization expansion as separate work.
