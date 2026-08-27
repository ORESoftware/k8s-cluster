# Test-organization namespace boundary

Linear: [DEN-2786](https://linear.app/denman/issue/DEN-2786)  
GitHub: [ORESoftware/k8s-cluster#1104](https://github.com/ORESoftware/k8s-cluster/issues/1104)  
Implementation: [ORESoftware/k8s-cluster#1123](https://github.com/ORESoftware/k8s-cluster/pull/1123)

## Decision

A GitHub organization ending in `-test` is a separate test execution and resource owner. It is not an alias for its canonical production organization.

The canonical relationship is deterministic:

```text
<canonical-owner>-test -> <canonical-owner>
```

Examples:

```text
fiducia-cloud-test -> fiducia-cloud
zed-pkg-test -> zed-pkg
canonical-cloud-test -> canonical-cloud
networking-components-test -> networking-components
discrete-event-systems-test -> discrete-event-systems
```

Both entries remain registered. The test owner keeps its own stable namespace root for test-owned fixtures and ephemeral infrastructure, while the canonical owner retains all production resources.

## Resource policy

Test-owned resources use a non-production hierarchy such as:

```text
fiducia-cloud-test/dev/namespace-canary/runtime
zed-pkg-test/dev/package-interoperability/github-app
canonical-cloud-test/staging/quote-e2e/database
```

Production resources remain under their canonical owner:

```text
fiducia-cloud/prod/fiducia-node/runtime
zed-pkg/prod/registry/signing
canonical-cloud/prod/canonical-api/database
```

A test organization:

- may not own a `prod` target;
- may not be an alias of a production owner;
- may not receive implicit write access to the canonical owner's root;
- may read a canonical or shared-service resource only through an explicit, least-privilege consumer grant;
- should use test-only accounts, buckets, namespaces, databases, keys, and service identities whenever provider-backed testing is required.

This distinction prevents CI identity from becoming infrastructure ownership and prevents a compromise in a test repository from reaching production by namespace convention alone.

## Pull-request CI policy

Cross-organization namespace tests use public, immutable source commits and repository-scoped `GITHUB_TOKEN` permissions only. Pull-request validation must not load:

- a personal access token;
- an account-wide Cloudflare token;
- an account-wide R2 access key;
- a production signing, sealing, database, or service credential.

Provider-backed acceptance tests belong behind an explicit non-pull-request gate and must use credentials scoped to the test owner and exact resources under test. A skipped credentialed tier is preferable to widening an account-wide secret into a test repository.

## Machine contract

Run:

```bash
python3 tools/namespace_test_owner_contract.py --root . --format text
python3 tools/test_namespace_test_owner_contract.py
```

The validator requires every `kind: test` registry entry to:

1. end in `-test`;
2. exactly match its lowercase GitHub owner;
3. have no aliases;
4. resolve, by suffix removal, to a registered product or shared-service owner;
5. keep any test-owned target under its own root;
6. avoid production-environment targets.

It also rejects a non-test owner that targets a test-owned root.

## Canary execution

The independent canary runs from a `*-test` GitHub organization. It checks out the exact proposed k8s-cluster commit without submodules or credentials, runs both namespace contract suites, verifies the current test-to-canonical binding, and proves that the pull-request ratchet rejects newly introduced legacy references.

The canary is read-only. It performs no Cloudflare, R2, IAM, Kubernetes, Argo CD, database, secret-store, or host mutation.
