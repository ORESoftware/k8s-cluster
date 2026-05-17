import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { randomBytes, randomUUID } from 'node:crypto';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);

const repoRoot = resolve(new URL('../../..', import.meta.url).pathname);
const sshHost = process.env.REMOTE_DEV_EC2_HOST ?? '54.91.17.58';
const sshUser = process.env.REMOTE_DEV_EC2_USER ?? 'ec2-user';
const sshKeyPath =
  process.env.REMOTE_DEV_EC2_KEY_PATH ?? '/Users/maca5/Downloads/main-key-pair.pem';
const shouldRun = process.env.DD_EC2_GLEAM_LAMBDA_INTEGRATION === '1';

type RuntimeName = 'nodejs' | 'python3' | 'ruby' | 'bash';

type LambdaCase = {
  id: string;
  slug: string;
  runtime: RuntimeName;
  mode: 'host' | 'container';
  functionBody: string;
  expected?: Record<string, unknown>;
  repeat?: number;
};

type ErrorCase = {
  id: string;
  slug: string;
  runtime: RuntimeName;
  mode: 'host' | 'container';
  functionBody: string;
  errorPattern: string;
};

type BlockedCase = {
  id: string;
  slug: string;
  runtime: RuntimeName;
  mode: 'host' | 'container';
  functionBody: string;
};

const entryCommands: Record<RuntimeName, string> = {
  nodejs:
    'env -i PATH="$PATH" NODE_ENV=production node --permission --allow-net child-runtimes/js-function-runner.mjs',
  python3:
    'env -i PATH="$PATH" PYTHONUNBUFFERED=1 python3 child-runtimes/python-function-runner.py',
  ruby: 'env -i PATH="$PATH" ruby child-runtimes/ruby-function-runner.rb',
  bash:
    'env -i PATH="$PATH" node --permission --allow-net --allow-child-process child-runtimes/bash-function-runner.mjs',
};

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\"'\"'")}'`;
}

function sqlString(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

function imageFor(runtime: RuntimeName, runId: string): string {
  return `docker.io/library/dd-lambda-${runtime}-runtime:${runId}`;
}

function insertSql(testCase: LambdaCase | ErrorCase | BlockedCase, runId: string): string {
  const containerized = testCase.mode === 'container';
  const containerImage = containerized ? imageFor(testCase.runtime, runId) : null;
  return `
insert into lambda_functions (
  id, slug, display_name, description, runtime, entry_command, function_body,
  reuse_key, idle_timeout_seconds, max_run_ms, containerized, container_image,
  container_build_status, container_built_at, status, labels, meta_data,
  is_soft_deleted, created_at, updated_at
) values (
  ${sqlString(testCase.id)}::uuid,
  ${sqlString(testCase.slug)},
  ${sqlString(`EC2 ${testCase.runtime} ${testCase.mode}`)},
  ${sqlString('EC2 true Gleam/containerd integration test')},
  ${sqlString(testCase.runtime)},
  ${sqlString(entryCommands[testCase.runtime])},
  ${sqlString(testCase.functionBody)},
  null,
  90,
  180000,
  ${containerized ? 'true' : 'false'},
  ${containerImage ? sqlString(containerImage) : 'null'},
  ${containerized ? sqlString('built') : sqlString('not_requested')},
  ${containerized ? 'now()' : 'null'},
  'active',
  ${sqlString(JSON.stringify([{ source: 'ec2-gleam-lambda-integration', runId }]))}::jsonb,
  ${sqlString(JSON.stringify({ runId, runtime: testCase.runtime, mode: testCase.mode }))}::jsonb,
  false,
  now(),
  now()
);`;
}

function buildCases(runId: string): {
  cases: LambdaCase[];
  errorCase: ErrorCase;
  blockedCase: BlockedCase;
} {
  const suffix = runId.toLowerCase();
  const cases: LambdaCase[] = [
    {
      id: randomUUID(),
      slug: `ec2-it-${suffix}-node-host`,
      runtime: 'nodejs',
      mode: 'host',
      functionBody:
        'return { status: 200, body: { runtime: "nodejs", echo: request.body, processHidden: globalThis.process === undefined } };',
      expected: { processHidden: true },
    },
    {
      id: randomUUID(),
      slug: `ec2-it-${suffix}-node-container`,
      runtime: 'nodejs',
      mode: 'container',
      functionBody:
        'return { status: 200, body: { runtime: "nodejs", echo: request.body, processHidden: globalThis.process === undefined } };',
      expected: { processHidden: true },
      repeat: 2,
    },
    {
      id: randomUUID(),
      slug: `ec2-it-${suffix}-python-container`,
      runtime: 'python3',
      mode: 'container',
      functionBody:
        'result = { "status": 200, "body": { "runtime": "python3", "echo": request.get("body"), "openAvailable": "open" in __builtins__ } }',
      expected: { openAvailable: false },
    },
    {
      id: randomUUID(),
      slug: `ec2-it-${suffix}-ruby-container`,
      runtime: 'ruby',
      mode: 'container',
      functionBody:
        'write_denied = false\nbegin\n  ::File.write("/opt/dd-lambda/write-denied", "x")\nrescue StandardError\n  write_denied = true\nend\n{ status: 200, body: { runtime: "ruby", echo: request["body"], uid: ::Process.uid, writeDenied: write_denied } }',
      expected: { uid: 10001, writeDenied: true },
    },
    {
      id: randomUUID(),
      slug: `ec2-it-${suffix}-bash-container`,
      runtime: 'bash',
      mode: 'container',
      functionBody:
        'if touch /opt/dd-lambda/write-denied 2>/tmp/write-denied.err; then denied=false; else denied=true; fi\nprintf \'%s\\n\' "{\\"status\\":200,\\"body\\":{\\"runtime\\":\\"bash\\",\\"uid\\":$(id -u),\\"writeDenied\\":$denied}}"',
      expected: { uid: 10001, writeDenied: true },
    },
  ];
  const errorCase: ErrorCase = {
    id: randomUUID(),
    slug: `ec2-it-${suffix}-node-error`,
    runtime: 'nodejs',
    mode: 'host',
    functionBody: 'throw new Error("ec2-intentional-node-error");',
    errorPattern: 'ec2-intentional-node-error',
  };
  const blockedCase: BlockedCase = {
    id: randomUUID(),
    slug: `ec2-it-${suffix}-python-host-blocked`,
    runtime: 'python3',
    mode: 'host',
    functionBody:
      'result = { "status": 200, "body": { "runtime": "python3", "blocked": False } }',
  };
  return { cases, errorCase, blockedCase };
}

async function run(
  file: string,
  args: string[],
  options: { cwd?: string; timeout?: number; maxBuffer?: number } = {},
): Promise<string> {
  const { stdout, stderr } = await execFileAsync(file, args, {
    cwd: options.cwd,
    timeout: options.timeout ?? 60_000,
    maxBuffer: options.maxBuffer ?? 10 * 1024 * 1024,
  });
  return `${stdout}${stderr}`.trim();
}

async function sshRun(command: string, timeout = 60_000): Promise<string> {
  return run(
    'ssh',
    [
      '-i',
      sshKeyPath,
      '-o',
      'StrictHostKeyChecking=accept-new',
      '-o',
      'BatchMode=yes',
      '-o',
      'ConnectTimeout=20',
      `${sshUser}@${sshHost}`,
      command,
    ],
    { timeout, maxBuffer: 30 * 1024 * 1024 },
  );
}

async function scpToRemote(localPath: string, remotePath: string, timeout = 120_000): Promise<void> {
  await run(
    'scp',
    [
      '-i',
      sshKeyPath,
      '-o',
      'StrictHostKeyChecking=accept-new',
      '-o',
      'BatchMode=yes',
      localPath,
      `${sshUser}@${sshHost}:${remotePath}`,
    ],
    { timeout, maxBuffer: 10 * 1024 * 1024 },
  );
}

function remoteScript(runId: string, remoteArchive: string, remoteCases: string, remoteSeed: string): string {
  const nodeImage = imageFor('nodejs', runId);
  const pythonImage = imageFor('python3', runId);
  const rubyImage = imageFor('ruby', runId);
  const bashImage = imageFor('bash', runId);

  return `
set -Eeuo pipefail
run_id=${shellQuote(runId)}
remote_archive=${shellQuote(remoteArchive)}
remote_cases=${shellQuote(remoteCases)}
remote_seed=${shellQuote(remoteSeed)}
work="/tmp/dd-gleam-lambda-it-$run_id"
repo="$work/repo"
pg_name="dd-lambda-it-pg-$run_id"
runner_name="dd-lambda-it-runner-$run_id"
node_image=${shellQuote(nodeImage)}
python_image=${shellQuote(pythonImage)}
ruby_image=${shellQuote(rubyImage)}
bash_image=${shellQuote(bashImage)}

cleanup() {
  kubectl delete pod "$runner_name" "$pg_name" -n default --ignore-not-found --force --grace-period=0 >/dev/null 2>&1 || true
  kubectl delete service "$pg_name" -n default --ignore-not-found >/dev/null 2>&1 || true
  sudo -n nerdctl -n k8s.io rmi "$node_image" "$python_image" "$ruby_image" "$bash_image" >/dev/null 2>&1 || true
  sudo -n rm -rf "$work" "$remote_archive" "$remote_cases" "$remote_seed"
}
trap cleanup EXIT

command -v sudo >/dev/null
command -v nerdctl >/dev/null
command -v curl >/dev/null
command -v kubectl >/dev/null
command -v python3 >/dev/null
command -v ctr >/dev/null
test -d /run/containerd
test -S /run/containerd/containerd.sock
sudo -n nerdctl -n k8s.io version >/dev/null
sudo -n ctr -n k8s.io version >/dev/null

sudo -n rm -rf "$work"
mkdir -p "$repo"
tar -xzf "$remote_archive" -C "$repo"
find "$repo" -name '._*' -delete
rm -rf "$repo/remote/gleam-lambda-runner/build" "$repo/remote/libs/pg-defs/generated/gleam/build"
cp "$remote_cases" "$work/cases.json"
cp "$remote_seed" "$work/seed.sql"

build_runtime() {
  runtime="$1"
  image="$2"
  log="$work/build-$runtime.log"
  if ! sudo -n nerdctl -n k8s.io build -f "$repo/remote/gleam-lambda-runner/runtime-images/$runtime.Dockerfile" -t "$image" "$repo/remote/gleam-lambda-runner" >"$log" 2>&1; then
    cat "$log" >&2
    exit 1
  fi
}

build_runtime nodejs "$node_image"
build_runtime python3 "$python_image"
build_runtime ruby "$ruby_image"
build_runtime bash "$bash_image"

cat >"$work/postgres.yaml" <<YAML
apiVersion: v1
kind: Pod
metadata:
  name: $pg_name
  namespace: default
  labels:
    app: $pg_name
spec:
  restartPolicy: Never
  containers:
    - name: postgres
      image: docker.io/library/postgres:16-alpine
      env:
        - name: POSTGRES_PASSWORD
          value: postgres
      ports:
        - containerPort: 5432
---
apiVersion: v1
kind: Service
metadata:
  name: $pg_name
  namespace: default
spec:
  selector:
    app: $pg_name
  ports:
    - name: postgres
      port: 5432
      targetPort: 5432
YAML

kubectl apply -f "$work/postgres.yaml" >/dev/null
kubectl wait --for=condition=Ready "pod/$pg_name" -n default --timeout=180s >/dev/null

for _ in $(seq 1 90); do
  if kubectl exec "$pg_name" -n default -- pg_isready -U postgres -d postgres >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
kubectl exec "$pg_name" -n default -- pg_isready -U postgres -d postgres >/dev/null

{
  echo 'create extension if not exists pgcrypto;'
  cat "$repo/remote/libs/pg-defs/schema/schema.sql"
  cat "$work/seed.sql"
} | kubectl exec -i "$pg_name" -n default -- psql -U postgres -d postgres -v ON_ERROR_STOP=1 >/dev/null

cat >"$work/runner.yaml" <<YAML
apiVersion: v1
kind: Pod
metadata:
  name: $runner_name
  namespace: default
  labels:
    app: $runner_name
spec:
  restartPolicy: Never
  containers:
    - name: gleam-lambda-runner
      image: ghcr.io/gleam-lang/gleam:v1.16.0-erlang-alpine
      securityContext:
        privileged: true
      command:
        - /bin/sh
        - -lc
      args:
        - |
          set -eu
          if ! apk add --no-cache --repository=https://dl-cdn.alpinelinux.org/alpine/edge/main --repository=https://dl-cdn.alpinelinux.org/alpine/edge/community nodejs-current python3 ruby bash postgresql-client gcompat libc6-compat >/tmp/dd-lambda-runner-apk.log 2>&1; then
            cat /tmp/dd-lambda-runner-apk.log
            exit 1
          fi
          cd /opt/dd-next-1/remote/gleam-lambda-runner
          gleam deps download || { sleep 10; gleam deps download; } || { sleep 10; gleam deps download; }
          exec gleam run
      env:
        - name: LAMBDA_DATABASE_URL
          value: postgres://postgres:postgres@$pg_name.default.svc.cluster.local:5432/postgres
        - name: LAMBDA_PREWARM_RUNTIMES
          value: ""
        - name: LAMBDA_PREWARM_CONTAINER_RUNTIMES
          value: ""
        - name: LAMBDA_ALLOW_HOST_RUNTIMES
          value: nodejs
        - name: LAMBDA_CONTAINER_NERDCTL
          value: /usr/local/bin/nerdctl
        - name: LAMBDA_CONTAINER_NAMESPACE
          value: k8s.io
        - name: LAMBDA_CONTAINER_NETWORK
          value: host
        - name: LAMBDA_CONTAINER_RUNNER
          value: ctr
        - name: LAMBDA_CONTAINER_CTR
          value: /usr/local/bin/ctr
        - name: LAMBDA_CONTAINER_MEMORY_BYTES
          value: "268435456"
        - name: LAMBDA_NODEJS_CONTAINER_IMAGE
          value: $node_image
        - name: LAMBDA_PYTHON3_CONTAINER_IMAGE
          value: $python_image
        - name: LAMBDA_RUBY_CONTAINER_IMAGE
          value: $ruby_image
        - name: LAMBDA_BASH_CONTAINER_IMAGE
          value: $bash_image
      ports:
        - containerPort: 8083
      volumeMounts:
        - name: repo
          mountPath: /opt/dd-next-1
        - name: work
          mountPath: /opt/dd-it
          readOnly: true
        - name: containerd-run
          mountPath: /run/containerd
          mountPropagation: Bidirectional
        - name: containerd-root
          mountPath: /var/lib/containerd
          mountPropagation: Bidirectional
        - name: nerdctl-bin
          mountPath: /usr/local/bin/nerdctl
          readOnly: true
        - name: ctr-bin
          mountPath: /usr/local/bin/ctr
          readOnly: true
  volumes:
    - name: repo
      hostPath:
        path: $repo
        type: Directory
    - name: work
      hostPath:
        path: $work
        type: Directory
    - name: containerd-run
      hostPath:
        path: /run/containerd
        type: Directory
    - name: containerd-root
      hostPath:
        path: /var/lib/containerd
        type: Directory
    - name: nerdctl-bin
      hostPath:
        path: /usr/local/bin/nerdctl
        type: File
    - name: ctr-bin
      hostPath:
        path: /usr/local/bin/ctr
        type: File
YAML

kubectl apply -f "$work/runner.yaml" >/dev/null

for _ in $(seq 1 120); do
  if kubectl exec "$runner_name" -n default -- wget -qO- http://127.0.0.1:8083/healthz >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
if ! kubectl exec "$runner_name" -n default -- wget -qO- http://127.0.0.1:8083/healthz >/dev/null 2>&1; then
  kubectl logs "$runner_name" -n default >&2 || true
  exit 1
fi

cat >"$work/verify.py" <<'PY'
import json
import os
import re
import urllib.error
import urllib.request

work = os.environ["WORK_DIR"]
with open(os.path.join(work, "cases.json"), "r", encoding="utf-8") as handle:
    payload = json.load(handle)

def post_json(function_id, body):
    request = urllib.request.Request(
        f"http://127.0.0.1:8083/invoke/{function_id}",
        data=json.dumps(body).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=240) as response:
            raw = response.read().decode("utf-8")
    except urllib.error.HTTPError as error:
        raw = error.read().decode("utf-8", errors="replace")
        raise AssertionError(f"outer HTTP {error.code} for {function_id}: {raw}") from error
    outer = json.loads(raw)
    if not outer.get("ok"):
        raise AssertionError(f"outer runner error for {function_id}: {outer}")
    child = json.loads(outer["output"])
    return child

def post_outer(function_id, body):
    request = urllib.request.Request(
        f"http://127.0.0.1:8083/invoke/{function_id}",
        data=json.dumps(body).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=240) as response:
            return response.status, json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read().decode("utf-8"))

results = []
for case in payload["cases"]:
    repeats = int(case.get("repeat") or 1)
    last_child = None
    for index in range(repeats):
        last_child = post_json(case["id"], {"body": {"runtime": case["runtime"], "mode": case["mode"], "index": index}})
        if not last_child.get("ok"):
            raise AssertionError(f"child error for {case['slug']}: {last_child}")
        result = last_child.get("result") or {}
        body = result.get("body") or {}
        if body.get("runtime") != case["runtime"]:
            raise AssertionError(f"runtime mismatch for {case['slug']}: {body}")
        for key, expected in (case.get("expected") or {}).items():
            if body.get(key) != expected:
                raise AssertionError(f"expected {case['slug']} body[{key}]={expected!r}, got {body.get(key)!r}: {body}")
    results.append({
        "slug": case["slug"],
        "runtime": case["runtime"],
        "mode": case["mode"],
        "cachedFunctions": last_child.get("cachedFunctions"),
    })

error_case = payload["errorCase"]
error_child = post_json(error_case["id"], {"body": {"intent": "fail"}})
if error_child.get("ok") is not False:
    raise AssertionError(f"expected invocation-level error, got {error_child}")
if not re.search(error_case["errorPattern"], error_child.get("error") or ""):
    raise AssertionError(f"error did not match {error_case['errorPattern']}: {error_child}")

blocked_case = payload["blockedCase"]
blocked_status, blocked_outer = post_outer(blocked_case["id"], {"body": {"intent": "blocked"}})
if blocked_status != 502 or blocked_outer.get("ok") is not False:
    raise AssertionError(f"expected host runtime policy rejection, got {blocked_status}: {blocked_outer}")
if "requires containerized=true" not in (blocked_outer.get("error") or ""):
    raise AssertionError(f"blocked host runtime error was not explicit: {blocked_outer}")

after_error = post_json(payload["cases"][0]["id"], {"body": {"runtime": payload["cases"][0]["runtime"], "mode": "after-error"}})
if not after_error.get("ok"):
    raise AssertionError(f"runner did not recover after failed invocation: {after_error}")

metrics = urllib.request.urlopen("http://127.0.0.1:8083/metrics", timeout=30).read().decode("utf-8")
if "dd_lambda_runner_invocations_total" not in metrics:
    raise AssertionError("runner metrics missing invocation counter")

print("DD_GLEAM_LAMBDA_IT_RESULT=" + json.dumps({
    "ok": True,
    "invoked": results,
    "errorCase": {"slug": error_case["slug"], "ok": error_child.get("ok")},
    "blockedCase": {"slug": blocked_case["slug"], "ok": blocked_outer.get("ok"), "status": blocked_status},
    "afterErrorOk": after_error.get("ok"),
    "metricsSample": [line for line in metrics.splitlines() if line.startswith("dd_lambda_runner_")][:8],
}, sort_keys=True))
PY

if ! kubectl exec "$runner_name" -n default -- env WORK_DIR=/opt/dd-it python3 /opt/dd-it/verify.py; then
  kubectl logs "$runner_name" -n default >&2 || true
  exit 1
fi
`;
}

test(
  'EC2 Gleam lambda runner invokes host and real containerd containers',
  { timeout: 20 * 60_000 },
  async (t) => {
    if (!shouldRun) {
      t.skip('set DD_EC2_GLEAM_LAMBDA_INTEGRATION=1 to run the EC2/containerd integration');
      return;
    }
    if (!existsSync(sshKeyPath)) {
      t.skip(`EC2 SSH key not found at ${sshKeyPath}`);
      return;
    }

    const runId = randomBytes(4).toString('hex');
    const { cases, errorCase, blockedCase } = buildCases(runId);
    const tempDir = mkdtempSync(join(tmpdir(), 'dd-gleam-lambda-it-'));
    const archivePath = join(tempDir, 'runner-input.tgz');
    const casesPath = join(tempDir, 'cases.json');
    const seedPath = join(tempDir, 'seed.sql');
    const remoteArchive = `/tmp/dd-gleam-lambda-it-${runId}.tgz`;
    const remoteCases = `/tmp/dd-gleam-lambda-it-${runId}.cases.json`;
    const remoteSeed = `/tmp/dd-gleam-lambda-it-${runId}.seed.sql`;

    try {
      await run(
        'env',
        [
          'COPYFILE_DISABLE=1',
          'tar',
          '--exclude=._*',
          '--exclude=*/._*',
          '--exclude=remote/gleam-lambda-runner/build',
          '--exclude=remote/libs/pg-defs/generated/gleam/build',
          '-czf',
          archivePath,
          'remote/gleam-lambda-runner',
          'remote/libs/pg-defs/generated/gleam',
          'remote/libs/pg-defs/schema/schema.sql',
        ],
        { cwd: repoRoot, timeout: 60_000 },
      );
      writeFileSync(casesPath, JSON.stringify({ cases, errorCase, blockedCase }, null, 2));
      writeFileSync(
        seedPath,
        [...cases, errorCase, blockedCase].map((row) => insertSql(row, runId)).join('\n'),
      );

      await scpToRemote(archivePath, remoteArchive);
      await scpToRemote(casesPath, remoteCases);
      await scpToRemote(seedPath, remoteSeed);

      const output = await sshRun(remoteScript(runId, remoteArchive, remoteCases, remoteSeed), 20 * 60_000);
      const resultLine = output
        .split('\n')
        .find((line) => line.startsWith('DD_GLEAM_LAMBDA_IT_RESULT='));
      assert.ok(resultLine, `missing integration result marker in output:\n${output}`);
      const parsed = JSON.parse(resultLine.slice('DD_GLEAM_LAMBDA_IT_RESULT='.length));
      assert.equal(parsed.ok, true);
      assert.equal(parsed.invoked.length, cases.length);
      assert.equal(parsed.errorCase.ok, false);
      assert.equal(parsed.blockedCase.ok, false);
      assert.equal(parsed.afterErrorOk, true);
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  },
);
