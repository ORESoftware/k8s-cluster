#!/usr/bin/env node
// Validate a JSON payload against one $def of a JSON Schema file.
//
//   node scripts/validate-against-schema.mjs <schema.json> <DefName> <payload.json|->
//
// Dependency-free on purpose: this superproject has no npm dependencies, and
// the interfaces schemas only use a small keyword set. The validator FAILS on
// any keyword it does not implement, so a future schema feature can never be
// silently ignored.

import { readFileSync } from "node:fs";

export const HANDLED = new Set([
  "type",
  "required",
  "properties",
  "additionalProperties",
  "enum",
  "const",
  "items",
  "minItems",
  "maxItems",
  "uniqueItems",
  "description",
  "$ref",
]);

export function validate(schema, value, defs, path = "$") {
  const errors = [];
  for (const keyword of Object.keys(schema)) {
    if (!HANDLED.has(keyword)) {
      errors.push(`${path}: unsupported schema keyword "${keyword}" — extend validate-against-schema.mjs`);
    }
  }
  if (typeof schema.$ref === "string") {
    const match = schema.$ref.match(/^#\/\$defs\/(.+)$/);
    if (match === null || !(match[1] in defs)) {
      errors.push(`${path}: unresolvable $ref ${schema.$ref}`);
      return errors;
    }
    return [...errors, ...validate(defs[match[1]], value, defs, path)];
  }
  if (schema.type === "object") {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return [...errors, `${path}: expected object, got ${JSON.stringify(value)}`];
    }
    for (const key of schema.required ?? []) {
      if (!(key in value)) {
        errors.push(`${path}: missing required property "${key}"`);
      }
    }
    for (const [key, member] of Object.entries(value)) {
      const propertySchema = schema.properties?.[key];
      if (propertySchema === undefined) {
        if (schema.additionalProperties === false) {
          errors.push(`${path}: unexpected property "${key}"`);
        }
        continue;
      }
      errors.push(...validate(propertySchema, member, defs, `${path}.${key}`));
    }
    return errors;
  }
  if (schema.type === "array") {
    if (!Array.isArray(value)) {
      return [...errors, `${path}: expected array, got ${JSON.stringify(value)}`];
    }
    if (schema.minItems !== undefined && value.length < schema.minItems) {
      errors.push(`${path}: expected at least ${schema.minItems} items, got ${value.length}`);
    }
    if (schema.maxItems !== undefined && value.length > schema.maxItems) {
      errors.push(`${path}: expected at most ${schema.maxItems} items, got ${value.length}`);
    }
    if (schema.uniqueItems === true) {
      const seen = new Set(value.map((item) => JSON.stringify(item)));
      if (seen.size !== value.length) {
        errors.push(`${path}: items are not unique`);
      }
    }
    if (schema.items !== undefined) {
      value.forEach((item, index) => {
        errors.push(...validate(schema.items, item, defs, `${path}[${index}]`));
      });
    }
    return errors;
  }
  if (schema.type === "string") {
    if (typeof value !== "string") {
      return [...errors, `${path}: expected string, got ${JSON.stringify(value)}`];
    }
  } else if (schema.type !== undefined) {
    errors.push(`${path}: unsupported schema type "${schema.type}" — extend validate-against-schema.mjs`);
  }
  if (schema.enum !== undefined && !schema.enum.includes(value)) {
    errors.push(`${path}: ${JSON.stringify(value)} is not one of ${JSON.stringify(schema.enum)}`);
  }
  if (schema.const !== undefined && value !== schema.const) {
    errors.push(`${path}: expected ${JSON.stringify(schema.const)}, got ${JSON.stringify(value)}`);
  }
  return errors;
}

const invokedDirectly = process.argv[1]?.endsWith("validate-against-schema.mjs") ?? false;
if (invokedDirectly) {
  const [schemaPath, defName, payloadPath] = process.argv.slice(2);
  if (schemaPath === undefined || defName === undefined || payloadPath === undefined) {
    console.error("usage: validate-against-schema.mjs <schema.json> <DefName> <payload.json|->");
    process.exit(64);
  }
  const schemaFile = JSON.parse(readFileSync(schemaPath, "utf8"));
  const defs = schemaFile.$defs ?? {};
  if (!(defName in defs)) {
    console.error(`no $def named "${defName}" in ${schemaPath}; have: ${Object.keys(defs).join(", ")}`);
    process.exit(65);
  }
  const raw = payloadPath === "-" ? readFileSync(0, "utf8") : readFileSync(payloadPath, "utf8");
  const errors = validate(defs[defName], JSON.parse(raw), defs);
  if (errors.length > 0) {
    console.error(`payload does not conform to ${defName}:`);
    for (const error of errors) {
      console.error(`  ${error}`);
    }
    process.exit(1);
  }
  console.log(`payload conforms to ${defName}`);
}
