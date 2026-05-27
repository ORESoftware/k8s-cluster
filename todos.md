# Todos

Open hardening / follow-up items captured from agent sessions. Append-only;
remove an entry only when its concrete change is merged to `dev` and reconciled
by ArgoCD.

## Security hardening — bigger items not yet applied

Captured 2026-05-23 from the AWS-credential-leak audit. The narrow fixes (dev-server
`sanitizeEventText` AWS coverage, container-pool nerdctl scrubber, gitleaks CI
workflow + `.github/gitleaks.toml`, locked-in tests) shipped in the same session.
The items below each have meaningful blast radius and need an explicit go-ahead
before being applied.

### 1) `dd-build-server`: switch from static AWS keys in `dd-agent-secrets` to instance-role / IRSA credential chain

This is the only kube-secret-backed AWS key path in the repo and the readme
already calls for it. Concrete shape:

- Drop `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` env
  entries from `remote/argocd/dd-next-runtime/dd-build-server.deployment.yaml`.
- Replace `aws_credentials_from_env()` in
  `remote/deployments/build-server-rs/src/main.rs:803-810` with the AWS SDK
  default credential chain (or an explicit ECR token request via the SDK). The
  deployment already runs on EC2 with a node instance role; on EKS, attach an
  IAM role to the `dd-build-server` ServiceAccount.
- Remove the corresponding three keys from the `dd/remote-dev/agent-secrets`
  AWS Secret.

### 2) Block IMDS for host-network warm workers

Today the warm Node.js/Claude worker containers run with `--network host` and
can hit `169.254.169.254` to grab the node's instance-role credentials.
Mitigations, in order of preference:

- On the EC2 instance metadata options, set `HttpPutResponseHopLimit=1` so
  containers running under a bridge can't fetch IMDS through a hop. Doesn't
  help host-network containers.
- Add an `iptables` / `nftables` rule on the EC2 host that drops outbound
  `dst 169.254.169.254` from any UID/PID outside the kubelet / EC2 metadata
  client. This is the only fix that catches host-network containers.
- Or move warm workers off host-network onto a CNI-managed bridge so the
  existing thread-pod-style NetworkPolicy applies.

### 3) Transcript-redaction sweep

Add a sweep that runs `sanitizeEventText` over `agent-transcripts/*/<id>.jsonl`
and `tmp/convos/thread.log` on a schedule (or before any export), so a pasted
secret doesn't sit unredacted in those files. Today only the live event stream
is sanitized; the persisted artifacts are not.

### 4) Local pre-commit hook mirroring the gitleaks rules

So devs catch keys before they hit CI. `.husky/pre-commit` calling
`gitleaks protect --staged --config .github/gitleaks.toml`.

## Decision pending

Want to do (1) the build-server credential-chain swap, (2) the IMDS netfilter,
(3) transcript sweep, and/or (4) the local pre-commit hook? Each is a separate,
contained change. (1) and (3) are the highest-value follow-ups given the leak
that triggered this audit.

## Reminder — credential revocation

The STS credentials pasted into chat on 2026-05-23 are still hot until the
revocation steps are run. The redaction work shipped in the same session only
stops future leaks; it does not revoke the access key id (literal value
omitted from this file: `<redacted-aws-access-key>` — secret-scan CI gate
would fail if the raw `ASIA…` shape were committed here). The issuing
principal must be rotated through CloudTrail lookup → IAM key rotation /
`AWSRevokeOlderSessions` policy, depending on whether the session was issued
from a long-lived IAM user key, an `AssumeRole` chain, or IAM Identity Center
SSO.
