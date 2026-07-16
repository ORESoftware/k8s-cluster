import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { HANDLED, validate } from "../scripts/validate-against-schema.mjs";

const apiSchemaPath = fileURLToPath(
  new URL("../apps/canonical-interfaces/schema/api.schema.json", import.meta.url),
);

function loadDefs() {
  return JSON.parse(readFileSync(apiSchemaPath, "utf8")).$defs;
}

test("validator accepts a conforming HealthStatus", () => {
  const defs = loadDefs();
  assert.deepEqual(validate(defs.HealthStatus, { status: "ok", service: "canonical-web-server" }, defs), []);
});

test("validator rejects a wrong enum value and an extra property", () => {
  const defs = loadDefs();
  const errors = validate(
    defs.HealthStatus,
    { status: "on-fire", service: "canonical-web-server", extra: true },
    defs,
  );
  assert.equal(errors.length, 2, errors.join("\n"));
  assert.match(errors.join("\n"), /not one of/);
  assert.match(errors.join("\n"), /unexpected property "extra"/);
});

test("validator enforces ServiceInfo const, array bounds, and uniqueness", () => {
  const defs = loadDefs();
  const good = {
    service: "canonical-web-server",
    version: "0.1.0",
    domain: "canonical.cloud",
    stack: ["supabase", "maud", "axum", "seaorm", "htmx"],
  };
  assert.deepEqual(validate(defs.ServiceInfo, good, defs), []);

  const bad = { ...good, service: "other-service", stack: ["maud", "maud", "axum", "seaorm", "htmx"] };
  const errors = validate(defs.ServiceInfo, bad, defs);
  assert.match(errors.join("\n"), /expected "canonical-web-server"/);
  assert.match(errors.join("\n"), /not unique/);
});

test("validator refuses schemas that use keywords it does not implement", () => {
  const errors = validate({ type: "string", pattern: "^x" }, "x", {});
  assert.equal(errors.length, 1);
  assert.match(errors[0], /unsupported schema keyword "pattern"/);
});

// Static walk: every keyword in the defs the stack smoke validates at runtime
// (and anything they $ref) must be implemented by the validator, so schema
// evolution can never silently weaken the conformance gate.
test("smoke-validated schema defs stay within the implemented keyword set", () => {
  const defs = loadDefs();
  const queue = ["HealthStatus", "ServiceInfo"];
  const visited = new Set();
  const unsupported = [];
  const walk = (node, path) => {
    if (Array.isArray(node)) {
      node.forEach((item, index) => walk(item, `${path}[${index}]`));
      return;
    }
    if (typeof node !== "object" || node === null) {
      return;
    }
    for (const [keyword, child] of Object.entries(node)) {
      if (!HANDLED.has(keyword)) {
        unsupported.push(`${path}: ${keyword}`);
      }
      if (keyword === "$ref" && typeof child === "string") {
        const match = child.match(/^#\/\$defs\/(.+)$/);
        if (match !== null) {
          queue.push(match[1]);
        }
        continue;
      }
      if (keyword === "properties") {
        for (const [name, propertySchema] of Object.entries(child)) {
          walk(propertySchema, `${path}.properties.${name}`);
        }
        continue;
      }
      if (keyword === "items") {
        walk(child, `${path}.items`);
      }
    }
  };
  while (queue.length > 0) {
    const name = queue.pop();
    if (visited.has(name)) {
      continue;
    }
    visited.add(name);
    assert.ok(name in defs, `missing $def ${name}`);
    walk(defs[name], name);
  }
  assert.deepEqual(unsupported, []);
});
