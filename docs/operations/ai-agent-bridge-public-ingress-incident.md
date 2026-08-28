# AI Agent Bridge public-ingress incident runbook

This runbook covers `api.fiducia.cloud` when the credential-free Slack-route probe resolves DNS and verifies edge TLS but cannot reach the origin application contract.

## Credential-free evidence

Run from any trusted workstation or GitHub-hosted runner:

```bash
AI_BRIDGE_PUBLIC_EVIDENCE_PATH=/tmp/ai-agent-bridge-public-ingress.json \
  bash scripts/ci/run-ai-agent-bridge-public-ingress-diagnostic.sh
jq .diagnosis /tmp/ai-agent-bridge-public-ingress.json
```

The expected healthy boundary is:

- unsigned Slack command and interaction requests return `401`;
- a wrong method on an exact command route returns `405`;
- an unknown Slack route returns `404`;
- DNS resolves and TLS verifies without redirects; and
- no response body is retained—only size and SHA-256 metadata.

When every route returns `522` while DNS and edge TLS succeed, the classifier emits:

```text
cloudflare_origin_unreachable
```

The workflow remains failed. Diagnosis must never convert an outage into success.

## Operator-only cluster checks

Use rotated, least-privilege cluster and edge credentials from the approved secret manager. Do not use any credential pasted into chat.

1. Confirm the public DNS record still targets the intended ingress origin and has the intended proxy state.
2. Inspect ingress controller availability and external address:

   ```bash
   kubectl -n ingress-nginx get deployment,pod,service,endpoints,endpointslice -o wide
   ```

3. Inspect the exact Slack ingress, service, and endpoints:

   ```bash
   kubectl -n default get ingress dd-slack-command -o yaml
   kubectl -n default get service dd-slack-command -o yaml
   kubectl -n default get endpoints dd-slack-command -o yaml
   kubectl -n default get endpointslice \
     -l kubernetes.io/service-name=dd-slack-command -o yaml
   ```

4. Confirm the deployed Slack command pod is ready and the selector matches the service:

   ```bash
   kubectl -n default get deployment,pod \
     -l app.kubernetes.io/component=slack-command -o wide
   kubectl -n default describe deployment dd-slack-command
   ```

5. From an authorized diagnostic pod or port-forward, send an unsigned request to the service. It must reach the application and fail authentication with `401`; do not construct a valid Slack signature.
6. Probe the origin address directly with `api.fiducia.cloud` as the TLS SNI and HTTP Host, bypassing the proxy only for diagnosis. Confirm the origin certificate, ingress listener, firewall, security group, and upstream network path.
7. Review ingress-controller events and logs for backend-unavailable, timeout, endpoint, certificate, or upstream-connect failures.
8. After remediation, rerun the credential-free probe and retain its metadata-only artifact.

## Activation boundary

Incident remediation must not weaken Slack signature validation, route exactness, TLS verification, or response-size limits. Keep:

```text
SLACK_COMMAND_DRY_RUN=true
provider runner replicas=0
```

Do not add provider credentials, enable model execution, change the canary authorization boundary, or consume model credits while restoring public origin reachability.
