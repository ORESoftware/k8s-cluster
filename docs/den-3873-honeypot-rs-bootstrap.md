# DEN-3873 honeypot.rs repository bootstrap

This change stages a deterministic, reviewable source materializer for `ORESoftware/honeypot.rs` and an exact owner-gated one-time publication path.

The connected GitHub application can write to existing repositories but does not expose repository creation. Publication therefore remains fail-closed until this change is reviewed, merged to trusted `main`, exact-head validation passes, and the repository owner completes a fresh RSA-OAEP encrypted credential challenge.

## Required history

- `main`: minimal README, license, and ignore file only.
- `agent/DEN-3873-honeypot-foundation`: full Rust/Leptos implementation plus generated `Cargo.lock`.
- Draft pull request into `main`; implementation is never committed directly to the default branch.

## Safety boundaries

- The target is hardcoded to the public repository `ORESoftware/honeypot.rs`.
- The publisher rejects divergent existing branches and never force-pushes.
- Source is validated, formatted, linted, tested, and release-built before requesting any repository-admin credential.
- The one-time credential is encrypted to a fresh 3072-bit RSA public key, authenticated as `ORESoftware`, masked, held only in runner memory, and never committed.
- The encrypted response comment is deleted after use; only sanitized repository, commit, PR, actor, and workflow-run evidence remains.
- The honeypot contains no production credentials, Cloudflare mutation token, Kubernetes API token, persistent volume, general-purpose shell, malware execution path, or unrestricted upload endpoint.
- DDoS traffic remains at the Cloudflare edge. Only narrowly bounded impossible-path traffic may reach the origin through Cloudflare Tunnel.
- Automated response recommendations are temporary and reversible; permanent IP-only blacklists and hack-back are prohibited.

Linear tracking: DEN-3873.
