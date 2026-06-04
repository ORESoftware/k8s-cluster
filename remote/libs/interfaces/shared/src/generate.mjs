// JSON Schema -> per-language types/interfaces generator.
//
// Source of truth: every *.schema.json file listed in schema/index.json.
// Each schema document SHOULD declare its types under `$defs`; the
// top-level `type` is only used for documentation. Cross-schema refs are
// not supported in this first pass — keep related types together in one
// document (use `$defs`).
//
// Supported JSON Schema features:
//   - object types with named `properties`, `required`, `additionalProperties`
//   - primitive types: string, number, integer, boolean, null
//   - union types via JSON-Schema array form (e.g. ["string", "null"])
//   - arrays via `items` (including arrays of $refs)
//   - enums via `enum` on strings (emitted as a tagged string union)
//   - local `$ref` to `#/$defs/Name`
//
// Outputs (kept idiomatic per-language and free of external runtime deps):
//   generated/typescript/<file>.ts + index.ts
//   generated/rust/Cargo.toml + src/lib.rs
//   generated/python/dd_shared_interfaces.py
//   generated/gleam/gleam.toml + src/dd_shared_interfaces.gleam
//
// Run `pnpm --filter @dd/shared-interfaces generate` to write files;
// run `--check` mode in CI to fail if generated outputs drift from source.

import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function main() {
  const args = new Set(process.argv.slice(2));

  const indexRaw = await readFile(path.join(packageRoot, 'schema', 'index.json'), 'utf8');
  const index = JSON.parse(indexRaw);
  if (!Array.isArray(index.schemas) || index.schemas.length === 0) {
    throw new Error('schema/index.json must declare a non-empty "schemas" array');
  }

  const schemas = [];
  for (const filename of [...index.schemas].sort()) {
    const raw = await readFile(path.join(packageRoot, 'schema', filename), 'utf8');
    schemas.push({ filename, doc: JSON.parse(raw) });
  }

  const model = buildModel(schemas);
  const outputs = renderOutputs(model);

  if (args.has('--check')) {
    const stale = [];
    for (const [relativePath, contents] of outputs) {
      const absolutePath = path.join(packageRoot, relativePath);
      let existing = '';
      try {
        existing = await readFile(absolutePath, 'utf8');
      } catch {
        stale.push(relativePath);
        continue;
      }
      if (existing !== contents) {
        stale.push(relativePath);
      }
    }

    if (stale.length > 0) {
      console.error(
        `shared-interfaces generated outputs are stale:\n${stale.map((file) => `  - ${file}`).join('\n')}`,
      );
      process.exitCode = 1;
      return;
    }

    console.log('shared-interfaces generated outputs are up to date.');
    return;
  }

  for (const [relativePath, contents] of outputs) {
    const absolutePath = path.join(packageRoot, relativePath);
    await mkdir(path.dirname(absolutePath), { recursive: true });
    await writeFile(absolutePath, contents);
  }
  console.log(`Generated ${outputs.size} shared-interfaces files.`);
}

// ---------- Model ----------

/**
 * @typedef {{ kind: 'object', name: string, description?: string, fields: Field[] }} ObjectType
 * @typedef {{ kind: 'enum', name: string, description?: string, values: string[] }} EnumType
 * @typedef {ObjectType | EnumType} NamedType
 * @typedef {{ name: string, description?: string, required: boolean, nullable: boolean, type: TypeRef }} Field
 * @typedef {{ tag: 'primitive', name: 'string' | 'number' | 'integer' | 'boolean' | 'any' } |
 *           { tag: 'array', items: TypeRef } |
 *           { tag: 'ref', name: string } |
 *           { tag: 'union', members: TypeRef[] }} TypeRef
 */

function buildModel(schemaFiles) {
  /** @type {NamedType[]} */
  const named = [];
  const seen = new Set();
  for (const { filename, doc } of schemaFiles) {
    const defs = doc.$defs ?? doc.definitions ?? {};
    for (const [name, def] of Object.entries(defs)) {
      if (seen.has(name)) {
        throw new Error(`Duplicate $defs name across schema files: ${name}`);
      }
      seen.add(name);
      named.push(resolveNamed(name, def, filename));
    }
  }
  named.sort((a, b) => a.name.localeCompare(b.name));
  return { named };
}

function resolveNamed(name, def, filename) {
  if (Array.isArray(def.enum) && (def.type === 'string' || def.type === undefined)) {
    return {
      kind: 'enum',
      name,
      description: def.description,
      values: def.enum.map((value) => {
        if (typeof value !== 'string') {
          throw new Error(`enum values must be strings (in ${filename} / ${name})`);
        }
        return value;
      }),
    };
  }

  if (def.type !== 'object') {
    throw new Error(`Top-level $defs entry "${name}" in ${filename} must be type "object" or a string enum`);
  }

  const required = new Set(def.required ?? []);
  const fields = [];
  for (const [fieldName, fieldDef] of Object.entries(def.properties ?? {})) {
    const { typeRef, nullable } = resolveTypeRef(fieldDef, `${name}.${fieldName}`);
    fields.push({
      name: fieldName,
      description: fieldDef.description,
      required: required.has(fieldName),
      nullable,
      type: typeRef,
    });
  }
  return { kind: 'object', name, description: def.description, fields };
}

function resolveTypeRef(def, label) {
  if (def.$ref) {
    const match = /^#\/\$defs\/(.+)$/.exec(def.$ref);
    if (!match) throw new Error(`Unsupported $ref at ${label}: ${def.$ref}`);
    return { typeRef: { tag: 'ref', name: match[1] }, nullable: false };
  }

  if (Array.isArray(def.type)) {
    const nullable = def.type.includes('null');
    const others = def.type.filter((entry) => entry !== 'null');
    if (others.length === 0) {
      return { typeRef: { tag: 'primitive', name: 'any' }, nullable: true };
    }
    if (others.length === 1) {
      const single = { ...def, type: others[0] };
      const { typeRef } = resolveTypeRef(single, label);
      return { typeRef, nullable };
    }
    // Multi-type union containing array/object collapses to an opaque JSON value.
    // We don't try to build a discriminated union for that case.
    if (others.includes('array') || others.includes('object')) {
      return { typeRef: { tag: 'primitive', name: 'any' }, nullable };
    }
    const members = others.map((entry) => {
      const single = { ...def, type: entry };
      return resolveTypeRef(single, label).typeRef;
    });
    return { typeRef: { tag: 'union', members }, nullable };
  }

  if (def.type === 'string') {
    return { typeRef: { tag: 'primitive', name: 'string' }, nullable: false };
  }
  if (def.type === 'integer') {
    return { typeRef: { tag: 'primitive', name: 'integer' }, nullable: false };
  }
  if (def.type === 'number') {
    return { typeRef: { tag: 'primitive', name: 'number' }, nullable: false };
  }
  if (def.type === 'boolean') {
    return { typeRef: { tag: 'primitive', name: 'boolean' }, nullable: false };
  }
  if (def.type === 'null') {
    return { typeRef: { tag: 'primitive', name: 'any' }, nullable: true };
  }
  if (def.type === 'array') {
    if (!def.items) throw new Error(`array type missing items at ${label}`);
    const { typeRef } = resolveTypeRef(def.items, `${label}[]`);
    return { typeRef: { tag: 'array', items: typeRef }, nullable: false };
  }
  if (def.type === 'object' && (!def.properties || Object.keys(def.properties).length === 0)) {
    return { typeRef: { tag: 'primitive', name: 'any' }, nullable: false };
  }
  if (def.type === undefined) {
    // free-form JSON value
    return { typeRef: { tag: 'primitive', name: 'any' }, nullable: false };
  }
  throw new Error(`Unsupported type "${def.type}" at ${label}`);
}

// ---------- Outputs ----------

function renderOutputs(model) {
  const outputs = new Map();
  const add = (relativePath, contents) => {
    if (outputs.has(relativePath)) {
      throw new Error(`Duplicate generated output path: ${relativePath}`);
    }
    outputs.set(relativePath, contents);
  };

  add('generated/typescript/index.ts', renderTypeScript(model));
  add('generated/rust/Cargo.toml', renderRustCargo());
  add('generated/rust/src/lib.rs', renderRust(model));
  add('generated/python/dd_shared_interfaces.py', renderPython(model));
  add('generated/python/__init__.py', 'from .dd_shared_interfaces import *  # noqa: F401,F403\n');
  add('generated/gleam/gleam.toml', renderGleamToml());
  add('generated/gleam/src/dd_shared_interfaces.gleam', renderGleam(model));
  return outputs;
}

// ---------- TypeScript ----------

function renderTypeScript(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs');
  lines.push('// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.');
  lines.push('// Source schemas: remote/libs/interfaces/shared/schema/*.schema.json');
  lines.push('');
  for (const named of model.named) {
    if (named.description) {
      for (const line of splitDoc(named.description)) lines.push(`// ${line}`);
    }
    if (named.kind === 'enum') {
      const union = named.values.map((value) => JSON.stringify(value)).join(' | ');
      lines.push(`export type ${named.name} = ${union};`);
    } else {
      lines.push(`export type ${named.name} = {`);
      for (const field of named.fields) {
        if (field.description) {
          for (const line of splitDoc(field.description)) lines.push(`  /** ${line} */`);
        }
        const optional = field.required ? '' : '?';
        const tsType = renderTsType(field.type, field.nullable);
        lines.push(`  ${field.name}${optional}: ${tsType};`);
      }
      lines.push('};');
    }
    lines.push('');
  }
  return lines.join('\n');
}

function renderTsType(type, nullable) {
  const base = (() => {
    switch (type.tag) {
      case 'primitive':
        switch (type.name) {
          case 'string': return 'string';
          case 'number': return 'number';
          case 'integer': return 'number';
          case 'boolean': return 'boolean';
          case 'any': return 'unknown';
          default: throw new Error(`unknown primitive ${type.name}`);
        }
      case 'array': return `${renderTsType(type.items, false)}[]`;
      case 'ref': return type.name;
      case 'union': return type.members.map((member) => renderTsType(member, false)).join(' | ');
      default: throw new Error(`unhandled type tag ${type.tag}`);
    }
  })();
  return nullable ? `${base} | null` : base;
}

// ---------- Rust ----------

function renderRustCargo() {
  return [
    '[package]',
    'name = "dd-shared-interfaces"',
    'version = "0.1.0"',
    'edition = "2021"',
    'description = "Generated Rust types for dd shared cross-runtime interfaces. Do not edit by hand."',
    '',
    '[lib]',
    'path = "src/lib.rs"',
    '',
    '[dependencies]',
    'serde = { version = "1", features = ["derive"] }',
    'serde_json = "1"',
    '',
  ].join('\n');
}

function renderRust(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs');
  lines.push('// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.');
  lines.push('');
  lines.push('use serde::{Deserialize, Serialize};');
  lines.push('use serde_json::Value;');
  lines.push('');
  for (const named of model.named) {
    if (named.description) {
      for (const line of splitDoc(named.description)) lines.push(`/// ${line}`);
    }
    if (named.kind === 'enum') {
      lines.push('#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]');
      lines.push('#[serde(rename_all = "lowercase")]');
      lines.push(`pub enum ${named.name} {`);
      for (const value of named.values) {
        lines.push(`    #[serde(rename = ${JSON.stringify(value)})]`);
        lines.push(`    ${rustEnumVariant(value)},`);
      }
      lines.push('}');
    } else {
      lines.push('#[derive(Debug, Clone, Serialize, Deserialize)]');
      lines.push(`pub struct ${named.name} {`);
      for (const field of named.fields) {
        if (field.description) {
          for (const line of splitDoc(field.description)) lines.push(`    /// ${line}`);
        }
        const snake = camelToSnake(field.name);
        const rename = snake === field.name ? '' : `#[serde(rename = ${JSON.stringify(field.name)})]`;
        const optional = !field.required || field.nullable;
        let rustType = renderRustType(field.type);
        if (optional) rustType = `Option<${rustType}>`;
        const annotations = [];
        if (rename) annotations.push(rename);
        if (!field.required) annotations.push('#[serde(default, skip_serializing_if = "Option::is_none")]');
        for (const annotation of annotations) lines.push(`    ${annotation}`);
        lines.push(`    pub ${snake}: ${rustType},`);
      }
      lines.push('}');
    }
    lines.push('');
  }
  return lines.join('\n');
}

function rustEnumVariant(value) {
  const camel = value.replace(/(^|[-_\s]+)([a-zA-Z0-9])/g, (_match, _sep, ch) => ch.toUpperCase());
  return camel.replace(/[^A-Za-z0-9]/g, '') || 'Unknown';
}

function renderRustType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'String';
        case 'integer': return 'i64';
        case 'number': return 'f64';
        case 'boolean': return 'bool';
        case 'any': return 'Value';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `Vec<${renderRustType(type.items)}>`;
    case 'ref': return type.name;
    case 'union': return 'Value';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

// ---------- Python ----------

function renderPython(model) {
  const lines = [];
  lines.push('"""AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs.');
  lines.push('');
  lines.push('Do not edit by hand; edit the JSON Schema under schema/ and regenerate.');
  lines.push('"""');
  lines.push('');
  lines.push('from __future__ import annotations');
  lines.push('');
  lines.push('from dataclasses import dataclass, field');
  lines.push('from typing import Any, List, Literal, Optional, Union');
  lines.push('');
  for (const named of model.named) {
    if (named.description) {
      for (const line of splitDoc(named.description)) lines.push(`# ${line}`);
    }
    if (named.kind === 'enum') {
      const literals = named.values.map((value) => JSON.stringify(value)).join(', ');
      lines.push(`${named.name} = Literal[${literals}]`);
    } else {
      lines.push('@dataclass');
      lines.push(`class ${named.name}:`);
      if (named.description) {
        lines.push(`    """${named.description.replace(/"/g, "'")}"""`);
      }
      const sortedFields = [...named.fields].sort((a, b) => {
        const aOpt = !a.required || a.nullable;
        const bOpt = !b.required || b.nullable;
        if (aOpt === bOpt) return 0;
        return aOpt ? 1 : -1;
      });
      if (sortedFields.length === 0) {
        lines.push('    pass');
      } else {
        for (const fieldDef of sortedFields) {
          const optional = !fieldDef.required || fieldDef.nullable;
          let pyType = renderPyType(fieldDef.type);
          if (optional) pyType = `Optional[${pyType}]`;
          let suffix;
          if (!fieldDef.required) {
            if (fieldDef.type.tag === 'array') suffix = ' = field(default_factory=list)';
            else suffix = ' = None';
          } else if (fieldDef.nullable) {
            suffix = ' = None';
          } else {
            suffix = '';
          }
          if (fieldDef.description) lines.push(`    # ${fieldDef.description.replace(/\n/g, ' ')}`);
          lines.push(`    ${fieldDef.name}: ${pyType}${suffix}`);
        }
      }
    }
    lines.push('');
  }
  return lines.join('\n');
}

function renderPyType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'str';
        case 'integer': return 'int';
        case 'number': return 'float';
        case 'boolean': return 'bool';
        case 'any': return 'Any';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `List[${renderPyType(type.items)}]`;
    case 'ref': return `'${type.name}'`;
    case 'union': return `Union[${type.members.map(renderPyType).join(', ')}]`;
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

// ---------- Gleam ----------

function renderGleamToml() {
  return [
    'name = "dd_shared_interfaces"',
    'version = "0.1.0"',
    'description = "Generated Gleam types for dd shared cross-runtime interfaces. Do not edit by hand."',
    'target = "erlang"',
    '',
    '[dependencies]',
    'gleam_stdlib = ">= 0.40.0 and < 2.0.0"',
    'gleam_json = ">= 1.0.0 and < 3.0.0"',
    '',
  ].join('\n');
}

function renderGleam(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs');
  lines.push('// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.');
  lines.push('');
  lines.push('import gleam/dynamic.{type Dynamic}');
  lines.push('import gleam/option.{type Option}');
  lines.push('');
  for (const named of model.named) {
    if (named.description) {
      for (const line of splitDoc(named.description)) lines.push(`/// ${line}`);
    }
    if (named.kind === 'enum') {
      lines.push(`pub type ${named.name} {`);
      for (const value of named.values) {
        lines.push(`  ${gleamEnumVariant(value)}`);
      }
      lines.push('}');
    } else {
      lines.push(`pub type ${named.name} {`);
      lines.push(`  ${named.name}(`);
      const lastIndex = named.fields.length - 1;
      named.fields.forEach((fieldDef, index) => {
        const optional = !fieldDef.required || fieldDef.nullable;
        let gleamType = renderGleamType(fieldDef.type);
        if (optional) gleamType = `Option(${gleamType})`;
        const comma = index === lastIndex ? '' : ',';
        const snake = camelToSnake(fieldDef.name);
        lines.push(`    ${snake}: ${gleamType}${comma}`);
      });
      lines.push('  )');
      lines.push('}');
    }
    lines.push('');
  }
  return lines.join('\n');
}

function gleamEnumVariant(value) {
  const camel = value.replace(/(^|[-_\s]+)([a-zA-Z0-9])/g, (_match, _sep, ch) => ch.toUpperCase());
  return camel.replace(/[^A-Za-z0-9]/g, '') || 'Unknown';
}

function renderGleamType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'String';
        case 'integer': return 'Int';
        case 'number': return 'Float';
        case 'boolean': return 'Bool';
        case 'any': return 'Dynamic';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `List(${renderGleamType(type.items)})`;
    case 'ref': return type.name;
    case 'union': return 'Dynamic';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

// ---------- helpers ----------

function camelToSnake(name) {
  return name.replace(/[A-Z]/g, (ch, index) => (index === 0 ? ch.toLowerCase() : `_${ch.toLowerCase()}`));
}

function splitDoc(text) {
  return text.split(/\n+/).map((line) => line.trim()).filter((line) => line.length > 0);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
