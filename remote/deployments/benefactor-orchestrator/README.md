# Benefactor lead-discovery pipeline

This directory owns Benefactor's discovery and pre-outreach ingestion boundary tracked by
`DEN-260`. It searches for public business pages, asks the private `dd-web-scraper` service to
retrieve them, normalizes contact candidates, writes provenance-aware lead rows, and emits a
redacted deterministic report. It does **not** send email, SMS, chat, or any other outbound message.
Campaign execution belongs to the downstream outreach service and `DEN-833`.

## Flow

1. Load active ICP queries from `benefactor.benefactor_scrape_queries` in deterministic priority
   order.
2. Run each configured search adapter independently. Missing credentials disable only that adapter.
3. Normalize and reject unsafe/internal/non-HTTP candidate URLs.
4. Send arbitrary-domain retrieval only to the allowlisted private scraper service. The orchestrator
   has no direct-domain fallback.
5. Extract role email addresses and bounded E.164 phone candidates, following at most one same-origin
   contact/about page through the private scraper.
6. Dedupe candidates by normalized email and business domain, apply domain and request throttles, and
   preserve suppression state.
7. Persist each lead and its throttle record in one Postgres transaction, including provider, source
   URL, ICP/query, confidence, collection time, verification status, and phone candidates in
   `meta_data`.
8. Emit one `BENEFACTOR_PIPELINE_REPORT` JSON record. Email addresses are represented by SHA-256 in
   the report; raw recipient identifiers are not logged.

## Provider status

| Adapter | State | Credential/config |
| --- | --- | --- |
| Serper | Implemented, isolated search adapter | `SERPER_API_KEY` |
| Brave Search | Implemented, isolated search adapter | `BRAVE_SEARCH_API_KEY` |
| Apollo | Explicitly disabled until the authorized adapter contract is implemented | `APOLLO_API_KEY` |
| Hunter | Explicitly disabled until verification semantics are implemented | `HUNTER_API_KEY` |
| Datalane | Explicitly disabled until the authorized adapter contract is implemented | `DATALANE_API_KEY` |
| LinkedIn Sales Navigator | Explicitly disabled until an authorized export/import contract exists | `LINKEDIN_SALES_NAVIGATOR_EXPORT` |

Configured-but-unimplemented adapters report `disabled_adapter_not_implemented`; they never fall
through to another provider or receive traffic accidentally. Provider status includes bounded
request/result/failure counts, never credentials or raw provider responses.

## Security invariants

- `SCRAPER_AUTH` is sent only to a host in `SCRAPER_ALLOWED_HOSTS`; the default is
  `dd-web-scraper.default.svc.cluster.local`.
- Plain HTTP scraper traffic is accepted only for Kubernetes Service DNS. A non-cluster scraper must
  use HTTPS and be explicitly allowlisted.
- Discovered URLs reject credentials, IP literals, nonstandard ports, IDN labels, single-label hosts,
  and private/internal/reserved DNS suffixes before they reach the scraper.
- The private scraper request asks for robots enforcement and private-network rejection. Its own
  DNS-resolution/rebinding guard remains the authoritative network boundary.
- Provider and scraper bodies have byte limits plus total request/body deadlines. Database statements
  have a bounded timeout.
- The legacy `ALLOW_DIRECT_FALLBACK=true` path is rejected at startup rather than silently bypassing
  the private scraper.
- The pipeline never sends outreach and never adds a suppressed lead back to an eligible state.

## Dry run

Set `PIPELINE_DRY_RUN=true` to execute discovery and produce the same normalized report without
mutating query statistics, domain memory, leads, or throttle rows:

```bash
PIPELINE_DRY_RUN=true \
ICP_CATEGORY=roofing \
RDS_URL='postgresql://…' \
PG_SSL_CA_FILE=/secure/rds-ca.pem \
SCRAPER_AUTH='…' \
SERPER_API_KEY='…' \
node orchestrate.mjs
```

The report is canonicalized and sorted. Its `reportDigest` is stable for the same normalized provider
results, provider statuses, and counters even when inputs arrive in a different order.

## Configuration bounds

Every integer setting is validated and bounded. Defaults are intentionally conservative:

- `MAX_QUERIES=8`
- `TARGET_EMAILS=30`
- `MAX_PAGES_PER_QUERY=8`
- `DEADLINE_SECONDS=420`
- `PROVIDER_TIMEOUT_MS=15000`
- `SCRAPER_TIMEOUT_MS=45000`
- `MAX_PROVIDER_RESPONSE_BYTES=2097152`
- `MAX_SCRAPER_RESPONSE_BYTES=3145728`
- `DB_STATEMENT_TIMEOUT_MS=30000`

Invalid boolean or integer values fail startup instead of becoming `NaN`, negative limits, or
unbounded work.

## Validation

The focused suite uses only Node built-ins, so it does not need production credentials or package
installation:

```bash
node --check pipeline-lib.mjs
node --check providers/serper.mjs
node --check providers/brave.mjs
node --check providers/index.mjs
node --check orchestrate.mjs
node --test orchestrate.test.mjs
```

The GitHub Actions workflow runs these checks on Node 22 and Node 24 and ratchets the source boundary:
no direct arbitrary-domain fetch, no outreach API, provider-attributed persistence, dry-run write
guards, and transaction/rollback coverage.

## Remaining DEN-260 work

The next provider increment should implement Apollo, Hunter, and Datalane behind the same adapter
interface with provider-specific fixtures, rate-limit/retry tests, stable provider record IDs, and
verification fields. LinkedIn Sales Navigator must remain an operator-authorized export/import path;
do not automate access-control or CAPTCHA bypass. A separate `benefactor-e2e` repository remains the
right place for real-service fixture and browser coverage once provisioned under `DEN-1389`.
