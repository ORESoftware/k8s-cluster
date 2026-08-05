# ChatGPT rate-limit thread resumer

`dd-chatgpt-rate-limit-resumer` is an AWS-only scheduled browser recovery workload applied by the AWS cluster-root Kustomization. It runs every day at **03:00 Central time**. The Kubernetes CronJob uses `timeZone: America/Chicago`, so it follows CST/CDT daylight-saving transitions instead of hard-coding a UTC offset.

## Behavior

The job opens the operator-authorized ChatGPT account with a Playwright storage-state file, inspects recent conversation links from the ChatGPT sidebar, and resumes only a conversation whose latest visible conversation state is a recognized rate/request-limit failure. It does not broadly replay old prompts.

Before sending anything, the runner requires all of these conditions:

- the page is an authenticated `https://chatgpt.com/.../c/<conversation-id>` conversation;
- the latest assistant turn or trailing alert is a short, recognized rate-limit error such as `Too many requests`, `rate limit reached`, or `please try again later`;
- the composer is available;
- no generation is currently in progress;
- the conversation has not been attempted inside the 20-hour cooldown;
- the conversation has not exceeded seven attempts in the seven-day attempt window.

The continuation prompt tells ChatGPT to read the full thread, preserve completed work, avoid repeating finished sections, and complete only the unfinished work. A run scans at most 60 recent conversations and submits at most eight continuations.

## Authentication state

No ChatGPT cookie, token, conversation content, or title belongs in Git.

The existing ExternalSecret `dd-agent-secrets` extracts the AWS Secrets Manager JSON object at `dd/remote-dev/agent-secrets`. Add one property to that object:

```text
CHATGPT_STORAGE_STATE_JSON
```

Its value must be a complete Playwright storage-state JSON object with `cookies` and `origins` arrays, exported from a browser session that the operator has authenticated normally. Do not paste that JSON into chat, logs, shell history, issues, or pull requests.

The CronJob projects only that single Secret key. On the first run, or whenever the seed changes, it copies the state into the encrypted `dd-block` PVC. Successful runs write refreshed cookies back to the PVC. If neither a valid seed nor a previously persisted state exists, the job exits with configuration status 78 and sends nothing.

If ChatGPT redirects to login, requests an interactive human/CAPTCHA check, or removes the expected conversation UI, the job fails closed and sends nothing. It never automates login, MFA, or CAPTCHA solving.

## Idempotency and privacy

The encrypted PVC stores refreshed browser state plus a compact run ledger. Conversation IDs are SHA-256 hashed before they enter the ledger or logs. The runner does not persist conversation titles or message text, does not take screenshots, and does not log page content.

The ledger records cooldowns, attempt counts, and outcomes after every submitted continuation. Kubernetes `concurrencyPolicy: Forbid` prevents overlapping scheduler-created runs. An atomic lock on the encrypted PVC also blocks overlap with manual Jobs or automatic Job retries; locks older than 90 minutes are reaped because the pod's hard runtime ceiling is 65 minutes. The workload is included directly by the AWS cluster root and omitted from the GCP and Hetzner cluster roots, so another cluster cannot perform a duplicate run.

## Network and runtime boundaries

The pod has no service-account token, no ingress, a read-only root filesystem, and no Linux capabilities. Egress is limited to DNS and public TCP/443, with private, loopback, link-local, documentation, multicast, and cloud-metadata ranges denied. The Playwright context adds a second hostname boundary limited to ChatGPT/OpenAI static and content domains.

The job reuses the immutable `dd-web-scraper` image already deployed in the cluster because that image contains the matching Playwright package and Chromium runtime. The scheduled script itself is mounted from a generated ConfigMap.

## Operations

Create a one-off run after provisioning or rotating the storage state:

```bash
kubectl -n default create job dd-chatgpt-rate-limit-resumer-manual \
  --from=cronjob/dd-chatgpt-rate-limit-resumer
```

Inspect structured results without printing Secret data:

```bash
kubectl -n default logs job/dd-chatgpt-rate-limit-resumer-manual
```

Expected terminal events are `run_completed` or `run_failed`. Per-conversation log entries contain only a 20-character hash and an outcome such as `resumed`, `still_rate_limited`, `submitted_in_progress`, or `response_timeout`.
