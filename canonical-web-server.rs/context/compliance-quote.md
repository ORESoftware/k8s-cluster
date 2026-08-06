# Canonical compliance quote analysis context

This file is versioned with the application and is combined with the active
`canonical_context` row whose key is `quote-analysis`.

## Purpose

Produce a preliminary scope, timing range, and investment range for a human
Canonical reviewer. The result is not an audit opinion, certification,
attestation, legal conclusion, or final proposal.

## Framework scoping cues

- **SOC 2:** distinguish readiness, Type I, and Type II; surface observation
  period assumptions and whether security-only or additional Trust Services
  Criteria are in scope.
- **NIST CSF / NIST SP 800-53:** identify the desired profile or control
  baseline, system boundary, customer contract requirements, and assessment
  evidence expectations.
- **HIPAA:** determine whether the organization is a covered entity, business
  associate, or subcontractor; identify PHI/ePHI flows and business associate
  agreements.
- **ISO 27001:** identify ISMS scope, sites, legal entities, interested parties,
  and whether certification-stage support is requested.
- **FedRAMP:** identify target impact level, agency or marketplace path, cloud
  service offering boundary, inherited controls, and 3PAO coordination needs.
- **PCI DSS:** identify merchant/service-provider role, payment channels,
  segmentation, annual transaction volume, and expected SAQ/ROC path.

## Estimate rules

1. Prefer ranges over false precision.
2. State assumptions and missing inputs.
3. Separate readiness/remediation effort from independent assessor fees.
4. Call out accelerated timelines and multi-framework overlap.
5. Recommend a human scoping call before treating the estimate as final.
