import { readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptPath = fileURLToPath(import.meta.url);
const serviceDir = resolve(dirname(scriptPath), '..');
const serverPath = resolve(serviceDir, 'src/server.ts');
const packagePath = resolve(serviceDir, 'package.json');

function replaceOnce(text, oldText, newText, label) {
  const first = text.indexOf(oldText);
  const last = text.lastIndexOf(oldText);
  if (first < 0 || first !== last) {
    throw new Error(`${label}: expected exactly one match`);
  }
  return `${text.slice(0, first)}${newText}${text.slice(first + oldText.length)}`;
}

let server = readFileSync(serverPath, 'utf8');
server = replaceOnce(
  server,
  `function serviceDescriptor() {\n  return {`,
  `function serviceDescriptor() {\n  return ServiceDescriptorSchema.parse({`,
  'service descriptor output validation',
);
server = replaceOnce(
  server,
  `    allowEvaluate: config.allowEvaluate,\n  };\n}\n\nfunction toolsDescriptor() {`,
  `    allowEvaluate: config.allowEvaluate,\n  });\n}\n\nfunction toolsDescriptor() {`,
  'service descriptor parse close',
);
server = replaceOnce(
  server,
  `function statusDescriptor(maxConcurrent = config.maxConcurrent) {\n  return {`,
  `function statusDescriptor(maxConcurrent = config.maxConcurrent) {\n  return StatusDescriptorSchema.parse({`,
  'status descriptor output validation',
);
server = replaceOnce(
  server,
  `    allowEvaluate: config.allowEvaluate,\n  };\n}\n\nfunction healthDescriptor() {`,
  `    allowEvaluate: config.allowEvaluate,\n  });\n}\n\nfunction healthDescriptor() {`,
  'status descriptor parse close',
);
server = replaceOnce(
  server,
  `function healthDescriptor() {\n  return {`,
  `function healthDescriptor() {\n  return HealthDescriptorSchema.parse({`,
  'health descriptor output validation',
);
server = replaceOnce(
  server,
  `    inFlight: metrics.inFlight,\n  };\n}\n\nfunction resolveToolVersion(tool: Tool): string {`,
  `    inFlight: metrics.inFlight,\n  });\n}\n\nfunction resolveToolVersion(tool: Tool): string {`,
  'health descriptor parse close',
);
server = replaceOnce(
  server,
  `          fastify.log.warn(\n            { err: error, requestId },\n            'browser-test final screenshot failed',\n          );`,
  `          console.warn('browser-test final screenshot failed', { err: error, requestId });`,
  'stale Fastify screenshot logger',
);
server = replaceOnce(
  server,
  `function isMainModule(): boolean {\n  const entry = process.argv[1];\n  return Boolean(entry) && pathToFileURL(entry).href === import.meta.url;\n}`,
  `function isMainModule(): boolean {\n  const entry = process.argv[1];\n  if (!entry) return false;\n  return pathToFileURL(entry).href === import.meta.url;\n}`,
  'main-module argv narrowing',
);
writeFileSync(serverPath, server, 'utf8');

const packageJson = JSON.parse(readFileSync(packagePath, 'utf8'));
const bootstrapTypecheck = 'node scripts/den-479-post-apply-fix.mjs && tsc --noEmit';
if (packageJson.scripts?.typecheck !== bootstrapTypecheck) {
  throw new Error(`unexpected bootstrap typecheck command: ${String(packageJson.scripts?.typecheck)}`);
}
packageJson.scripts.typecheck = 'tsc --noEmit';
const nativeOpenApiExport = 'node dist/server.js --export-openapi';
if (packageJson.scripts?.['openapi:export'] !== nativeOpenApiExport) {
  throw new Error(
    `unexpected native OpenAPI export command: ${String(packageJson.scripts?.['openapi:export'])}`,
  );
}
packageJson.scripts['openapi:export'] = 'node scripts/den-479-export-wrapper.mjs';
writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`, 'utf8');
unlinkSync(scriptPath);
