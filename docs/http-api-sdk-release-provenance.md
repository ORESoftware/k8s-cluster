# Fleet HTTP SDK release bundles

The fleet SDK source tree under `remote/api-sdks` is generated from the committed public and
internal OpenAPI 3.1 catalogs. A release must not introduce another source of truth. It is a
verified, deterministic projection of the existing `sdk-lock.json`, package manifests, catalogs,
and generated package files.

## Release artifacts

`remote/tools/build-api-sdk-release.mjs` emits two scope-isolated archives:

- `oresoftware-k8s-api-sdks-public.tar` contains the public catalog and the public TypeScript,
  Rust, Dart, and Gleam packages.
- `oresoftware-k8s-api-sdks-internal.tar` contains the internal catalog and the internal packages
  for those four languages.

Each archive contains a `release-manifest.json` with:

- the exact source Git commit and package version;
- the SHA-256 digest of `remote/api-sdks/sdk-lock.json`;
- the generator and generated API-docs index digests;
- the matching catalog identity and file digest;
- each package-manifest digest; and
- a size and SHA-256 digest for every file in that archive.

The output directory also contains the two scope manifests, `release-index.json`, and
`SHA256SUMS`. No wall-clock timestamp, runner path, random value, or mutable dependency reference
is written into an artifact.

`internal` is an API visibility classification, not a claim that bytes committed to this public
repository are confidential. The separate archive prevents public consumers from accidentally
installing or indexing internal operations.

## Mandatory preconditions

The release builder fails unless all of the following hold:

1. `remote/api-sdks` is clean and contains only regular tracked files.
2. The SDK generator digest and generated API-docs index digest match `sdk-lock.json`.
3. Both catalog files match their lock entries, operation counts, service counts, and skipped
   gitlink inventories.
4. All eight language/scope package pairs exist exactly once.
5. Every package manifest and every generated package file matches its recorded SHA-256 digest.
6. The tracked files in each package directory exactly equal the manifest inventory plus the
   manifest itself.
7. The entire tracked SDK tree is accounted for by the lock graph, catalogs, and generated README.
8. Every package has the same semantic version.
9. A requested release version exactly equals that generated package version.

The existing generator and independent SDK validator still run first. The release builder then
rechecks the lock graph independently before it stages any files.

## CI flow

`.github/workflows/openapi-sdk-release-bundles.yml` builds the release set twice from the same
revision and requires byte-identical archives, manifests, indexes, and checksums. It also opens each
tar archive and rejects cross-scope package paths or manifest entries.

The CI job writes only to `$RUNNER_TEMP`; the repository must remain clean afterward. It does not
publish packages, upload release artifacts, or receive registry credentials. Registry publication
and artifact attestation remain an explicit promotion step after the deterministic source bundle
has passed review. A promoter must use these exact archives or reproduce the same release identity.

## Build locally

The output directory must be outside the repository because it is deleted before each build.

```bash
node remote/tools/generate-api-sdks.mjs --check
node remote/tools/validate-api-sdks.mjs
node remote/tools/build-api-sdk-release.mjs \
  --output-dir /tmp/k8s-api-sdk-release \
  --source-revision "$(git rev-parse HEAD)" \
  --release-version 0.1.0

(cd /tmp/k8s-api-sdk-release && sha256sum --check SHA256SUMS)
```

GNU tar is required. The builder normalizes archive ordering, modification times, ownership, and
permissions, so two builds from the same commit and package version must be byte-identical.

## Inspect a bundle

Verify checksums and inspect the immutable release identity before promotion:

```bash
sha256sum --check SHA256SUMS
jq '{version, sourceRevision, releaseIdentitySha256, bundles}' release-index.json

tar -xf oresoftware-k8s-api-sdks-public.tar release-manifest.json
jq '{scope, version, sourceRevision, sdkLock, releaseIdentitySha256}' release-manifest.json
```

A registry publisher must never regenerate clients from a different contract, modify package
source, or combine the public and internal scopes during promotion. Any GitHub artifact attestation
must name the archive and `release-index.json` digests produced by this builder.
