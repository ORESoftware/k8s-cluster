# Benefactor 200–300 public-business-contact discovery runbook

The orchestrator uses Benefactor ICP query rows from AWS RDS, Brave Search and/or Serper, and the
private in-cluster `dd-web-scraper` browser service. It extracts public role inboxes and bounded phone
candidates, stores provenance-aware leads, and never sends outbound messages.

## Preflight

1. Confirm active ICP and query rows in `benefactor.benefactor_scrape_queries` for the target category.
2. Confirm the RDS TLS CA, private scraper authentication, and at least one search provider credential.
3. Confirm `dd-web-scraper.default.svc.cluster.local` is healthy and that its Playwright escalation,
   robots enforcement, DNS/rebinding protection, and private-network rejection are enabled.
4. Start with `PIPELINE_DRY_RUN=true`; review provider counts, query exhaustion, exclusions, and the
   redacted digest before persistence.

## Bounded dry run

```bash
PIPELINE_DRY_RUN=true \
ICP_CATEGORY=roofing \
TARGET_EMAILS=300 \
MAX_QUERIES=100 \
MAX_PAGES_PER_QUERY=8 \
DEADLINE_SECONDS=3600 \
RDS_URL='postgresql://…' \
PG_SSL_CA_FILE=/run/secrets/rds-ca.pem \
SCRAPER_AUTH='…' \
SERPER_API_KEY='…' \
BRAVE_SEARCH_API_KEY='…' \
node orchestrate.mjs
```

Use one or both search providers. The provider adapters remain isolated; a missing credential disables
only that adapter. Do not enable direct arbitrary-domain fallback or automate CAPTCHA/access-control
bypass.

## Persistence run

After the dry-run report is reviewed, rerun with `PIPELINE_DRY_RUN=false`. The orchestrator:

- deduplicates normalized role email addresses;
- skips recently scraped/blocked domains and throttled identities;
- writes each lead and throttle record transactionally;
- stores the first verified E.164 business phone in `primary_phone` only when that field is blank;
- retains all verified phone candidates and discovery provenance in `meta_data`;
- preserves unsubscribe/do-not-contact state;
- emits only hashed contact/search identifiers in logs.

The target is a discovery count, not a send count. New leads stay in `lead_status='new'` and
`outreach_status='pending'`; discovery does not create consent.

## Downstream handoff

1. `benefactor-automations` creates a protected 200–300-contact RDS review snapshot.
2. Human review marks CRM-eligible public business contacts.
3. The automations sync idempotently upserts reviewed identities into
   `public.benefactor_marketing_contacts` with `consent_status='unknown'` and into HubSpot.
4. `benefactor-sendgrid-outreach` can see only explicitly opted-in identities and additionally requires
   a signed exact-recipient approval manifest.
5. The first live batch is capped at 20; scale requires reviewed canary evidence.
