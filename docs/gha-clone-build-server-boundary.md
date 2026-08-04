# GHA clone build-server trust boundary

The independent clone-server submits only fixed, operator-reviewed profiles to `dd-build-server`. It is not a general GitHub Actions runner.

## Transport origin

`GHA_CLONE_BUILD_SERVER_URL` is an origin, not an arbitrary URL. It must contain no credentials, query, fragment, or non-root path. HTTPS is required except for loopback test servers and Kubernetes service DNS such as `service.namespace.svc` or `service.namespace.svc.cluster.local`. The HTTP exception accepts exactly those three- or five-label service forms, with DNS-safe service and namespace labels; extra-prefix lookalikes are rejected. HTTP redirects are disabled so authentication headers and immutable job identity cannot be moved to another origin.

## Build identity

Every accepted build response must contain a bounded path-safe identifier. Empty IDs and URL dot-segments (`.` and `..`) are rejected explicitly before a polling URL is formed. Poll responses must return the same identifier that was accepted at submission. Unknown, malformed, or mismatched identifiers fail the workflow run before URL construction or state mutation.

## Runtime bounds

Planner limits, polling interval, execution timeout, and retained-run capacity are strictly positive configuration values. Zero is configuration failure rather than an instruction to disable a safety bound.

This boundary complements the AWS/Hetzner executor router: provider selection happens before submission, and status remains pinned to the accepted provider and build identity.
