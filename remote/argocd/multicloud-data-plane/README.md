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
6. that provider's independent NATS R3 JetStream cluster plus gateway.

All six Applications deliberately omit `syncPolicy.automated`. The AWS and GCP cluster roots only
register them for operator visibility; the Azure bootstrap root does the same when it is created.
The existing `remote/argocd/messaging` NATS Deployment is not mutated by this bundle.

Render the registration layer with:

```sh
for provider in aws gcp azure; do
  kubectl kustomize "remote/argocd/multicloud-data-plane/clusters/${provider}" >/dev/null
done
```

Render the pinned Helm payloads with the commands exercised by
`.github/workflows/multicloud-data-plane-contract.yml`. A render proves only declarative shape; it
does not prove cloud routing, DNS, certificates, storage, quorum, restore, or failover.
