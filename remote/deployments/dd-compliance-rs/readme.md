# `dd-compliance-rs`

Rust/Axum compliance readiness server for bounded audit jobs over artifacts, codebases, networks,
and systems. It maps evidence to reusable control families and scores coverage across the requested
frameworks. The result is readiness evidence, not legal advice, certification, or auditor sign-off.

## Routes

- `GET /healthz`, `GET /readyz`, `GET /metrics`
- `GET /standards`, `GET /standards/:standardId`
- `GET /controls`
- `GET /schema`, `GET /example`
- `GET /diagrams/example`, `POST /diagrams/infrastructure`
- `GET /reports/example`, `POST /reports/system`
- `GET /vulnerability-scan/example`, `POST /vulnerability-scan`
- `GET /malware-scan/example`, `POST /malware-scan`
- `GET /dependency-audit/example`, `POST /dependency-audit`
- `GET /secret-scan/example`, `POST /secret-scan`
- `GET /fraud-detection/example`, `POST /fraud-detection`
- `GET /bot-detection/example`, `POST /bot-detection`
- `GET /login-anomaly/example`, `POST /login-anomaly`
- `POST /audits` queues an async audit job.
- `GET /audits`, `GET /audits/:jobId` read job state.
- `POST /audit-sync` runs a bounded synchronous audit.
- `GET /docs/api`, `GET /api/docs`, `GET /api/docs.json`

Non-probe audit routes require `X-Server-Auth`, `X-Compliance-Auth`, or the legacy `Auth` header
unless `COMPLIANCE_ALLOW_UNAUTHENTICATED=true`.

## Production Behavior

Async audit state is stored as durable JSON job records under `COMPLIANCE_WORK_ROOT`, not only in
memory. On restart, previously queued or running jobs are marked failed with an interruption reason
instead of remaining ambiguous or disappearing. `GET /readyz` verifies that the job store is writable,
and `/metrics` exports retained job counts by status.

The deployment runs one replica with `strategy: Recreate` because the hostPath-backed job store is a
single-writer ledger. `COMPLIANCE_MAX_CONCURRENT_JOBS` bounds worker fan-out inside that replica.
Infrastructure diagrams are generated locally as Mermaid. System reports can return
Markdown plus a valid base64-encoded PDF, and vulnerability scans run bounded static checks over
submitted IaC, Kubernetes YAML, dependency/config text, scanner exports, or operator evidence.

## Detection & Scanning

Beyond compliance scoring and vulnerability scanning, the server runs additional bounded,
self-contained analyzers. All of them operate only over caller-submitted evidence and never consult
external threat, reputation, or geolocation feeds — callers supply any reference data (indicators,
advisories, blocklists). The output is readiness evidence, not a substitute for dedicated AV, SCA,
secret-scanning, anti-fraud, bot-management, or identity-protection products.

- **Malware scanning** (`POST /malware-scan`): heuristic indicator scan for download-and-execute,
  reverse shells, obfuscated/encoded execution, web shells, cryptominer markers, persistence
  mechanisms, the EICAR test signature, and caller-supplied indicators of compromise.
- **Dependency auditing** (`POST /dependency-audit`): parses manifests (`package.json`, `Cargo.toml`,
  `requirements.txt`, `go.mod`, etc.), flags unpinned/`latest` specifiers, VCS- and plaintext-HTTP
  sources, missing lockfiles, a small set of well-known compromised packages, and any
  caller-supplied advisories matched by name and version.
- **Secret leak detection** (`POST /secret-scan`): prefix and keyword detectors for AWS, GitHub,
  Slack, Stripe, and Google credentials, PEM private-key blocks, JWTs, inline credential
  assignments, and URL-embedded credentials, with redacted output so reports can be retained safely.
- **Fraud detection** (`POST /fraud-detection`): deterministic rule scoring over transaction records
  (high-value, new-account, geo/BIN mismatch, disposable email, velocity, blocklist, chargebacks).
- **Bot detection** (`POST /bot-detection`): scores request records on User-Agent, request rate,
  datacenter/proxy origin, missing headers, and honeypot hits.
- **Login anomaly detection** (`POST /login-anomaly`): per-user impossible-travel (haversine over
  coordinates, country-change fallback), new device/geo, brute-force and credential-stuffing
  velocity, and missing-MFA scoring.

The scanner and behavioral routes are stateless and synchronous; the durable job ledger is used only
by the async `/audits` flow.

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
- `COMPLIANCE_MAX_CONCURRENT_JOBS=2`

When repo cloning is enabled, use `COMPLIANCE_ALLOWED_REPO_PREFIXES` to restrict trusted sources.
The scanner uses shallow clones, a blob-size filter, allowlisted file extensions, and byte/file
limits.
