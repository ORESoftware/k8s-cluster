# dd-image-builder split — cutover runbook

## Goal
`dd-remote-rest-api` is the public (gateway-fronted) API **and** the on-demand
image builder, so it runs `privileged: true` + mounts the host containerd socket
(`nerdctl`/`buildctl`, Bidirectional mounts). That fuses **node-root** power into
the internet-facing surface — an API compromise = root on the node.

This split moves the build work to an **internal-only** `dd-image-builder` (same
image, keeps the privileges, **no gateway route**, NetworkPolicy-restricted), and
lets the public API run **unprivileged with no socket**, delegating builds over
HTTP. Net: the internet-facing surface is no longer node-root-capable.

## Why it must land staged (not one-shot)
- `dd-remote-rest-api` **builds from source on deploy** (`cargo run --release` in
  a `rust:1.90` pod). A delegation change that doesn't compile = public-API
  CrashLoop. So the code must be `cargo check`/CI-verified before deploy.
- The repo working tree is **auto-committed/deployed** by the sync automation, so
  every intermediate state must be safe. The delegation code is therefore
  **flag-gated (default-off)** — inert until `IMAGE_BUILD_DELEGATE_URL` is set.
- The build path is **not runtime-testable off-cluster** (needs real containerd),
  so the flag-flip + de-privilege are explicit, verified cutover steps.

## Files deployed by the additive builder stage
- `dd-image-builder.deployment.yaml` — internal builder (same image, privileged +
  socket + build mounts, `replicas: 1`, build env enabled, `IMAGE_BUILD_DELEGATE_URL` unset → builds locally).
- `dd-image-builder.service.yaml` — ClusterIP `:8082`, no gateway route.
- `dd-image-builder.networkpolicy.yaml` — ingress to `:8082` only from the REST API, runtime-config, and observability; egress is default-deny with DNS, Postgres, OTLP, runtime-config, and HTTPS allowances.

## The delegation code seam
Add one env: `IMAGE_BUILD_DELEGATE_URL` (e.g. `http://dd-image-builder.default.svc.cluster.local:8082`).
- **Unset** (builder + today's rest-api): run the local `nerdctl`/`buildctl` path — *unchanged behavior*.
- **Set** (public rest-api after cutover): forward build work to the builder instead of shelling out.

Two seams in `remote/deployments/rest-api-rs`:
1. **Container-pool builds** — `container_pool_routes.rs`: `trigger_build_test()`
   (`POST /api/container-pool/images/:slug/build-test`) → `run_build_and_test()` (the
   `nerdctl build/run` shell-out). Delegation: when the flag is set, **forward the
   trigger request** to `{DELEGATE_URL}/api/container-pool/images/{slug}/build-test`
   and return its response. Build **status is Postgres-backed** (`fetch_build_by_id`),
   so `GET /api/container-pool/builds/:id` keeps working from the public API with
   no delegation. Only the *trigger* forwards.
2. **Lambda images** — `main.rs`: `maybe_package_lambda_image()` /
   `package_lambda_image_sync()` (the `nerdctl` build), invoked **inline** during
   lambda CRUD. With delegation enabled it calls the authenticated
   `POST /internal/lambda-images/:function_id/package` builder endpoint instead.

`DD_SERVICE_ROLE=image-builder` selects a minimal router: health/readiness,
metrics, generated docs, runtime-config, and the two authenticated build triggers.
The privileged pod does not expose the general REST, GraphQL, CDC, or internal DB
surfaces and receives only the shared server-auth and Postgres secrets.

Keep the change additive and flag-gated; with the flag unset there must be **zero**
behavioral diff (so it deploys safely while you test).

## Staged cutover
1. **Deploy the builder (additive, no behavior change).** The three
   `dd-image-builder.*.yaml` resources are in `kustomization.yaml`. Confirm the
   pod builds from source and goes Ready (`/healthz`). It’s idle until step 3.
2. **Land the delegation code** (`cargo check` + CI). It is flag-gated and
   deployed with `IMAGE_BUILD_DELEGATE_URL` **unset** everywhere → inert.
3. **Flip delegation on the public API.** Set
   `IMAGE_BUILD_DELEGATE_URL=http://dd-image-builder.default.svc.cluster.local:8082`
   on `dd-remote-rest-api`. Trigger a container-pool build and a lambda build;
   confirm they execute **on the builder** (builder logs / `nerdctl -n k8s.io images`
   on the builder pod) and status/rows still update.
4. **De-privilege the public API** (`dd-remote-rest-api.deployment.yaml`): remove
   `securityContext.privileged: true`; remove the volumeMounts **and** volumes for
   `containerd-sock`, `containerd-root`, `nerdctl-bin`, `buildctl-bin`,
   `buildkit-run`, `nerdctl-state`. Keep `IMAGE_BUILD_DELEGATE_URL`. Re-verify a
   build still succeeds (now fully via the builder) and the public API is healthy.
   (Re-check whether the `repo`, `lambda-image-builds`, `container-pool-image-builds`
   mounts are still read by non-build code paths before removing them too.)
5. **Confirm** the public pod is unprivileged and socket-free:
   `kubectl exec deploy/dd-remote-rest-api -- ls /run/containerd/containerd.sock` → absent.

## Rollback
At any step: unset `IMAGE_BUILD_DELEGATE_URL` (reverts to local builds) and/or
restore `privileged` + the socket mounts on `dd-remote-rest-api`. The builder can
stay deployed (idle) safely.

## Same pattern, lower priority
`dd-container-pool`, `dd-browser-job-runner`, `dd-build-server`, `dd-idle-reaper`,
`dd-gleam-lambda-runner` also mount the containerd socket / run privileged, but
they have **no public gateway route** (internal infra), so they're lower risk than
the public API. Apply the same delegate-to-builder pattern later if desired.
