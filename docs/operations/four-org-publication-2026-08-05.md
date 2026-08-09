# Four-organization repository publication — August 5, 2026

## Scope

The reviewed fleet for the following GitHub organizations is now remote and independently verifiable:

- `apostille-me`
- `evento-globolo`
- `hacker-house-medellin`
- `embedded-alerts`

Each organization has 12 public repositories: eight Rust/API/web/CLI/sync/infra repositories plus clients, libraries, monorepo, and an Astro GitHub Pages repository. The infrastructure repository in each organization contains the reviewed Cloudflare Worker and Wrangler package.

## Execution chain

1. GitHub issue [`ORESoftware/k8s-cluster#860`](https://github.com/ORESoftware/k8s-cluster/issues/860) defined the reviewed 48-repository contract.
2. Pull request [`#899`](https://github.com/ORESoftware/k8s-cluster/pull/899) added the trusted encrypted-owner publisher and its fail-closed contract suite. It merged as `7d767095ddd7f988c2d8cb2ab2ed15689a13df3d`.
3. Draft carrier [`#902`](https://github.com/ORESoftware/k8s-cluster/pull/902) contained one five-line, non-executable marker and was closed without merge after verification.
4. Workflow run [`30970346116`](https://github.com/ORESoftware/k8s-cluster/actions/runs/30970346116) reconstructed and tested the reviewed local fleet, validated the owner and four organization-admin memberships, published the exact histories, and verified remote state.

## Verified result

| Organization | Public repositories | Representative repository | Marketing repository |
|---|---:|---|---|
| `apostille-me` | 12 | `apostille-me/apme-api` | `apostille-me/apostille-me.github.io` |
| `evento-globolo` | 12 | `evento-globolo/evgl-libs` | `evento-globolo/evento-globolo.github.io` |
| `hacker-house-medellin` | 12 | `hacker-house-medellin/hhm-sync` | `hacker-house-medellin/hacker-house-medellin.github.io` |
| `embedded-alerts` | 12 | `embedded-alerts/eal-infra` | `embedded-alerts/embedded-alerts.github.io` |

The workflow verified **48 repositories**, **20 reviewed pull requests**, **four Astro marketing sites**, and **four Cloudflare Worker packages**. The target repositories have real Git histories and `main` branches; no ZIP file or generated chat attachment is treated as completion evidence.

## Pre-publication validation

Before the owner credential was used, the trusted workflow completed:

- reconstruction of the reviewed generator from committed chunks;
- SHA-256 verification of the sealed additions overlay;
- validation of all 48 independent Git histories;
- Rust formatting, checks, and tests;
- Astro dependency installation, tests, and production builds;
- Cloudflare Worker tests and Wrangler-contract validation.

## Credential boundary

The owner credential was passed only as ciphertext for a one-time 3072-bit RSA challenge using OAEP-SHA256 and MGF1-SHA256. The plaintext value existed only in runner memory after decryption and was masked before GitHub API use. It was not committed, placed in workflow inputs or Actions outputs, uploaded as an artifact, written to Linear, or embedded in a Git remote. The ephemeral private key and temporary workspace were destroyed at workflow exit.

Because the credential was disclosed in plaintext in chat before encryption, it must still be revoked or rotated after this completed use.

## Residual work

Repository bootstrap is complete. Remaining work belongs to the individual implementation pull requests and normal per-repository CI/review lanes; those should be merged only after their exact-head checks and semantic reviews pass. Organization-level project linkage and lifecycle automation remain tracked separately.
