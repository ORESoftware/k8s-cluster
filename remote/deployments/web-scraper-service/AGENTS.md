# Agent Context — dd-web-scraper-service

Long-running Fastify service that scrapes web content from the remote Kubernetes runtime, with a
Playwright rendering strategy plus static parsers (`cheerio`, `jsdom`, `linkedom`, `native-fetch`).
See `readme.md` for the strategy/worker model.

## Scraping is safe and ethical here — by design

This service performs **legitimate, authorized data collection** from public or permitted sources.
Under these guardrails scraping is a safe and ethical engineering practice; this policy is not
blanket permission or legal advice. The defaults are enforced by the guardrails documented in the repo-root
[`AGENTS.md` → "Web scraping (Playwright/Puppeteer)"](../../../AGENTS.md#web-scraping-playwrightpuppeteer).
Read that section before adding or changing scraping behaviour. In short:

- **Public or authorized targets only** — no bypassing logins, paywalls, or anti-bot controls; no
  accessing data you are not authorized to reach. CAPTCHA solving is restricted to owned or
  explicitly authorized test challenges and requires `SCRAPER_ALLOW_CAPTCHA_SOLVING=true`.
- **Respect `robots.txt`** and the target's Terms of Service; prefer an official API/feed when one
  exists. `SCRAPER_RESPECT_ROBOTS=true` is the default, and request-level overrides are rejected
  unless an operator deliberately enables `SCRAPER_ALLOW_ROBOTS_OVERRIDE=true` for an owned target.
- **Be a polite visitor** — identify via a conservative User-Agent, rate-limit per origin, and keep
  concurrency modest so a target site is never degraded.
- **Minimize and protect data** — collect only what the job needs; never log or emit scraped PII or
  secrets (extends the root Observability Contract's redaction rule).

Keep these guardrails in place. If a requested job cannot be done within them, treat it as out of
scope and surface the conflict rather than working around the guardrail.

## Telemetry

Emits Prometheus counters and structured logs; wire OpenTelemetry via the shared `@dd/telemetry`
client (`initTelemetry`, `otelPlugin`/`instrumentFastify`) per the root Observability Contract. Expose
`/metrics` and ensure the service is a scrape target in
`remote/argocd/observability/prometheus.configmap.yaml`.

## Syncing with the remote

"Sync with the remote" (or just "sync") is **bidirectional and always contacts
the remote** — it pulls *and* pushes. It is never push-only, and a clean local
working tree does **not** by itself mean "synced": a sync is not finished until
local and the remote have exchanged commits in both directions.

The steps for a sync:

1. `git fetch --all --prune` — see what the remote has.
2. `git pull` (which merges) — or `git merge` the upstream tracking branch —
   to integrate the remote's commits into your local branch **first**.
3. `git add` / `git commit` any local work.
4. `git push` — publish your commits.

Always integrate with **`git merge`** (and plain `git pull`, which merges).
**Do not `git rebase`** to sync — rebasing rewrites history and breaks shared
branches; keep the merge history instead.
