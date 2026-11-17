#!/usr/bin/env node

import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import {
  closeSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const natsServerBin = process.env.NATS_SERVER_BIN ?? 'nats-server';
const natsCliBin = process.env.NATS_CLI_BIN ?? 'nats';
const keepArtifacts = process.env.KEEP_NATS_E2E_ARTIFACTS === '1';
const workDir = mkdtempSync(join(tmpdir(), 'dd-multicloud-nats-'));
const appUser = 'dd-app';
const appPassword = `dd-app-${randomBytes(24).toString('hex')}`;
const systemUser = 'dd-system';
const systemPassword = `dd-system-${randomBytes(24).toString('hex')}`;

const regions = [
  { name: 'aws', cluster: 'ores-aws', domain: 'ORES_MULTICLOUD', base: 14000 },
  { name: 'gcp', cluster: 'ores-gcp', domain: 'ORES_MULTICLOUD', base: 24000 },
  { name: 'azure', cluster: 'ores-azure', domain: 'ORES_MULTICLOUD', base: 34000 },
];

const processes = new Map();

function redact(value) {
  return String(value)
    .replaceAll(appPassword, '<redacted-app-password>')
    .replaceAll(systemPassword, '<redacted-system-password>');
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: workDir,
    encoding: 'utf8',
    timeout: options.timeout ?? 30_000,
    env: { ...process.env, ...(options.env ?? {}) },
    stdio: options.stdio ?? 'pipe',
  });

  if (result.error || result.status !== 0) {
    const details = [result.error?.message, result.stdout, result.stderr]
      .filter(Boolean)
      .map(redact)
      .join('\n');
    throw new Error(
      `${basename(command)} ${redact(args.join(' '))} failed with status ${result.status ?? 'unknown'}\n${details}`,
    );
  }

  return result.stdout ?? '';
}

function runExpectFailure(command, args, expectedPattern) {
  const result = spawnSync(command, args, {
    cwd: workDir,
    encoding: 'utf8',
    timeout: 10_000,
    env: process.env,
  });
  assert.notEqual(result.status, 0, `${basename(command)} unexpectedly succeeded`);
  const output = redact(`${result.stdout ?? ''}\n${result.stderr ?? ''}`);
  assert.match(output, expectedPattern, `${basename(command)} failed for an unexpected reason: ${output}`);
}

function nodePorts(region, ordinal) {
  return {
    client: region.base + 220 + ordinal,
    route: region.base + 620 + ordinal,
    gateway: region.base + 720 + ordinal,
    monitor: region.base + 820 + ordinal,
  };
}

function regionByName(name) {
  const region = regions.find((candidate) => candidate.name === name);
  assert.ok(region, `unknown region ${name}`);
  return region;
}

function nodeName(region, ordinal) {
  return `${region.name}-${ordinal}`;
}

function certificateBlock(cert, key) {
  return `{
      cert_file: ${JSON.stringify(cert)}
      key_file: ${JSON.stringify(key)}
      ca_file: ${JSON.stringify(join(workDir, 'pki', 'ca.crt'))}
      verify: true
      timeout: 2
    }`;
}

function writePki() {
  const pkiDir = join(workDir, 'pki');
  mkdirSync(pkiDir, { recursive: true });
  const caKey = join(pkiDir, 'ca.key');
  const caCert = join(pkiDir, 'ca.crt');

  run('openssl', [
    'req',
    '-x509',
    '-newkey',
    'rsa:2048',
    '-nodes',
    '-sha256',
    '-days',
    '2',
    '-keyout',
    caKey,
    '-out',
    caCert,
    '-subj',
    '/CN=dd-multicloud-nats-e2e-ca',
  ]);

  const serverExt = join(pkiDir, 'server.ext');
  writeFileSync(
    serverExt,
    [
      'basicConstraints=CA:FALSE',
      'keyUsage=digitalSignature,keyEncipherment',
      'extendedKeyUsage=serverAuth,clientAuth',
      'subjectAltName=IP:127.0.0.1,DNS:localhost',
      '',
    ].join('\n'),
  );

  for (const role of ['client', 'route', 'gateway']) {
    const key = join(pkiDir, `${role}.key`);
    const csr = join(pkiDir, `${role}.csr`);
    const cert = join(pkiDir, `${role}.crt`);
    run('openssl', [
      'req',
      '-newkey',
      'rsa:2048',
      '-nodes',
      '-sha256',
      '-keyout',
      key,
      '-out',
      csr,
      '-subj',
      `/CN=dd-nats-${role}`,
    ]);
    run('openssl', [
      'x509',
      '-req',
      '-sha256',
      '-days',
      '2',
      '-in',
      csr,
      '-CA',
      caCert,
      '-CAkey',
      caKey,
      '-CAcreateserial',
      '-out',
      cert,
      '-extfile',
      serverExt,
    ]);
  }
}

function writeConfigs() {
  const pkiDir = join(workDir, 'pki');

  for (const region of regions) {
    const regionDir = join(workDir, region.name);
    mkdirSync(regionDir, { recursive: true });
    const routeUrls = [1, 2, 3].map((ordinal) => {
      const ports = nodePorts(region, ordinal);
      return `tls://127.0.0.1:${ports.route}`;
    });

    for (const ordinal of [1, 2, 3]) {
      const ports = nodePorts(region, ordinal);
      const remotes = regions
        .filter((candidate) => candidate.name !== region.name)
        .map((peer) => ({
          name: peer.cluster,
          urls: [1, 2, 3].map(
            (peerOrdinal) => `nats://127.0.0.1:${nodePorts(peer, peerOrdinal).gateway}`,
          ),
        }));
      const storeDir = join(regionDir, `store-${ordinal}`);
      mkdirSync(storeDir, { recursive: true });
      const config = `
server_name: ${JSON.stringify(nodeName(region, ordinal))}
host: "127.0.0.1"
port: ${ports.client}
http_port: ${ports.monitor}
accounts {
  DD_APP {
    jetstream: enabled
    users: [
      { user: ${JSON.stringify(appUser)}, password: ${JSON.stringify(appPassword)} }
    ]
  }
  SYS {
    users: [
      { user: ${JSON.stringify(systemUser)}, password: ${JSON.stringify(systemPassword)} }
    ]
  }
}
system_account: SYS
tls: ${certificateBlock(join(pkiDir, 'client.crt'), join(pkiDir, 'client.key'))}

cluster {
  name: ${JSON.stringify(region.cluster)}
  listen: ${JSON.stringify(`127.0.0.1:${ports.route}`)}
  routes: ${JSON.stringify(routeUrls)}
  tls: ${certificateBlock(join(pkiDir, 'route.crt'), join(pkiDir, 'route.key'))}
}

gateway {
  name: ${JSON.stringify(region.cluster)}
  listen: ${JSON.stringify(`127.0.0.1:${ports.gateway}`)}
  reject_unknown_cluster: true
  gateways: ${JSON.stringify(remotes)}
  tls: ${certificateBlock(join(pkiDir, 'gateway.crt'), join(pkiDir, 'gateway.key'))}
}

jetstream {
  domain: ${JSON.stringify(region.domain)}
  store_dir: ${JSON.stringify(storeDir)}
  max_memory_store: 128MB
  max_file_store: 1GB
}
`;
      writeFileSync(join(regionDir, `${nodeName(region, ordinal)}.conf`), config);
    }
  }
}

function startNode(region, ordinal) {
  const name = nodeName(region, ordinal);
  assert.equal(processes.has(name), false, `${name} is already running`);
  const logPath = join(workDir, region.name, `${name}.log`);
  const logFd = openSync(logPath, 'a');
  const child = spawn(
    natsServerBin,
    ['--config', join(workDir, region.name, `${name}.conf`)],
    {
      cwd: workDir,
      env: process.env,
      stdio: ['ignore', logFd, logFd],
    },
  );
  closeSync(logFd);
  child.on('exit', () => processes.delete(name));
  processes.set(name, child);
}

function startRegion(region) {
  for (const ordinal of [1, 2, 3]) startNode(region, ordinal);
}

async function stopRegion(region) {
  const stopping = [];
  for (const ordinal of [1, 2, 3]) {
    const name = nodeName(region, ordinal);
    const child = processes.get(name);
    if (!child) continue;
    stopping.push(
      new Promise((resolve) => {
        child.once('exit', resolve);
        child.kill('SIGTERM');
      }),
    );
  }
  await Promise.race([Promise.all(stopping), delay(10_000)]);
  for (const ordinal of [1, 2, 3]) {
    const child = processes.get(nodeName(region, ordinal));
    if (child) child.kill('SIGKILL');
  }
}

async function stopAll() {
  for (const region of regions) await stopRegion(region);
}

async function fetchJson(url) {
  const response = await fetch(url, { signal: AbortSignal.timeout(2_000) });
  assert.equal(response.ok, true, `${url} returned ${response.status}`);
  return response.json();
}

async function waitFor(description, predicate, timeoutMs = 45_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      if (await predicate()) return;
    } catch (error) {
      lastError = error;
    }
    await delay(250);
  }
  throw new Error(
    `timed out waiting for ${description}${lastError ? `: ${redact(lastError.message)}` : ''}`,
  );
}

async function waitForRegion(region, expectedGateways = 2) {
  for (const ordinal of [1, 2, 3]) {
    const ports = nodePorts(region, ordinal);
    await waitFor(`${nodeName(region, ordinal)} health`, async () => {
      const response = await fetch(
        `http://127.0.0.1:${ports.monitor}/healthz?js-enabled-only=true`,
        { signal: AbortSignal.timeout(2_000) },
      );
      return response.ok;
    });
    await waitFor(`${nodeName(region, ordinal)} regional routes`, async () => {
      const routez = await fetchJson(`http://127.0.0.1:${ports.monitor}/routez`);
      // NATS may open a route pool between the two peer servers. The exact
      // connection count is an implementation detail; two reachable peers is
      // the invariant required by a three-server regional cluster.
      return routez.num_routes >= 2;
    });
    await waitFor(`${nodeName(region, ordinal)} outbound gateways`, async () => {
      const gatewayz = await fetchJson(`http://127.0.0.1:${ports.monitor}/gatewayz`);
      return (
        Object.values(gatewayz.outbound_gateways ?? {}).filter((gateway) => gateway.connection)
          .length === expectedGateways
      );
    });
  }
}

async function waitForMetaCluster(region, expectedSize) {
  const ports = nodePorts(region, 1);
  await waitFor(`${region.name} JetStream meta cluster size ${expectedSize}`, async () => {
    const jsz = await fetchJson(`http://127.0.0.1:${ports.monitor}/jsz`);
    return (
      typeof jsz.meta_cluster?.leader === 'string' &&
      jsz.meta_cluster.leader.length > 0 &&
      jsz.meta_cluster.cluster_size === expectedSize &&
      jsz.meta_cluster.pending === 0
    );
  });
}

function cliConnection(regionName, options = {}) {
  const region = regionByName(regionName);
  const ports = nodePorts(region, options.ordinal ?? 1);
  const args = [
    `--server=nats://127.0.0.1:${ports.client}`,
    `--tlsca=${join(workDir, 'pki', 'ca.crt')}`,
    '--timeout=5s',
    '--no-context',
  ];
  if (options.includeCertificate !== false) {
    args.push(
      `--tlscert=${join(workDir, 'pki', 'client.crt')}`,
      `--tlskey=${join(workDir, 'pki', 'client.key')}`,
    );
  }
  if (options.credentials !== false) {
    args.push(`--user=${appUser}`, `--password=${options.password ?? appPassword}`);
  }
  if (options.domain !== false) args.push(`--js-domain=${region.domain}`);
  return args;
}

function systemConnection(regionName) {
  const region = regionByName(regionName);
  const ports = nodePorts(region, 1);
  return [
    `--server=nats://127.0.0.1:${ports.client}`,
    `--tlsca=${join(workDir, 'pki', 'ca.crt')}`,
    `--tlscert=${join(workDir, 'pki', 'client.crt')}`,
    `--tlskey=${join(workDir, 'pki', 'client.key')}`,
    `--user=${systemUser}`,
    `--password=${systemPassword}`,
    '--timeout=5s',
    '--no-context',
  ];
}

function nats(regionName, commandArgs, options = {}) {
  return run(natsCliBin, [...cliConnection(regionName, options), ...commandArgs], options);
}

async function waitForChild(child, timeoutMs, description) {
  let stdout = '';
  let stderr = '';
  child.stdout?.on('data', (chunk) => {
    stdout += chunk.toString();
  });
  child.stderr?.on('data', (chunk) => {
    stderr += chunk.toString();
  });
  const outcome = await Promise.race([
    new Promise((resolve) => child.once('exit', (code, signal) => resolve({ code, signal }))),
    delay(timeoutMs).then(() => ({ timeout: true })),
  ]);
  if (outcome.timeout) {
    child.kill('SIGKILL');
    throw new Error(`${description} timed out\n${redact(stdout)}\n${redact(stderr)}`);
  }
  if (outcome.code !== 0) {
    throw new Error(
      `${description} exited with ${outcome.code ?? outcome.signal}\n${redact(stdout)}\n${redact(stderr)}`,
    );
  }
  return stdout;
}

async function assertCoreDelivery(fromRegion, toRegion, subject, payload) {
  const subscriber = spawn(
    natsCliBin,
    [...cliConnection(toRegion, { domain: false }), 'subscribe', '--raw', '--count=1', subject],
    { cwd: workDir, env: process.env, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  await delay(750);
  nats(fromRegion, ['publish', '--quiet', subject, payload], { domain: false });
  const received = await waitForChild(
    subscriber,
    10_000,
    `${fromRegion} -> ${toRegion} Core NATS delivery`,
  );
  assert.equal(received.trim(), payload);
}

function assertServerVersion() {
  const version = run(natsServerBin, ['--version']).trim();
  assert.match(version, /v2\.14\.6$/);
  run(natsCliBin, ['--version']);
}

function assertAuthBoundaries() {
  const validArgs = [...cliConnection('aws', { domain: false }), 'server', 'check', 'connection'];
  run(natsCliBin, validArgs);
  run(natsCliBin, [...systemConnection('azure'), 'server', 'check', 'connection']);

  runExpectFailure(
    natsCliBin,
    [...cliConnection('aws', { domain: false, includeCertificate: false }), 'server', 'check', 'connection'],
    /certificate required|tls|handshake|connection closed/i,
  );
  runExpectFailure(
    natsCliBin,
    [
      ...cliConnection('aws', { domain: false, password: 'intentionally-wrong' }),
      'server',
      'check',
      'connection',
    ],
    /authorization violation|authentication|connection closed/i,
  );
}

function createPlacedStream(regionName, streamName, subjectRoot) {
  const region = regionByName(regionName);
  nats(regionName, [
    'stream',
    'add',
    streamName,
    `--subjects=${subjectRoot}.>`,
    '--storage=file',
    '--replicas=3',
    `--cluster=${region.cluster}`,
    '--defaults',
  ]);

  nats(regionName, ['publish', '--jetstream', '--quiet', `${subjectRoot}.order`, '{"id":1}']);
  const info = JSON.parse(nats(regionName, ['stream', 'info', streamName, '--json']));
  assert.equal(info.config.num_replicas, 3);
  assert.equal(info.state.messages, 1);
  assert.equal(info.cluster.name, region.cluster);
  assert.match(info.cluster.leader, new RegExp(`^${regionName}-`));
  assert.equal(info.cluster.replicas.length, 2);
  assert.ok(info.cluster.replicas.every((replica) => replica.current === true));
  assert.ok(info.cluster.replicas.every((replica) => replica.name.startsWith(`${regionName}-`)));

  // The JetStream catalog is supercluster-wide. Explicit stream placement,
  // rather than the API domain, is what keeps durable replicas region-local.
  for (const remoteRegion of regions.filter((candidate) => candidate.name !== regionName)) {
    const remoteView = JSON.parse(nats(remoteRegion.name, ['stream', 'info', streamName, '--json']));
    assert.equal(remoteView.cluster.name, region.cluster);
    assert.match(remoteView.cluster.leader, new RegExp(`^${regionName}-`));
    assert.ok(
      remoteView.cluster.replicas.every((replica) => replica.name.startsWith(`${regionName}-`)),
    );
  }
}

function assertRegionalJetStream() {
  runExpectFailure(
    natsCliBin,
    [
      ...cliConnection('aws'),
      'stream',
      'add',
      'DD_INVALID_E2E',
      '--subjects=dd.e2e.invalid.>',
      '--storage=file',
      '--replicas=3',
      '--cluster=ores-missing',
      '--defaults',
    ],
    /no suitable peers|placement|cluster.*not found/i,
  );
  createPlacedStream('aws', 'DD_AWS_E2E', 'dd.e2e.aws');
  createPlacedStream('gcp', 'DD_GCP_E2E', 'dd.e2e.gcp');
}

async function waitForPlacedStreamHealthy(regionName, streamName) {
  await waitFor(`${streamName} regional R3 group to recover`, () => {
    const info = JSON.parse(nats(regionName, ['stream', 'info', streamName, '--json']));
    return (
      info.cluster?.name === regionByName(regionName).cluster &&
      info.cluster?.leader?.startsWith(`${regionName}-`) &&
      info.cluster?.replicas?.length === 2 &&
      info.cluster.replicas.every((replica) =>
        replica.current === true && replica.name.startsWith(`${regionName}-`)
      )
    );
  });
}

function printLogTails() {
  for (const region of regions) {
    for (const ordinal of [1, 2, 3]) {
      const path = join(workDir, region.name, `${nodeName(region, ordinal)}.log`);
      if (!existsSync(path)) continue;
      const lines = readFileSync(path, 'utf8').trim().split('\n').slice(-20);
      process.stderr.write(`\n--- ${nodeName(region, ordinal)} log tail ---\n${redact(lines.join('\n'))}\n`);
    }
  }
}

async function main() {
  assertServerVersion();
  writePki();
  writeConfigs();

  const aws = regionByName('aws');
  const gcp = regionByName('gcp');

  // One JetStream domain has one metadata quorum. All configured gateway
  // clusters must be reachable before activation; a lone region must not
  // bootstrap a competing partial authority.
  for (const region of regions) startRegion(region);
  for (const region of regions) await waitForRegion(region, 2);
  for (const region of regions) await waitForMetaCluster(region, 9);
  // Allow the elected metadata leaders to finish exchanging cluster
  // inventory before the first explicitly placed stream is created.
  await delay(1_000);

  assertAuthBoundaries();
  await assertCoreDelivery('aws', 'azure', 'dd.e2e.core.initial', 'aws-to-azure');
  await assertCoreDelivery('gcp', 'aws', 'dd.e2e.core.reverse', 'gcp-to-aws');
  assertRegionalJetStream();

  await stopRegion(gcp);
  await waitFor('AWS to observe only the Azure gateway', async () => {
    const gatewayz = await fetchJson(`http://127.0.0.1:${nodePorts(regionByName('aws'), 1).monitor}/gatewayz`);
    return (
      Object.values(gatewayz.outbound_gateways ?? {}).filter((gateway) => gateway.connection)
        .length === 1
    );
  });
  nats('aws', ['publish', '--jetstream', '--quiet', 'dd.e2e.aws.order', '{"id":2}']);
  const awsStreamDuringOutage = JSON.parse(
    nats('aws', ['stream', 'info', 'DD_AWS_E2E', '--json']),
  );
  assert.equal(awsStreamDuringOutage.state.messages, 2);
  runExpectFailure(
    natsCliBin,
    [
      ...cliConnection('aws'),
      'publish',
      '--jetstream',
      '--quiet',
      'dd.e2e.gcp.order',
      '{"id":2}',
    ],
    /no response|no responders|timeout|unavailable|context deadline|no stream response/i,
  );
  await assertCoreDelivery('aws', 'azure', 'dd.e2e.core.during-gcp-outage', 'survives-gcp-outage');

  startRegion(gcp);
  await waitForRegion(gcp);
  await waitForRegion(regionByName('aws'));
  await waitForRegion(regionByName('azure'));
  for (const region of regions) await waitForMetaCluster(region, 9);
  await waitForPlacedStreamHealthy('gcp', 'DD_GCP_E2E');
  nats('gcp', ['publish', '--jetstream', '--quiet', 'dd.e2e.gcp.order', '{"id":3}']);
  const gcpStreamAfterRecovery = JSON.parse(
    nats('gcp', ['stream', 'info', 'DD_GCP_E2E', '--json']),
  );
  assert.equal(gcpStreamAfterRecovery.state.messages, 2);
  await assertCoreDelivery('gcp', 'aws', 'dd.e2e.core.after-recovery', 'gcp-rejoined');

  process.stdout.write(
    [
      'PASS: 9 NATS servers formed three regional R3 clusters and a three-region gateway supercluster.',
      'PASS: mTLS and credential failures were rejected; application and system-account clients connected.',
      'PASS: Core NATS messages crossed AWS/GCP/Azure gateways in both directions.',
      'PASS: explicit placement kept AWS and GCP R3 stream replicas regional while metadata remained visible supercluster-wide.',
      'PASS: AWS/Azure and the AWS-placed stream stayed available during a full GCP outage; the GCP-placed stream failed closed and recovered after rejoin.',
    ].join('\n') + '\n',
  );
}

let failed = false;
try {
  await main();
} catch (error) {
  failed = true;
  process.stderr.write(`${redact(error.stack ?? error)}\n`);
  printLogTails();
} finally {
  await stopAll();
  if (keepArtifacts) {
    process.stdout.write(`NATS E2E artifacts retained at ${workDir}\n`);
  } else {
    rmSync(workDir, { recursive: true, force: true });
  }
}

if (failed) process.exitCode = 1;
