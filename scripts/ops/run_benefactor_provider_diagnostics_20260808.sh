#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:-}"
orchestrator_sha="${2:-}"
orchestrator_digest="${3:-}"
if [[ ! "$trusted_sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo 'trusted revision must be a 40-character lowercase commit SHA' >&2
  exit 64
fi
if [[ ! "$orchestrator_sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo 'orchestrator image revision must be a 40-character lowercase commit SHA' >&2
  exit 64
fi
if [[ ! "$orchestrator_digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo 'orchestrator image digest must be a sha256 digest' >&2
  exit 64
fi

KUBECTL=/usr/bin/kubectl
KUBECONFIG_PATH=/home/ec2-user/.kube/config
namespace=default
short_sha="${trusted_sha:0:8}"
image="ghcr.io/oresoftware/benefactor-contact-orchestrator@${orchestrator_digest}"
job="benefactor-provider-probe-${short_sha}-$(date -u +%H%M%S)"
manifest="$(mktemp /tmp/benefactor-provider-probe-manifest.XXXXXX.yaml)"
log_file="$(mktemp /tmp/benefactor-provider-probe-log.XXXXXX)"
job_created=false
failure_reported=false
stage=cluster_preflight

cleanup() {
  local status=$?
  if (( status != 0 )) && [[ "$failure_reported" != true ]]; then
    printf 'BENEFACTOR_PROVIDER_PROBE_FAILED stage=%s status=script_error exit_code=%s trusted_k8s_cluster_sha=%s\n' \
      "$stage" "$status" "$trusted_sha" >&2
  fi
  if [[ "$job_created" == true ]]; then
    "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" delete job "$job" \
      --ignore-not-found=true --wait=false >/dev/null 2>&1 || true
  fi
  rm -f "$manifest" "$log_file"
  exit "$status"
}
trap cleanup EXIT

secret_has_key() {
  local secret_name=$1
  local key_name=$2
  "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get secret "$secret_name" -o json \
    | python3 -c 'import json,sys; key=sys.argv[1]; obj=json.load(sys.stdin); raise SystemExit(0 if key in (obj.get("data") or {}) else 1)' "$key_name"
}

"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" --request-timeout=20s get --raw=/readyz >/dev/null
"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get service dd-web-scraper >/dev/null
scraper_endpoints="$(
  "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get endpoints dd-web-scraper \
    -o jsonpath='{.subsets[*].addresses[*].ip}' 2>/dev/null || true
)"
test -n "$scraper_endpoints"

stage=secret_preflight
"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get secret benefactor-ghcr >/dev/null
secret_has_key dd-remote-rest-api-secrets AGENT_TASKS_RDS_DATABASE_URL
secret_has_key dd-agent-secrets SERVER_AUTH_SECRET

provider_secret="$(
  "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get secrets -o json \
    | python3 -c '
import json, sys
items = json.load(sys.stdin).get("items", [])
candidates = []
for item in items:
    keys = item.get("data") or {}
    if "SERPER_API_KEY" in keys or "BRAVE_SEARCH_API_KEY" in keys:
        candidates.append(item.get("metadata", {}).get("name", ""))
priority = {"dd-agent-secrets": 0, "benefactor-contact-pipeline": 1}
candidates = sorted({name for name in candidates if name}, key=lambda name: (priority.get(name, 100), name))
print(candidates[0] if candidates else "")
'
)"
if [[ ! "$provider_secret" =~ ^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$ ]]; then
  echo 'no Kubernetes Secret with a supported search-provider credential is available' >&2
  exit 69
fi

stage=manifest_apply
cat > "$manifest" <<YAML
apiVersion: batch/v1
kind: Job
metadata:
  name: ${job}
  namespace: ${namespace}
  labels:
    app.kubernetes.io/name: benefactor-provider-probe
    app.kubernetes.io/component: credentialed-dry-run
    app.kubernetes.io/part-of: benefactor-cc
  annotations:
    benefactor.cc/trusted-k8s-cluster-sha: ${trusted_sha}
    benefactor.cc/orchestrator-image-sha: ${orchestrator_sha}
    benefactor.cc/orchestrator-image-digest: ${orchestrator_digest}
spec:
  backoffLimit: 0
  activeDeadlineSeconds: 600
  ttlSecondsAfterFinished: 600
  template:
    metadata:
      labels:
        app.kubernetes.io/name: benefactor-provider-probe
        app.kubernetes.io/component: credentialed-dry-run
        app.kubernetes.io/part-of: benefactor-cc
    spec:
      automountServiceAccountToken: false
      enableServiceLinks: false
      restartPolicy: Never
      imagePullSecrets:
        - name: benefactor-ghcr
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        runAsGroup: 1000
        fsGroup: 1000
        seccompProfile:
          type: RuntimeDefault
      initContainers:
        - name: fetch-rds-ca
          image: ${image}
          imagePullPolicy: Always
          command:
            - node
            - --input-type=module
            - -e
          args:
            - |
              import { writeFileSync } from 'node:fs';
              const response = await fetch('https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem', {
                redirect: 'error',
                signal: AbortSignal.timeout(30000),
                headers: { 'User-Agent': 'benefactor-provider-probe/2' },
              });
              if (!response.ok) throw new Error('rds_ca_http_' + response.status);
              const declared = Number(response.headers.get('content-length') || 0);
              if (declared && declared > 2000000) throw new Error('rds_ca_declared_too_large');
              const body = Buffer.from(await response.arrayBuffer());
              if (body.length < 1000 || body.length > 2000000) throw new Error('rds_ca_size_invalid');
              const text = body.toString('utf8');
              if (!text.includes('-----BEGIN CERTIFICATE-----') || !text.includes('-----END CERTIFICATE-----')) {
                throw new Error('rds_ca_pem_invalid');
              }
              writeFileSync('/run/secrets/rds/global-bundle.pem', body, { mode: 0o600 });
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop:
                - ALL
            seccompProfile:
              type: RuntimeDefault
          resources:
            requests:
              cpu: 50m
              memory: 64Mi
            limits:
              cpu: 250m
              memory: 256Mi
          volumeMounts:
            - name: rds-ca
              mountPath: /run/secrets/rds
            - name: tmp
              mountPath: /tmp
      containers:
        - name: provider-probe
          image: ${image}
          imagePullPolicy: Always
          command:
            - node
            - --input-type=module
            - -e
          args:
            - |
              import { spawnSync } from 'node:child_process';
              import { readFileSync } from 'node:fs';
              import { createRequire } from 'node:module';
              const require = createRequire('/work/package.json');
              const pg = require('pg');
              const databaseUrl = new URL(process.env.RDS_URL);
              databaseUrl.searchParams.delete('sslmode');
              databaseUrl.searchParams.delete('uselibpqcompat');
              const client = new pg.Client({
                connectionString: databaseUrl.toString(),
                ssl: {
                  ca: readFileSync(process.env.PG_SSL_CA_FILE, 'utf8'),
                  rejectUnauthorized: true,
                },
                statement_timeout: 30000,
                query_timeout: 35000,
                application_name: 'benefactor-provider-probe',
              });
              await client.connect();
              let category = '';
              try {
                const result = await client.query(`
                  SELECT service_category
                  FROM benefactor.benefactor_scrape_queries
                  WHERE is_active = true
                    AND is_soft_deleted = false
                    AND service_category IS NOT NULL
                    AND BTRIM(service_category) <> ''
                    AND (cooldown_until IS NULL OR cooldown_until <= now())
                  GROUP BY service_category
                  ORDER BY MAX(priority) DESC, MIN(total_runs) ASC, service_category ASC
                  LIMIT 1
                `);
                category = String(result.rows[0]?.service_category || '').trim();
              } finally {
                await client.end().catch(() => {});
              }
              if (!category) throw new Error('no_active_probe_category');
              const child = spawnSync(process.execPath, ['/work/orchestrate.mjs'], {
                env: { ...process.env, ICP_CATEGORY: category },
                stdio: 'inherit',
              });
              if (child.error) throw child.error;
              process.exit(child.status ?? 1);
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop:
                - ALL
            seccompProfile:
              type: RuntimeDefault
          env:
            - name: PIPELINE_DRY_RUN
              value: "true"
            - name: TARGET_EMAILS
              value: "1"
            - name: MAX_QUERIES
              value: "1"
            - name: MAX_PAGES_PER_QUERY
              value: "1"
            - name: DOMAIN_SKIP_DAYS
              value: "0"
            - name: QUERY_COOLDOWN_DAYS
              value: "0"
            - name: DEADLINE_SECONDS
              value: "180"
            - name: SCRAPER_URL
              value: http://dd-web-scraper.default.svc.cluster.local:8097
            - name: SCRAPER_ALLOWED_HOSTS
              value: dd-web-scraper.default.svc.cluster.local
            - name: REQUIRE_ROLE_EMAIL
              value: "true"
            - name: RDS_URL
              valueFrom:
                secretKeyRef:
                  name: dd-remote-rest-api-secrets
                  key: AGENT_TASKS_RDS_DATABASE_URL
            - name: SCRAPER_AUTH
              valueFrom:
                secretKeyRef:
                  name: dd-agent-secrets
                  key: SERVER_AUTH_SECRET
            - name: SERPER_API_KEY
              valueFrom:
                secretKeyRef:
                  name: ${provider_secret}
                  key: SERPER_API_KEY
                  optional: true
            - name: BRAVE_SEARCH_API_KEY
              valueFrom:
                secretKeyRef:
                  name: ${provider_secret}
                  key: BRAVE_SEARCH_API_KEY
                  optional: true
            - name: PG_SSL_CA_FILE
              value: /run/secrets/rds/global-bundle.pem
          resources:
            requests:
              cpu: 100m
              memory: 256Mi
            limits:
              cpu: "1"
              memory: 1Gi
          volumeMounts:
            - name: rds-ca
              mountPath: /run/secrets/rds
              readOnly: true
            - name: tmp
              mountPath: /tmp
      volumes:
        - name: rds-ca
          emptyDir:
            sizeLimit: 4Mi
        - name: tmp
          emptyDir:
            sizeLimit: 64Mi
YAML

"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" apply -f "$manifest" >/dev/null
job_created=true

stage=job_wait
terminal_status=timeout
for _ in $(seq 1 120); do
  complete="$(
    "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" \
      -o jsonpath='{.status.conditions[?(@.type=="Complete")].status}' 2>/dev/null || true
  )"
  failed="$(
    "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" \
      -o jsonpath='{.status.conditions[?(@.type=="Failed")].status}' 2>/dev/null || true
  )"
  if [[ "$complete" == True ]]; then
    terminal_status=complete
    break
  fi
  if [[ "$failed" == True ]]; then
    terminal_status=failed
    break
  fi
  sleep 5
done

stage=log_collection
"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" logs "job/$job" \
  -c provider-probe > "$log_file" 2>/dev/null || true

if [[ "$terminal_status" != complete ]]; then
  safe_status="$(
    "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get pods -l "job-name=$job" -o json \
      | python3 -c '
import json, sys
obj = json.load(sys.stdin)
summary = []
for pod in obj.get("items", []):
    status = pod.get("status", {})
    item = {"phase": status.get("phase", "Unknown"), "containers": []}
    for field in ("initContainerStatuses", "containerStatuses"):
        for container in status.get(field, []) or []:
            state = container.get("state") or {}
            terminated = state.get("terminated") or {}
            waiting = state.get("waiting") or {}
            item["containers"].append({
                "name": container.get("name"),
                "reason": terminated.get("reason") or waiting.get("reason") or "",
                "exitCode": terminated.get("exitCode"),
            })
    summary.append(item)
print(json.dumps(summary, separators=(",", ":")))
'
  )"
  failure_reported=true
  printf 'BENEFACTOR_PROVIDER_PROBE_FAILED stage=job_wait status=%s pod_status=%s trusted_k8s_cluster_sha=%s\n' \
    "$terminal_status" "$safe_status" "$trusted_sha" >&2
  exit 1
fi

stage=report_validation
python3 - "$log_file" "$trusted_sha" "$orchestrator_sha" "$orchestrator_digest" "$image" <<'PYREPORT'
import json
import pathlib
import sys

log_path, trusted_sha, orchestrator_sha, orchestrator_digest, image = sys.argv[1:]
lines = pathlib.Path(log_path).read_text(encoding='utf-8', errors='replace').splitlines()
pipeline_lines = [line for line in lines if line.startswith('BENEFACTOR_PIPELINE_REPORT ')]
diagnostic_lines = [line for line in lines if line.startswith('BENEFACTOR_PROVIDER_DIAGNOSTICS ')]
if len(pipeline_lines) != 1:
    raise SystemExit(f'expected one pipeline report, found {len(pipeline_lines)}')
if len(diagnostic_lines) != 1:
    raise SystemExit(f'expected one provider diagnostic report, found {len(diagnostic_lines)}')

pipeline = json.loads(pipeline_lines[0].split(' ', 1)[1])
counters = pipeline.get('counters') or {}
queries_run = int(counters.get('queriesRun') or 0)
urls_visited = int(counters.get('urlsVisited') or 0)
contacts_collected = int(counters.get('contactsCollected') or 0)
phones_collected = int(counters.get('phonesCollected') or 0)
provider_failures = int(counters.get('providerFailures') or 0)
if queries_run != 1:
    raise SystemExit('provider probe did not run exactly one query')
if urls_visited < 0 or urls_visited > 1:
    raise SystemExit('provider probe exceeded one candidate page')
if min(contacts_collected, phones_collected, provider_failures) < 0:
    raise SystemExit('pipeline counters are invalid')

diagnostic = json.loads(diagnostic_lines[0].split(' ', 1)[1])
if diagnostic.get('reportVersion') != 'benefactor.provider-diagnostics.v1':
    raise SystemExit('provider diagnostic version is invalid')
providers = diagnostic.get('providers')
if not isinstance(providers, list) or not providers:
    raise SystemExit('provider diagnostics are missing')
normalized = []
total_requests = 0
for provider in providers:
    if not isinstance(provider, dict):
        raise SystemExit('provider diagnostic entry is invalid')
    name = str(provider.get('provider') or '')
    if name not in {'brave', 'serper'}:
        raise SystemExit('unexpected provider diagnostic name')
    requests = int(provider.get('requests') or 0)
    successes = int(provider.get('successes') or 0)
    failures = int(provider.get('failures') or 0)
    codes = provider.get('failureCodes') or {}
    if requests < 0 or requests > 1 or successes < 0 or failures < 0 or successes + failures != requests:
        raise SystemExit('provider diagnostic counters are inconsistent')
    if not isinstance(codes, dict):
        raise SystemExit('provider diagnostic codes are invalid')
    for code, count in codes.items():
        if not isinstance(code, str) or not code.replace('_', '').isalnum():
            raise SystemExit('provider failure code is invalid')
        if not isinstance(count, int) or count < 1:
            raise SystemExit('provider failure count is invalid')
    total_requests += requests
    normalized.append({
        'provider': name,
        'requests': requests,
        'successes': successes,
        'failures': failures,
        'failureCodes': dict(sorted(codes.items())),
    })
if total_requests < 1 or total_requests > 2:
    raise SystemExit('provider probe request count is outside the expected range')
normalized.sort(key=lambda item: item['provider'])
summary = {
    'reportVersion': 'benefactor.provider-probe.v2',
    'providers': normalized,
    'counters': {
        'queriesRun': queries_run,
        'urlsVisited': urls_visited,
        'contactsCollected': contacts_collected,
        'phonesCollected': phones_collected,
        'providerFailures': provider_failures,
    },
    'image': image,
    'orchestratorImageSha': orchestrator_sha,
    'orchestratorImageDigest': orchestrator_digest,
    'trustedK8sClusterSha': trusted_sha,
}
print('BENEFACTOR_PROVIDER_PROBE_SUMMARY ' + json.dumps(summary, sort_keys=True, separators=(',', ':')))
PYREPORT
