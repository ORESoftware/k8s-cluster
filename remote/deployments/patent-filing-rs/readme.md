# `dd-patent-filing-rs`

Rust/Axum patent filing **preparation** service.

The service helps an operator turn invention intake into a draft filing package and
runs deterministic pre-filing checks:

- guided HTML intake at `/` using htmx form posts
- JSON provisional-package generation at `/packages/provisional`
- readiness scoring at `/readiness`
- prior-art search planning at `/search/plan`
- package review at `/review/package`
- **claim formality / proofreading checks at `/claims/check`**
- **USPTO fee estimation at `/fees/estimate`**
- **filing deadline analysis at `/deadlines`**
- **AI-assisted drafting at `/draft/ai`** (Claude, with a deterministic self-audit + repair loop)
- generated API docs at `/docs/api`, `/api/docs`, and `/api/docs.json`

A generated package now also embeds a claim audit, a USPTO fee estimate, and a
filing-deadline report alongside the readiness review, draft, and search plan.

It prepares package evidence and checklists only. It does **not** hold USPTO Patent
Center credentials, file applications, give legal advice, or replace attorney review.
Fee figures are estimates of standard USPTO fees effective 2025-01-19 and exclude
attorney fees, extensions, petitions, IDS, and issue/maintenance fees.

## Routes

- `GET /` - htmx home page and package generator.
- `GET /descriptor` - service descriptor.
- `GET /schema` - request/response contract summary.
- `GET /example` - sample intake request.
- `GET /matters` - recent in-memory matters.
- `GET /matters/:matter_id` - one generated matter package.
- `POST /packages/provisional` - authenticated JSON package generation.
- `POST /ui/packages` - authenticated htmx form package generation.
- `POST /readiness` - authenticated readiness review from intake.
- `POST /search/plan` - authenticated prior-art search plan from intake.
- `POST /review/package` - authenticated review of a generated package.
- `POST /claims/check` - authenticated claim-set formality audit (`{ claims: [..], abstract? }`).
- `POST /fees/estimate` - authenticated USPTO fee estimate by entity status.
- `POST /deadlines` - authenticated filing-deadline analysis from key dates.
- `POST /draft/ai` - authenticated AI-assisted draft generation (requires `ANTHROPIC_API_KEY`).
- `GET /healthz`, `GET /readyz`, `GET /metrics`.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json`.

Mutating and sensitive routes require `X-Server-Auth` or `Auth` to match
`SERVER_AUTH_SECRET`, unless `PATENT_FILING_ALLOW_UNAUTHENTICATED=true` is set for
local development.

### Claim audit (`/claims/check`)

Deterministic, dependency-aware checks over a claim set:

- independent / dependent / multiple-dependent classification and counts;
- invalid claim references (pointing at a non-existent claim);
- improper dependency (self or forward reference, 35 USC 112(d));
- multiple-dependent-on-multiple-dependent (35 USC 112(e));
- advisory antecedent-basis scan (`the X` / `said X` with no introducing `a X`),
  inheriting terms down the dependency chain to avoid false positives;
- abstract word count vs the 150-word limit (37 CFR 1.72(b));
- excess-claim fee triggers (>3 independent, >20 total).

### Fee estimate (`/fees/estimate`)

Standard USPTO fee line items (effective 2025-01-19) for `large` / `small` / `micro`
entity status: provisional or utility filing/search/examination base fees, excess
independent claims (>3), excess total claims (>20), and the multiple-dependent-claim
surcharge. Small = 40% and micro = 20% of the undiscounted amount.

### Deadlines (`/deadlines`)

Milestones with computed due dates and days remaining from any of
`provisionalFilingDate`, `publicDisclosureDate`, `foreignPriorityDate`:

- 12-month provisional pendency / nonprovisional-or-PCT benefit (37 CFR 1.78);
- 14-month restoration-of-priority outer limit;
- 12-month Paris Convention foreign filing window;
- 12-month US grace-period statutory bar (35 USC 102(b)(1)), with a warning that
  most non-US jurisdictions require absolute novelty.

### AI-assisted drafting (`/draft/ai`)

Takes the same intake contract as `/packages/provisional` and uses Claude
(`claude-opus-4-8` by default, adaptive thinking + `effort: high`, structured
JSON output) to generate an abstract, a claim set, and specification sections.

The output is then run **back through the deterministic `/claims/check` auditor**.
If the generated claims contain blocking formality defects (invalid/forward
dependencies, multiple-dependent-on-multiple-dependent, etc.), the service feeds
those findings back to the model for **one automatic repair pass** and re-audits —
so the deterministic checker acts as the grader for the generative draft. The
response includes the draft, the final `claimAudit`, a `feeEstimate`, and a
`repairApplied` flag.

This endpoint is disabled (returns `503`) unless `ANTHROPIC_API_KEY` (or
`PATENT_FILING_ANTHROPIC_API_KEY`) is set. It is preparation support only — not
legal advice and not a filing. It has its own longer request timeout
(`AI_REQUEST_TIMEOUT_SECS`, 150s) since model generation far exceeds the 15s used
by the deterministic endpoints.

## Hardening

- Graceful shutdown on **both** SIGINT and SIGTERM (k8s sends SIGTERM).
- Structured `tracing` logs (configure with `RUST_LOG`).
- Request body limit (`DefaultBodyLimit` + `RequestBodyLimitLayer`) and a 15s
  per-request timeout.
- Security response headers on every route: `Content-Security-Policy`,
  `X-Content-Type-Options`, `X-Frame-Options`, `Referrer-Policy`,
  `Cross-Origin-Opener-Policy`.
- The htmx asset is pinned and loaded with a Subresource Integrity (`integrity`)
  hash plus `crossorigin`/`referrerpolicy` (supply-chain hardening).
- Constant-time comparison of the auth secret.
- Release profile with `lto = "thin"`, single codegen unit, and stripped symbols.

## Environment

| Var | Default | Purpose |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | Bind host. |
| `PORT` | `8116` | Bind port. |
| `SERVER_AUTH_SECRET` / `PATENT_FILING_SERVER_AUTH_SECRET` | unset | Required shared secret for package generation and matter reads. |
| `PATENT_FILING_ALLOW_UNAUTHENTICATED` | `false` | Local-dev escape hatch. |
| `PATENT_FILING_CENTER_URL` | `https://patentcenter.uspto.gov/` | Operator handoff URL surfaced in generated checklists. |
| `PATENT_FILING_MAX_MATTERS` | `200` | In-memory matter retention cap. |
| `ANTHROPIC_API_KEY` / `PATENT_FILING_ANTHROPIC_API_KEY` | unset | Enables `/draft/ai`. When unset the endpoint returns `503`. |
| `PATENT_FILING_AI_MODEL` | `claude-opus-4-8` | Model used for AI drafting. |
| `PATENT_FILING_AI_MAX_CONCURRENCY` | `4` | Max concurrent `/draft/ai` model calls; excess returns `429`. |
| `PATENT_FILING_ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Anthropic API base URL override. |
| `RUST_LOG` | `dd_patent_filing_rs=info,tower_http=info` | Tracing filter. |

## Local Smoke

```bash
cd remote/deployments/patent-filing-rs
PATENT_FILING_ALLOW_UNAUTHENTICATED=true cargo run
```

Open `http://127.0.0.1:8116/` and submit the prefilled intake form, or:

```bash
curl -s localhost:8116/fees/estimate -H 'content-type: application/json' \
  -d '{"entityStatus":"micro","filingTrack":"non-provisional","totalClaims":25,"independentClaims":4}'
curl -s localhost:8116/deadlines -H 'content-type: application/json' \
  -d '{"provisionalFilingDate":"2025-06-01","publicDisclosureDate":"2025-01-01"}'
curl -s localhost:8116/claims/check -H 'content-type: application/json' \
  -d '{"claims":["A device comprising a sensor.","The device of claim 1, wherein the sensor is thermal."]}'
```

## Container & Kubernetes

```bash
# build context is the repo root
docker build -f remote/deployments/patent-filing-rs/Dockerfile -t dd-patent-filing-rs:dev .
```

Manifests in `k8s/ec2/` (`kubectl apply -k k8s/ec2`) deploy the service with
non-root securityContext, dropped capabilities, `RuntimeDefault` seccomp, and
startup/readiness/liveness probes, matching the sibling deployments.
