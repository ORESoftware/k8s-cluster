# Messaging Intel exact continuity planner

This planner reserves one exact private repository, revision, workflow path, workflow name, trigger, ordered DAG, action/input set, and command set. A reserved identity or contract mismatch is terminal and cannot fall back to the generic Node classifier.

## Reserved source identity

```text
repository: messaging-intel/msgint-connectors
revision:   a9cc977d78347ec0efdbe8e6766967f80d425882
workflow:   .github/workflows/gha-clone-operator-config.yml
name:       Messaging Intel GHA clone operator verification
trigger:    workflow_dispatch with no inputs
```

The GitOps rule grants no organization wildcard, sibling repository, other workflow path, branch, or tag.

## Exact graph and profiles

```text
operator_config -> repository_tests
```

The only accepted mapping is:

```text
operator_config  -> node-hardened-verify
repository_tests -> node-hardened-test
```

Both jobs must run on `ubuntu-latest` and contain exactly three steps in this order.

### Checkout

```text
actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1
persist-credentials: false
```

### Node setup

```text
actions/setup-node@820762786026740c76f36085b0efc47a31fe5020
node-version: "22.23.1"
cache: npm
```

No registry URL, token, alternate cache, extra input, mutable action reference, missing step, reordered step, or extra action is accepted.

### Operator commands

```text
npm ci --ignore-scripts
npm run check
npm run test:operator-config
npm audit --audit-level=high
```

### Repository-test commands

```text
npm ci --ignore-scripts
npm test
```

The command lists are exact after newline normalization and trimming. Reordering, prepending, appending, shell overrides, publication, ignored failures, or command lookalikes fail closed.

## Terminal reserved mismatch

The repository and workflow path form a reserved namespace. Any of these conditions produces a non-executable plan with no generic fallback:

- exact repository with another workflow path;
- exact workflow path with another repository;
- any revision other than the reviewed commit;
- a different workflow name or trigger;
- added workflow/job keys such as permissions, env, defaults, concurrency, outputs, strategy, services, containers, or environment approval;
- secret, GitHub token, or OIDC expressions;
- changed job set/order/dependency/runner;
- changed action SHA or input; or
- changed command sequence.

Unrelated repositories and workflow paths remain on their existing reviewed classifiers.

## Real-process execution evidence

The `msgint_http` integration starts the actual `gha-clone-server` binary with API execution enabled only inside the test process and points it at a recording local build-server mock.

One accepted immutable request must produce exactly two authenticated build submissions in deterministic topological order:

```text
operator_config  -> node-hardened-verify
repository_tests -> node-hardened-test
```

Every submission must contain:

- `schemaVersion=build-server.v1`;
- `jobKind=run-profile`;
- exact `https://github.com/messaging-intel/msgint-connectors.git`;
- exact reviewed immutable SHA;
- no caller-supplied command or image;
- deterministic `gha-clone:{planId}:{jobId}` identity; and
- `x-build-server-auth`.

Repeating the exact run must reuse each job's request identity while keeping the operator and repository-test identities distinct.

The actual-binary test also requires zero `/builds` submissions for:

- a different revision;
- the reserved repository under another workflow path;
- the reserved workflow path under a lookalike repository;
- mutable setup action refs;
- changed checkout credential persistence;
- changed Node version;
- added publication commands; and
- secret-bearing setup inputs.

## Separation from live private-source execution

This is local executable contract evidence against a recording build-server mock. It does not fetch the private repository with live credentials and does not activate the deployed service.

The service remains at zero replicas with API and webhook execution disabled. The build server independently binds the exact repository to `node-hardened-verify` and `node-hardened-test`.

Live activation still requires the least-privilege GitHub App, reconciled ExternalSecret, plan-only deployment evidence, a live immutable private-source smoke, and explicit review. No classic PAT belongs in this path.
