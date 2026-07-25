# Patch 5 — provision the fiducia key and make coordination fail closed

**Finding (HIGH, audit F10).** `FIDUCIA_API_KEY` is `optional: true` (from a
`dataFrom: extract` bag) and `BUILD_SERVER_COORDINATION_REQUIRED=false`. If the
key is absent, `src/fiducia.rs` sends no bearer token → fiducia returns 401 →
`LockOutcome::Unavailable` → build proceeds on the local semaphore while metrics
and logs *look* coordinated. Distributed build locks + idempotency leases are
inert. Masked today by `replicas: 1` + `MAX_CONCURRENT_BUILDS=1`; goes wrong the
moment replicas increase.

## Step 1 — verify (run against the EC2 cluster, not a local kind context)

```bash
# Is the key in the AWS Secrets Manager bag the ExternalSecret extracts?
aws secretsmanager get-secret-value \
  --secret-id dd/remote-dev/build-server-secrets \
  --query SecretString --output text | jq 'keys'

# Did it materialize into the k8s Secret?
kubectl --context <ec2> -n default get secret dd-build-server-secrets \
  -o jsonpath='{.data.FIDUCIA_API_KEY}' | head -c 12 ; echo

# Is it failing open right now?
kubectl --context <ec2> -n default logs deploy/dd-build-server \
  | grep -c 'coordination unavailable'      # >0 == inert coordination
```

`kubectl get secret …/FIDUCIA_API_KEY` returning empty = unprovisioned = failing
open. (Note: the pod running at all only proves the `optional: false` keys —
AWS creds — are present; it says nothing about this optional one.)

## Step 2 — mint + place the key

1. Mint a fiducia API key scoped to **`locks:write`** plus the idempotency-lease
   scope, via fiducia-auth, for the build-server's org (see the "one org per
   service" isolation note in `docs/build-server-hardening.md` and the
   `[[build-server-integration]]` memory).
2. Put it in the AWS Secrets Manager object the ExternalSecret actually reads:
   `dd/remote-dev/build-server-secrets` (key `FIDUCIA_API_KEY`).
   - **Consolidation note:** dd-contract-service was told to place its key at
     `dd-agent-secrets/FIDUCIA_API_KEY` — a *different* bag — and
     dd-fabrication-server reads a *third*. Three consumers, three sources. Prefer
     a single `dd/remote-dev/fiducia-*` object extracted by all three
     ExternalSecrets so there is one provisioning target.
3. Confirm it lands (Step 1 again). ExternalSecret `refreshInterval` is 15m.

## Step 3 — fail closed (deployment diff)

Only after Step 1 shows the key present and a smoke test passes:

```yaml
# dd-build-server.deployment.yaml
    - name: FIDUCIA_API_KEY
      valueFrom:
        secretKeyRef:
          name: dd-build-server-secrets
          key: FIDUCIA_API_KEY
          optional: false          # was true — a missing key must now block boot
    - name: BUILD_SERVER_COORDINATION_REQUIRED
      value: 'true'                # was 'false' — Unavailable fails the build
```

## Step 4 — alert (interim + permanent)

The `lock_failures` counter is already exported to Prometheus. Add an alert:

```yaml
# a PrometheusRule near remote/argocd/observability/
- alert: DdBuildServerCoordinationFailing
  expr: increase(dd_build_server_lock_failures_total[15m]) > 0
  for: 15m
  labels: { severity: warning }
  annotations:
    summary: "dd-build-server fiducia lock acquisition is failing"
    description: "Coordination is degraded; builds may be running unserialized. Check FIDUCIA_API_KEY provisioning."
```
(Confirm the exact metric name via `GET /metrics`; the code increments a
`lock_failures` counter in `main.rs`.)

## Rollback

Set `optional: true` / `COORDINATION_REQUIRED=false` again — reverts to today's
fail-open behavior. No data migration.
