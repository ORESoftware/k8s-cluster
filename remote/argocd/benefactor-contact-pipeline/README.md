# Benefactor contact-discovery CronJob

This directory owns the GitOps deployment contract for the existing Node.js Benefactor contact orchestrator.

## Schedule and posture

- Schedule: `0 6 * * *`
- Time zone: `America/Chicago`
- Target: 250 contacts, bounded to 200–300
- Concurrency: one active batch through Kubernetes `Forbid` plus the existing Postgres advisory lock
- Deployment state: **suspended**
- Collection state: **dry run**
- HubSpot state: **dry run**
- Outreach capability: **absent**

The immutable image was produced by the successful `benefactor-contact-batch.yml` build for source commit `dc87c40176e897b1d8f93662711d081c8f0e6df2`:

```text
ghcr.io/oresoftware/benefactor-contact-orchestrator@sha256:06e93d31b6d252efb98a8a0aa81fd439ee7f6d0067db11d6a2a08d3cee7b51c5
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

## Activation sequence

1. Verify the Secret keys exist without printing their values.
2. Verify RDS TLS, active ICP/search-query rows, private scraper health, and at least one configured search provider.
3. Keep the CronJob suspended and create one manual Job from this template.
4. Require a successful aggregate `BENEFACTOR_CONTACT_BATCH_REPORT` with no contact details in logs.
5. Enable the daily schedule only in a reviewed PR by changing `suspend` to `false`; keep both dry-run flags true for the first scheduled runs.
6. Permit contact persistence only through a separate reviewed change that sets the existing collection confirmation gate.
7. Keep Gmail and SendGrid execution under DEN-833. They are intentionally absent from this CronJob.

Changing this manifest never grants marketing consent or campaign approval.
