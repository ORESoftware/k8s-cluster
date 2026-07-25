# Browser MCP production hardening — AWS EC2 and Hetzner

Date: 2026-07-25

## Outcome

The browser MCP and its Playwright worker now have a portable, image-based
deployment path for both clouds. Neither workload reads source from the shared
EC2 node checkout at runtime. The browser MCP is declared in both cluster
profiles, has rolling availability, health checks, a Prometheus readiness
signal, alerts, a narrower public rate limit, and an end-to-end verifier.

The public ChatGPT mode remains intentionally anonymous because ChatGPT custom
MCP apps do not accept the existing static operator bearer. Anonymous mode now
fails closed unless a non-empty hostname allowlist is configured. The initial
production ceiling is only `benefactor.cc` and its subdomains.

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
the pinned `remote/libs` submodule and the existing `REMOTE_DEV_GH_PAT`
repository secret. Pull requests build without pushing; pushes to `dev` or
`main` publish.

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

## Anonymous endpoint controls

The current no-auth posture exists only for ChatGPT custom-app compatibility.
It is not equivalent to authorization.

Controls now enforced:

- Startup fails when anonymous mode has an empty or malformed allowlist.
- Both the Rust MCP and Node worker have the same `benefactor.cc` ceiling.
- The Rust layer always overwrites caller-supplied `owner` and
  `allowed_domains`; it no longer accepts a caller's wider list.
- The worker intersects any per-call list with its process-level ceiling.
- The Playwright context enforces that ceiling for every navigation, click
  redirect, iframe, subresource, fetch/XHR, and WebSocket—not only explicit
  model-authored `goto` actions. URL credentials and non-default ports are
  denied.
- Anonymous session ownership is derived from the last
  `X-Forwarded-For` address appended by the trusted gateway. Caller-supplied
  bearer text cannot bypass per-owner quotas.
- Gateway limit: 60 requests/minute per source, burst 15, at most 10 concurrent
  connections.
- MCP body, worker response, action-count, timeout, session, idle, and absolute
  TTL limits remain enforced.
- Only HTTPS/WSS network traffic to the approved hostname tree is accepted.
  Private, loopback, link-local, metadata, and reserved networks are blocked in
  application policy and NetworkPolicy.
- CAPTCHA, MFA, payment, signature, legal attestation, secret entry, and
  consequential submission boundaries remain in the browser worker.
- The prebuilt MCP pod has no general Internet egress; it can reach only DNS,
  the browser worker, the telemetry collector, and runtime config.

### Remaining risks

- Anonymous users are not strongly identified. Several users behind one OpenAI
  egress IP may share quotas and an owner namespace.
- An attacker can still spend browser/CPU capacity and interact with public
  pages on the allowed Benefactor domain.
- IP rate limits are not a substitute for user authorization and are weak
  against distributed traffic.
- Tool-call audit metrics are aggregate, not durable per-user authorization
  records.
- A broad list of CFP, government, webmail, or arbitrary prospect domains would
  materially expand the anonymous endpoint's power. Do not add such a list as a
  permanent global setting.
- The correct long-term control is OAuth/OIDC with user identity and scopes,
  or OpenAI's secure MCP tunnel/private-network route. After OAuth, separate
  `browser:observe`, `browser:act`, and `browser:approve` scopes and retain the
  server-side domain ceiling.

### Future task-scoped profiles

The related workstreams identified these exact hosts, but they are
intentionally **not** in the anonymous production ceiling. Enable only one
reviewed profile for one task window after OAuth or another strong
authorization boundary exists:

- Colorado registration: `coloradosos.gov`, `www.coloradosos.gov`,
  `sos.state.co.us`, and `www.sos.state.co.us`.
- EIN preparation: `irs.gov`, `www.irs.gov`, and `sa.www4.irs.gov`.
- Immediate CFP/application work: `allthingsopen.org`, `www.pulumi.com`,
  `talks.devopsdays.org`, `forms.gle`, `docs.google.com`, `sessionize.com`, and
  `tally.so`. Resolve embedded Wufoo and other redirect hosts first and add the
  exact observed hostname, not a provider wildcard.
- Approved-program portal actions: `app.confluent.cloud`, `signoz.io`,
  `tailscale.com`, `www.tailscale.com`, `planetscale.com`,
  `support.planetscale.com`, and `cfp.awscommunitydaysoflo.com`.

The Benefactor B2B dry run should not widen this generic write-capable endpoint
to arbitrary prospect domains. It needs a separate read-only scraper profile:
Serper discovers a candidate, policy normalizes and approves the business
hostname for that run, every redirect is revalidated, only navigation/GET is
allowed, and HubSpot/Postgres remain read-only until the staged 25-record
mapping and suppression review is approved.

ChatGPT full MCP write actions currently require Business, Enterprise, or Edu
on the web. Pro supports only read/fetch custom-MCP access. Search/fetch tools
are no longer mandatory for a custom app, although company knowledge uses only
search/fetch capabilities:

- <https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt>

## Health and observability

- `/healthz`: process liveness.
- `/readyz`: succeeds only when `dd-web-scraper /agent/healthz` succeeds.
- `/metrics`: counters plus `dd_browser_mcp_worker_ready` and build/config info.
- Prometheus jobs scrape the MCP and worker.
- Alerts cover MCP target down, worker target down, worker-not-ready, worker
  errors, and robots-policy overrides.
- Structured stdout continues through Promtail/Loki and OTLP traces continue
  through the collector.

## Deploy

1. Resolve the existing superproject merge conflict without discarding either
   side, then merge these changes to `dev`.
2. Confirm the `browser runtime images` workflow publishes both `:dev` images
   and that the packages are readable by both clusters.
3. Reconcile the cluster profiles so the new ArgoCD Application exists:

   ```bash
   kubectl --context dd-ec2-runtime apply -k remote/argocd/clusters/aws
   ssh hetzner-k8s-bastion \
     'kubectl apply -k /path/to/k8s-cluster/remote/argocd/clusters/hetzner'
   ```

   Use the actual GitOps bootstrap path on the Hetzner control plane; do not
   update the shared node checkout merely to restart one service.

4. Wait for `dd-web-scraper`, `dd-browser-mcp-rs`, and the gateway rollout.
5. Run the verifier against both public edges:

   ```bash
   scripts/verify-browser-mcp.sh https://98.90.186.114/browser-mcp
   scripts/verify-browser-mcp.sh \
     https://hello.95-217-171-250.sslip.io/browser-mcp
   ```

The verifier checks health, correct SSE refusal, `initialize`,
`notifications/initialized`, exact `tools/list`, a real `browser_act` navigation
to Benefactor, `browser_observe`, denial of an off-allowlist host, and session
cleanup.

## ChatGPT setup

In an eligible ChatGPT workspace on the web:

1. Enable Developer mode.
2. Create a custom app with the stable public HTTPS MCP URL.
3. Choose no authentication only while this narrow anonymous configuration is
   in force.
4. Scan and verify exactly `browser_act` and `browser_observe`.
5. Keep write-action approvals enabled.
6. Recreate or refresh the app after tool-schema changes; ChatGPT uses a frozen
   tool snapshot after workspace approval.

A stable DNS name such as `browser-mcp.<owned-domain>` is preferable to the
bare AWS IP and the IP-derived Hetzner `sslip.io` hostname. DNS and certificate
creation remain cloud/operator actions.
