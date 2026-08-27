# Scheduled-task digest

Tracking: `DEN-3562`

`ScheduledTaskDigest.gs` sends one consolidated email to
`alexander.d.mills@gmail.com` every day at approximately **07:00
America/Chicago**. Google Apps Script clock triggers support `nearMinute(0)`, so
the actual start may vary by roughly fifteen minutes while remaining DST-aware.

## Evidence window

Each scheduled delivery covers the immediately preceding 24 hours. It:

- discovers all `schedule`-event GitHub Actions runs in the configured central
  control repositories;
- fetches job evidence for critical workflows instead of trusting a green
  workflow conclusion;
- classifies a green workflow with skipped/missing execution jobs as
  `FALSE_GREEN`;
- reports expected-but-absent critical workflows as `MISSED`;
- keeps Kubernetes, ChatGPT-native, Messaging-Intel, Benefactor, and other
  schedules visible as `UNVERIFIED` until their authoritative runtime feeds are
  connected;
- emits plain text and HTML in one email.

The initial GitHub control repositories are:

- `ORESoftware/ai-agent-coordinator.rs`
- `ORESoftware/k8s-cluster`
- `ORESoftware/project-registry`

Public repositories require no token. A future private-repository extension may
set `SCHEDULED_TASK_DIGEST_GITHUB_TOKEN` as an Apps Script property manually,
but credentials must never be committed, logged, emailed, or placed in Linear.

## Trigger and deployment bootstrap

The Apps Script project initializes `ScheduledTaskDigest.gs` whenever any entry
point executes. The existing deployment workflow calls the public bridge health
endpoint after redeployment. Global initialization therefore fails the health
check unless:

1. exactly one `runScheduledTaskDigest` trigger exists;
2. the trigger is configured for hour 7, minute 0, every day, in
   `America/Chicago` when first installed;
3. a bounded GitHub API probe returns the expected control repository; and
4. at least one unit of Apps Script mail quota remains.

The trigger repair routine only manages its own handler and does not modify the
Google Chat export continuation trigger.

## Duplicate-delivery behavior

The scheduled path stores a fixed logical-date delivery record in Script
Properties before sending. A second invocation for the same Central date is
suppressed. A known send failure clears the pending record for retry; an
unobservable crash after the mail provider accepts a message remains
at-most-once to favor the user requirement of one daily digest.

`sendScheduledTaskDigestNow()` is an explicit manual canary. Its subject starts
with `[MANUAL CANARY]`, and it does not consume the scheduled logical-date key.

## Status meanings

- `SUCCESS`: the required execution job succeeded.
- `FAILED`: the workflow or required execution job failed.
- `FALSE_GREEN`: the workflow was green while execution was skipped or absent.
- `MISSED`: a due task had no completed execution evidence or was skipped.
- `RUNNING`: execution was still in progress at digest time.
- `NOT_DUE`: the registered task was outside its schedule window.
- `UNVERIFIED`: the evidence source was unavailable or no authoritative runtime
  feed is connected.
- `OBSERVED_SUCCESS`: a non-critical GitHub workflow was green, but no stronger
  task-specific execution contract was configured.

No unverified or configuration-only state is represented as success.
