#!/usr/bin/env node

import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..', '..');

async function read(path) {
  return readFile(resolve(repoRoot, path), 'utf8');
}

async function write(path, content) {
  await writeFile(resolve(repoRoot, path), content);
}

function replaceOnce(source, pattern, replacement, label) {
  const matches = source.match(new RegExp(pattern.source, pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`));
  if (matches?.length !== 1) {
    throw new Error(`${label}: expected exactly one source match, found ${matches?.length ?? 0}`);
  }
  return source.replace(pattern, replacement);
}

async function patchGenerator() {
  const path = 'remote/tools/generate-api-docs.mjs';
  let source = await read(path);

  const helperMarker = 'function canonicalGeneratedArtifacts(openapiPath) {';
  if (!source.includes(helperMarker)) {
    const helper = `function canonicalGeneratedArtifacts(openapiPath) {
  if (
    typeof openapiPath !== 'string' ||
    !openapiPath.endsWith('.json') ||
    openapiPath.endsWith('.public.json') ||
    openapiPath.endsWith('.metadata.json')
  ) {
    throw new Error(\`invalid canonical OpenAPI artifact path: \${openapiPath}\`);
  }
  return [
    openapiPath,
    openapiPath.replace(/\\.json$/, '.html'),
    openapiPath.replace(/\\.json$/, '.public.json'),
    openapiPath.replace(/\\.json$/, '.metadata.json'),
  ];
}

function normalizeIndexedServiceArtifacts(service) {
  const canonical = service.generated?.find(
    (path) =>
      typeof path === 'string' &&
      path.endsWith('.json') &&
      !path.endsWith('.public.json') &&
      !path.endsWith('.metadata.json'),
  );
  if (!canonical) {
    throw new Error(\`\${service.service ?? 'unknown service'} has no canonical OpenAPI JSON artifact\`);
  }
  return { ...service, generated: canonicalGeneratedArtifacts(canonical) };
}

`;
    source = replaceOnce(
      source,
      /async function main\(\) \{/,
      `${helper}async function main() {`,
      'insert central-index artifact helpers',
    );
  }

  const partialCheckoutPattern = /    const unavailableServices = await unavailableIndexedGitlinkServices\(centralIndexJson\);[\s\S]*?      await writeOrCheck\(centralIndexHtml, renderDocsIndexHtml\(indexItems\)\);\n    \}/;
  const replacement = `    const unavailableServices = await unavailableIndexedGitlinkServices(centralIndexJson);
    if (unavailableServices.length > 0) {
      for (const path of [centralIndexJson, centralIndexHtml]) {
        if (!existsSync(path)) {
          throw new Error(
            \`missing central API docs index during partial checkout: \${relative(repoRoot, path)}\`,
          );
        }
      }

      const unavailable = new Set(unavailableServices);
      const currentPayload = JSON.parse(await readUtf8(centralIndexJson));
      const currentByService = new Map(
        (currentPayload.services ?? []).map((service) => [service.service, service]),
      );
      const availableByService = new Map(index.map((service) => [service.service, service]));
      const serviceNames = [...new Set([...currentByService.keys(), ...availableByService.keys()])].sort();
      const mergedServices = serviceNames.map((serviceName) => {
        const availableService = availableByService.get(serviceName);
        if (availableService) {
          return normalizeIndexedServiceArtifacts(availableService);
        }
        const preservedService = currentByService.get(serviceName);
        if (!preservedService || !unavailable.has(serviceName)) {
          throw new Error(
            \`central API docs index contains non-gitlink service that is no longer discoverable: \${serviceName}\`,
          );
        }
        return normalizeIndexedServiceArtifacts(preservedService);
      });
      const mergedPayload = { ...indexPayload, services: mergedServices };
      await writeOrCheck(
        centralIndexJson,
        \`\${JSON.stringify(mergedPayload, null, 2)}\\n\`,
      );
      console.log(
        \`updated central API docs JSON index while preserving HTML route details for \${unavailableServices.length} uninitialized gitlink service(s): \${unavailableServices.join(', ')}\`,
      );
    } else {
      const normalizedPayload = {
        ...indexPayload,
        services: indexPayload.services.map(normalizeIndexedServiceArtifacts),
      };
      await writeOrCheck(
        centralIndexJson,
        \`\${JSON.stringify(normalizedPayload, null, 2)}\\n\`,
      );
      await writeOrCheck(centralIndexHtml, renderDocsIndexHtml(indexItems));
    }`;

  if (!source.includes('updated central API docs JSON index while preserving HTML route details')) {
    source = replaceOnce(
      source,
      partialCheckoutPattern,
      replacement,
      'replace partial-checkout central index handling',
    );
  }

  await write(path, source);
}

async function patchValidator() {
  const path = 'remote/tools/validate-openapi-contracts.mjs';
  let source = await read(path);

  if (!source.includes('function expectedGeneratedArtifacts(openapiRelative) {')) {
    const helper = `function expectedGeneratedArtifacts(openapiRelative) {
  return [
    openapiRelative,
    openapiRelative.replace(/\\.json$/, '.html'),
    openapiRelative.replace(/\\.json$/, '.public.json'),
    openapiRelative.replace(/\\.json$/, '.metadata.json'),
  ];
}

`;
    source = replaceOnce(
      source,
      /function verifyOpenApiShape\(/,
      `${helper}function verifyOpenApiShape(`,
      'insert expected generated artifact helper',
    );
  }

  const assertion = `  assert(
    JSON.stringify(item.generated) === JSON.stringify(expectedGeneratedArtifacts(openapiRelative)),
    \`\${item.service}: central index generated artifacts must list full JSON, HTML, public JSON, and metadata JSON in canonical order\`,
  );
`;
  if (!source.includes('central index generated artifacts must list full JSON')) {
    source = replaceOnce(
      source,
      /(  assert\(\n    typeof openapiRelative === 'string'[\s\S]*?\n  \);\n)(  if \(unavailableGitlink)/,
      `$1${assertion}$2`,
      'insert central index artifact assertion',
    );
  }

  await write(path, source);
}

await patchGenerator();
await patchValidator();
console.log('hardened central OpenAPI artifact indexing');
