# Browser MCP production hardening — AWS EC2 and Hetzner

Date: 2026-07-25

## Outcome

The browser MCP and its Playwright worker now have a portable, image-based
deployment path for both clouds. Neither workload reads source from the shared
EC2 node checkout at runtime. The browser MCP is declared in both cluster
profiles, has rolling availability, health checks, a Prometheus readiness
signal, alerts, a narrower public rate limit, and an end-to-end verifier.

The initial ChatGPT-compatible rollout was intentionally anonymous because a
custom app cannot attach an arbitrary fixed resource-server bearer. On
2026-07-26 it was superseded by the OAuth 2.1 profile described below. The
hostname ceiling remains mandatory in every auth mode.

The change was deployed to AWS and Hetzner on 2026-07-25. Both public edges
passed the repository verifier through `initialize`, `tools/list`, a real
`browser_act` and `browser_state`, off-allowlist rejection, and session
cleanup. The target pods are image-based, Ready, and at zero restarts.

## Live state observed before the change

### AWS EC2

- `dd-browser-mcp-rs`: `1/1` Ready with a Service endpoint.
- Public TLS at `https://98.90.186.114` was valid and `/browser-mcp/healthz`
  returned 200.
- Anonymous `initialize` returned 401 because
  `BROWSER_MCP_REQUIRE_AUTH=true`.
- `GET /browser-mcp` with `Accept: text/event-stream` returned 200 JSON instead
  of the server code's intended 405. The running process was stale.
- The pod used `rust:1.90-bookworm`, mounted
  `/home/ec2-user/codes/dd/dd-next-1`, and ran `cargo run --release`. Its most
  recent restart spent 2 minutes 52 seconds compiling before it listened.
- The `dd-browser-mcp-rs` ArgoCD `Application` did not exist. The workload had
  been applied outside the authoritative cluster profile.

### Hetzner

- `dd-browser-mcp-rs` did not exist.
- `dd-web-scraper` was `0/1`, had no endpoints, and was in
  `CrashLoopBackOff`.
- The worker log ended at:

  ```text
  cd: /opt/dd-next-1/remote/deployments/web-scraper-service:
  No such file or directory
  ```

- The Deployment mounted the AWS-specific host path
  `/home/ec2-user/codes/dd/dd-next-1`.
- `https://hello.95-217-171-250.sslip.io/browser-mcp/healthz` had a valid
  certificate but returned 502 because the browser MCP upstream was absent.
- The shared `dd-next-runtime` ArgoCD application was OutOfSync/Degraded.

### Read-only cluster MCP

The read-only cluster MCP gateway and Loki integration responded, but every
Kubernetes inventory, Service, Ingress, and rollout request to
`https://kubernetes.default.svc` failed. Its Kubernetes API client or
service-account/network path needs a separate repair; it cannot currently be
the authoritative health source.

## Deployment design

### Images

`.github/workflows/browser-runtime-images.yml` builds:

- `ghcr.io/oresoftware/dd-browser-mcp-rs:dev`
- `ghcr.io/oresoftware/dd-web-scraper:dev`

It also emits immutable `sha-<commit>` tags, provenance, and SBOMs. Builds use
the pinned `remote/libs` submodule and a dedicated read-only deploy key scoped
only to `ORESoftware/k8s-libs-and-shared-defs`. Pull requests build without
pushing; pushes to `dev` or `main` publish. Production manifests pin the
successful 2026-07-26 images by immutable OCI digest instead of following the
mutable branch tags.

The browser MCP runtime is distroless and starts the already-built Rust binary
directly. The Playwright worker image starts compiled JavaScript directly.
Neither image installs packages, compiles code, clones repositories, or needs
outbound package-registry access at pod startup.

### GitOps ownership

Both authoritative cluster profiles now declare `dd-browser-mcp-rs`:

- `remote/argocd/clusters/aws/applications.yaml`
- `remote/argocd/clusters/hetzner/applications.yaml`

The application tracks `dev` and reconciles
`remote/deployments/browser-mcp-rs/k8s/ec2`. Despite the historical directory
name, those manifests no longer contain EC2 host paths or AWS-only runtime
dependencies.

The browser MCP runs two replicas with a zero-unavailable rolling update and a
PDB of one. The private browser worker remains one replica because browser
sessions are process-local and the Service does not yet provide sticky routing
or a shared session store.

## OAuth and browser controls

The public MCP endpoint now returns a deliberate 401 with RFC 9728
protected-resource discovery. Its in-process OAuth server supports dynamic
public-client registration, authorization code plus PKCE S256, audience-bound
scoped access tokens, and single-use authorization/rotating refresh grants in
Redis. Only OAuth discovery/authorization routes and `/healthz` are anonymous.

Controls now enforced:

- Startup fails in every auth mode when the allowlist is empty or malformed.
- The Rust MCP and Node worker use byte-identical hostname ceilings.
- The Rust layer always overwrites caller-supplied `owner` and
  `allowed_domains`; it no longer accepts a caller's wider list.
- The worker intersects any per-call list with its process-level ceiling.
- The Playwright context enforces that ceiling for every navigation, click
  redirect, iframe, subresource, fetch/XHR, and WebSocket—not only explicit
  model-authored `goto` actions. URL credentials and non-default ports are
  denied.
- OAuth token subjects isolate browser ownership. The trusted gateway also
  normalizes client identity for per-client throttling across direct AWS and
  ingress-proxied Hetzner traffic.
- Gateway limit: 60 requests/minute per source, burst 15, at most 10 concurrent
  connections. OAuth POSTs have a second 10 requests/minute limiter.
- Gateway request bodies are capped at 1 MiB with a 10-second body timeout.
- Access logs contain method, path, status, sizes, timing, and upstream state,
  but never query strings, authorization headers, cookies, or request bodies.
- MCP body, worker response, action-count, timeout, session, idle, and absolute
  TTL limits remain enforced.
- Only HTTPS/WSS network traffic to the approved hostname tree is accepted.
  Private, loopback, link-local, metadata, and reserved networks are blocked in
  application policy and NetworkPolicy.
- CAPTCHA, MFA, payment, signature, legal attestation, secret entry, and
  consequential submission boundaries remain in the browser worker.
- The prebuilt MCP pod has no general Internet egress; it can reach only DNS,
  the browser worker, Redis, the telemetry collector, and runtime config.

### Remaining risks

- The local authorization server authenticates one operator secret; it is not a
  multi-user identity provider or an enterprise SSO integration.
- An authorized token can spend browser/CPU capacity and interact with public
  pages in the active hostname profile.
- Tool-call audit metrics are aggregate rather than durable compliance records.
- A broad list of CFP, government, webmail, or arbitrary prospect domains would
  materially expand the endpoint's power. Do not add such a list as a permanent
  global setting.
- A managed OAuth/OIDC provider with per-user identity, revocation, and audit
  should replace the local operator-secret issuer if this becomes a shared
  production service.

### 2026-07-26 Fiducia portal profile

The operator explicitly authorized a temporary profile for the active Fiducia
credit-redemption, startup-application, and CFP workstream. The same
comma-separated value is set on the Rust MCP and Playwright worker:

```text
confluent.cloud
confluent.io
signoz.io
tailscale.com
planetscale.com
clerk.com
algolia.com
app.posthog.com
elevenlabs.io
www.together.ai
support.snyk.io
us.ovhcloud.com
www.pulumi.com
tally.so
allthingsopen.org
allthingsopen.wufoo.com
static.wufoo.com
talks.devopsdays.org
sessionize.com
events.linuxfoundation.org
cfp.awscommunitydaysoflo.com
forms.gle
docs.google.com
www.gstatic.com
ssl.gstatic.com
fonts.googleapis.com
fonts.gstatic.com
```

`confluent.cloud` is the current Confluent console hostname; the older
`app.confluent.cloud` route does not resolve. `confluent.io` admits the
vendor-owned login and static asset hosts. Root vendor entries include that
vendor's subdomains. PostHog, Together AI, Snyk, OVHcloud, Pulumi, Wufoo,
Google static-asset, and AWS CFP entries use the exact portal or asset hostname
rather than a broad provider wildcard.

The filing profile remains disabled: `irs.gov`, `sos.state.co.us`, and
`dnb.com` are absent. Webmail, cloud metadata, arbitrary prospect domains, and
generic wildcard entries are also absent. Shrink or replace the Fiducia profile
when the workstream ends; it must not become a permanent global allowlist.

The Benefactor B2B dry run should not widen this generic write-capable endpoint
to arbitrary prospect domains. It needs a separate read-only scraper profile:
Serper discovers a candidate, policy normalizes and approves the business
hostname for that run, every redirect is revalidated, only navigation/GET is
allowed, and HubSpot/Postgres remain read-only until the staged 25-record
mapping and suppression review is approved.

Current ChatGPT developer mode supports read and write MCP tools for eligible
Pro, Plus, Business, Enterprise, and Education accounts on the web:

- <https://developers.openai.com/api/docs/guides/developer-mode>

## Health and observability

- `/healthz`: process liveness.
- `/readyz`: succeeds only when `dd-web-scraper /agent/healthz` and the OAuth
  Redis state store succeed.
- `/metrics`: counters plus `dd_browser_mcp_worker_ready` and build/config info.
- Prometheus jobs scrape the MCP and worker.
- Alerts cover MCP target down, worker target down, worker-not-ready, worker
  errors, and robots-policy overrides.
- Structured stdout continues through Promtail/Loki and OTLP traces continue
  through the collector.

## Deployment verification

Completed:

- The hardening was merged with current `dev`, committed, and pushed through
  `adaeb8db`.
- The `browser runtime images` workflow published both `:dev` images with
  provenance and SBOMs. Anonymous manifest reads succeeded.
- The expired broad PAT was replaced for this workflow by a repository-scoped,
  read-only deploy key.
- The AWS and Hetzner cluster profiles were reconciled. `dd-browser-mcp-rs` is
  now an ArgoCD-owned, Synced/Healthy Application on both clusters.
- `dd-browser-mcp-rs`, `dd-web-scraper`, and `dd-remote-gateway` completed their
  rollouts on both clusters. NGINX configuration tests passed in both live
  gateway pods.
- Two terminated Hetzner pods from the obsolete hostPath ReplicaSet were
  removed after confirming that ReplicaSet had zero desired replicas.

The final public checks were:

   ```bash
   scripts/verify-browser-mcp.sh https://98.90.186.114/browser-mcp
   scripts/verify-browser-mcp.sh \
     https://hello.95-217-171-250.sslip.io/browser-mcp
   ```

The verifier checks the initial 401, RFC 9728/RFC 8414 discovery, dynamic client
registration, PKCE authorization, rotating refresh tokens, authenticated
`405`/`406` negotiation, `initialize`, `notifications/initialized`, exact
`tools/list`, a real `browser_act` navigation to a harmless form, rich
`browser_state`, a harmless field fill, the explicit submit approval stop,
denial of an off-allowlist host, and session cleanup.

The broader `dd-next-runtime` Application still reports the historical
`Degraded` aggregate health state (its transition timestamp predates this
change), although its latest sync operation succeeded and all three target
deployments are Ready. Diagnose that aggregate status separately; it is not a
browser MCP availability failure.

## ChatGPT setup

In an eligible ChatGPT workspace on the web:

1. Enable Developer mode.
2. Create a custom app with the stable public HTTPS MCP URL.
3. Choose OAuth and complete the operator-secret consent screen.
4. Scan and verify exactly `browser_act` and `browser_state`.
5. Keep write-action approvals enabled.
6. Recreate or refresh the app after tool-schema changes; ChatGPT uses a frozen
   tool snapshot after workspace approval.

A stable DNS name such as `browser-mcp.<owned-domain>` is preferable to the
bare AWS IP and the IP-derived Hetzner `sslip.io` hostname. DNS and certificate
creation remain cloud/operator actions.
