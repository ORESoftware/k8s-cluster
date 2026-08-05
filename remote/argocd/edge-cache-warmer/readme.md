# Edge cache warmer GitOps bootstrap

This directory is the cluster-owned bootstrap for the centralized Cloudflare edge cache warmer tracked by:

- portfolio epic: [`ORESoftware/.github#1`](https://github.com/ORESoftware/.github/issues/1)
- implementation issue: [`ORESoftware/k8s-cluster#961`](https://github.com/ORESoftware/k8s-cluster/issues/961)
- Linear mirror: `DEN-2156`
- architecture and operating policy: [`ORESoftware/.github/docs/cloudflare-edge-cache-warming.md`](https://github.com/ORESoftware/.github/blob/main/docs/cloudflare-edge-cache-warming.md)

## Current state: intentionally inert

Argo CD may reconcile these objects, but the workload cannot warm anything:

1. the `CronJob` has `spec.suspend: true`;
2. `EDGE_CACHE_WARMER_GLOBAL_PAUSE` is `true`;
3. the portfolio manifest has `global_pause: true` and no domains;
4. the container is a fail-closed bootstrap command, not the production worker;
5. no provider or Cloudflare credentials are referenced.

The placeholder job validates that configuration is mounted, emits one structured stdout event, and exits without making DNS or HTTP requests. Do not describe this scaffold as an active cache warmer.

## Activation gates

Activation requires a reviewed pull request that completes every gate:

1. Record the provider benchmark and decision in the portfolio epic. Bright Data is a candidate, not a fixed dependency.
2. Publish the reviewed worker image and pin its immutable digest.
3. Add the selected provider credential through External Secrets or the cluster's approved secret-delivery path.
4. Add only approved pilot apex domains to `targets.yaml`.
5. Keep `api`, `app`, and `www` in the non-overridable denied-label policy.
6. Pass unit, property, integration, end-to-end, SSRF, redirect, and budget tests.
7. Verify Cloudflare proxy/cache eligibility and safe HTML Cache Rules for each pilot domain.
8. Review request, URL, redirect, retry, byte, response-size, duration, concurrency, and spend caps.
9. Change all three gates together: production image, `global_pause: false`, and `spec.suspend: false`.

A partial activation must fail closed.

## Schedule

The bootstrap declares a conservative weekly slot at `07:15 UTC` on Sunday. The production cadence must be derived from measured TTLs, traffic, deployments, purges, and provider cost. Do not replace this with an unbounded or frequent fleet crawl.

## Network policy

The pod has no ingress. Egress is limited to cluster DNS and public IPv4 HTTPS, with private, loopback, link-local, carrier-grade NAT, benchmarking, documentation, multicast, and reserved ranges excluded.

The worker must independently validate the resolved IP before every connection and after every redirect. NetworkPolicy is defense in depth and is not a substitute for DNS-rebinding-safe application checks. IPv6 remains disabled until an equivalent public-only policy and application validation are reviewed.

## Manual runs

Do not create a manual Job while this bootstrap image is installed; it only proves the fail-closed state. After activation, create a bounded one-domain Job from the CronJob template through a reviewed operator workflow, retain its machine-readable report, and verify `CF-Cache-Status`, `Age`, and the observed `CF-Ray` colo.

## Rollback

Set `spec.suspend: true` and all global pause controls to `true` in Git, then let Argo CD reconcile. Credential revocation is a separate defense-in-depth action and must not replace the GitOps pause.
