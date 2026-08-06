import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const schema = JSON.parse(
  await readFile(new URL("schema/quote.schema.json", root), "utf8"),
);
const manifest = JSON.parse(
  await readFile(new URL("fixtures/quote/v1/manifest.json", root), "utf8"),
);

async function fixture(name) {
  return JSON.parse(
    await readFile(new URL(`fixtures/quote/v1/${name}`, root), "utf8"),
  );
}

function validateValue(value, rule, field) {
  if (rule.type === "string") {
    assert.equal(typeof value, "string", `${field} must be a string`);
    if (rule.minLength !== undefined) assert.ok(value.length >= rule.minLength, field);
    if (rule.maxLength !== undefined) assert.ok(value.length <= rule.maxLength, field);
    if (rule.pattern !== undefined) assert.match(value, new RegExp(rule.pattern), field);
    if (rule.enum !== undefined) assert.ok(rule.enum.includes(value), field);
    return;
  }
  if (rule.type === "integer") {
    assert.ok(Number.isInteger(value), `${field} must be an integer`);
    if (rule.minimum !== undefined) assert.ok(value >= rule.minimum, field);
    if (rule.maximum !== undefined) assert.ok(value <= rule.maximum, field);
    if (rule.const !== undefined) assert.equal(value, rule.const, field);
    return;
  }
  if (rule.type === "boolean") {
    assert.equal(typeof value, "boolean", `${field} must be a boolean`);
    return;
  }
  if (rule.type === "array") {
    assert.ok(Array.isArray(value), `${field} must be an array`);
    if (rule.minItems !== undefined) assert.ok(value.length >= rule.minItems, field);
    if (rule.maxItems !== undefined) assert.ok(value.length <= rule.maxItems, field);
    if (rule.uniqueItems) assert.equal(new Set(value).size, value.length, field);
    if (rule.items) value.forEach((item, index) => validateValue(item, rule.items, `${field}[${index}]`));
  }
}

async function assertFixture(definitionName, fileName) {
  const value = await fixture(fileName);
  const definition = schema.$defs[definitionName];
  assert.ok(definition, `missing schema definition ${definitionName}`);
  assert.equal(definition.type, "object");
  for (const required of definition.required ?? []) {
    assert.ok(Object.hasOwn(value, required), `${fileName} misses ${required}`);
  }
  if (definition.additionalProperties === false) {
    assert.deepEqual(
      Object.keys(value).filter((key) => !Object.hasOwn(definition.properties, key)),
      [],
      `${fileName} contains unknown properties`,
    );
  }
  for (const [field, fieldValue] of Object.entries(value)) {
    const rule = definition.properties[field];
    if (rule.$ref) continue;
    validateValue(fieldValue, rule, `${fileName}.${field}`);
  }
  return value;
}

test("quote v1 manifest is immutable and complete", () => {
  assert.equal(manifest.schemaVersion, 1);
  assert.equal(manifest.schemaPath, "schema/quote.schema.json");
  assert.equal(manifest.wireCase, "camelCase");
  assert.deepEqual(Object.keys(manifest.fixtures).sort(), [
    "QuoteEstimate",
    "QuoteProblem",
    "QuoteRequest",
    "QuoteStatusEvent",
    "QuoteSubmissionResponse",
  ]);
});

test("golden fixtures conform to the public quote definitions", async () => {
  const request = await assertFixture("QuoteRequest", manifest.fixtures.QuoteRequest);
  const submission = await assertFixture(
    "QuoteSubmissionResponse",
    manifest.fixtures.QuoteSubmissionResponse,
  );
  const estimate = await assertFixture("QuoteEstimate", manifest.fixtures.QuoteEstimate);
  const event = await assertFixture("QuoteStatusEvent", manifest.fixtures.QuoteStatusEvent);
  await assertFixture("QuoteProblem", manifest.fixtures.QuoteProblem);

  assert.equal(request.answersVersion, 1);
  assert.ok(estimate.lowerBoundCents <= estimate.upperBoundCents);
  assert.ok(estimate.durationWeeksLow <= estimate.durationWeeksHigh);
  assert.equal(event.quoteId, submission.quoteId);
  assert.equal(estimate.quoteId, submission.quoteId);
});

test("golden request rejects legacy transport names", async () => {
  const request = await fixture(manifest.fixtures.QuoteRequest);
  for (const legacy of [
    "company",
    "company_name",
    "companyName",
    "target_frameworks",
    "cloud_providers",
    "handles_phi",
    "employee_count",
  ]) {
    assert.ok(!Object.hasOwn(request, legacy), `legacy field ${legacy} reappeared`);
  }
  assert.ok(Object.hasOwn(request, "organizationName"));
  assert.ok(Object.hasOwn(request, "frameworks"));
});
