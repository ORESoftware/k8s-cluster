# AI agent coordinator platform registration

This directory is the cluster-owned half of the coordinator deployment boundary.
The workload manifests remain in
`ORESoftware/ai-agent-coordinator.rs@d25e04e50be4a9fad039cfcfa6c321e9c99a1e02`
at `deploy/overlays/cross-org-linear-pilot`.

The bundle is self-contained so Argo CD and `kubectl kustomize` can render it
with the default load restriction. It must not reach outside this directory for
resources.

## Ownership

The platform bundle owns:

- namespace `ai-agent-coordinator`;
- ResourceQuota and LimitRange;
- the workload ServiceAccount with token automount disabled;
- default-deny ingress;
- the strict `ai-agent-coordinator` AppProject.

The app repository owns namespace-scoped workload resources, including its
Deployment, Service, PVC, app-specific NetworkPolicy, ConfigMaps, CronJobs, and
ExternalSecrets. The AppProject rejects cluster-scoped resources and prevents the
app from replacing platform-owned quota, limits, or ServiceAccount objects.

## Immutable pilot source

The workload Application is pinned to:

```text
repository: https://github.com/ORESoftware/ai-agent-coordinator.rs.git
revision:   d25e04e50be4a9fad039cfcfa6c321e9c99a1e02
path:       deploy/overlays/cross-org-linear-pilot
```

That revision contains the signed multi-organization push intake, protected
Linear delivery worker, dry-run-only Sonus Auris/Daedalus Fab overlay, reusable
manifest validator, locked-down container canary, and the bounded one-shot
Meta Agents repository-bootstrap Job reviewed under DEN-1058.

## Bootstrap order

Each cloud cluster root includes two Applications:

1. `ai-agent-coordinator-platform` sync wave `-1` creates this tenant and strict
   AppProject from the `k8s-cluster` `dev` branch.
2. `ai-agent-coordinator` sync wave `0` renders the immutable app-repo pilot into
   the tenant namespace.

The workload Application does not use `CreateNamespace=true`. If the platform
boundary is absent, the workload remains unhealthy rather than widening its own
permissions.

## Meta Agents repository bootstrap

The pinned revision includes a fixed-name, no-retry Job that can create only
`meta-agents-demo/meta-agent-control-plane.rs`. It must verify the exact
`ORESoftware` identity and active organization-admin membership before mutation,
and it accepts an existing repository only when the canonical full name and
public visibility match.

Promotion to the live `dev` branch is not completion evidence. After Argo CD
reconciles the revision, verify the repository directly through GitHub, including
visibility and default branch, before publishing source or closing DEN-1058. Then
remove the bootstrap Job and its narrow deployment-contract exception through a
separate reviewed revision; do not leave repository-administration machinery in
steady-state configuration.

## Protected prerequisite

Before allowing the workload Application to sync, provision this AWS Secrets
Manager object through the approved operator path:

```text
dd/remote-dev/ai-agent-coordinator-linear-pilot
```

It must contain exactly these properties:

```text
LINEAR_API_TOKEN
GITHUB_WEBHOOK_SECRET_SONUS_AURIS
GITHUB_WEBHOOK_SECRET_DAEDALUS_FAB
```

Do not put values in Git, Linear, Argo Application parameters, manifests,
terminal transcripts, or CI logs. The app repo commits only an ExternalSecret
reference to `ClusterSecretStore/dd-cluster-secrets`.

The checked-in overlay remains fail-closed:

```text
LINEAR_DELIVERY_ENABLED=true
LINEAR_DELIVERY_DRY_RUN=true
```

It contains no completed-state IDs. A separate reviewed activation is required
before any Linear mutation can run.

## Verification

The focused GitHub Actions contract:

- checks out the app repository at the exact immutable revision;
- renders this platform bundle and all AWS/GCP/Hetzner cluster roots;
- renders the upstream pilot overlay;
- rejects copied workload manifests, cluster-scoped app resources, plaintext
  Secrets, wrong namespaces, mutable workload revisions, duplicate coordinator
  Applications, and `CreateNamespace=true` on the workload;
- verifies the ExternalSecret store and remote bundle;
- verifies Linear delivery remains dry-run with no completed-state IDs.

After cluster credentials are available, record only redacted evidence for:

1. platform and workload Argo Applications Healthy/Synced;
2. ExternalSecret Ready;
3. coordinator Deployment Available and `/readyz` healthy;
4. one disposable `Refs` push from Sonus Auris and one from Daedalus Fab;
5. duplicate-commit idempotency and invalid-signature/repository/branch cases;
6. dry-run plans resolving to the matching Linear projects;
7. `/v1/linear/deliver-next` remaining blocked;
8. the canonical Meta Agents repository existing publicly before bootstrap cleanup.

## Rollback

To stop the pilot without deleting durable state:

1. disable or delete the workload Application `ai-agent-coordinator`;
2. retain the platform Application while investigating, so the namespace and
   boundary remain stable;
3. rotate either webhook secret or the Linear token after suspected exposure;
4. do not enable live delivery during recovery;
5. repin the workload Application only to a reviewed immutable coordinator
   revision that passes the upstream and cluster GitOps contracts.

When PostgreSQL replaces the current PVC-backed SQLite runtime, preserve this
source/destination boundary, secret prerequisite, dry-run activation gate, and
multi-cluster registration. Change persistence semantically; do not replace the
pilot policy by choosing one side of a manifest conflict.
