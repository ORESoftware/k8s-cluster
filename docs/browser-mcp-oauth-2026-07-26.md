# Browser MCP OAuth 2.1 deployment

Date: 2026-07-26

## Outcome

`dd-browser-mcp-rs` now implements the OAuth path used by ChatGPT custom MCP
apps instead of depending on a user-entered static resource-server bearer or an
anonymous MCP endpoint.

The implementation provides:

- HTTP 401 for an unauthenticated MCP request, with an RFC 6750 Bearer
  challenge containing RFC 9728 `resource_metadata` and the required scopes.
- OAuth protected-resource metadata and authorization-server metadata.
- Dynamic registration for public clients with exact redirect-URI binding.
- Authorization-code flow with mandatory PKCE S256 and the RFC 8707 `resource`
  parameter.
- A human consent page protected by a separate operator secret.
- Short-lived HMAC-signed access tokens bound to the exact issuer, audience,
  client, subject, and scope set.
- Single-use authorization codes and rotating opaque refresh tokens stored as
  hashed, TTL-bound Redis keys shared by both replicas.
- HTTP 403 plus an `insufficient_scope` challenge for a valid token that lacks
  the scope required by a tool call.
- Public health and OAuth endpoints, while readiness, metrics, and runtime
  configuration remain cluster-internal.

The implementation follows the MCP 2025-11-25 authorization specification and
the OAuth specifications it profiles:

- <https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization>
- <https://www.rfc-editor.org/rfc/rfc9728>
- <https://www.rfc-editor.org/rfc/rfc8414>
- <https://www.rfc-editor.org/rfc/rfc7591>
- <https://www.rfc-editor.org/rfc/rfc8707>

OpenAI's current ChatGPT custom-app guidance supports OAuth, no authentication,
and mixed authentication. It documents OAuth discovery, DCR or CIMD,
PKCE-capable public clients, and resource-server token validation. The
"static credentials" option belongs to OAuth client configuration; the
ChatGPT custom-app flow does not document a field for injecting an arbitrary
fixed bearer into resource requests. The Responses API is a separate surface
whose remote-MCP `authorization` field accepts an OAuth access token supplied
on every API request.

- <https://developers.openai.com/plugins/build/auth>
- <https://developers.openai.com/api/docs/guides/developer-mode>
- <https://developers.openai.com/api/docs/guides/tools-connectors-mcp>

## Public resources

Both public edges are trusted OAuth issuer/resource URLs:

```text
https://98.90.186.114/browser-mcp
https://hello.95-217-171-250.sslip.io/browser-mcp
```

The service selects the issuer by the trusted gateway `Host` header. Tokens
issued for one edge are rejected at the other edge because `iss` and `aud` must
both equal the canonical MCP URL receiving the request.

For either base URL, the public paths are:

```text
POST /browser-mcp
GET  /browser-mcp/healthz
GET  /browser-mcp/.well-known/oauth-protected-resource
GET  /browser-mcp/.well-known/oauth-authorization-server
POST /browser-mcp/oauth/register
GET  /browser-mcp/oauth/authorize
POST /browser-mcp/oauth/authorize
POST /browser-mcp/oauth/token
```

The gateway also exposes the path-derived RFC discovery fallbacks:

```text
/.well-known/oauth-protected-resource/browser-mcp
/.well-known/oauth-authorization-server/browser-mcp
```

The existing Hetzner ingress and certificate terminate public TLS before
re-encrypting to `dd-remote-gateway`. AWS continues to use the gateway's
hostPort 443 and valid IP-address certificate. A second direct ingress to the
browser MCP is intentionally not created because it would bypass the gateway's
rate, connection, body-size, trusted-forwarding, and redacted-log controls.

## Scopes

```text
mcp:tools       initialize, ping, notifications/initialized, tools/list
browser:read    browser_state
browser:act     browser_act
offline_access request a rotating refresh token
```

Every OAuth authorization requires `mcp:tools`. The resource metadata advertises
the three resource scopes. Authorization-server metadata also advertises
`offline_access`, which lets ChatGPT request a refresh token without incorrectly
treating refresh as a resource-server requirement.

The browser worker still independently enforces:

- the process-level hostname ceiling;
- a caller-selected, server-defined per-workflow hostname profile that cannot
  widen the process ceiling;
- HTTPS-only navigation and redirect revalidation;
- private, link-local, loopback, metadata, and reserved-network denial;
- prompt-injection boundaries;
- per-owner/session quotas;
- CAPTCHA, MFA, payment, signature, attestation, and sensitive-field stops;
- in-memory inline uploads capped at 256 KiB, with validated filenames, MIME
  types, canonical base64, no persistence, and no file-content logging;
- a separate confirmation digest for consequential submissions.

OAuth does not widen the hostname ceiling. Caller-supplied `owner` and
`allowed_domains` values are overwritten before requests reach Playwright.

## Required secrets and state

AWS Secrets Manager entry `dd/remote-dev/browser-mcp-secrets` must contain:

```text
BROWSER_MCP_OAUTH_SIGNING_SECRET
BROWSER_MCP_OAUTH_OPERATOR_SECRET
```

The signing secret must be at least 32 bytes. The operator secret must be at
least 20 bytes. The ExternalSecret maps both into
`default/dd-browser-mcp-rs-secrets`. The historical
`BROWSER_MCP_AUTH_SECRET` may remain in Secrets Manager for a code rollback but
is not loaded or accepted by the OAuth deployment.

On 2026-07-26 both new properties were generated and stored without printing
their values. The observed lengths are 64 characters for the signing key and
44 characters for the operator secret.

Authorization codes and refresh grants use Redis database 4:

```text
redis://dd-redis-cache.default.svc.cluster.local:6379/4
```

Keys use the prefix `dd:browser-mcp:oauth:v1`. Raw authorization codes and
refresh tokens are never stored; only a truncated SHA-256 lookup key and the
grant payload are stored. `GETDEL` makes both token types single-use. Redis is
ephemeral cluster cache state, so a Redis replacement forces users to
reauthorize but does not expose or invalidate already-issued 15-minute access
tokens. This is an availability tradeoff, not an authorization bypass.

## Kubernetes and GitOps

The deployment remains a prebuilt distroless image, pinned to the OCI index
published from this branch:

```text
ghcr.io/oresoftware/dd-browser-mcp-rs@sha256:f54dd077bae876ac36b2ddd8676ce0bc8cb6f6d31df063c5c526e873048b74d7
```

It does not mount the shared EC2 checkout and does not run Cargo in the pod.
The AWS and Hetzner cluster profiles both declare the standalone ArgoCD
Application tracking `dev`. The deployment retains:

- two replicas;
- zero-unavailable rolling updates;
- liveness, readiness, and startup probes;
- resource requests and limits;
- non-root, read-only filesystem, no capabilities, and no service-account token;
- Service, NetworkPolicy, and PodDisruptionBudget;
- structured tracing/logging through the existing telemetry stack.

The browser MCP pod receives the existing
`dd.dev/redis-cache-client: "true"` label. Its NetworkPolicy permits only DNS,
the private Playwright worker, Redis, the telemetry collector, and runtime
configuration. The Redis NetworkPolicy already admits only pods with that
client label.

## Verification completed before deployment

The current live AWS and Hetzner endpoints were checked before the change and
both still returned anonymous HTTP 200 for `initialize`; this established the
no-auth baseline that the OAuth rollout must replace.

Local and live-worker verification completed:

```text
cargo fmt --package dd-browser-mcp-rs -- --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
pnpm exec tsx --test general/browser-mcp-exposure.test.ts
kubectl kustomize remote/deployments/browser-mcp-rs/k8s/ec2
kubectl --context dd-ec2-runtime apply --dry-run=server -f <rendered>
bash -n scripts/verify-browser-mcp.sh
nginx -t against the rendered, env-substituted gateway template
```

The protocol integration test returned:

```text
unauthenticated initialize       401
protected-resource metadata     200
authorization-server metadata   200
dynamic client registration     201
authorization + PKCE redirect   303
token exchange                  200
authenticated initialize        200
tools/list                      browser_act, browser_state
authenticated SSE GET           405
refresh rotation                200
old refresh-token replay        400 invalid_grant
```

A second integration run used the OAuth access token against the real
`dd-web-scraper` service through a local Kubernetes port-forward:

```text
browser_act -> https://benefactor.cc/   success
browser_state                           success
browser_act close                       success
```

No HubSpot or Benefactor Postgres reads/writes and no outreach occurred.

## Post-merge external verification

The deployment is pinned to an immutable image digest. Publishing source to
`dev` builds the image but does not move the deployment automatically: wait for
the browser-runtime image workflow, resolve the digest for the newly published
browser MCP image, commit that digest to the Deployment, and then let ArgoCD
reconcile both clusters. After both applications are Synced/Healthy, retrieve
the operator value without printing it and run the repository verifier against
each edge:

```bash
export BROWSER_MCP_OAUTH_OPERATOR_SECRET="$(
  aws secretsmanager get-secret-value \
    --profile dd-codex \
    --region us-east-1 \
    --secret-id dd/remote-dev/browser-mcp-secrets \
    --query SecretString \
    --output text |
  jq -r .BROWSER_MCP_OAUTH_OPERATOR_SECRET
)"

scripts/verify-browser-mcp.sh https://98.90.186.114/browser-mcp
scripts/verify-browser-mcp.sh \
  https://hello.95-217-171-250.sslip.io/browser-mcp

unset BROWSER_MCP_OAUTH_OPERATOR_SECRET
```

The verifier performs the complete discovery, dynamic registration, PKCE,
operator consent, token, authenticated MCP, real `browser_act`,
`browser_state`, off-allowlist rejection, and session-cleanup sequence. It
never prints access, refresh, signing, worker, or operator secrets.

## Live validation

On 2026-07-26, GitHub Actions published the fully reconciled browser control
plane and Playwright worker from `agent/browser-mcp-oauth`. The GitOps
manifests pin both runtime images by OCI index digest:

```text
dd-browser-mcp-rs: sha256:f54dd077bae876ac36b2ddd8676ce0bc8cb6f6d31df063c5c526e873048b74d7
dd-web-scraper:    sha256:fec450d14e203d7e747b9eb8046c18e48c8e617798228ac914296a994decde1f
```

The `dd-browser-mcp-rs` ArgoCD Applications reported `Synced/Healthy` on AWS and
the five-node Hetzner cluster, with two ready OAuth replicas and zero restarts
in each cluster. The Playwright worker was Ready with zero active sessions out
of a 12-session limit after verification. The full verifier passed
independently through both public load-balanced URLs. The passing sequence
covered:

- trusted public TLS and `/healthz`;
- unauthenticated MCP `401` with the RFC 9728 discovery challenge;
- protected-resource and authorization-server metadata;
- DCR, PKCE S256, operator consent, audience-bound access token, and refresh
  token issuance, rotation, and consumed-token replay rejection;
- authenticated `GET` with `Accept: text/event-stream` returning `405`;
- authenticated JSON-only `GET` returning `406`;
- `initialize`, `notifications/initialized` returning `202`, and `tools/list`
  returning exactly `browser_act` and `browser_state`;
- a harmless `browser_act` start on `https://httpbingo.org/forms/post`,
  `browser_state` returning the page, accessibility snapshot, visible text,
  forms, fields, buttons, links, validation errors, and downloads;
- typing one harmless test value, then proving an explicit `submit` stops at
  `needs_confirmation` without final submission;
- a real Chromium regression test uploading a five-byte text fixture entirely in
  memory and observing the selected filename and byte count;
- rejection of navigation to a hostname outside the deployment allowlist.
- session cleanup with zero browser sessions left behind.

The existing internal browser grid was also exercised on both clouds.
Playwright 1.56.0, Puppeteer 24.43.1, and Selenium 4.44.0 each navigated
`https://example.com` and extracted `Example Domain`. These adapters remain
ClusterIP-only; the MCP tool path continues to prefer the persistent Playwright
worker.

AWS Security Group ingress exposes only public HTTP/HTTPS plus administrator
allowlisted SSH/Kubernetes API access. The short-lived Let's Encrypt IP
certificate is managed by the enabled `dd-letsencrypt-renew.timer`; its latest
run completed successfully and its next run was scheduled. Hetzner's
`gateway-public-tls` Certificate is Ready and cert-manager has a renewal time.

## Rollback

Preferred secure rollback:

1. Make the gateway return 503 for `/browser-mcp` and its OAuth paths, leaving
   only `/browser-mcp/healthz` available.
2. Before merge, point both ArgoCD Applications back to `dev`; after merge,
   revert the browser-MCP change on `dev`.
3. Let both ArgoCD Applications reconcile the rollback.
4. Re-run the historical no-auth verifier only if the operator explicitly
   accepts restoring anonymous browser access.

Do not restore the stale `cargo run` plus hostPath deployment. Do not update the
shared node checkout; it remains used by many unrelated deployments.

Credential rollback:

- Rotating `BROWSER_MCP_OAUTH_SIGNING_SECRET` invalidates all signed client IDs
  and access tokens.
- Rotating `BROWSER_MCP_OAUTH_OPERATOR_SECRET` prevents new consent while
  leaving already-issued grants to expire normally.
- Deleting only the Redis prefix revokes authorization codes and refresh grants;
  existing access tokens expire within 15 minutes.
- Restoring a pre-OAuth image can still use the preserved historical static
  bearer, but ChatGPT custom apps will not be able to supply it.

## Remaining operator improvement

A stable owned DNS name such as `browser-mcp.example.com` remains preferable to
the bare AWS IP and IP-derived Hetzner hostname. Adding it requires DNS and
certificate ownership; it is not necessary for the OAuth protocol flow.
