#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

trusted_sha="${1:-}"
image_sha="${2:-}"
image_digest="${3:-}"
[[ "$trusted_sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'invalid trusted SHA' >&2; exit 64; }
[[ "$image_sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'invalid image SHA' >&2; exit 64; }
[[ "$image_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo 'invalid image digest' >&2; exit 64; }

KUBECTL=/usr/bin/kubectl
KUBECONFIG_PATH=/home/ec2-user/.kube/config
namespace=default
image="ghcr.io/oresoftware/benefactor-contact-orchestrator@${image_digest}"
job="benefactor-scraper-ready-${trusted_sha:0:8}-$(date -u +%H%M%S)"
manifest="$(mktemp /tmp/benefactor-scraper-ready.XXXXXX.yaml)"
log_file="$(mktemp /tmp/benefactor-scraper-ready.XXXXXX.log)"
job_created=false
stage=cluster_preflight

cleanup() {
  local status=$?
  if [[ "$job_created" == true ]]; then
    "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" delete job "$job" \
      --ignore-not-found=true --wait=false >/dev/null 2>&1 || true
  fi
  rm -f "$manifest" "$log_file"
  if (( status != 0 )); then
    printf 'BENEFACTOR_SCRAPER_READINESS_FAILED stage=%s exit_code=%s trusted_sha=%s\n' \
      "$stage" "$status" "$trusted_sha" >&2
  fi
  exit "$status"
}
trap cleanup EXIT

"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" --request-timeout=20s get --raw=/readyz >/dev/null
"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get service dd-web-scraper >/dev/null
endpoints="$("$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get endpoints dd-web-scraper -o jsonpath='{.subsets[*].addresses[*].ip}' 2>/dev/null || true)"
test -n "$endpoints"
"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get secret benefactor-ghcr >/dev/null

stage=policy_reconciliation
selector_ready=false
for _ in $(seq 1 60); do
  if "$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get networkpolicy dd-web-scraper -o json \
    | python3 -c '
import json, sys
policy = json.load(sys.stdin)
expected = {
  "app.kubernetes.io/name": "benefactor-search-scraper-contract-probe",
  "app.kubernetes.io/component": "credentialed-dry-run",
  "app.kubernetes.io/part-of": "benefactor-cc",
}
for rule in policy.get("spec", {}).get("ingress", []):
  ports = rule.get("ports", [])
  port_ok = any(str(p.get("port")) == "8097" and p.get("protocol", "TCP") == "TCP" for p in ports)
  if not port_ok:
    continue
  for source in rule.get("from", []):
    labels = source.get("podSelector", {}).get("matchLabels", {})
    if all(labels.get(k) == v for k, v in expected.items()):
      raise SystemExit(0)
raise SystemExit(1)
'; then
    selector_ready=true
    break
  fi
  sleep 5
done
if [[ "$selector_ready" != true ]]; then
  echo 'BENEFACTOR_SCRAPER_READINESS_FAILED stage=policy_reconciliation status=selector_not_reconciled' >&2
  exit 1
fi

fixture_url="https://raw.githubusercontent.com/ORESoftware/k8s-cluster/${trusted_sha}/remote/deployments/benefactor-orchestrator/fixtures/search-scraper-contract-contact.html"
stage=job_apply
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
spec:
  backoffLimit: 0
  activeDeadlineSeconds: 240
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
        seccompProfile: { type: RuntimeDefault }
      containers:
        - name: probe
          image: ${image}
          imagePullPolicy: Always
          command: [node, --input-type=module, -e]
          args:
            - |
              const expectedEmail = 'contact@fixture.benefactor.cc';
              const expectedPhone = '+12025550199';
              const endpoint = new URL('/scrape', process.env.SCRAPER_URL);

              async function readJsonCapped(response, maxBytes = 4 * 1024 * 1024) {
                const declared = Number.parseInt(response.headers.get('content-length') || '0', 10);
                if (Number.isFinite(declared) && declared > maxBytes) throw new Error('response_too_large');
                if (!response.body) return null;
                const reader = response.body.getReader();
                const chunks = [];
                let total = 0;
                try {
                  for (;;) {
                    const { done, value } = await reader.read();
                    if (done) break;
                    if (!value) continue;
                    total += value.byteLength;
                    if (total > maxBytes) {
                      await reader.cancel().catch(() => {});
                      throw new Error('response_too_large');
                    }
                    chunks.push(Buffer.from(value));
                  }
                } finally {
                  reader.releaseLock();
                }
                try {
                  const value = JSON.parse(Buffer.concat(chunks, total).toString('utf8'));
                  return value && typeof value === 'object' && !Array.isArray(value) ? value : null;
                } catch {
                  return null;
                }
              }

              function normalizePhone(value) {
                const raw = String(value || '').trim();
                const digits = raw.replace(/\D/g, '');
                if (digits.length === 10) return '+1' + digits;
                if (digits.length === 11 && digits.startsWith('1')) return '+' + digits;
                if (raw.startsWith('+') && digits.length >= 10 && digits.length <= 15) return '+' + digits;
                return '';
              }

              function summarize(body, variant, status, errorClass = 'none') {
                const extraction = body?.extraction && typeof body.extraction === 'object' ? body.extraction : {};
                const contacts = extraction.contacts && typeof extraction.contacts === 'object' ? extraction.contacts : {};
                const emails = (Array.isArray(contacts.emails) ? contacts.emails : [])
                  .map(item => String(typeof item === 'string' ? item : item?.address || '').trim().toLowerCase())
                  .filter(Boolean);
                const phones = [];
                for (const item of Array.isArray(contacts.phones) ? contacts.phones : []) {
                  for (const value of typeof item === 'string' ? [item] : [item?.e164, item?.raw, item?.national]) {
                    const phone = normalizePhone(value);
                    if (phone) { phones.push(phone); break; }
                  }
                }
                return {
                  variant,
                  status,
                  errorClass,
                  responseOk: status >= 200 && status < 300,
                  bodyOk: body?.ok !== false,
                  htmlPresent: typeof extraction.html === 'string' && extraction.html.length > 0,
                  textPresent: typeof extraction.text === 'string' && extraction.text.length > 0,
                  emailCount: new Set(emails).size,
                  phoneCount: new Set(phones).size,
                  expectedEmail: emails.includes(expectedEmail),
                  expectedPhone: phones.includes(expectedPhone),
                };
              }

              async function attempt(variant, strategy) {
                try {
                  const response = await fetch(endpoint, {
                    method: 'POST',
                    headers: {
                      'content-type': 'application/json',
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
                  return summarize(await readJsonCapped(response), variant, response.status);
                } catch (error) {
                  const errorClass = error?.name === 'TimeoutError' ? 'timeout' :
                    error?.name === 'AbortError' ? 'aborted' :
                    error?.name === 'TypeError' ? 'network_or_client' : 'probe_error';
                  return summarize(null, variant, 0, errorClass);
                }
              }

              const attempts = [];
              attempts.push(await attempt('static', 'cheerio'));
              if (!(attempts[0].expectedEmail && attempts[0].expectedPhone)) {
                attempts.push(await attempt('rendered', 'playwright'));
              }
              const resolved = attempts.some(item => item.expectedEmail && item.expectedPhone);
              const report = {
                reportVersion: 'benefactor.scraper-readiness.v1',
                trustedK8sClusterSha: process.env.TRUSTED_K8S_CLUSTER_SHA,
                imageSha: process.env.IMAGE_SHA,
                imageDigest: process.env.IMAGE_DIGEST,
                resolved,
                attempts,
                safeToMutate: false,
                outboundTransportPresent: false,
              };
              process.stdout.write('BENEFACTOR_SCRAPER_READINESS ' + JSON.stringify(report) + '\n');
              if (!resolved) process.exitCode = 2;
          env:
            - { name: NODE_OPTIONS, value: "" }
            - { name: TRUSTED_K8S_CLUSTER_SHA, value: "${trusted_sha}" }
            - { name: IMAGE_SHA, value: "${image_sha}" }
            - { name: IMAGE_DIGEST, value: "${image_digest}" }
            - { name: FIXTURE_URL, value: "${fixture_url}" }
            - { name: SCRAPER_URL, value: http://dd-web-scraper.default.svc.cluster.local:8097 }
            - name: SCRAPER_AUTH
              valueFrom: { secretKeyRef: { name: dd-agent-secrets, key: SERVER_AUTH_SECRET } }
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
terminal=timeout
for _ in $(seq 1 60); do
  complete="$("$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" -o jsonpath='{.status.conditions[?(@.type=="Complete")].status}' 2>/dev/null || true)"
  failed="$("$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" get job "$job" -o jsonpath='{.status.conditions[?(@.type=="Failed")].status}' 2>/dev/null || true)"
  [[ "$complete" == True ]] && { terminal=complete; break; }
  [[ "$failed" == True ]] && { terminal=failed; break; }
  sleep 5
done

stage=report_validation
"$KUBECTL" --kubeconfig "$KUBECONFIG_PATH" -n "$namespace" logs "job/$job" -c probe > "$log_file" 2>/dev/null || true
python3 - "$log_file" "$trusted_sha" "$image_sha" "$image_digest" <<'PY'
import json, pathlib, sys
path, trusted_sha, image_sha, image_digest = sys.argv[1:]
lines = pathlib.Path(path).read_text(encoding='utf-8', errors='replace').splitlines()
rows = [line for line in lines if line.startswith('BENEFACTOR_SCRAPER_READINESS ')]
if len(rows) != 1:
    raise SystemExit(f'expected one readiness report, found {len(rows)}')
report = json.loads(rows[0].split(' ', 1)[1])
if report.get('reportVersion') != 'benefactor.scraper-readiness.v1': raise SystemExit('bad report version')
if report.get('trustedK8sClusterSha') != trusted_sha: raise SystemExit('trusted SHA mismatch')
if report.get('imageSha') != image_sha: raise SystemExit('image SHA mismatch')
if report.get('imageDigest') != image_digest: raise SystemExit('image digest mismatch')
if report.get('safeToMutate') is not False or report.get('outboundTransportPresent') is not False: raise SystemExit('safety boundary changed')
attempts = report.get('attempts') or []
if not (1 <= len(attempts) <= 2): raise SystemExit('bad attempts')
for index, attempt in enumerate(attempts):
    if attempt.get('variant') != ['static', 'rendered'][index]: raise SystemExit('bad variant order')
    for field in ('emailCount', 'phoneCount'):
        if not isinstance(attempt.get(field), int) or attempt[field] < 0: raise SystemExit('bad count')
    for field in ('expectedEmail', 'expectedPhone', 'htmlPresent', 'textPresent', 'responseOk', 'bodyOk'):
        if not isinstance(attempt.get(field), bool): raise SystemExit('bad boolean')
print('BENEFACTOR_SCRAPER_READINESS ' + json.dumps(report, separators=(',', ':'), sort_keys=True))
if report.get('resolved') is not True: raise SystemExit(2)
PY
[[ "$terminal" == complete ]]
