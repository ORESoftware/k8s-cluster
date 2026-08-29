# GHA clone build-server trust boundary

The independent clone-server submits only fixed, operator-reviewed profiles to `dd-build-server`. It is not a general GitHub Actions runner.

## Transport origin

`GHA_CLONE_BUILD_SERVER_URL` is an origin, not an arbitrary URL. It must contain no credentials, query, fragment, or non-root path. HTTPS is required except for loopback test servers and Kubernetes service DNS such as `service.namespace.svc` or `service.namespace.svc.cluster.local`. The HTTP exception accepts exactly those three- or five-label service forms, with DNS-safe service and namespace labels; extra-prefix lookalikes are rejected. HTTP redirects are disabled so authentication headers and immutable job identity cannot be moved to another origin.

## Build identity

Every accepted build response must contain a bounded path-safe identifier. Empty IDs and URL dot-segments (`.` and `..`) are rejected explicitly before a polling URL is formed. Poll responses must return the same identifier that was accepted at submission. Unknown, malformed, or mismatched identifiers fail the workflow run before URL construction or state mutation.

## Runtime bounds

Planner limits, polling interval, execution timeout, and retained-run capacity are strictly positive configuration values. Zero is configuration failure rather than an instruction to disable a safety bound.

## Bounded message transport

Inbound JSON is capped before Axum allocates the complete body, with bounded headroom for JSON string escaping and the request envelope. GitHub workflow content is streamed with the configured workflow limit, and build-server submission and status bodies are streamed with a 64 KiB ceiling. Declared and chunked bodies are both rejected as soon as they cross their limit. Build URLs are constructed from parsed origin path segments rather than string concatenation.

## Authentication authority

Each API request may present exactly one authentication authority: either `X-Server-Auth` or the compatibility alias `X-GHA-Clone-Auth`. Duplicate values or simultaneous aliases are rejected so intermediaries cannot create ambiguous credential precedence.

## Activation evidence

Independent execution remains disabled and the GitOps deployment remains at zero replicas until exact-head formatting, warnings-denied Clippy, all-target tests, deployment contracts, and immutable image evidence are green together.

This boundary complements the AWS/Hetzner executor router: provider selection happens before submission, and status remains pinned to the accepted provider and build identity.
