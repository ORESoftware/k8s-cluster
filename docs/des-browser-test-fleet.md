# DES browser test fleet

The `discrete-event-systems-test` organization owns black-box and cross-repository browser verification for the canonical `discrete-event-systems/des-web.rs` server.

## Repository ownership

| Repository | Primary responsibility |
| --- | --- |
| `discrete-event-systems-test/des-web-playwright-e2e` | Canonical `/des` route families, route catalog, mounted-prefix navigation, health/readiness, htmx, hardening headers, bounded errors, traces, videos, screenshots, and HTML/JUnit evidence |
| `discrete-event-systems-test/des-web-puppeteer-e2e` | Independent Chromium navigation, routing dashboard behavior, gateway and catalog contracts, hardening headers, screenshot/JUnit evidence, and the `dd-des-simulator:8099` compatibility-Service comparison when cluster networking is available |
| `discrete-event-systems-test/.github` | Organization profile, ownership boundary, contribution policy, CODEOWNERS, and browser-regression intake |

The application remains owned by `discrete-event-systems/des-web.rs`. Kubernetes Services, NetworkPolicy, Argo CD, the public gateway, and the independent executor remain owned by `ORESoftware/k8s-cluster`.

## Immutable application fixture

GitHub-hosted runners must not receive the production repository's private submodule key or private GHCR package access. The application repository therefore publishes a public deterministic fixture at release tag `des-browser-fixture-77741ec8`.

The fixture is built through the production Dockerfile from source revision:

```text
77741ec8b5331617f71416748ef5f06846e43a5d
```

It records the deployed image digest:

```text
sha256:c3b32a5ef767bcdba515c8199fce363871ba2916e4c824609a09a37b3adc02e5
```

The public archive is pinned by both test repositories to SHA-256:

```text
1d8fe97fc285055558fd2e723789a82118d998a595b57a6e8581562bfd18befa
```

Each suite verifies the archive and its provenance document before execution. Browser evidence never stores operator credentials, cookies, authorization headers, or private source material.

## GitHub Actions lane

Both repositories run on pull requests and merged `main` revisions. They install locked dependencies, run a real Chromium browser against the verified fixture, and retain evidence for 14 days.

The current merged suite revisions are:

```text
discrete-event-systems-test/des-web-playwright-e2e@1e1116ef6811c4e3e6be34ad3e1def39bc20ef59
discrete-event-systems-test/des-web-puppeteer-e2e@0547548429d937023a124de37afca7659a85c3dd
```

## `gha-indie-worker` lane

Each repository also contains a deliberately bounded workflow under `.gha/workflows/`. The workflows use only pinned checkout/setup actions and the fixed reviewed Playwright or Puppeteer command.

`dd-build-server` enforces all of the following before execution:

- an exact 40-hex commit revision;
- an exact allowlist containing only the two DES browser repositories;
- the fail-closed static workflow planner;
- one supported job using the reviewed `playwright` or `puppeteer` profile;
- bounded YAML, job, step, run-count, and execution-time limits.

`.github/workflows/ops-verify-des-indie-plans.yml` runs from trusted `k8s-cluster/main`, reaches the protected cluster through AWS OIDC and SSM, checksum-verifies `scripts/ops/run_des_indie_browser_workflows.sh` from the exact GitOps SHA, then plans and executes both immutable suite revisions. It uploads the SSM plan/run evidence for 30 days.

The independent worker is intentionally isolated from the DES gateway-only NetworkPolicy. Each suite first probes deployed targets and then uses the same verified fixture when its sandbox cannot reach them. This preserves production ingress isolation rather than granting a broad worker exception.

## Gateway and cluster targets

Manual or separately authorized runs resolve targets in this order:

1. explicit `DES_BASE_URL`;
2. canonical Service `dd-des-web.default.svc.cluster.local:8130`;
3. compatibility Service `dd-des-simulator.default.svc.cluster.local:8099`;
4. configured public gateway;
5. checksum-pinned fixture.

Service-local requests use local application paths and `X-Forwarded-Prefix: /des`; public requests use canonical `/des/*` paths. The test repositories therefore verify the application/gateway ownership boundary rather than duplicating the gateway route table inside the Rust server.

## Tracking

- Production GitHub Project: `https://github.com/orgs/discrete-event-systems/projects/2`
- Test GitHub Project: `https://github.com/orgs/discrete-event-systems-test/projects/1`
- Linear project: `github.com/discrete-event-systems-test`
- Linear delivery issue: `DEN-2444`

A route, gateway, fixture, compatibility-Service, or executor-policy change must update the owning browser contract and tracking item in the same rollout.
