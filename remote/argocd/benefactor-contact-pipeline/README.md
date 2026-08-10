# Benefactor contact-discovery CronJob

This directory owns the GitOps deployment contract for the existing Node.js Benefactor contact orchestrator.

## Schedule and posture

- Schedule: `0 6 * * *`
- Time zone: `America/Chicago`
- Target: 250 contacts, bounded to 200–300
- Concurrency: one active batch through Kubernetes `Forbid` plus the existing Postgres advisory lock
- Deployment state: **active scheduled dry run**
- Collection state: **dry run**
- HubSpot state: **dry run**
- Outreach capability: **absent**

The immutable image was produced by the successful `benefactor-contact-batch.yml` build for source commit `5212868d77616d0cc3661dc27b9e5ada6ab48a26`:

```text
ghcr.io/oresoftware/benefactor-contact-orchestrator@sha256:17ba7b0dd64aad31e9ab9486d73897481d861c72123f87ad3f42f7ae0a8f34eb
```

The pipeline reads active ICP categories and search-query rows from Postgres, uses the configured Brave/Serper providers, and sends arbitrary-domain retrieval only through the private `dd-web-scraper` service. That service uses static extraction first and Playwright when browser rendering is required. Puppeteer and Selenium remain compatibility/e2e lanes in the shared browser infrastructure; this production discovery contract does not switch engines to bypass source restrictions.

## Required secret

A Secret named `benefactor-contact-pipeline` must provide:

- `RDS_URL`
- `RDS_CA_PEM`
- `SCRAPER_AUTH`

It may additionally provide:

- `SERPER_API_KEY`
- `BRAVE_SEARCH_API_KEY`
- `HUBSPOT_ACCESS_TOKEN`

Do not commit secret values. The long-lived database identity must be non-owner and non-`BYPASSRLS`.

## Activation evidence

The trusted synthetic scraper-readiness rerun in GitHub Actions run `31397763253`, attempt 2, passed after the GitOps-tracked `dev` NetworkPolicy was reconciled. The probe extracted the controlled email and phone fixture, reported no outbound transport, and remained `safeToMutate: false`.

The earlier protected full-batch dry run visited 100 pages but collected zero contacts. That proved the execution and privacy boundaries, not production lead volume. Enabling this CronJob therefore creates recurring aggregate diagnostic evidence; it does not authorize contact persistence, HubSpot mutation, or live outreach.

## Active dry-run sequence

1. Review every aggregate `BENEFACTOR_CONTACT_BATCH_REPORT` for provider coverage, pages visited, contact counts, suppression/deduplication behavior, and bounded failure classes.
2. Keep `BATCH_DRY_RUN=true` and `HUBSPOT_DRY_RUN=true` until the target/minimum-volume gates and provider reliability are demonstrated across scheduled runs.
3. Permit contact persistence only through a separate reviewed change that sets the existing collection confirmation gate.
4. Permit HubSpot mutation only through a separate reviewed change with deduplication and suppression evidence.
5. Keep Gmail and SendGrid execution under DEN-833. They are intentionally absent from this CronJob and require consent plus exact-recipient approval.

Changing this manifest never grants marketing consent or campaign approval.
