# Benefactor 200–300 contact acquisition and CRM handoff

This runbook extends the hardened `DEN-260` discovery boundary without turning public contact discovery into permission to market. The target batch size is 250, bounded to 200–300. New records are written to `benefactor.benefactor_leads`, tagged with one `contactBatchId`, and synchronized to HubSpot as ordinary CRM contacts/companies. Every new record remains `marketingConsent=unknown` and `outreachApproval=required` until an existing, active `public.benefactor_marketing_contacts` row says `consent_status='opted_in'`.

## Pipeline

1. `contact-batch.mjs` reads active categories from `benefactor.benefactor_scrape_queries`; no query strings are hard-coded in the image.
2. It rotates across categories and invokes the existing `orchestrate.mjs` with bounded per-category targets until 250 newly inserted leads are tagged or all planned work is exhausted.
3. `orchestrate.mjs` uses the independent Brave and Serper adapters, then retrieves candidate business pages only through the private `dd-web-scraper` service. Cheerio is attempted first and Playwright is the bounded fallback.
4. RDS remains the source of truth for provenance, duplicate/suppression state, scrape throttles, email addresses, and phone candidates.
5. `hubspot-sync.mjs` searches then creates/updates standard HubSpot company/contact properties. It records only HubSpot IDs and bounded status codes back into `meta_data.hubspotSync`.
6. Neither process writes `benefactor_marketing_contacts`, changes consent, claims outreach throttles, or sends Gmail/SendGrid messages.

## Dry run

The committed CronJob is suspended and has both dry-run switches enabled. Run it manually after the secret named `benefactor-contact-pipeline` is present:

```bash
kubectl -n default create job benefactor-contact-batch-dry-run \
  --from=cronjob/benefactor-contact-batch
kubectl -n default logs -f job/benefactor-contact-batch-dry-run
```

Look for `BENEFACTOR_PIPELINE_REPORT`, `HUBSPOT_SYNC_REPORT`, and the final `BENEFACTOR_CONTACT_BATCH_REPORT`. Reports contain aggregate values and hashes rather than raw contact identifiers.

## Live collection and HubSpot sync

Patch a one-off Job rather than enabling the recurring schedule first. Required switches are intentionally separate:

```text
BATCH_DRY_RUN=false
BATCH_PERSIST_CONFIRM=collect-benefactor-contact-batch
HUBSPOT_DRY_RUN=false
HUBSPOT_WRITE_CONFIRM=sync-benefactor-contact-batch
```

The HubSpot token needs only the CRM read/write scopes for contacts and companies. No marketing-contact scope is used. Keep the Kubernetes Secret outside git and rotate any credential that has appeared in chat, shell history, or CI logs.

## Reconciliation queries

Replace the example batch ID with the ID from the final report.

```sql
SELECT
  COUNT(*) AS contacts,
  COUNT(*) FILTER (WHERE jsonb_array_length(COALESCE(meta_data->'phones', '[]'::jsonb)) > 0) AS with_phone,
  COUNT(*) FILTER (WHERE meta_data->'hubspotSync'->>'status' = 'synced') AS hubspot_synced
FROM benefactor.benefactor_leads
WHERE is_soft_deleted = false
  AND meta_data->>'contactBatchId' = 'benefactor-YYYYMMDDTHHMMSSZ-1234abcd';
```

The count must be between 200 and 300 before the batch is treated as complete. A lower count exits with status 2; a count above 300 exits with status 3.

```sql
SELECT COUNT(DISTINCT LOWER(bl.primary_email)) AS opted_in_and_active
FROM benefactor.benefactor_leads bl
JOIN public.benefactor_marketing_contacts mc
  ON LOWER(mc.email) = LOWER(bl.primary_email)
WHERE bl.is_soft_deleted = false
  AND bl.meta_data->>'contactBatchId' = 'benefactor-YYYYMMDDTHHMMSSZ-1234abcd'
  AND mc.status = 'active'
  AND mc.consent_status = 'opted_in';
```

Only this second count is eligible for an outbound dry run. CRM presence, a public business email, or a phone number is not sufficient authorization.

## Outreach handoff

Use `benefactor-cc/benefactor-sendgrid-outreach` as the authoritative sender. Its SendGrid lane already requires active opt-in records, DNS readiness, suppression checks, rolling caps, advisory locks, reminders, and exact live confirmation. The Gmail lane added alongside this change uses the same reminder type and throttle table, so a recipient claimed by one transport cannot be claimed by the other.

Run both providers in dry-run mode, review a canary of 10–20 approved recipients, and use one provider per campaign batch. Do not split the same recipient set across Gmail and SendGrid merely to increase volume.
