# Rust SSR protected certification runner

This directory defines an inert, review-first Actions Runner Controller (ARC)
scaffold for the protected `rust-ssr-demos/rust-ssr-e2e` certification lane in
Linear `DEN-1074` and `rust-ssr-e2e` pull request 13.

The runner registration is deliberately repository-scoped. Its
`githubConfigUrl` names only `rust-ssr-demos/rust-ssr-e2e`, and the dedicated
ARC GitHub App must be installed only on that repository with repository
**Administration: write**. It does not receive source Contents permission and
cannot read the six private comparison repositories.

The protected workflow's source-checkout App is a different credential. That
App is installed on exactly the seven reviewed Rust SSR repositories with
**Contents: read-only** and is projected only into the GitHub environment named
`private-fleet-certification`. Never reuse either App for the other's role.

## Deliberately inert state

- `rust-ssr-e2e-ci.applications.template.yaml` is not referenced by an active
  AWS or Hetzner Argo CD root.
- The prerequisite, controller, and runner-set Applications have no automated
  sync policy.
- The ExternalSecret and smoke workflow are templates excluded from the base
  Kustomization.
- The runner image reference contains `REPLACE_IMAGE_DIGEST`, so the scale set
  cannot be promoted from an unreviewed mutable tag.
- `minRunners` is zero and `maxRunners` is one. Merging this scaffold creates
  no runner, GitHub App, secret, cluster workload, or Actions spend.

## Runner boundary

The one-job ephemeral runner has no Docker or containerd socket, no hostPath,
no Kubernetes service-account token, no ingress, and no route to loopback,
link-local, carrier-grade NAT, RFC1918, or multicast networks. DNS and public
HTTPS are the only allowed egress paths. The lane therefore cannot reach
cluster-internal browser services or cloud metadata.

That is intentional: DEN-1074 clean-checkout certification, synthetic MCP
stack evidence, and protected deployed-cluster browser evidence are distinct
classes. This runner executes the first two. The existing protected
`browser-e2e` environment remains the only route for deployed Playwright,
Puppeteer, Selenium, and MCP evidence.

The runner image is built from exact Linux AMD64 manifests for GitHub Actions
Runner 2.334.0 and Rust 1.97.1. It adds checksum-pinned Chrome for Testing and
ChromeDriver 152.0.7977.64 plus the native build packages needed by the reviewed
suite. A registry digest is still mandatory because Ubuntu package resolution
happens at build time.

## Activation gates

1. Refresh the operator-owned `dd-codex` AWS profile and prove the intended
   cluster identity without printing credential values.
2. Audit existing ARC controllers and `actions.github.com` CRD ownership. Do
   not sync this controller if another release would compete for the namespace
   or CRDs.
3. Create a dedicated GitHub App installed only on
   `rust-ssr-demos/rust-ssr-e2e` with repository Administration write and no
   organization self-hosted-runner, Contents, Packages, or unrelated rights.
4. Store its three ARC fields at
   `dd/ci/github-apps/rust-ssr-e2e-arc`, independently review the
   ExternalSecret template, and intentionally materialize it. Do not use a
   personal access token.
5. Build the runner image for `linux/amd64`, scan it, generate SBOM and
   provenance, push it to the reviewed registry, and replace
   `REPLACE_IMAGE_DIGEST` with the resulting immutable SHA-256 digest.
6. Run the credential-free validator and image build. Review the complete
   rendered namespace, quota, LimitRange, NetworkPolicy, controller, and scale
   set before adding the Application file to an active cloud root.
7. Manually sync prerequisites, then the controller only if the controller/CRD
   audit permits it, then the runner set. Keep every step separately
   reversible.
8. Install the smoke template in `rust-ssr-e2e`, dispatch it manually, and
   record the exact commit, workflow run, runner identity, image digest, and
   successful isolation checks.
9. Set repository or protected-environment variable `RUST_SSR_CERT_RUNNER` to
   the JSON string `"rust-ssr-e2e-ci"`, provision the separate seven-repository
   Contents-read App in `private-fleet-certification`, and dispatch the
   protected certification workflow on the exact PR SHA.
10. Require all seven matrix jobs and bounded evidence artifacts to succeed
    before readying or merging the Rust SSR pull request.

## Rollback

Set the protected workflow back to a funded hosted label or a deliberately
nonexistent hold label before touching runner resources. Set `maxRunners` to
zero, let any active ephemeral job finish, remove the runner-set Application,
and remove its controller only if no other scale set depends on it. Preserve
workflow history and sanitized diagnostics. Revoke and rotate the ARC App key
after any suspected compromise.
