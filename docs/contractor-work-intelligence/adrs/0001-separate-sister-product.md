# ADR-0001: Keep contractor work intelligence separate from Sonus Auris

- **Status:** Accepted for incubation
- **Date:** 2026-07-31
- **Owners:** DEN-989, DEN-990, DEN-991

## Context

Sonus Auris is a user-controlled audio capture, retention, and evidence product.
The contractor concept introduces a materially different business domain:
organizations, workers, crews, customers, jobs, contracts, schedules, approvals,
rate cards, reports, change orders, invoice drafts, exports, and customer portals.
It also introduces employer/worker power dynamics and consequence-bearing records.

Putting these features directly into Sonus Auris would couple:

- release-critical recording work to contractor workflow;
- personal audio/evidence data to business accounting data;
- consumer consent expectations to employer/crew policies;
- app-store identities and marketing;
- database migrations and authorization models;
- outages, secrets, backups, and incident response;
- future pricing and go-to-market decisions.

## Decision

Build contractor work intelligence as a separate sister product.

During discovery, incubate only portable contracts, documentation, and explicitly
feature-flagged prototypes in existing Sonus repositories. After DEN-990 selects
the launch vertical and final identity, create a dedicated GitHub organization,
repositories, cloud environments, database, credentials, signing identities, and
operational ownership.

Reuse Sonus capabilities only through versioned, domain-neutral packages or APIs.
Contractor jobs, rates, reports, and invoices must not enter generic Sonus capture
libraries.

## Consequences

### Positive

- independent product and release decisions;
- clearer privacy and worker-trust boundary;
- separate tenant/accounting data and incident blast radius;
- reusable capture primitives can improve both products;
- contractor product can evolve toward field-service workflows without distorting
  Sonus Auris;
- public claims and app-store disclosures remain coherent.

### Negative

- initial extraction and packaging work;
- duplicate deployment scaffolding;
- version compatibility must be maintained;
- cross-product coordination is required for shared capabilities;
- discovery must finish before final repository bootstrap.

### Risks

- incubation code could become permanent coupling;
- teams may copy rather than extract shared code;
- empty repositories could be created before ownership is clear;
- a shared database may be proposed as a shortcut.

Mitigations are explicit versioned contracts, migration gates, no shared production
credentials, and architecture fitness tests.

## Alternatives considered

### Add a contractor mode to Sonus Auris

Rejected. The consequence, role, data, commercial, and trust domains differ too
substantially.

### Create a large repository fleet immediately

Rejected. Final name, launch scope, and boundaries are unresolved. Empty
repositories create false progress and migration burden.

### Share one production database with separate schemas

Rejected. This still couples migrations, credentials, backups, RLS mistakes,
outages, data residency, and release freezes.

### Duplicate all capture code

Rejected. It would drift on security, retention, platform behavior, and battery
optimizations. Shared capabilities should be extracted behind neutral versions.

## Compliance checks

A proposed change violates this ADR if it:

- adds contractor billing/domain tables to Sonus production storage;
- requires sister services to use Sonus production credentials;
- imports one app's internal source tree directly into the other;
- exposes contractor roles or manager controls inside generic capture packages;
- makes Sonus release availability depend on contractor reporting/billing;
- creates a final public name before DEN-990 resolves branding.

## Review trigger

Revisit only if validated user research proves that the same customer, consent,
role, release, and consequence model applies to both products. Convenience or
short-term deployment speed is not sufficient evidence.