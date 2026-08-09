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
job="benefactor-search-scraper-probe-${short_sha}-$(date -u +%H%M%S)"
manifest="$(mktemp /tmp/benefactor-search-scraper-probe.XXXXXX.yaml)"
log_file="$(mktemp /tmp/benefactor-search-scraper-probe.XXXXXX.log)"
job_created=false
failure_reported=false
stage=cluster_preflight

cleanup() {
  local status=$?
  if (( status != 0 )) && [[ "$failure_reported" != true ]]; then
    printf 'BENEFACTOR_SEARCH_SCRAPER_PROBE_FAILED stage=%s status=script_error exit_code=%s trusted_k8s_cluster_sha=%s\n' \
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
secret_has_key dd-agent-secrets SERVER_AUTH_SECRET

provider_secret="$(
  "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get secrets -o json \
    | python3 -c '
import json, sys
items = json.load(sys.stdin).get("items", [])
candidates = []
for item in items:
    keys = item.get("data") or {}
    if "SERPER_API_KEY" in keys:
        candidates.append(item.get("metadata", {}).get("name", ""))
priority = {"dd-agent-secrets": 0, "benefactor-contact-pipeline": 1}
candidates = sorted({name for name in candidates if name}, key=lambda name: (priority.get(name, 100), name))
print(candidates[0] if candidates else "")
'
)"
if [[ ! "$provider_secret" =~ ^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$ ]]; then
  echo 'no Kubernetes Secret with SERPER_API_KEY is available' >&2
  exit 69
fi

fixture_url="https://raw.githubusercontent.com/ORESoftware/k8s-cluster/${trusted_sha}/remote/deployments/benefactor-orchestrator/fixtures/search-scraper-contract-contact.html"
stage=manifest_apply
cat > "$manifest" <<YAML
apiVersion: batch/v1
kind: Job
metadata:
  name: ${job}
  namespace: ${namespace}
  labels:
    app.kubernetes.io/name: benefactor-search-scraper-contract-probe
    app.kubernetes.io/component: credentialed-dry-run
    app.kubernetes.io/part-of: benefactor-cc
  annotations:
    benefactor.cc/trusted-k8s-cluster-sha: ${trusted_sha}
    benefactor.cc/orchestrator-image-sha: ${orchestrator_sha}
    benefactor.cc/orchestrator-image-digest: ${orchestrator_digest}
spec:
  backoffLimit: 0
  activeDeadlineSeconds: 300
  ttlSecondsAfterFinished: 600
  template:
    metadata:
      labels:
        app.kubernetes.io/name: benefactor-search-scraper-contract-probe
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
      containers:
        - name: probe
          image: ${image}
          imagePullPolicy: Always
          command: [node, --input-type=module, -e]
          args:
            - |
              const MAX_BODY_BYTES = 4 * 1024 * 1024;
              const fixedQuery = 'roofing contractor';
              const expectedEmail = 'contact@fixture.benefactor.cc';
              const expectedPhone = '+12025550199';

              async function readTextCapped(response, maxBytes) {
                const declared = Number.parseInt(response.headers.get('content-length') || '0', 10);
                if (Number.isFinite(declared) && declared > maxBytes) throw new Error('declared_response_too_large');
                if (!response.body) return '';
                const reader = response.body.getReader();
                const chunks = [];
                let total = 0;
                try {
                  for (;;) {
                    const item = await reader.read();
                    if (item.done) break;
                    if (!item.value) continue;
                    total += item.value.byteLength;
                    if (total > maxBytes) {
                      await reader.cancel().catch(() => {});
                      throw new Error('streamed_response_too_large');
                    }
                    chunks.push(Buffer.from(item.value));
                  }
                } finally {
                  reader.releaseLock();
                }
                return Buffer.concat(chunks, total).toString('utf8');
              }

              function parseJson(text) {
                try {
                  const value = JSON.parse(text);
                  return value && typeof value === 'object' && !Array.isArray(value) ? value : null;
                } catch {
                  return null;
                }
              }

              function classifyProviderError(text, body) {
                const message = String(body?.message || body?.error || body?.detail || text || '').toLowerCase().slice(0, 2048);
                if (!message) return 'empty_error';
                if ((message.includes('api') || message.includes('key')) && (message.includes('invalid') || message.includes('unauthor'))) return 'auth_invalid';
                if (message.includes('credit') || message.includes('quota') || message.includes('billing') || message.includes('payment')) return 'quota_or_billing';
                if ((message.includes('query') || message.includes(' q ')) && (message.includes('required') || message.includes('missing'))) return 'query_required';
                if ((message.includes('query') || message.includes(' q ')) && (message.includes('long') || message.includes('length') || message.includes('invalid'))) return 'query_invalid';
                if (message.includes('num') || message.includes('number of results')) return 'num_invalid';
                if (message.includes('country') || message.includes(' gl ')) return 'country_invalid';
                if (message.includes('language') || message.includes(' hl ')) return 'language_invalid';
                if (body) return 'json_error_other';
                return 'non_json_error';
              }

              async function serperAttempt(variant, payload) {
                const response = await fetch('https://google.serper.dev/search', {
                  method: 'POST',
                  headers: {
                    'Content-Type': 'application/json',
                    'X-API-KEY': process.env.SERPER_API_KEY,
                  },
                  body: JSON.stringify(payload),
                  redirect: 'error',
                  signal: AbortSignal.timeout(15000),
                });
                const text = await readTextCapped(response, 32768);
                const body = parseJson(text);
                const resultCount = response.ok && Array.isArray(body?.organic) ? body.organic.length : 0;
                return {
                  variant,
                  status: response.status,
                  ok: response.ok,
                  errorClass: response.ok ? 'none' : classifyProviderError(text, body),
                  resultCount,
                };
              }

              async function runSerperMatrix() {
                const variants = [
                  ['current', { q: fixedQuery, num: 10, gl: 'us', hl: 'en' }],
                  ['no_locale', { q: fixedQuery, num: 10 }],
                  ['q_only', { q: fixedQuery }],
                ];
                const attempts = [];
                for (const item of variants) {
                  try {
                    const attempt = await serperAttempt(item[0], item[1]);
                    attempts.push(attempt);
                    if (attempt.ok) break;
                  } catch (error) {
                    attempts.push({
                      variant: item[0],
                      status: 0,
                      ok: false,
                      errorClass: error?.name === 'TimeoutError' ? 'timeout' : 'network_or_client',
                      resultCount: 0,
                    });
                  }
                }
                return {
                  configured: Boolean(process.env.SERPER_API_KEY),
                  resolved: attempts.some((attempt) => attempt.ok),
                  attempts,
                };
              }

              function normalizePhone(value) {
                const raw = String(value || '').trim();
                const digits = raw.replace(/\D/g, '');
                if (digits.length === 10) return '+1' + digits;
                if (digits.length === 11 && digits.startsWith('1')) return '+' + digits;
                if (raw.startsWith('+') && digits.length >= 10 && digits.length <= 15) return '+' + digits;
                return '';
              }

              function contactSummary(body, variant, status) {
                const extraction = body?.extraction && typeof body.extraction === 'object' ? body.extraction : {};
                const contacts = extraction.contacts && typeof extraction.contacts === 'object' ? extraction.contacts : {};
                const emails = (Array.isArray(contacts.emails) ? contacts.emails : [])
                  .map((item) => String(typeof item === 'string' ? item : item?.address || '').trim().toLowerCase())
                  .filter(Boolean);
                const phones = [];
                for (const item of Array.isArray(contacts.phones) ? contacts.phones : []) {
                  const values = typeof item === 'string' ? [item] : [item?.e164, item?.raw, item?.national];
                  for (const value of values) {
                    const phone = normalizePhone(value);
                    if (phone) {
                      phones.push(phone);
                      break;
                    }
                  }
                }
                return {
                  variant,
                  status,
                  providerOk: status >= 200 && status < 300,
                  bodyOk: body?.ok !== false,
                  emailCount: new Set(emails).size,
                  phoneCount: new Set(phones).size,
                  expectedEmail: emails.includes(expectedEmail),
                  expectedPhone: phones.includes(expectedPhone),
                  htmlPresent: typeof extraction.html === 'string' && extraction.html.length > 0,
                  textPresent: typeof extraction.text === 'string' && extraction.text.length > 0,
                };
              }

              async function scraperAttempt(variant, strategy) {
                const response = await fetch(new URL('/scrape', process.env.SCRAPER_URL), {
                  method: 'POST',
                  headers: {
                    'Content-Type': 'application/json',
                    'x-server-auth': process.env.SCRAPER_AUTH,
                  },
                  body: JSON.stringify({
                    url: process.env.FIXTURE_URL,
                    strategy,
                    renderJavaScript: strategy === 'playwright',
                    includeHtml: true,
                    includeText: true,
                    includeLinks: true,
                    includeContacts: true,
                    includeEmails: true,
                    includePhones: true,
                    maxEmails: 10,
                    maxPhones: 10,
                    maxHtmlChars: 200000,
                    respectRobots: true,
                    rejectPrivateNetwork: true,
                    waitUntil: 'domcontentloaded',
                    timeoutMs: 30000,
                  }),
                  redirect: 'error',
                  signal: AbortSignal.timeout(45000),
                });
                const text = await readTextCapped(response, MAX_BODY_BYTES);
                return contactSummary(parseJson(text), variant, response.status);
              }

              async function runScraperProbe() {
                const attempts = [];
                for (const item of [['static', 'cheerio'], ['rendered', 'playwright']]) {
                  try {
                    const attempt = await scraperAttempt(item[0], item[1]);
                    attempts.push(attempt);
                    if (attempt.expectedEmail && attempt.expectedPhone) break;
                  } catch (error) {
                    attempts.push({
                      variant: item[0],
                      status: 0,
                      providerOk: false,
                      bodyOk: false,
                      emailCount: 0,
                      phoneCount: 0,
                      expectedEmail: false,
                      expectedPhone: false,
                      htmlPresent: false,
                      textPresent: false,
                      errorClass: error?.name === 'TimeoutError' ? 'timeout' : 'network_or_client',
                    });
                  }
                }
                return {
                  resolved: attempts.some((attempt) => attempt.expectedEmail && attempt.expectedPhone),
                  attempts,
                };
              }

              const serper = await runSerperMatrix();
              const scraper = await runScraperProbe();
              const report = {
                reportVersion: 'benefactor.search-scraper-contract-probe.v1',
                trustedK8sClusterSha: process.env.TRUSTED_K8S_CLUSTER_SHA,
                orchestratorImageSha: process.env.ORCHESTRATOR_IMAGE_SHA,
                orchestratorImageDigest: process.env.ORCHESTRATOR_IMAGE_DIGEST,
                serper,
                scraper,
                safeToMutate: false,
                outboundTransportPresent: false,
              };
              process.stdout.write('BENEFACTOR_SEARCH_SCRAPER_PROBE ' + JSON.stringify(report) + '\n');
          env:
            - { name: NODE_OPTIONS, value: "" }
            - { name: TRUSTED_K8S_CLUSTER_SHA, value: "${trusted_sha}" }
            - { name: ORCHESTRATOR_IMAGE_SHA, value: "${orchestrator_sha}" }
            - { name: ORCHESTRATOR_IMAGE_DIGEST, value: "${orchestrator_digest}" }
            - { name: FIXTURE_URL, value: "${fixture_url}" }
            - { name: SCRAPER_URL, value: http://dd-web-scraper.default.svc.cluster.local:8097 }
            - name: SCRAPER_AUTH
              valueFrom: { secretKeyRef: { name: dd-agent-secrets, key: SERVER_AUTH_SECRET } }
            - name: SERPER_API_KEY
              valueFrom: { secretKeyRef: { name: ${provider_secret}, key: SERPER_API_KEY } }
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities: { drop: [ALL] }
            seccompProfile: { type: RuntimeDefault }
          resources:
            requests: { cpu: 100m, memory: 128Mi }
            limits: { cpu: 500m, memory: 512Mi }
          volumeMounts:
            - { name: tmp, mountPath: /tmp }
      volumes:
        - name: tmp
          emptyDir: { sizeLimit: 32Mi }
YAML

"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" apply -f "$manifest" >/dev/null
job_created=true

stage=job_wait
terminal_status=timeout
for _ in $(seq 1 60); do
  complete="$("$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" -o jsonpath='{.status.conditions[?(@.type=="Complete")].status}' 2>/dev/null || true)"
  failed="$("$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" -o jsonpath='{.status.conditions[?(@.type=="Failed")].status}' 2>/dev/null || true)"
  if [[ "$complete" == True ]]; then terminal_status=complete; break; fi
  if [[ "$failed" == True ]]; then terminal_status=failed; break; fi
  sleep 5
done

stage=log_collection
"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" logs "job/$job" -c probe > "$log_file" 2>/dev/null || true

if [[ "$terminal_status" != complete ]]; then
  failure_reported=true
  printf 'BENEFACTOR_SEARCH_SCRAPER_PROBE_FAILED stage=job_wait status=%s trusted_k8s_cluster_sha=%s\n' \
    "$terminal_status" "$trusted_sha" >&2
  exit 1
fi

stage=report_validation
python3 - "$log_file" "$trusted_sha" "$orchestrator_sha" "$orchestrator_digest" <<'PYREPORT'
import json
import pathlib
import sys

log_path, trusted_sha, orchestrator_sha, orchestrator_digest = sys.argv[1:]
lines = pathlib.Path(log_path).read_text(encoding='utf-8', errors='replace').splitlines()
reports = [line for line in lines if line.startswith('BENEFACTOR_SEARCH_SCRAPER_PROBE ')]
if len(reports) != 1:
    raise SystemExit(f'expected one search/scraper report, found {len(reports)}')
report = json.loads(reports[0].split(' ', 1)[1])
if report.get('reportVersion') != 'benefactor.search-scraper-contract-probe.v1':
    raise SystemExit('unexpected report version')
if report.get('trustedK8sClusterSha') != trusted_sha:
    raise SystemExit('trusted revision mismatch')
if report.get('orchestratorImageSha') != orchestrator_sha:
    raise SystemExit('image revision mismatch')
if report.get('orchestratorImageDigest') != orchestrator_digest:
    raise SystemExit('image digest mismatch')
if report.get('safeToMutate') is not False or report.get('outboundTransportPresent') is not False:
    raise SystemExit('probe safety boundary changed')

serper = report.get('serper') or {}
attempts = serper.get('attempts') or []
if serper.get('configured') is not True or not (1 <= len(attempts) <= 3):
    raise SystemExit('invalid Serper probe shape')
allowed_variants = ['current', 'no_locale', 'q_only']
for index, attempt in enumerate(attempts):
    if attempt.get('variant') != allowed_variants[index]:
        raise SystemExit('unexpected Serper variant ordering')
    status = attempt.get('status')
    if not isinstance(status, int) or status < 0 or status > 599:
        raise SystemExit('invalid Serper status')
    if not isinstance(attempt.get('resultCount'), int) or attempt['resultCount'] < 0:
        raise SystemExit('invalid Serper result count')
    if not isinstance(attempt.get('errorClass'), str) or len(attempt['errorClass']) > 64:
        raise SystemExit('invalid Serper error class')

scraper = report.get('scraper') or {}
scraper_attempts = scraper.get('attempts') or []
if not (1 <= len(scraper_attempts) <= 2):
    raise SystemExit('invalid scraper probe shape')
for index, attempt in enumerate(scraper_attempts):
    expected = ['static', 'rendered'][index]
    if attempt.get('variant') != expected:
        raise SystemExit('unexpected scraper variant ordering')
    status = attempt.get('status')
    if not isinstance(status, int) or status < 0 or status > 599:
        raise SystemExit('invalid scraper status')
    for field in ('emailCount', 'phoneCount'):
        if not isinstance(attempt.get(field), int) or attempt[field] < 0:
            raise SystemExit('invalid scraper contact count')
    for field in ('expectedEmail', 'expectedPhone', 'htmlPresent', 'textPresent'):
        if not isinstance(attempt.get(field), bool):
            raise SystemExit('invalid scraper boolean')

print('BENEFACTOR_SEARCH_SCRAPER_PROBE ' + json.dumps(report, separators=(',', ':'), sort_keys=True))
if not serper.get('resolved') or not scraper.get('resolved'):
    raise SystemExit(2)
PYREPORT
