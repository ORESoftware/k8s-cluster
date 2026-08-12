# Daily scheduled-task digest

Tracking: `DEN-3562`

The production scheduler is an AWS-only Kubernetes `CronJob` named
`dd-scheduled-task-digest`. It runs at nominal **07:00 America/Chicago** every
day (`0 7 * * *`) and sends one text-plus-HTML email to
`alexander.d.mills@gmail.com` covering the immediately preceding 24 hours.
Kubernetes owns daylight-saving transitions through `spec.timeZone`.

## Why AWS-only

The shared runtime is mirrored into AWS and Hetzner. Deploying the sender in the
shared overlay would create two independent 07:00 jobs and could deliver two
emails. The `dd-scheduled-task-digest` Argo CD `Application` is therefore
registered only in `remote/argocd/clusters/aws`.

## Evidence sources

The digest reads:

- every Kubernetes `CronJob` and retained `Job` in every namespace;
- all `schedule`-event workflow runs in the three central control repositories;
- job-level evidence for the registered critical GitHub workflows.

A green GitHub workflow is not certified when its real enqueue/maintenance job
was skipped or absent. A Kubernetes task is certified only when retained Job
evidence covers every due execution. High-frequency CronJobs whose history
limits cannot retain the full 24-hour window are labeled `OBSERVED_SUCCESS`, not
certified. Missing feeds remain `UNVERIFIED`.

## One-email invariant

The scheduled path claims a Kubernetes `Lease` keyed by the Central logical
date before sending. A second invocation seeing the same date in `claimed` or
`sent` state suppresses delivery. The design favors at-most-once delivery after
an ambiguous network failure so a provider response timeout cannot create a
second email.

The sender also uses `concurrencyPolicy: Forbid`, an active deadline, and a
fixed recipient. Manual canaries set `FORCE_SEND=1`, carry a
`[MANUAL CANARY]` subject prefix, and are labeled so they do not pollute the
scheduled Job counts.

## Mail transport

The pod calls the existing in-cluster `dd-email-sms-contact-rs` service. The
request is authenticated with `SERVER_AUTH_SECRET` read from the existing
`dd-agent-secrets` Kubernetes Secret; SendGrid credentials remain inside the
contact service. The scheduler never receives or stores the SendGrid key.

Before collecting or sending a digest, the pod requires `/readyz` to report
`email.sendgrid_configured=true`. Provider error bodies and credential-shaped
metadata are not included in logs or email.

## Deployment and canary

`.github/workflows/scheduled-task-digest.yml` validates source, manifests, RBAC,
DST behavior, classification, redaction, and duplicate suppression. On a push
to `dev`, it waits for the AWS Argo CD application and runs an in-cluster dry
probe copied from the CronJob. A manual workflow dispatch with
`send_canary=true` runs the same Job with `FORCE_SEND=1` and proves real email
delivery.

The earlier Google Apps Script implementation remains in `main`, but its clasp
redeployment is blocked until the protected environment contains valid
`CLASPRC_JSON` and `CLASP_JSON`. It is not treated as active and does not affect
the AWS CronJob.
