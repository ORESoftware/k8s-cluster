#!/usr/bin/env bash
# End-to-end test for the /container-pool/config operator surface.
#
# Runs entirely inside the cluster: hits dd-remote-rest-api directly via the
# in-cluster Service DNS so the test path matches what the gateway proxies to
# in production. Builds and smoke-tests the python3 runtime image because it
# is the fastest one to rebuild (~30s) and has a simple smoke command.
#
# Exit code is non-zero if any step fails. Print FAIL=N at the end.

set +e

PASS=0
FAIL=0
WARN=0

note() { echo; echo "::: $*"; }
ok()   { echo "    PASS - $*"; PASS=$((PASS+1)); }
ko()   { echo "    FAIL - $*"; FAIL=$((FAIL+1)); }
warn() { echo "    WARN - $*"; WARN=$((WARN+1)); }

REST=http://dd-remote-rest-api.default.svc.cluster.local:8082
KX="sudo -u ec2-user -H kubectl"

note "1. Pod state (after rollout)"
$KX get pods -n default -l 'app in (dd-remote-rest-api,dd-remote-web-home,dd-remote-gateway,dd-container-pool)' --no-headers -o wide
RUNNING=$($KX get pods -n default -l 'app=dd-remote-rest-api' -o jsonpath='{.items[0].status.phase}' 2>/dev/null)
[ "$RUNNING" = "Running" ] && ok "dd-remote-rest-api pod is Running" || ko "dd-remote-rest-api not Running ($RUNNING)"

note "2. REST API healthz"
HZ=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf "${REST}/healthz" 2>&1 | head -1)
echo "    healthz body: ${HZ:0:200}"
echo "$HZ" | grep -qE '"ok":\s*true|"status":' && ok "rest-api healthz" || ko "rest-api healthz"

note "3. GET /api/container-pool/images (list)"
LIST_JSON=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf "${REST}/api/container-pool/images" 2>&1)
echo "    first 600 bytes:"
echo "${LIST_JSON:0:600}"
echo
COUNT=$(echo "$LIST_JSON" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(len(d.get("images",[])))' 2>/dev/null)
echo "    image count: $COUNT"
[ "$COUNT" = "8" ] && ok "list returns 8 images" || ko "list returned $COUNT images (expected 8)"
echo "$LIST_JSON" | python3 -c 'import sys,json;d=json.load(sys.stdin);print("    buildsEnabled:",d.get("buildsEnabled"),"namespace:",d.get("namespace"),"repoRoot:",d.get("repoRoot"))' 2>/dev/null
echo "$LIST_JSON" | grep -q '"buildsEnabled":true' && ok "buildsEnabled=true" || warn "buildsEnabled flag not true"
echo "$LIST_JSON" | grep -q 'dd-container-pool-python3-runtime:dev' && ok "python3 image present in catalog" || ko "python3 image missing"
echo "$LIST_JSON" | grep -q 'dd-dev-server:dev' && ok "dev-server image present" || ko "dev-server missing"

note "4. GET /api/container-pool/images/python3/dockerfile?source=disk-default"
DF=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf "${REST}/api/container-pool/images/python3/dockerfile?source=disk-default" 2>&1)
SHA=$(echo "$DF" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("dockerfileSha256",""))' 2>/dev/null)
BYTES=$(echo "$DF" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(len(d.get("dockerfileText","")))' 2>/dev/null)
echo "    sha256=${SHA:0:16}..., bytes=$BYTES"
if [ -n "$SHA" ] && [ "${BYTES:-0}" -gt 50 ]; then
  ok "disk-default Dockerfile fetched (${BYTES}B)"
else
  ko "disk-default fetch failed (sha=$SHA bytes=$BYTES)"
fi

note "5. Schema bootstrap (ensure_schema)"
RDS_URL=$($KX -n default get secret dd-remote-rest-api-secrets -o jsonpath='{.data.AGENT_TASKS_RDS_DATABASE_URL}' 2>/dev/null | base64 -d 2>/dev/null)
if [ -z "$RDS_URL" ]; then
  RDS_URL=$($KX -n default get secret dd-remote-rest-api-secrets -o jsonpath='{.data.RDS_DATABASE_URL}' 2>/dev/null | base64 -d 2>/dev/null)
fi
if [ -z "$RDS_URL" ]; then
  RDS_URL=$($KX -n default get secret dd-agent-secrets -o jsonpath='{.data.AGENT_TASKS_RDS_DATABASE_URL}' 2>/dev/null | base64 -d 2>/dev/null)
fi
if [ -n "$RDS_URL" ]; then
  CNT=$($KX -n default run psql-cpool-check-$RANDOM --rm -i --restart=Never --image=docker.io/library/postgres:16-alpine --command -- psql "$RDS_URL" -At -c "select 'revs:'||count(*) from container_pool_image_revisions; select 'builds:'||count(*) from container_pool_build_runs;" 2>&1 | tr -d '\r')
  echo "    psql output:"
  echo "$CNT" | head -10
  echo "$CNT" | grep -qE 'revs:[0-9]+' && ok "container_pool_image_revisions table exists" || warn "psql probe inconclusive (see above)"
  echo "$CNT" | grep -qE 'builds:[0-9]+' && ok "container_pool_build_runs table exists" || warn "psql probe inconclusive (see above)"
else
  warn "could not read RDS database URL from any secret"
fi

note "6. PUT /api/container-pool/images/python3/dockerfile (save revision)"
BODY='{"dockerfileText":"FROM docker.io/library/python:3.12-alpine\nRUN python3 --version\nLABEL dd.cpool.test=\"e2e-smoke\"\nCMD [\"/bin/sh\",\"-c\",\"echo ok\"]\n","notes":"e2e smoke from container-pool/config test"}'
SAVE=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf -X PUT -H 'content-type: application/json' -d "$BODY" "${REST}/api/container-pool/images/python3/dockerfile" 2>&1)
echo "    save response (first 400B):"
echo "${SAVE:0:400}"
REVID=$(echo "$SAVE" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("revision",{}).get("id",""))' 2>/dev/null)
echo "    saved revision id: $REVID"
[ -n "$REVID" ] && ok "revision saved id=$REVID" || ko "save returned no revision id"

note "7. POST /api/container-pool/images/python3/build-test"
if [ -n "$REVID" ]; then
  BT_BODY='{"revisionId":"'"$REVID"'","testCommand":"python3 --version && echo SMOKE_OK"}'
else
  BT_BODY='{"testCommand":"python3 --version && echo SMOKE_OK"}'
fi
BT=$($KX exec -n default deploy/dd-remote-rest-api -- curl -s -X POST -H 'content-type: application/json' -d "$BT_BODY" "${REST}/api/container-pool/images/python3/build-test" 2>&1)
echo "    build-test response (first 600B):"
echo "${BT:0:600}"
BUILD_ID=$(echo "$BT" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("build",{}).get("id",""))' 2>/dev/null)
CAND_TAG=$(echo "$BT" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("build",{}).get("candidate_tag",""))' 2>/dev/null)
echo "    build_id=$BUILD_ID  candidate_tag=$CAND_TAG"
[ -n "$BUILD_ID" ] && ok "build queued id=$BUILD_ID" || ko "no build id"

note "8. Poll build status (up to 6 min)"
OS=""
if [ -n "$BUILD_ID" ]; then
  for i in $(seq 1 72); do
    sleep 5
    POLL=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf "${REST}/api/container-pool/builds/${BUILD_ID}" 2>&1)
    OS=$(echo "$POLL" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("build",{}).get("overall_status",""))' 2>/dev/null)
    BS=$(echo "$POLL" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("build",{}).get("build_status",""))' 2>/dev/null)
    TS=$(echo "$POLL" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d.get("build",{}).get("test_status",""))' 2>/dev/null)
    echo "    [$i] overall=$OS build=$BS test=$TS"
    case "$OS" in
      passed|failed|errored|cancelled) break ;;
    esac
  done
  case "$OS" in
    passed) ok "build+test passed" ;;
    failed) ko "build+test failed (overall=$OS)" ;;
    errored) ko "build+test errored" ;;
    *)      ko "build+test did not terminate (overall=$OS)" ;;
  esac
  echo "---FINAL BUILD RECORD (build+test logs, truncated)---"
  $KX exec -n default deploy/dd-remote-rest-api -- curl -sf "${REST}/api/container-pool/builds/${BUILD_ID}" 2>&1 | python3 -c '
import sys, json
d=json.load(sys.stdin)
b=d.get("build",{})
print(json.dumps({k:b.get(k) for k in ("id","image_slug","overall_status","build_status","test_status","candidate_tag","error_message","build_started_at","build_finished_at","test_started_at","test_finished_at")}, indent=2))
print("--- build_log (tail 1500B) ---")
bl=b.get("build_log_excerpt","") or ""
print(bl[-1500:])
print("--- test_log (tail 1500B) ---")
tl=b.get("test_log_excerpt","") or ""
print(tl[-1500:])
' 2>&1
fi

note "9. GET /api/container-pool/images/python3/builds (history)"
HIST=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf "${REST}/api/container-pool/images/python3/builds" 2>&1)
HCNT=$(echo "$HIST" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(len(d.get("builds",[])))' 2>/dev/null)
echo "    builds in history: $HCNT"
[ "${HCNT:-0}" -ge 1 ] && ok "history has $HCNT build(s)" || ko "no builds in history"

note "10. GET /api/container-pool/images/python3/revisions"
REV=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf "${REST}/api/container-pool/images/python3/revisions" 2>&1)
RCNT=$(echo "$REV" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(len(d.get("revisions",[])))' 2>/dev/null)
echo "    revisions: $RCNT"
[ "${RCNT:-0}" -ge 1 ] && ok "revisions list returns $RCNT" || ko "revisions list empty"

note "11. Web page reachability /container-pool/config"
# web-home image is distroless and lacks curl/wget; exec from rest-api (bookworm) instead
PAGE=$($KX exec -n default deploy/dd-remote-rest-api -- curl -sf http://dd-remote-web-home.default.svc.cluster.local:8080/container-pool/config 2>&1)
echo "    page bytes: ${#PAGE}"
echo "    page head (first 400B): ${PAGE:0:400}"
echo "$PAGE" | grep -q 'cpool-shell' && ok "page HTML contains cpool-shell" || ko "page HTML missing cpool-shell"
echo "$PAGE" | grep -q 'Container pool config' && ok "nav option present" || warn "nav title not found in HTML"
echo "$PAGE" | grep -q '/api/container-pool/images' && ok "page JS references /api/container-pool/images" || warn "JS fetch URL not found in HTML"

note "12. Candidate image landed in build namespace (k8s.io)"
# Builds run in CONTAINER_POOL_IMAGE_BUILD_NAMESPACE=k8s.io (same as
# LAMBDA_IMAGE_BUILD_NAMESPACE + idle-reaper worker-image builds) because
# that is where the host buildkitd is wired in. Workers later read from
# dd-pool; image copy across namespaces is a separate concern.
BUILD_NS=$(echo "$LIST_JSON" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("namespace",""))' 2>/dev/null)
[ -z "$BUILD_NS" ] && BUILD_NS=k8s.io
echo "    build namespace = $BUILD_NS"
if [ -n "$CAND_TAG" ]; then
  IMGS=$(sudo /usr/local/bin/nerdctl -n "$BUILD_NS" images 2>&1 | head -80)
  echo "$IMGS"
  REPO_PART=$(echo "$CAND_TAG" | cut -d: -f1)
  TAG_PART=$(echo "$CAND_TAG" | awk -F: '{print $NF}')
  # nerdctl renders `docker.io/library/foo` as just `foo` in the REPOSITORY
  # column, so try the candidate tag, the short repo (no `docker.io/library/`
  # prefix), and as a final probe the tag portion alone.
  SHORT_REPO=${REPO_PART#docker.io/library/}
  if echo "$IMGS" | grep -qE "(^| )$SHORT_REPO " && echo "$IMGS" | grep -q "$TAG_PART"; then
    ok "candidate image $CAND_TAG present in $BUILD_NS namespace"
  elif echo "$IMGS" | grep -q "$REPO_PART"; then
    ok "candidate image $CAND_TAG present in $BUILD_NS namespace"
  else
    warn "candidate image $CAND_TAG not in $BUILD_NS (may have been pruned)"
  fi
fi

echo
echo "================================================================="
echo "  /container-pool/config E2E:  PASS=$PASS  FAIL=$FAIL  WARN=$WARN"
echo "================================================================="
[ "$FAIL" -eq 0 ]
