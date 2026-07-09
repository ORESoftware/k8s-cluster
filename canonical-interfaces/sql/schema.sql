-- Canonical Postgres schema for the canonical.cloud compliance store.
--
-- Hand-authored source of truth for the relational store (mirrors the domain
-- types in schema/compliance.schema.json). Apply with your migration tool of
-- choice; keep column names in sync with the JSON Schema field names.

create table if not exists audit_engagement (
    id                  uuid primary key,
    company             text        not null,
    framework           text        not null
        check (framework in ('soc2', 'fedramp', 'hipaa', 'iso_27001', 'pci_dss', 'gdpr')),
    status              text        not null default 'scoping'
        check (status in ('scoping', 'remediation', 'in_audit', 'complete')),
    opened_at           timestamptz not null default now(),
    target_report_date  date
);

create index if not exists audit_engagement_framework_idx on audit_engagement (framework);
create index if not exists audit_engagement_status_idx    on audit_engagement (status);
