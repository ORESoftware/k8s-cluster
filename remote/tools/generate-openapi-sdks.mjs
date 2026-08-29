#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, rm } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const manifest = JSON.parse(
  await readFile(resolve(repoRoot, 'remote/api-contracts/manifest.json'), 'utf8'),
);
const serviceIndex = process.argv.indexOf('--service');
const serviceName = serviceIndex >= 0 ? process.argv[serviceIndex + 1] : undefined;
const outputIndex = process.argv.indexOf('--output');
const outputRoot = outputIndex >= 0 ? process.argv[outputIndex + 1] : undefined;
if (!serviceName || !manifest.services[serviceName]) {
  throw new Error('--service must name a service in remote/api-contracts/manifest.json');
}
if (!outputRoot || outputRoot.startsWith('--')) {
  throw new Error('--output requires a repository-relative output directory');
}

const service = manifest.services[serviceName];
const resolvedOutput = resolve(repoRoot, outputRoot);
const relativeOutput = relative(repoRoot, resolvedOutput);
if (relativeOutput.startsWith('..') || relativeOutput === '') {
  throw new Error('--output must stay inside the repository working tree');
}
await rm(resolvedOutput, { recursive: true, force: true });
await mkdir(resolvedOutput, { recursive: true });

const uid = process.getuid?.() ?? 1000;
const gid = process.getgid?.() ?? 1000;
const mount = `${repoRoot}:/local`;
const contract = `/local/${service.contract.split(sep).join('/')}`;
const image = 'openapitools/openapi-generator-cli:v7.22.0';

function docker(args) {
  execFileSync('docker', ['run', '--rm', '-u', `${uid}:${gid}`, '-v', mount, image, ...args], {
    cwd: repoRoot,
    stdio: 'inherit',
  });
}

docker(['validate', '-i', contract]);
const targets = [
  [
    'rust',
    service.sdk.rust.generator,
    `packageName=${service.sdk.rust.packageName},packageVersion=0.1.0,library=reqwest`,
  ],
  [
    'typescript',
    service.sdk.typescript.generator,
    `npmName=${service.sdk.typescript.packageName},npmVersion=0.1.0,supportsES6=true,typescriptThreePlus=true`,
  ],
  [
    'dart',
    service.sdk.dart.generator,
    `pubName=${service.sdk.dart.packageName},pubVersion=0.1.0`,
  ],
];
for (const [directory, generator, properties] of targets) {
  const output = `/local/${relative(repoRoot, resolve(resolvedOutput, directory)).split(sep).join('/')}`;
  docker([
    'generate',
    '-i',
    contract,
    '-g',
    generator,
    '-o',
    output,
    '--additional-properties',
    properties,
    '--global-property',
    'apiTests=false,modelTests=false,apiDocs=true,modelDocs=true',
  ]);
}
console.log(`generated ${serviceName} SDK smoke trees at ${relativeOutput}`);
