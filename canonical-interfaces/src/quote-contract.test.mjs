import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const schema = JSON.parse(await readFile(new URL("schema/quote.schema.json", root), "utf8"));
const docs = await readFile(new URL("docs/quote-api-v1.md", root), "utf8");

const fixtures = {
  QuoteRequest: "create-request.json",
  QuoteSubmissionResponse: "submission-response.json",
  QuoteDetail: "detail-ready.json",
  QuoteListResponse: "list-response.json",
  QuoteStatusEvent: "status-event.json",
  QuoteProblem: "problem.json",
};

function dereference(candidate) {
  if (!candidate?.$ref) return candidate;
  const name = candidate.$ref.split("/").pop();
  const resolved = schema.$defs[name];
  assert.ok(resolved, `unknown schema reference ${candidate.$ref}`);
  return resolved;
}

function validate(candidate, value, path = "$", required = true) {
  candidate = dereference(candidate);
  if (value === undefined) {
    assert.equal(required, false, `${path} is required`);
    return;
  }
  if (value === null) {
    assert.ok(Array.isArray(candidate.type) && candidate.type.includes("null"), `${path} must not be null`);
    return;
  }
  const type = Array.isArray(candidate.type) ? candidate.type.find((item) => item !== "null") : candidate.type;
  switch (type) {
    case "object": {
      assert.equal(typeof value, "object", `${path} must be an object`);
      assert.ok(!Array.isArray(value), `${path} must not be an array`);
      const properties = candidate.properties ?? {};
      const requiredNames = new Set(candidate.required ?? []);
      if (candidate.additionalProperties === false) {
        for (const key of Object.keys(value)) assert.ok(key in properties, `${path}.${key} is not allowed`);
      }
      for (const [key, property] of Object.entries(properties)) validate(property, value[key], `${path}.${key}`, requiredNames.has(key));
      break;
    }
    case "array":
      assert.ok(Array.isArray(value), `${path} must be an array`);
      if (candidate.minItems != null) assert.ok(value.length >= candidate.minItems, `${path} has too few items`);
      if (candidate.maxItems != null) assert.ok(value.length <= candidate.maxItems, `${path} has too many items`);
      if (candidate.uniqueItems) assert.equal(new Set(value.map(JSON.stringify)).size, value.length, `${path} contains duplicates`);
      value.forEach((item, index) => validate(candidate.items ?? {}, item, `${path}[${index}]`, true));
      break;
    case "string":
      assert.equal(typeof value, "string", `${path} must be a string`);
      if (candidate.minLength != null) assert.ok(value.length >= candidate.minLength, `${path} is too short`);
      if (candidate.maxLength != null) assert.ok(value.length <= candidate.maxLength, `${path} is too long`);
      if (candidate.enum) assert.ok(candidate.enum.includes(value), `${path} has an unsupported value`);
      if (candidate.pattern) assert.match(value, new RegExp(candidate.pattern), `${path} does not match its pattern`);
      break;
    case "integer":
      assert.ok(Number.isSafeInteger(value), `${path} must be an integer`);
      if (candidate.minimum != null) assert.ok(value >= candidate.minimum, `${path} is below minimum`);
      if (candidate.maximum != null) assert.ok(value <= candidate.maximum, `${path} exceeds maximum`);
      if (candidate.const != null) assert.equal(value, candidate.const, `${path} must equal ${candidate.const}`);
      break;
    case "number": assert.equal(typeof value, "number", `${path} must be a number`); break;
    case "boolean": assert.equal(typeof value, "boolean", `${path} must be a boolean`); break;
    default: assert.fail(`${path} uses unsupported test schema type ${String(type)}`);
  }
}

test("golden quote v1 fixtures conform to the canonical JSON Schemas", async () => {
  for (const [type, filename] of Object.entries(fixtures)) {
    const value = JSON.parse(await readFile(new URL(`fixtures/quote-v1/${filename}`, root), "utf8"));
    validate(schema.$defs[type], value, type, true);
  }
});

test("framework and status wire names are exact and shared", () => {
  assert.deepEqual(schema.$defs.QuoteRequest.properties.frameworks.items.enum, [
    "soc2_type_1", "soc2_type_2", "nist_csf_2", "nist_800_53", "hipaa",
    "iso_27001", "pci_dss_4", "fedramp", "gdpr", "custom",
  ]);
  const expectedStatuses = ["queued", "analyzing", "ready", "failed"];
  for (const type of ["QuoteSubmissionResponse", "QuoteSummary", "QuoteDetail"]) {
    assert.deepEqual(schema.$defs[type].properties.status.enum, expectedStatuses, type);
  }
  assert.equal(schema.$defs.QuoteRequest.properties.contextKey.default, "quote-analysis");
});

test("the auth and route matrix rejects identity ambiguity", () => {
  for (const route of [
    "POST` | `/api/v1/quotes`",
    "GET` | `/api/v1/quotes/{quoteId}`",
    "POST` | `/api/v1/quotes/{quoteId}/retry`",
    "`/api/v1/quotes/{quoteId}/events`",
  ]) assert.match(docs, new RegExp(route.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  assert.match(docs, /x-canonical-internal-token/);
  assert.match(docs, /x-canonical-subject/);
  assert.match(docs, /never accepts `x-canonical-user-id`/);
  assert.match(docs, /Query-string bearer tokens are prohibited/);
});
