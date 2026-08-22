# Honeypot platform tenancy

This directory stages only Layer-1 platform prerequisites for the low-interaction defensive-deception service. It intentionally creates no workload, Argo `Application`, public Ingress, DNS record, or Cloudflare route.

## Activation gates

1. Create and review `ORESoftware/honeypot.rs`; generate and commit `Cargo.lock` from a connected Rust 1.97.1 environment.
2. Pass fmt, clippy with warnings denied, unit/integration tests, image build, vulnerability review, and secret scanning.
3. Publish an immutable signed image and replace the app manifest tag with a digest.
4. Provision `signing_key_b64` and `edge_proof_key_b64` at `dd/remote-dev/honeypot-secrets` through the approved secret-management path.
5. Deploy a reviewed `cloudflared` connector in the expected `cloudflare` namespace; the origin must remain ClusterIP-only and unreachable from the public Internet.
6. Configure Cloudflare DDoS managed rules, WAF/rate limits, verified-bot and owned-scanner exclusions, short-lived action state, and the signed edge-evidence contract. Never forward volumetric DDoS traffic to the pod.
7. Add a separate Argo `Application` pinned to a reviewed `v0.1.0` tag and regenerate `catalog/applications.json` in the same PR.
8. Run bounded canaries for lure retrieval, invalid/valid decoy-token use, proof tamper/replay/expiry, action-header stripping, expiry of temporary blocks, and rollback.

Longer than 24-hour blocking, account/ASN-wide action, or external abuse reporting requires human review. Hack-back and retaliatory traffic are prohibited.
