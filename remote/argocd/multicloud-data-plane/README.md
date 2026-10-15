# Multi-cloud data-plane Applications

This directory holds the manual, fail-closed GitOps entry points for CockroachDB and the NATS
supercluster across AWS, GCP, and Azure. Read
[`docs/multicloud-cockroachdb-nats.md`](../../../docs/multicloud-cockroachdb-nats.md) before syncing
anything.

`clusters/<provider>` renders six ArgoCD Applications:

1. a manual cert-manager adoption/install;
2. trust-manager for projecting the public CockroachDB CA into a ConfigMap;
3. namespace, ExternalSecret, Issuer, Certificate, and Bundle prerequisites;
4. the CockroachDB v1beta1 operator;
5. that provider's region of the shared CockroachDB cluster; and
6. that provider's three-server NATS region in the shared `ORES_MULTICLOUD` supercluster.

All six Applications deliberately omit `syncPolicy.automated`. The AWS and GCP cluster roots only
register them for operator visibility; the Azure bootstrap root does the same when it is created.
The existing `remote/argocd/messaging` NATS Deployment is not mutated by this bundle.

Render the registration layer with:

```sh
for provider in aws gcp azure; do
  kubectl kustomize "remote/argocd/multicloud-data-plane/clusters/${provider}" >/dev/null
done
```

The `.github/workflows/multicloud-data-plane-contract.yml` workflow renders the pinned Helm
payloads, submits them with strict server-side dry-run against their pinned CRDs in an ephemeral
Kubernetes API server, and runs a real nine-process NATS supercluster test with a complete regional
outage and rejoin. Those checks prove substantially more than a render, but still do not prove the
real clouds' routing, DNS, certificates, storage, restore path, or workload behavior.
