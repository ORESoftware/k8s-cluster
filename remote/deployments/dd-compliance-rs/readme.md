# `dd-compliance-rs`

Rust/Axum compliance readiness server for bounded audit jobs over artifacts, codebases, networks,
and systems. It maps evidence to reusable control families and scores coverage across the requested
frameworks. The result is readiness evidence, not legal advice, certification, or auditor sign-off.

## Routes

- `GET /healthz`, `GET /readyz`, `GET /metrics`
- `GET /standards`, `GET /standards/:standardId`
- `GET /controls`
- `GET /schema`, `GET /example`
- `POST /audits` queues an async audit job.
- `GET /audits`, `GET /audits/:jobId` read job state.
- `POST /audit-sync` runs a bounded synchronous audit.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json`

Non-probe audit routes require `X-Server-Auth`, `X-Compliance-Auth`, or the legacy `Auth` header
unless `COMPLIANCE_ALLOW_UNAUTHENTICATED=true`.

## Standards

The registry covers HIPAA, SOC 2, FedRAMP, NIST CSF, NIST SP 800-53, GDPR, ISO/IEC 27001,
ISO/IEC 27017, ISO/IEC 27018, ISO/IEC 27701, CIS Controls, Cyber Essentials, Essential Eight,
TISAX, CMMC, CCPA, CPRA, LGPD, PIPEDA, PDPA for Singapore and Thailand, Privacy Act 1988, UK GDPR,
Data Protection Act 2018, PCI DSS, SWIFT CSP, PSD2, SOX, GLBA, Basel III, HITECH Act, HITRUST CSF,
FDA 21 CFR Part 11, MDR, NIS2, FISMA, CJIS Security Policy, ITAR, EAR, EU AI Act, ISO/IEC 42001,
NIST AI RMF, ISO 9001, ISO 22301, ISO 31000, ISO 20000, ISO 14001, CSRD, TCFD, and GRI Standards.

## Collection Gates

Inline evidence is always supported. External artifact fetching and repository cloning are present
but fail closed unless enabled at the service level:

- `COMPLIANCE_ALLOW_EXTERNAL_FETCH=false`
- `COMPLIANCE_ALLOW_REPO_CLONE=false`
- `COMPLIANCE_ALLOW_PRIVATE_TARGETS=false`

When repo cloning is enabled, use `COMPLIANCE_ALLOWED_REPO_PREFIXES` to restrict trusted sources.
The scanner uses shallow clones, a blob-size filter, allowlisted file extensions, and byte/file
limits.
