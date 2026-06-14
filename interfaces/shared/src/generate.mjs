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
  add('generated/haskell/src/DdSharedInterfaces.hs', renderHaskellShared(model));
  add('generated/haskell/dd-shared-interfaces.cabal', renderHaskellSharedCabal());
  add('generated/ocaml/lib/dd_shared_interfaces.ml', renderOcamlShared(model));
  add('generated/ocaml/lib/dune', renderOcamlSharedDune());
  add('generated/ocaml/dune-project', renderOcamlSharedDuneProject());
  add('generated/fsharp/DdSharedInterfaces.fs', renderFSharpShared(model));
  add('generated/fsharp/DdSharedInterfaces.fsproj', renderFSharpSharedProject());
  add('generated/cpp/dd_shared_interfaces.hpp', renderCppShared(model));
  add('generated/cpp/CMakeLists.txt', renderCppSharedCMake());
  add('generated/zig/dd_shared_interfaces.zig', renderZigShared(model));
  add('generated/zig/build.zig', renderZigSharedBuild());
  add('generated/elixir/lib/dd_shared_interfaces.ex', renderElixirShared(model));
  add('generated/elixir/mix.exs', renderElixirSharedMix());
  add('generated/go/interfaces.go', renderGoShared(model));
  add('generated/go/go.mod', renderGoSharedMod());
  add('generated/jvm/src/main/java/dd/shared/DdSharedInterfaces.java', renderJavaShared(model));
  add('generated/jvm/pom.xml', renderJavaSharedPom());
  add('generated/dart/lib/dd_shared_interfaces.dart', renderDartShared(model));
  add('generated/dart/pubspec.yaml', renderDartSharedPubspec());
  add('generated/erlang/include/dd_shared_interfaces.hrl', renderErlangSharedHrl(model));
  add('generated/erlang/src/dd_shared_interfaces.erl', renderErlangShared(model));
  add('generated/erlang/rebar.config', renderErlangSharedRebar());
  add('generated/erlang/src/dd_shared_interfaces.app.src', renderErlangSharedApp());
  add('generated/javascript/index.mjs', renderJavaScriptShared(model));
  add('generated/javascript/package.json', renderJavaScriptSharedPackageJson());
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

// ---------- Haskell ----------

function renderHaskellShared(model) {
  const lines = [
    '{-# LANGUAGE OverloadedStrings #-}',
    '-- AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '-- Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    '',
    'module DdSharedInterfaces where',
    '',
    'import Data.Text (Text)',
    '',
  ];

  for (const named of model.named) {
    if (named.description) {
      for (const line of splitDoc(named.description)) lines.push(`-- ${line}`);
    }
    if (named.kind === 'enum') {
      const ctors = named.values.map((value) => `${named.name}${gleamEnumVariant(value)}`);
      lines.push(`data ${named.name} = ${ctors.join(' | ')}`);
      lines.push('  deriving (Eq, Show)');
    } else if (named.fields.length === 0) {
      lines.push(`data ${named.name} = ${named.name}`);
      lines.push('  deriving (Eq, Show)');
    } else {
      lines.push(`data ${named.name} = ${named.name}`);
      named.fields.forEach((fieldDef, index) => {
        const prefix = index === 0 ? '  {' : '  ,';
        let type = haskellSharedType(fieldDef.type);
        if (!fieldDef.required || fieldDef.nullable) type = `(Maybe ${type})`;
        lines.push(`${prefix} ${sharedLowerCamel(named.name)}${fieldDef.name.charAt(0).toUpperCase()}${fieldDef.name.slice(1)} :: ${type}`);
      });
      lines.push('  } deriving (Eq, Show)');
    }
    lines.push('');
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function haskellSharedType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'Text';
        case 'integer': return 'Int';
        case 'number': return 'Double';
        case 'boolean': return 'Bool';
        case 'any': return 'Text';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `[${haskellSharedType(type.items)}]`;
    case 'ref': return type.name;
    case 'union': return 'Text';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderHaskellSharedCabal() {
  return `${[
    '-- AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs. Do not edit by hand.',
    'cabal-version: 2.4',
    'name: dd-shared-interfaces',
    'version: 0.1.0.0',
    'synopsis: Generated Haskell types for dd cross-runtime shared interfaces.',
    'build-type: Simple',
    '',
    'library',
    '  exposed-modules: DdSharedInterfaces',
    '  hs-source-dirs: src',
    '  build-depends: base >=4.14 && <5, text',
    '  default-language: Haskell2010',
  ].join('\n')}\n`;
}

// ---------- OCaml ----------

function renderOcamlShared(model) {
  const lines = [
    '(* AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs *)',
    '(* Do not edit by hand; edit the JSON Schema under schema/ and regenerate. *)',
    '',
  ];

  // Single mutually-recursive block so cross-references resolve regardless of declaration order.
  model.named.forEach((named, index) => {
    const keyword = index === 0 ? 'type' : 'and';
    const typeName = camelToSnake(named.name);
    if (named.kind === 'enum') {
      const tags = named.values.map((value) => `\`${gleamEnumVariant(value)}`);
      lines.push(`${keyword} ${typeName} = [ ${tags.join(' | ')} ]`);
    } else if (named.fields.length === 0) {
      lines.push(`${keyword} ${typeName} = unit`);
    } else {
      lines.push(`${keyword} ${typeName} = {`);
      for (const fieldDef of named.fields) {
        let type = ocamlSharedType(fieldDef.type);
        if (!fieldDef.required || fieldDef.nullable) type = `${type} option`;
        lines.push(`  ${typeName}_${camelToSnake(fieldDef.name)} : ${type};`);
      }
      lines.push('}');
    }
  });

  return `${lines.join('\n').trimEnd()}\n`;
}

function ocamlSharedType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'string';
        case 'integer': return 'int';
        case 'number': return 'float';
        case 'boolean': return 'bool';
        case 'any': return 'string';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `${ocamlSharedType(type.items)} list`;
    case 'ref': return camelToSnake(type.name);
    case 'union': return 'string';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderOcamlSharedDune() {
  return `${['(library', ' (name dd_shared_interfaces))'].join('\n')}\n`;
}

function renderOcamlSharedDuneProject() {
  return '(lang dune 3.0)\n';
}

function sharedLowerCamel(name) {
  return name ? name.charAt(0).toLowerCase() + name.slice(1) : name;
}

// ---------- shared codegen helpers for F#/C++/Zig ----------

function collectTypeRefs(type) {
  if (type.tag === 'ref') return [type.name];
  if (type.tag === 'array') return collectTypeRefs(type.items);
  if (type.tag === 'union') return type.members.flatMap(collectTypeRefs);
  return [];
}

function topoSortNamed(named) {
  const byName = new Map(named.map((n) => [n.name, n]));
  const sorted = [];
  const visited = new Set();
  const visit = (node) => {
    if (visited.has(node.name)) return;
    visited.add(node.name);
    const refs = node.kind === 'object' ? node.fields.flatMap((f) => collectTypeRefs(f.type)) : [];
    for (const ref of refs) {
      const dep = byName.get(ref);
      if (dep) visit(dep);
    }
    sorted.push(node);
  };
  for (const node of named) visit(node);
  return sorted;
}

const ZIG_KEYWORDS_IFACE = new Set([
  'addrspace', 'align', 'allowzero', 'and', 'anyframe', 'anytype', 'asm', 'async', 'await', 'break',
  'callconv', 'catch', 'comptime', 'const', 'continue', 'defer', 'else', 'enum', 'errdefer', 'error',
  'export', 'extern', 'fn', 'for', 'if', 'inline', 'linksection', 'noalias', 'noinline', 'nosuspend',
  'opaque', 'or', 'orelse', 'packed', 'pub', 'resume', 'return', 'struct', 'suspend', 'switch', 'test',
  'threadlocal', 'try', 'union', 'unreachable', 'usingnamespace', 'var', 'volatile', 'while',
]);

function zigIdentIface(name) {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(name) && !ZIG_KEYWORDS_IFACE.has(name)
    ? name
    : `@${JSON.stringify(name)}`;
}

function pascalField(name) {
  return `${name.charAt(0).toUpperCase()}${name.slice(1)}`;
}

function renderFSharpShared(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'module DdSharedInterfaces',
  ];
  model.named.forEach((named, index) => {
    const keyword = index === 0 ? 'type' : 'and';
    lines.push('');
    if (named.kind === 'enum') {
      lines.push(`${keyword} [<RequireQualifiedAccess>] ${named.name} =`);
      for (const value of named.values) lines.push(`    | ${gleamEnumVariant(value)}`);
    } else if (named.fields.length === 0) {
      lines.push(`${keyword} ${named.name} = obj`);
    } else {
      lines.push(`${keyword} ${named.name} =`);
      named.fields.forEach((field, fieldIndex) => {
        const open = fieldIndex === 0 ? '    { ' : '      ';
        const close = fieldIndex === named.fields.length - 1 ? ' }' : '';
        let type = fsharpSharedType(field.type);
        if (!field.required || field.nullable) type = `${type} option`;
        lines.push(`${open}${named.name}${pascalField(field.name)}: ${type}${close}`);
      });
    }
  });
  return `${lines.join('\n')}\n`;
}

function fsharpSharedType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'string';
        case 'integer': return 'int';
        case 'number': return 'float';
        case 'boolean': return 'bool';
        case 'any': return 'string';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `${fsharpSharedType(type.items)} list`;
    case 'ref': return type.name;
    case 'union': return 'string';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderFSharpSharedProject() {
  return `${[
    '<!-- AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs. Do not edit by hand. -->',
    '<Project Sdk="Microsoft.NET.Sdk">',
    '  <PropertyGroup>',
    '    <TargetFramework>net8.0</TargetFramework>',
    '  </PropertyGroup>',
    '  <ItemGroup>',
    '    <Compile Include="DdSharedInterfaces.fs" />',
    '  </ItemGroup>',
    '</Project>',
  ].join('\n')}\n`;
}

function renderCppShared(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    '#pragma once',
    '',
    '#include <cstdint>',
    '#include <optional>',
    '#include <string>',
    '#include <vector>',
    '',
    'namespace dd_shared_interfaces {',
  ];
  for (const named of model.named.filter((n) => n.kind === 'enum')) {
    lines.push('');
    lines.push(`enum class ${named.name} { ${named.values.map((v) => gleamEnumVariant(v)).join(', ')} };`);
  }
  for (const named of topoSortNamed(model.named).filter((n) => n.kind === 'object')) {
    lines.push('');
    lines.push(`struct ${named.name} {`);
    for (const field of named.fields) {
      const optional = !field.required || field.nullable;
      const type = optional ? `std::optional<${cppSharedType(field.type)}>` : cppSharedType(field.type);
      lines.push(`    ${type} ${camelToSnake(field.name)};`);
    }
    lines.push('};');
  }
  lines.push('');
  lines.push('}  // namespace dd_shared_interfaces');
  return `${lines.join('\n')}\n`;
}

function cppSharedType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'std::string';
        case 'integer': return 'int64_t';
        case 'number': return 'double';
        case 'boolean': return 'bool';
        case 'any': return 'std::string';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `std::vector<${cppSharedType(type.items)}>`;
    case 'ref': return type.name;
    case 'union': return 'std::string';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderCppSharedCMake() {
  return `${[
    '# AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs. Do not edit by hand.',
    'cmake_minimum_required(VERSION 3.14)',
    'project(dd_shared_interfaces CXX)',
    'add_library(dd_shared_interfaces INTERFACE)',
    'target_include_directories(dd_shared_interfaces INTERFACE ${CMAKE_CURRENT_SOURCE_DIR})',
    'target_compile_features(dd_shared_interfaces INTERFACE cxx_std_17)',
  ].join('\n')}\n`;
}

function renderZigShared(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
  ];
  for (const named of model.named) {
    lines.push('');
    if (named.kind === 'enum') {
      lines.push(`pub const ${named.name} = enum {`);
      for (const value of named.values) lines.push(`    ${zigIdentIface(gleamEnumVariant(value))},`);
      lines.push('};');
    } else if (named.fields.length === 0) {
      lines.push(`pub const ${named.name} = struct {};`);
    } else {
      lines.push(`pub const ${named.name} = struct {`);
      for (const field of named.fields) {
        const optional = !field.required || field.nullable;
        const type = optional ? `?${zigSharedType(field.type)}` : zigSharedType(field.type);
        lines.push(`    ${zigIdentIface(camelToSnake(field.name))}: ${type},`);
      }
      lines.push('};');
    }
  }
  return `${lines.join('\n')}\n`;
}

function zigSharedType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return '[]const u8';
        case 'integer': return 'i64';
        case 'number': return 'f64';
        case 'boolean': return 'bool';
        case 'any': return '[]const u8';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `[]const ${zigSharedType(type.items)}`;
    case 'ref': return type.name;
    case 'union': return '[]const u8';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderZigSharedBuild() {
  return `${[
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs. Do not edit by hand.',
    'const std = @import("std");',
    '',
    'pub fn build(b: *std.Build) void {',
    '    const target = b.standardTargetOptions(.{});',
    '    const optimize = b.standardOptimizeOption(.{});',
    '    _ = b.addModule("dd_shared_interfaces", .{',
    '        .root_source_file = b.path("dd_shared_interfaces.zig"),',
    '        .target = target,',
    '        .optimize = optimize,',
    '    });',
    '}',
  ].join('\n')}\n`;
}

function renderElixirShared(model) {
  const lines = [
    '# AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '# Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'defmodule DdSharedInterfaces do',
  ];
  for (const named of model.named) {
    lines.push('');
    if (named.kind === 'enum') {
      lines.push(`  defmodule ${named.name} do`);
      lines.push(`    def values, do: [${named.values.map((v) => JSON.stringify(v)).join(', ')}]`);
      lines.push('  end');
    } else {
      const fields = named.fields.map((f) => `:${camelToSnake(f.name)}`).join(', ');
      lines.push(`  defmodule ${named.name} do`);
      lines.push(`    defstruct [${fields}]`);
      lines.push('  end');
    }
  }
  lines.push('end');
  return `${lines.join('\n')}\n`;
}

function renderElixirSharedMix() {
  return `${[
    '# AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs. Do not edit by hand.',
    'defmodule DdSharedInterfaces.MixProject do',
    '  use Mix.Project',
    '',
    '  def project do',
    '    [app: :dd_shared_interfaces, version: "0.1.0", elixir: "~> 1.14"]',
    '  end',
    'end',
  ].join('\n')}\n`;
}

function toCamel(name) {
  return name.replace(/_([a-z0-9])/g, (_match, ch) => ch.toUpperCase());
}

// ---------- Go ----------

function renderGoShared(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'package ddsharedinterfaces',
  ];
  for (const named of model.named.filter((n) => n.kind === 'enum')) {
    lines.push('');
    lines.push(`type ${named.name} = string`);
    lines.push('const (');
    for (const value of named.values) {
      lines.push(`\t${named.name}${gleamEnumVariant(value)} ${named.name} = ${JSON.stringify(value)}`);
    }
    lines.push(')');
  }
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    lines.push('');
    lines.push(`type ${named.name} struct {`);
    for (const field of named.fields) {
      const optional = !field.required || field.nullable;
      const tag = optional ? `${field.name},omitempty` : field.name;
      lines.push(`\t${pascalField(field.name)} ${goSharedFieldType(field)} \`json:"${tag}"\``);
    }
    lines.push('}');
  }
  return `${lines.join('\n')}\n`;
}

function goSharedBaseType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'string';
        case 'integer': return 'int64';
        case 'number': return 'float64';
        case 'boolean': return 'bool';
        case 'any': return 'any';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `[]${goSharedBaseType(type.items)}`;
    case 'ref': return type.name;
    case 'union': return 'any';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function goSharedFieldType(field) {
  const base = goSharedBaseType(field.type);
  if ((!field.required || field.nullable) && field.type.tag !== 'array') {
    return `*${base}`;
  }
  return base;
}

function renderGoSharedMod() {
  return `${['module dd/shared/interfaces', '', 'go 1.21'].join('\n')}\n`;
}

// ---------- Java ----------

function renderJavaShared(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'package dd.shared;',
    '',
    'import java.util.List;',
    '',
    'public final class DdSharedInterfaces {',
    '    private DdSharedInterfaces() {}',
  ];
  for (const named of model.named.filter((n) => n.kind === 'enum')) {
    lines.push('');
    lines.push(`    public enum ${named.name} { ${named.values.map((v) => gleamEnumVariant(v)).join(', ')} }`);
  }
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    lines.push('');
    const components = named.fields.map((f) => `${javaSharedComponentType(f)} ${toCamel(f.name)}`).join(', ');
    lines.push(`    public record ${named.name}(${components}) {}`);
  }
  lines.push('}');
  return `${lines.join('\n')}\n`;
}

function javaSharedComponentType(field) {
  const optional = !field.required || field.nullable;
  switch (field.type.tag) {
    case 'primitive':
      switch (field.type.name) {
        case 'string': return 'String';
        case 'integer': return optional ? 'Long' : 'long';
        case 'number': return optional ? 'Double' : 'double';
        case 'boolean': return optional ? 'Boolean' : 'boolean';
        case 'any': return 'Object';
        default: throw new Error(`unknown primitive ${field.type.name}`);
      }
    case 'array': return `List<${javaSharedBoxedType(field.type.items)}>`;
    case 'ref': return field.type.name;
    case 'union': return 'Object';
    default: throw new Error(`unhandled type tag ${field.type.tag}`);
  }
}

function javaSharedBoxedType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'String';
        case 'integer': return 'Long';
        case 'number': return 'Double';
        case 'boolean': return 'Boolean';
        case 'any': return 'Object';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `List<${javaSharedBoxedType(type.items)}>`;
    case 'ref': return type.name;
    case 'union': return 'Object';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderJavaSharedPom() {
  return `${[
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<project xmlns="http://maven.apache.org/POM/4.0.0">',
    '    <modelVersion>4.0.0</modelVersion>',
    '    <groupId>dev.dd.shared</groupId>',
    '    <artifactId>dd-shared-interfaces</artifactId>',
    '    <version>0.1.0</version>',
    '    <packaging>jar</packaging>',
    '    <description>Generated Java shared interfaces - do not edit by hand.</description>',
    '    <properties>',
    '        <maven.compiler.source>17</maven.compiler.source>',
    '        <maven.compiler.target>17</maven.compiler.target>',
    '        <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>',
    '    </properties>',
    '</project>',
  ].join('\n')}\n`;
}

// ---------- Dart ----------

function renderDartShared(model) {
  // Enum value types are string-valued, so a ref to one decodes as a plain String (no Dart enum
  // is emitted); object refs decode through the generated class's fromJson.
  const enumNames = new Set(model.named.filter((n) => n.kind === 'enum').map((n) => n.name));
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
  ];
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    lines.push('');
    lines.push(`class ${named.name} {`);
    for (const field of named.fields) {
      lines.push(`  final ${dartSharedType(field, enumNames)} ${toCamel(field.name)};`);
    }
    const ctorParams = named.fields
      .map((f) => (!f.required || f.nullable ? `this.${toCamel(f.name)}` : `required this.${toCamel(f.name)}`))
      .join(', ');
    lines.push(`  const ${named.name}({${ctorParams}});`);
    lines.push(`  factory ${named.name}.fromJson(Map<String, dynamic> json) => ${named.name}(`);
    for (const field of named.fields) {
      lines.push(`        ${toCamel(field.name)}: ${dartSharedFromJson(field.type, `json['${field.name}']`, !field.required || field.nullable, enumNames)},`);
    }
    lines.push('      );');
    lines.push('}');
  }
  return `${lines.join('\n')}\n`;
}

function dartSharedBaseType(type, enumNames) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'String';
        case 'integer': return 'int';
        case 'number': return 'double';
        case 'boolean': return 'bool';
        case 'any': return 'dynamic';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `List<${dartSharedBaseType(type.items, enumNames)}>`;
    case 'ref': return enumNames.has(type.name) ? 'String' : type.name;
    case 'union': return 'dynamic';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function dartSharedType(field, enumNames) {
  const base = dartSharedBaseType(field.type, enumNames);
  if ((!field.required || field.nullable) && base !== 'dynamic') {
    return `${base}?`;
  }
  return base;
}

function dartSharedFromJson(type, access, optional, enumNames) {
  const decode = (innerType, innerAccess) => {
    if (innerType.tag === 'ref' && !enumNames.has(innerType.name)) {
      return `${innerType.name}.fromJson(${innerAccess} as Map<String, dynamic>)`;
    }
    if (innerType.tag === 'array') {
      return `(${innerAccess} as List).map((e) => ${decode(innerType.items, 'e')}).toList()`;
    }
    return `${innerAccess} as ${dartSharedBaseType(innerType, enumNames)}`;
  };
  if (optional) {
    return `${access} == null ? null : ${decode(type, access)}`;
  }
  return decode(type, access);
}

function renderDartSharedPubspec() {
  return `${[
    'name: dd_shared_interfaces',
    'description: Generated Dart types for dd cross-runtime shared interfaces. Do not edit by hand.',
    'version: 0.1.0',
    'publish_to: none',
    '',
    'environment:',
    "  sdk: '>=3.0.0 <4.0.0'",
  ].join('\n')}\n`;
}

// ---------- Erlang ----------

function renderErlangSharedHrl(model) {
  const lines = [
    '%% AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '%% Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
  ];
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    const fields = named.fields.map((f) => camelToSnake(f.name)).join(', ');
    lines.push(`-record(${camelToSnake(named.name)}, {${fields}}).`);
  }
  return `${lines.join('\n')}\n`;
}

function renderErlangShared(model) {
  const fns = [];
  const exports = [];
  for (const named of model.named.filter((n) => n.kind === 'enum')) {
    exports.push(`${camelToSnake(named.name)}_values/0`);
    fns.push(`${camelToSnake(named.name)}_values() -> [${named.values.map((v) => JSON.stringify(v)).join(', ')}].`);
    fns.push('');
  }
  return `${[
    '%% AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '%% Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    '-module(dd_shared_interfaces).',
    `-export([${exports.join(', ')}]).`,
    '',
    ...fns,
  ].join('\n').trimEnd()}\n`;
}

function renderErlangSharedRebar() {
  return `${['{erl_opts, [debug_info]}.', '{deps, []}.'].join('\n')}\n`;
}

function renderErlangSharedApp() {
  return `${[
    '{application, dd_shared_interfaces,',
    ' [{description, "Generated Erlang shared interfaces"},',
    '  {vsn, "0.1.0"},',
    '  {registered, []},',
    '  {applications, [kernel, stdlib]},',
    '  {env, []},',
    '  {modules, [dd_shared_interfaces]}]}.',
  ].join('\n')}\n`;
}

// ---------- JavaScript ----------

function renderJavaScriptShared(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/shared/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'export {};',
  ];
  for (const named of model.named.filter((n) => n.kind === 'enum')) {
    lines.push('');
    lines.push(`/** @typedef {${named.values.map((v) => JSON.stringify(v)).join(' | ')}} ${named.name} */`);
  }
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    lines.push('');
    lines.push('/**');
    lines.push(` * @typedef {object} ${named.name}`);
    for (const field of named.fields) {
      const optional = !field.required || field.nullable;
      const name = optional ? `[${toCamel(field.name)}]` : toCamel(field.name);
      lines.push(` * @property {${jsSharedType(field.type)}} ${name}`);
    }
    lines.push(' */');
  }
  return `${lines.join('\n')}\n`;
}

function jsSharedType(type) {
  switch (type.tag) {
    case 'primitive':
      switch (type.name) {
        case 'string': return 'string';
        case 'integer': return 'number';
        case 'number': return 'number';
        case 'boolean': return 'boolean';
        case 'any': return '*';
        default: throw new Error(`unknown primitive ${type.name}`);
      }
    case 'array': return `${jsSharedType(type.items)}[]`;
    case 'ref': return type.name;
    case 'union': return '*';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderJavaScriptSharedPackageJson() {
  return `${JSON.stringify(
    {
      name: '@dd/shared-interfaces-js',
      version: '0.1.0',
      private: true,
      type: 'module',
      description: 'AUTOGENERATED pure-ESM JavaScript shared interface typedefs. Do not edit by hand.',
      main: 'index.mjs',
      exports: { '.': './index.mjs' },
    },
    null,
    2,
  )}\n`;
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
