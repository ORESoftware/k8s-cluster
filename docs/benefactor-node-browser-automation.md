# Benefactor Node.js browser automation

This document defines how the Benefactor daily prospecting workflow uses the shared Node.js browser-automation runtime in `ORESoftware/k8s-cluster`.

The browser runtime is an execution service, not the campaign system of record. Benefactor owns ICP queries, source policy, candidate normalization, provenance, deduplication, HubSpot/Postgres synchronization, ranking, consent, suppression, campaign approval, and delivery reconciliation. The cluster owns isolated browser execution, resource limits, driver compatibility, runtime telemetry, and Kubernetes deployment.

## Runtime inventory

| Runtime | In-cluster endpoint | Drivers | Purpose |
| --- | --- | --- | --- |
| `dd-browser-test-server` | `http://dd-browser-test-server.default.svc.cluster.local:8104/run` | Playwright, Puppeteer, Selenium | Primary bounded Node.js scenario runner |
| `dd-selenium-server` | `http://dd-selenium-server.default.svc.cluster.local:8105/run` | Selenium/WebDriver | Dedicated Selenium/Grid compatibility lane |

`dd-browser-test-server` is a Node 22 + Fastify service. Its executable Zod/OpenAPI contract supports `goto`, `click`, `fill`, `select`, `press`, `waitForSelector`, `waitForUrl`, `waitForTimeout`, `extractText`, `extractAttribute`, `screenshot`, and an explicitly gated `evaluate` step.

Current production defaults are intentionally bounded:

- Playwright is the default engine.
- Maximum concurrency is 2 scenarios per pod.
- Maximum scenario timeout is 180 seconds.
- Maximum scenario size is 64 steps.
- Screenshots are capped at 1.5 MB.
- Arbitrary JavaScript evaluation is disabled.
- Scenario execution requires `SERVER_AUTH_SECRET`.
- The execution endpoint remains in-cluster; public documentation routes do not expose `/run`.

## Engine selection

Do not execute every source through all three engines. Select one primary engine per source adapter and use another engine only for a classified compatibility failure.

### Playwright — default

Use Playwright for most discovery and verification flows:

- resilient locator and navigation behavior;
- deterministic waiting and page lifecycle handling;
- Chromium execution in the currently deployed image;
- screenshots, console entries, page errors, and step-level timing through the shared result contract.

A new source adapter should start with Playwright unless it requires a capability that is specific to another engine.

### Puppeteer — Chromium/CDP-specific

Use Puppeteer when a source needs Chromium DevTools Protocol behavior or a Chrome-specific integration that is awkward through the common Playwright path. Typical examples are specialized network inspection or existing Puppeteer-compatible code.

Puppeteer is not a general retry for a blocked source. A 401, 403, CAPTCHA, robots restriction, terms restriction, or source-policy rejection remains terminal or manual-review regardless of engine.

### Selenium — compatibility and Grid lane

Use Selenium when WebDriver compatibility itself is the requirement, when reproducing a Selenium-specific source adapter, or when validating behavior through the dedicated Selenium service. The dedicated service provides an isolated browser process and should be the fallback for known WebDriver/driver-compatibility cases, not a way to bypass source controls.

## Daily Benefactor execution flow

```text
ChatGPT schedule / operator trigger
  -> Benefactor control-plane or durable queue
  -> ICP + search-query rows in Postgres
  -> source-policy and domain-allowlist check
  -> bounded browser jobs
  -> dd-browser-test-server or dd-selenium-server
  -> source adapter normalization and provenance validation
  -> transactional Postgres dedupe/upsert
  -> HubSpot reconciliation
  -> ICP scoring and value ranking
  -> reviewed campaign manifest
  -> consent/suppression/idempotency preflight
  -> Gmail and/or SendGrid delivery
  -> provider outcome and reply reconciliation
```

The scheduled ChatGPT task is an orchestration signal. It must use a connected Benefactor control-plane or queue; it must never receive database credentials, browser session secrets, Gmail credentials, SendGrid credentials, or `SERVER_AUTH_SECRET`. When an integration is unavailable, the task reports the blocker rather than claiming work occurred.

### 1. Build jobs from Postgres policy

Each browser job is derived from a reviewed ICP/search-query row and a source-policy row. At minimum, the job envelope should carry:

- stable `job_id` and `query_id`;
- source identifier and policy version;
- exact allowed hostname;
- engine and adapter version;
- page/cursor identity;
- maximum attempts and deadline;
- tenant/campaign namespace;
- provenance destination;
- dry-run or collection mode.

A recommended idempotency identity is a SHA-256 hash of source, query ID, source-policy version, engine, adapter version, and page/cursor identity. Retries and engine fallbacks retain the originating job identity.

### 2. Enforce source policy before browser launch

A job is runnable only when the source policy explicitly allows the intended access pattern. The orchestrator must respect:

- source terms and contractual restrictions;
- robots directives where applicable;
- authentication and authorization boundaries;
- per-domain rate limits and concurrency;
- geographic or tenant restrictions;
- data-retention and provenance requirements.

Do not bypass CAPTCHAs, login controls, paywalls, bot defenses, or technical restrictions. Do not use browser automation to access private pages without authorization. A blocked source becomes terminal or manual-review; it is not automatically retried through another driver.

### 3. Submit a bounded scenario

A minimal Node.js caller inside the cluster can use the native Node 22 `fetch` API:

```js
const endpoint =
  process.env.BROWSER_TEST_URL ??
  'http://dd-browser-test-server.default.svc.cluster.local:8104/run';
const secret = process.env.SERVER_AUTH_SECRET;

if (!secret) throw new Error('SERVER_AUTH_SECRET is required');

const scenario = {
  requestId: 'benefactor:<query-id>:<source-id>:<page>',
  tool: 'playwright',
  timeoutMs: 30_000,
  captureFinalScreenshot: false,
  failOnConsoleError: false,
  steps: [
    {
      action: 'goto',
      url: 'https://example.com',
      waitUntil: 'domcontentloaded',
    },
    { action: 'waitForSelector', selector: 'main', state: 'visible' },
    { action: 'extractText', selector: 'title', name: 'pageTitle' },
  ],
};

const response = await fetch(endpoint, {
  method: 'POST',
  headers: {
    'content-type': 'application/json',
    'x-server-auth': secret,
  },
  body: JSON.stringify(scenario),
  signal: AbortSignal.timeout(35_000),
});

const result = await response.json();
console.log({
  ok: result.ok,
  requestId: result.requestId,
  tool: result.tool,
  durationMs: result.durationMs,
  stepCount: result.steps?.length ?? 0,
  extractedFieldCount: Object.keys(result.extracted ?? {}).length,
  screenshotCount: result.screenshots?.length ?? 0,
  consoleErrorCount: (result.consoleEntries ?? []).filter(
    (entry) => entry.level === 'error',
  ).length,
  pageErrorCount: result.pageErrors?.length ?? 0,
});
```

The example intentionally uses `example.com`. Production URLs and selectors come from reviewed source adapters. Do not place contact details, page text, screenshots, cookies, authorization headers, or browser storage in ordinary application logs.

When calling from an operator shell, execute inside the pod so the secret remains in the pod environment:

```bash
POD=$(kubectl get pod -n default -l app=dd-browser-test-server \
  -o jsonpath='{.items[0].metadata.name}')

kubectl exec -n default "$POD" -- sh -c '
  curl --fail --silent --show-error \
    -X POST http://localhost:8104/run \
    -H "content-type: application/json" \
    -H "x-server-auth: $SERVER_AUTH_SECRET" \
    --data @/work/scenario.json
'
```

Do not interpolate or print the secret in the outer SSM, SSH, CI, or ChatGPT command.

### 4. Normalize outside the shared runner

The shared browser service returns generic scenario results. Source-specific parsing and business rules belong in the Benefactor adapter layer, not in `dd-browser-test-server`.

A normalized candidate should preserve at least:

- source and exact source URL;
- discovery timestamp;
- query ID and browser job ID;
- engine and adapter version;
- business name and business domain;
- permitted business email and phone fields;
- contact name/title when collection is permitted;
- evidence locator or restricted artifact reference;
- source-policy decision and provenance status;
- normalization and verification status.

Raw page bodies, cookies, local/session storage, authorization data, and unrestricted screenshots are not candidate fields. Restricted artifacts should use encrypted object storage, a short retention period, and access-controlled references.

### 5. Dedupe and synchronize transactionally

Browser output never goes directly to a sender. The Benefactor service first:

1. normalizes domains, emails, phones, names, and company identities;
2. rejects malformed, prohibited, unverifiable, or policy-incomplete records;
3. deduplicates against Postgres, HubSpot, prior outreach, customers, active opportunities, replies, suppressions, unsubscribes, complaints, hard bounces, cooldowns, and do-not-contact rows;
4. writes provenance and candidate state in a Postgres transaction;
5. upserts the permitted subset to HubSpot;
6. records the HubSpot reconciliation outcome without losing the canonical Postgres identity.

### 6. Rank separately from collection

Ranking occurs after collection, policy validation, and deduplication. Keep the score explainable and versioned. Useful inputs include ICP match, business size, geography, role relevance, verified business-domain ownership, freshness, source reliability, prior engagement, and expected value.

A high score is not permission to contact someone. Delivery eligibility remains a separate consent, suppression, cooldown, and human-approval decision.

### 7. Deliver only from an approved manifest

Gmail and SendGrid consume the same deterministic, reviewed recipient manifest and canonical idempotency boundary. Browser jobs never call either provider.

For live delivery:

- use `hello@mail.benefactor.cc` only through the configured sender identities;
- use the GCP service account only behind the reviewed Gmail impersonation service;
- recheck every recipient immediately before provider submission;
- honor unsubscribe, complaint, hard-bounce, reply, customer, opportunity, do-not-contact, and cooldown precedence;
- preserve per-provider and cross-provider idempotency;
- reconcile provider outcomes and replies before the next touch.

When the exact approval or integration is unavailable, create drafts and an approval-ready manifest without sending.

## Retry and fallback policy

- Retry transient DNS, connection reset, 429, and 5xx failures with bounded exponential backoff and jitter.
- Honor `Retry-After` when present.
- Do not retry 401, 403, CAPTCHA, explicit bot-defense, terms, robots, or source-policy failures automatically.
- Do not exceed the source-specific daily request budget.
- Use a second browser engine only for a classified rendering, driver, or WebDriver compatibility failure.
- Preserve the same job ID, source cursor, and dedupe key across retries and fallbacks.
- Limit total attempts; a permanently failing source is quarantined for operator review.

## Session and secret handling

- Run each job in a fresh browser context/session unless a reviewed source adapter explicitly requires an authorized reusable session.
- Keep reusable sessions in a dedicated encrypted secret/session store with rotation and expiry; never in Git, Linear, ChatGPT, CI artifacts, screenshots, or logs.
- Reject caller-supplied `cookie` and `authorization` headers unless the adapter and source policy explicitly permit them.
- Keep `BROWSER_TEST_ALLOW_EVALUATE=false` for this workflow. Add a purpose-built DSL operation rather than enabling arbitrary JavaScript.
- Never return cookies or browser storage in the job result.

## Artifacts, privacy, and logging

The daily report contains aggregate counts only. It must not include contact data, source page content, screenshots, cookies, tokens, or browser storage.

Recommended artifact policy:

- screenshots off by default;
- enable only for a failed/manual-review job;
- encrypt at rest;
- retain for a short bounded period;
- store a redacted metadata record in Postgres;
- never attach prospect artifacts to GitHub, Linear, Slack, or ChatGPT;
- delete artifacts when the associated candidate is rejected or retention expires.

Telemetry labels must remain low-cardinality: engine, adapter, source class, status class, attempt number, and duration bucket. Do not use URLs, selectors, company names, emails, phones, or page text as metric labels.

## Testing strategy

1. Test source adapters against static local fixtures before hitting a live source.
2. Run contract tests against `example.com` for Playwright, Puppeteer, and Selenium.
3. Run one permitted-source canary with contact persistence disabled.
4. Verify normalization and provenance using synthetic records.
5. Test Postgres and HubSpot deduplication with replayed job IDs.
6. Test suppression and campaign-manifest checks independently from browser execution.
7. Test provider delivery only with controlled canary recipients and reviewed approval.

The existing browser runtime already exposes `/healthz`, `/status`, `/tools`, `/metrics`, public OpenAPI documentation, and authenticated internal OpenAPI documentation. Use those endpoints for readiness and contract discovery instead of inferring runtime capabilities.

## Ownership

| Concern | Owner |
| --- | --- |
| Node.js browser runtime and Kubernetes deployment | `ORESoftware/k8s-cluster` |
| Source adapters and ICP query construction | `benefactor-cc/benefactor-automations` and Benefactor services |
| Candidate/provenance records and deduplication | Benefactor Postgres services |
| CRM synchronization | Benefactor HubSpot integration |
| Ranking and manifest construction | Benefactor campaign control plane |
| Consent, suppression, idempotency, delivery, and reconciliation | Benefactor Gmail/SendGrid transports and canonical backend |
| Scheduling | ChatGPT daily task as an orchestration signal; in-cluster control plane as executor |

Do not create a second browser runtime inside a Benefactor repository. Extend the shared runner contract when a reusable, bounded capability is genuinely missing; keep source-specific selectors and business rules in Benefactor-owned code.
