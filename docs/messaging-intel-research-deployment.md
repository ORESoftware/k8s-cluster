# Messaging Intel restricted-research deployment

This document defines the platform boundary for the consented Messaging Intel research capture workload. It does **not** authorize production capture or replace participant-consent, ethics, privacy, security, or provider approvals.

## Ownership boundary

The platform repository owns:

- the `msgint-research` namespace and its `restricted-research` classification;
- Pod Security Admission labels;
- ResourceQuota and LimitRange;
- the tokenless `msgint-runtime` ServiceAccount;
- default-deny networking and the DNS-only baseline;
- the `msgint-research` Argo CD AppProject.

The application repository may own namespaced Deployments, suspended CronJobs, Services, ConfigMaps, workload-specific NetworkPolicies, and other explicitly permitted workload resources. The AppProject rejects cluster-scoped resources and platform-owned Secret, ServiceAccount, quota, limit, Role, and RoleBinding objects.

## Inert release posture

`msgint-capture.application.yaml` intentionally uses the all-zero `targetRevision` sentinel and has no automated sync. Replace the sentinel only with a reviewed **immutable commit** SHA after every DEN-32 production release gate has recorded evidence and approval.

The capture CronJob must remain **suspended**, and collection must remain disabled, until that approval is complete. This platform PR does not unsuspend capture, create credentials, grant provider access, or apply manifests to a live cluster.

## Secrets

**Do not commit secrets** to this repository, the application repository, pull requests, Linear, or CI logs. Provider tokens, webhook secrets, encryption keys, database credentials, and research-participant identifiers must be provisioned through the approved external secret manager and narrowly scoped runtime identities.

The AppProject deliberately blacklists Kubernetes `Secret` objects from the application source. Any secret materialization mechanism must be platform-reviewed and must not expose plaintext in Git history or Argo render output.

## Validation

Run the same checks used by CI:

```bash
ruby -e 'require "yaml"; ARGV.each { |path| YAML.load_stream(File.read(path)) }' \
  remote/argocd/projects/msgint-research.tenant.yaml \
  remote/argocd/projects/msgint-research.appproject.yaml \
  remote/argocd/apps/msgint-capture.application.yaml
python3 scripts/validate-msgint-gitops.py
```

The validator checks the namespace classification, restricted Pod Security labels, tokenless ServiceAccount, default-deny and DNS-only policies, AppProject repository/destination boundaries, platform-owned resource blacklist, private SSH source URL, immutable revision format, and absence of application-managed namespace creation or automated sync.

## Controlled bootstrap

After review and before any Application sync:

1. Apply `remote/argocd/projects/msgint-research.tenant.yaml` as a platform administrator.
2. Apply `remote/argocd/projects/msgint-research.appproject.yaml`.
3. Verify quota, Pod Security, ServiceAccount token automount, and NetworkPolicy enforcement on the target cluster.
4. Complete DEN-30 provider approval/secrets and DEN-31 dedicated test-account integration evidence.
5. Complete and approve every DEN-32 release gate.
6. Replace the all-zero revision with the approved 40-character commit SHA.
7. Review the rendered manifests and perform a manual first sync.

Rollback means disabling or deleting the Argo Application first while retaining the namespace and evidence needed for incident response, withdrawal/deletion obligations, and authorized forensic review. Destructive cleanup requires the same accountable approval process as release.
