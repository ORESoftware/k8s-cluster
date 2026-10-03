# ORES edge middleware in Kubernetes ingress and load balancers

`ORESoftware/k8s-cluster` is an execution/deployment target for shared middleware. It must not become a second source of truth for middleware behavior or rate-limit policy.

## Authorities

- `ORESoftware/ores-middleware`: portable middleware interfaces, execution-placement metadata, route-policy resolver, framework/edge adapters.
- `ores-rate-limit`: rate-limit algorithms, policy definitions, providers/stores, `.ores-rl.toml`, and generated policy artifacts.
- product `*-infra`: middleware ordering, enabled targets, route-policy bindings, environment-specific deployment.
- `ORESoftware/k8s-cluster`: reusable ingress/LB/service-mesh deployment modules and target compilers.

The consumer decides ordering. `k8s-cluster` consumes a compiled stack; it does not force every product into one global middleware order.

## Required target behavior

Ingress/LB integrations must accept canonical middleware and rate-limit policy IDs and compile them into target-specific configuration for the selected ingress implementation. Supported targets may include ingress-nginx, Envoy Gateway, HAProxy Ingress, Cilium/service-mesh policy, and an ORES Rust load balancer.

Numeric route limits must not be copied manually into deployment YAML when they are already declared by the canonical rate-limit policy. Generated manifests should retain the policy ID in labels/annotations or another low-cardinality target field so decisions correlate with ORES telemetry.

## Route-specific rate limits

Different routes require different policies. A deployment may intentionally look like:

```text
GET  /health                    -> health-read
GET  /search                    -> search-read
POST /auth/login                -> login-attempt
POST /auth/password-reset       -> auth-recovery
GET  /ledger/{ledger_id}        -> ledger-read
POST /ledger/{ledger_id}/entry  -> ledger-write
POST /jobs/{job_id}/retry       -> expensive-job-admission
```

Policy resolution is performed from method + registered route template/path + optional stable operation ID before any counter is consumed. Ingress should prefer a generated canonical route ID/template over raw-path cardinality.

A coarse edge/ingress shield and a strict application quota may use different policy IDs and different numbers. For example `/auth/login` can have an anonymous IP-prefix shield at ingress and a stricter account/principal policy at the authorization boundary. The ingress policy must never weaken an application denial.

## Fail-closed configuration behavior

If generated route-policy configuration contains two equally specific matches, an unknown policy ID, an invalid route template, or a policy requiring capabilities unavailable at the selected target, deployment generation/admission must fail. Do not choose the first match based on YAML order.

Authorization-grade policies are not moved to ingress merely because an ingress implementation can count requests. Application/authorization policy can depend on authenticated identity, tenant state, durable lockouts, or strict global coordination that is not available at a coarse ingress boundary.

## Target interface

A reusable target compiler should consume a normalized model equivalent to:

```text
EdgeStack {
  target
  middleware_order[]
  route_bindings[] {
    methods[]
    path_template?
    operation_id?
    policy_id
  }
  policy_artifacts[]
}
```

and emit the selected native representation plus a machine-readable receipt containing at least:

```text
target
source_digest
middleware_stack_digest
rate_limit_policy_digest
route_binding_digest
generated_at/build identity
```

The receipt allows CI to prove that a checked-in/generated ingress artifact corresponds to the reviewed ORES contracts.

## Consumer layout

Product-specific wiring belongs in the product's `*-infra` repository:

```text
*-infra/
  .ores-mw.toml
  .ores-rl.toml
  edge/
    middleware.rs
    cloudflare.ts
    generated/
  modules/
    kubernetes_ingress/
    load_balancer/
  environments/
    dev/
    stage/
    prod/
```

The checked-in `edge/middleware.*` composition root is analogous to Next.js `middleware.ts` / `proxy.ts`, while reusable behavior remains in ORES shared libraries.

## Rollout

New target adapters should ship disabled by default. Validate generated configuration in CI, then run audit/shadow mode where the policy permits it, then enable enforcement per application profile. Security routes that already require strict lockout semantics must not be downgraded to audit-only during migration.
