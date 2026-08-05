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
job="benefactor-contact-dry-${short_sha}-$(date -u +%H%M%S)"
manifest="$(mktemp /tmp/benefactor-contact-dry-manifest.XXXXXX.yaml)"
log_file="$(mktemp /tmp/benefactor-contact-dry-log.XXXXXX)"
job_created=false

cleanup() {
  local status=$?
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
  echo 'no Kubernetes Secret with SERPER_API_KEY or BRAVE_SEARCH_API_KEY is available' >&2
  exit 69
fi

cat > "$manifest" <<YAML
apiVersion: batch/v1
kind: Job
metadata:
  name: ${job}
  namespace: ${namespace}
  labels:
    app.kubernetes.io/name: benefactor-contact-batch
    app.kubernetes.io/component: credentialed-dry-run
    app.kubernetes.io/part-of: benefactor-cc
  annotations:
    benefactor.cc/trusted-k8s-cluster-sha: ${trusted_sha}
    benefactor.cc/orchestrator-image-sha: ${orchestrator_sha}
    benefactor.cc/orchestrator-image-digest: ${orchestrator_digest}
spec:
  backoffLimit: 0
  activeDeadlineSeconds: 3600
  ttlSecondsAfterFinished: 900
  template:
    metadata:
      labels:
        app.kubernetes.io/name: benefactor-contact-batch
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
              const url = 'https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem';
              const response = await fetch(url, {
                redirect: 'error',
                signal: AbortSignal.timeout(30000),
                headers: { 'User-Agent': 'benefactor-contact-dry-run/1' },
              });
              if (!response.ok) throw new Error('rds_ca_http_' + response.status);
              const declared = Number(response.headers.get('content-length') || 0);
              if (declared && declared > 2000000) throw new Error('rds_ca_declared_too_large');
              const body = Buffer.from(await response.arrayBuffer());
              if (body.length < 1000 || body.length > 2000000) throw new Error('rds_ca_size_invalid');
              const text = body.toString('utf8');
              const count = (text.match(/-----BEGIN CERTIFICATE-----/g) || []).length;
              if (count < 1 || !text.includes('-----END CERTIFICATE-----')) throw new Error('rds_ca_pem_invalid');
              writeFileSync('/run/secrets/rds/global-bundle.pem', body, { mode: 0o600 });
              console.log('rds_ca_ready certificates=' + count);
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
        - name: contact-batch
          image: ${image}
          imagePullPolicy: Always
          args:
            - contact-batch.mjs
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop:
                - ALL
            seccompProfile:
              type: RuntimeDefault
          env:
            - name: BATCH_DRY_RUN
              value: "true"
            - name: BATCH_TARGET_CONTACTS
              value: "250"
            - name: BATCH_MINIMUM_CONTACTS
              value: "200"
            - name: BATCH_MAXIMUM_CONTACTS
              value: "300"
            - name: BATCH_MAX_CONTACTS_PER_CATEGORY
              value: "50"
            - name: BATCH_MAX_CATEGORIES
              value: "20"
            - name: HUBSPOT_SYNC_AFTER_DISCOVERY
              value: "false"
            - name: HUBSPOT_DRY_RUN
              value: "true"
            - name: SCRAPER_URL
              value: http://dd-web-scraper.default.svc.cluster.local:8097
            - name: SCRAPER_ALLOWED_HOSTS
              value: dd-web-scraper.default.svc.cluster.local
            - name: REQUIRE_ROLE_EMAIL
              value: "true"
            - name: MAX_QUERIES
              value: "8"
            - name: MAX_PAGES_PER_QUERY
              value: "8"
            - name: DOMAIN_SKIP_DAYS
              value: "0"
            - name: QUERY_COOLDOWN_DAYS
              value: "0"
            - name: DEADLINE_SECONDS
              value: "420"
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
              cpu: 200m
              memory: 384Mi
            limits:
              cpu: "2"
              memory: 2Gi
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

terminal_status='timeout'
for _ in $(seq 1 720); do
  complete="$(
    "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" \
      -o jsonpath='{.status.conditions[?(@.type=="Complete")].status}' 2>/dev/null || true
  )"
  failed="$(
    "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" \
      -o jsonpath='{.status.conditions[?(@.type=="Failed")].status}' 2>/dev/null || true
  )"
  if [[ "$complete" == True ]]; then
    terminal_status='complete'
    break
  fi
  if [[ "$failed" == True ]]; then
    terminal_status='failed'
    break
  fi
  sleep 5
done

"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" logs "job/$job" \
  -c contact-batch > "$log_file" 2>/dev/null || true

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
        for c in status.get(field, []) or []:
            state = c.get("state") or {}
            terminated = state.get("terminated") or {}
            waiting = state.get("waiting") or {}
            item["containers"].append({
                "name": c.get("name"),
                "reason": terminated.get("reason") or waiting.get("reason") or "",
                "exitCode": terminated.get("exitCode"),
            })
    summary.append(item)
print(json.dumps(summary, separators=(",", ":")))
'
  )"
  printf 'BENEFACTOR_CONTACT_DRY_RUN_FAILED status=%s pod_status=%s trusted_k8s_cluster_sha=%s\n' \
    "$terminal_status" "$safe_status" "$trusted_sha" >&2
  exit 1
fi

python3 - "$log_file" "$trusted_sha" "$orchestrator_sha" "$orchestrator_digest" "$job" "$image" <<'PYREPORT'
import json
import pathlib
import sys

log_path, trusted_sha, orchestrator_sha, orchestrator_digest, job, image = sys.argv[1:]
lines = pathlib.Path(log_path).read_text(encoding='utf-8', errors='replace').splitlines()
pipeline_lines = [line for line in lines if line.startswith('BENEFACTOR_PIPELINE_REPORT ')]
batch_lines = [line for line in lines if line.startswith('BENEFACTOR_CONTACT_BATCH_REPORT ')]
if len(batch_lines) != 1:
    raise SystemExit(f'expected one batch report, found {len(batch_lines)}')

batch = json.loads(batch_lines[0].split(' ', 1)[1])
if batch.get('dryRun') is not True:
    raise SystemExit('batch report is not a dry run')
if batch.get('targetContacts') != 250:
    raise SystemExit('batch target is not 250')
if batch.get('minimumContacts') != 200 or batch.get('maximumContacts') != 300:
    raise SystemExit('batch bounds are not 200-300')
if batch.get('outreachDispatchRequested') is not False:
    raise SystemExit('batch report requested outreach')
if batch.get('contactsTagged') != 0:
    raise SystemExit('dry run unexpectedly tagged contacts')
if batch.get('approvedForOutreach') != 0:
    raise SystemExit('dry run unexpectedly reported approved outreach contacts')
if batch.get('status') != 'dry_run_complete':
    raise SystemExit('batch did not complete in dry-run state')
if (batch.get('hubspot') or {}).get('attempted') is not False:
    raise SystemExit('dry run unexpectedly attempted HubSpot synchronization')
planned = int(batch.get('categoriesPlanned') or 0)
ran = int(batch.get('categoriesRun') or 0)
if planned < 1 or ran != planned:
    raise SystemExit(f'category execution mismatch planned={planned} ran={ran}')
if len(pipeline_lines) != ran:
    raise SystemExit(f'pipeline report mismatch reports={len(pipeline_lines)} categories={ran}')

aggregate = {
    'contactsCollected': 0,
    'phonesCollected': 0,
    'queriesRun': 0,
    'urlsVisited': 0,
    'pagesWithEmail': 0,
    'providerFailures': 0,
}
provider_requests = 0
provider_results = 0
for line in pipeline_lines:
    report = json.loads(line.split(' ', 1)[1])
    if report.get('reportVersion') != 'benefactor.pipeline.dry-run.v1':
        raise SystemExit('pipeline report version is not the dry-run schema')
    digest = str(report.get('reportDigest') or '')
    if not digest.startswith('sha256:') or len(digest) != 71:
        raise SystemExit('pipeline report digest is invalid')
    counters = report.get('counters') or {}
    if not isinstance(counters, dict):
        raise SystemExit('pipeline counters are invalid')
    if int(counters.get('leadsInserted') or 0) != 0:
        raise SystemExit('pipeline dry run reported inserted leads')
    if int(counters.get('persistenceFailures') or 0) != 0:
        raise SystemExit('pipeline dry run reported persistence failures')
    for key in aggregate:
        aggregate[key] += int(counters.get(key) or 0)
    providers = report.get('providers') or []
    if not any(bool(provider.get('enabled')) for provider in providers if isinstance(provider, dict)):
        raise SystemExit('pipeline report has no enabled search provider')
    provider_requests += sum(int(provider.get('requests') or 0) for provider in providers if isinstance(provider, dict))
    provider_results += sum(int(provider.get('resultCount') or 0) for provider in providers if isinstance(provider, dict))

print(
    'BENEFACTOR_CONTACT_DRY_RUN_SUMMARY '
    f'categories={ran} contacts_collected={aggregate["contactsCollected"]} '
    f'phones_collected={aggregate["phonesCollected"]} queries_run={aggregate["queriesRun"]} '
    f'urls_visited={aggregate["urlsVisited"]} pages_with_email={aggregate["pagesWithEmail"]} '
    f'provider_requests={provider_requests} provider_results={provider_results} '
    f'provider_failures={aggregate["providerFailures"]} image={image} '
    f'orchestrator_image_sha={orchestrator_sha} orchestrator_image_digest={orchestrator_digest} trusted_k8s_cluster_sha={trusted_sha}'
)
print(
    'BENEFACTOR_CONTACT_DRY_RUN_READY '
    f'job={job} planned=250 categories={ran} pipeline_reports={len(pipeline_lines)} '
    f'image={image} orchestrator_image_sha={orchestrator_sha} orchestrator_image_digest={orchestrator_digest} trusted_k8s_cluster_sha={trusted_sha}'
)
PYREPORT
