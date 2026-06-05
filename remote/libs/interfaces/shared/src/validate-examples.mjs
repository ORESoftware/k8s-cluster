import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function readJson(relativePath) {
  return JSON.parse(await readFile(path.join(packageRoot, relativePath), 'utf8'));
}

function validateSchemaValue(schemaDoc, schema, value, label) {
  if (schema.$ref) {
    const match = /^#\/\$defs\/(.+)$/.exec(schema.$ref);
    assert.ok(match, `unsupported ref at ${label}: ${schema.$ref}`);
    const named = schemaDoc.$defs?.[match[1]];
    assert.ok(named, `missing schema def ${match[1]} at ${label}`);
    validateSchemaValue(schemaDoc, named, value, `${label}.${match[1]}`);
    return;
  }

  if (Array.isArray(schema.type)) {
    const errors = [];
    for (const type of schema.type) {
      try {
        validateSchemaValue(schemaDoc, { ...schema, type }, value, label);
        return;
      } catch (error) {
        errors.push(error.message);
      }
    }
    assert.fail(`${label} did not match any schema union type: ${errors.join('; ')}`);
  }

  switch (schema.type) {
    case 'object': {
      assert.equal(typeof value, 'object', `${label} must be an object`);
      assert.notEqual(value, null, `${label} must not be null`);
      assert.equal(Array.isArray(value), false, `${label} must not be an array`);
      const required = new Set(schema.required ?? []);
      for (const field of required) {
        assert.ok(Object.hasOwn(value, field), `${label}.${field} is required`);
      }
      const properties = schema.properties ?? {};
      if (schema.additionalProperties === false) {
        for (const field of Object.keys(value)) {
          assert.ok(Object.hasOwn(properties, field), `${label}.${field} is not declared`);
        }
      }
      for (const [field, fieldSchema] of Object.entries(properties)) {
        if (Object.hasOwn(value, field)) {
          validateSchemaValue(schemaDoc, fieldSchema, value[field], `${label}.${field}`);
        }
      }
      return;
    }
    case 'array':
      assert.ok(Array.isArray(value), `${label} must be an array`);
      value.forEach((entry, index) => {
        validateSchemaValue(schemaDoc, schema.items, entry, `${label}[${index}]`);
      });
      return;
    case 'string':
      assert.equal(typeof value, 'string', `${label} must be a string`);
      return;
    case 'integer':
      assert.equal(Number.isInteger(value), true, `${label} must be an integer`);
      if (schema.minimum !== undefined) assert.ok(value >= schema.minimum, `${label} below minimum`);
      return;
    case 'number':
      assert.equal(typeof value, 'number', `${label} must be a number`);
      assert.equal(Number.isFinite(value), true, `${label} must be finite`);
      if (schema.minimum !== undefined) assert.ok(value >= schema.minimum, `${label} below minimum`);
      return;
    case 'boolean':
      assert.equal(typeof value, 'boolean', `${label} must be a boolean`);
      return;
    case 'null':
      assert.equal(value, null, `${label} must be null`);
      return;
    case undefined:
      return;
    default:
      assert.fail(`unsupported schema type at ${label}: ${schema.type}`);
  }
}

function assertNoCredentialBearingUris(value, label = 'fixture') {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => assertNoCredentialBearingUris(entry, `${label}[${index}]`));
    return;
  }
  if (value === null || typeof value !== 'object') return;

  for (const [key, entry] of Object.entries(value)) {
    const nextLabel = `${label}.${key}`;
    if (/uri/i.test(key) && typeof entry === 'string') {
      const parsed = new URL(entry);
      assert.equal(parsed.username, '', `${nextLabel} must not contain URI username`);
      assert.equal(parsed.password, '', `${nextLabel} must not contain URI password`);
      assert.equal(parsed.search, '', `${nextLabel} must not contain query string`);
      assert.equal(parsed.hash, '', `${nextLabel} must not contain fragment`);
    }
    assertNoCredentialBearingUris(entry, nextLabel);
  }
}

function assertConversionCorrelation(request, result) {
  assert.equal(request.schema, 'dd.fabrication.design-conversion.request.v1');
  assert.equal(result.schema, 'dd.fabrication.design-conversion.result.v1');
  assert.equal(request.resultSubject, 'dd.remote.fabrication.design.conversion.results');
  assert.equal(result.requestId, request.requestId);
  assert.equal(result.planRequestId, request.planRequestId);
  assert.equal(result.jobId, request.jobId);

  const inputIds = new Set(request.designInputs.map((input) => input.inputId));
  const targetIds = new Set(request.targets.map((target) => target.targetId));
  for (const target of request.targets) {
    if (target.sourceInputId) assert.ok(inputIds.has(target.sourceInputId));
  }
  for (const blocker of request.blockers) {
    if (blocker.inputId) assert.ok(inputIds.has(blocker.inputId));
    if (blocker.targetId) assert.ok(targetIds.has(blocker.targetId));
  }
  for (const artifact of result.artifacts) {
    if (artifact.sourceInputId) assert.ok(inputIds.has(artifact.sourceInputId));
    if (artifact.targetId) assert.ok(targetIds.has(artifact.targetId));
    assert.notEqual(artifact.sha256, request.designInputs[0].sourceSha256);
  }
  for (const blocker of result.blockers) {
    if (blocker.inputId) assert.ok(inputIds.has(blocker.inputId));
    if (blocker.targetId) assert.ok(targetIds.has(blocker.targetId));
  }

  assert.equal(result.machineReady, false);
  assert.ok(result.artifacts.some((artifact) => artifact.format === 'STEP'));
  assert.ok(result.artifacts.some((artifact) => artifact.format === '3MF'));
  assert.ok(result.blockers.some((blocker) => blocker.machineReadyImpact === 'needs-operator-review'));
  assertNoCredentialBearingUris(request, 'request');
  assertNoCredentialBearingUris(result, 'result');
}

async function main() {
  const schemaDoc = await readJson('schema/fabrication-cad-conversion.schema.json');
  const request = await readJson('examples/fabrication-design-conversion-request.json');
  const result = await readJson('examples/fabrication-design-conversion-result.json');

  validateSchemaValue(
    schemaDoc,
    { $ref: '#/$defs/FabricationDesignConversionRequest' },
    request,
    'request',
  );
  validateSchemaValue(
    schemaDoc,
    { $ref: '#/$defs/FabricationDesignConversionResult' },
    result,
    'result',
  );
  assertConversionCorrelation(request, result);

  console.log('fabrication CAD conversion examples validate against shared schema.');
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
