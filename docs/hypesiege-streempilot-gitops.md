# Hypesiege and StreemPilot GitOps architecture

Hypesiege and StreemPilot are tenants of `ORESoftware/k8s-cluster`. Their
ordinary backend and web workloads run in that cluster; the intentionally
separate Fiducia data-plane exception does not apply to either product.

## Ownership and Argo CD hierarchy

Each product has one Argo CD app-of-apps root in `remote/argocd/apps/`.
The root is sourced from this repository and manages exactly three
platform-owned files: its Namespace/quotas/default-deny policy, its strict
AppProject, and its child Applications. The children point directly at the
individual Rust service repositories and render `deploy/k8s` from those
repositories. They never render a monorepo, submodule, or `*-infra` repo.

The StreemPilot web repository was transferred from
`StreemPilot/streempilot-web-server.rs` to the canonical private repository
`StreemPilot/sp-web-mash`. Argo and AppProject source authorization use the
new repository identity. The Kubernetes Application and workload names stay
`dd-streempilot-web-server`, and the container-image promotion contract stays
separate from the Git repository name.

```text
<product>-root (k8s-cluster)
├── <product> Namespace, quota, limits, default-deny ingress
├── <product> AppProject
├── Rust API child Application  -> service repo/deploy/k8s
└── Rust web child Application  -> service repo/deploy/k8s
```

The service repositories own only namespace-scoped Deployments, Services,
HPAs, PodDisruptionBudgets, allow-list NetworkPolicies, and ExternalSecrets.
`k8s-cluster` owns all cluster-scoped infrastructure and tenancy objects.

The initial child `targetRevision` and image tags use `main` only as a
bootstrap setting. Release automation must promote reviewed commit SHAs or
tags and immutable container digests before production activation.

## Runtime boundaries

Both products use Rust web servers to serve HTML and Rust API servers to
serve JSON. Cloudflare is the public edge for DNS, TLS, WAF, caching, and
the browser-facing routes. The in-cluster `dd-remote-gateway` is the only
allowed ingress caller until a reviewed Gateway/HTTPRoute and Cloudflare
hostname are configured; these manifests intentionally contain no guessed
public hostname.

Supabase remains available for the capabilities each product has selected,
including upstream identity, storage, and realtime features. Product data
can remain in its canonical Postgres system of record. Kubernetes receives
database and Supabase credentials only through External Secrets backed by
`ClusterSecretStore/dd-cluster-secrets`; no secret value belongs in Git.

`shared-auth/shared-auth-server.rs` is the fleet authentication authority.
Its canonical Kubernetes endpoint is
`http://dd-shared-auth.shared-auth.svc.cluster.local:8120`, while tokens keep
the externally stable issuer `https://auth.oresoftware.dev`. Hypesiege uses
the shared-auth JWKS endpoint and pins issuer and audience. StreemPilot uses
the same exchange endpoint and currently consumes its rotated ES256 public
key through External Secrets; migrating StreemPilot to JWKS verification is
tracked as a follow-up hardening step.

## StreemPilot media boundary

StreemPilot's Rust API and web pods are the control plane: rooms,
invitations, permissions, metadata, destinations, and server-rendered UI.
They do not become a TURN server, SFU, recorder, compositor, or RTMP media
relay. Internal signaling uses the existing
`dd-webrtc-signaling.default.svc.cluster.local:8095` service. Browsers use a
Cloudflare-managed public WSS URL supplied as runtime configuration.
Production media workers and public UDP reachability are separate scaling
and networking concerns and must not be hidden behind the ordinary
Cloudflare HTTP proxy.

## Activation order

1. Merge the API and web service-repository PRs so `deploy/k8s` exists.
2. Publish non-root container images and pin reviewed revisions/digests.
3. Seed the required `dd/remote-dev/...` objects behind
   `dd-cluster-secrets`, including the StreemPilot browser signaling URL.
4. Add Argo repository credentials for the private service repositories,
   including `StreemPilot/sp-web-mash`.
5. Apply `hypesiege-root.application.yaml` and
   `streempilot-root.application.yaml` in the `argocd` namespace.
6. Add reviewed Gateway/HTTPRoute and Cloudflare DNS/WAF configuration only
   after the services and certificates are healthy.
