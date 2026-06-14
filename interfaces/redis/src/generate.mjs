// JSON Schema -> per-language types + redis key formatters generator.
//
// Source of truth: every *.schema.json file listed in schema/index.json.
// Each schema document MAY declare value types under `$defs` (same conventions
// as @dd/shared-interfaces) AND key conventions under the `$dd:redis`
// extension block. Cross-schema refs are not supported in this first pass.
//
// `$dd:redis` shape:
//   {
//     "service": "dd-runtime-config",
//     "summary": "...",
//     "keys": [
//       {
//         "name": "RuntimeConfigEntryKey",
//         "description": "...",
//         "pattern": "{prefix}:{env}:entry:{scope}:{key}",
//         "defaultPrefix": "dd:rc",
//         "params": [
//           { "name": "prefix", "type": "string", "description": "..." },
//           { "name": "env",    "type": "string" },
//           ...
//         ],
//         "valueType": "json" | "json-shared-interface" | "opaque-string"
//                    | "set-of-string" | "integer",
//         "valueRef":  "RuntimeConfigEntry"   // for json / json-shared-interface
//       }
//     ]
//   }
//
// Outputs (kept idiomatic per-language and free of external runtime deps):
//   generated/typescript/index.ts
//   generated/rust/Cargo.toml + src/lib.rs
//   generated/python/dd_redis_interfaces.py + __init__.py
//   generated/gleam/gleam.toml + src/dd_redis_interfaces.gleam
//
// Run `pnpm --filter @dd/redis-interfaces generate` to write files;
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
        `redis-interfaces generated outputs are stale:\n${stale.map((file) => `  - ${file}`).join('\n')}`,
      );
      process.exitCode = 1;
      return;
    }

    console.log('redis-interfaces generated outputs are up to date.');
    return;
  }

  for (const [relativePath, contents] of outputs) {
    const absolutePath = path.join(packageRoot, relativePath);
    await mkdir(path.dirname(absolutePath), { recursive: true });
    await writeFile(absolutePath, contents);
  }
  console.log(`Generated ${outputs.size} redis-interfaces files.`);
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
 *
 * @typedef {{ name: string, description?: string, defaultValue?: string }} KeyParam
 * @typedef {{
 *   name: string,
 *   description?: string,
 *   pattern: string,
 *   defaultPrefix?: string,
 *   params: KeyParam[],
 *   valueType: string,
 *   valueRef?: string,
 *   valueDescription?: string,
 *   service?: string,
 * }} RedisKey
 */

const VALUE_TYPES = new Set([
  'json',
  'json-shared-interface',
  'opaque-string',
  'set-of-string',
  'integer',
]);

function buildModel(schemaFiles) {
  /** @type {NamedType[]} */
  const named = [];
  /** @type {RedisKey[]} */
  const keys = [];
  const seenTypes = new Set();
  const seenKeys = new Set();
  for (const { filename, doc } of schemaFiles) {
    const defs = doc.$defs ?? doc.definitions ?? {};
    for (const [name, def] of Object.entries(defs)) {
      if (seenTypes.has(name)) {
        throw new Error(`Duplicate $defs name across schema files: ${name}`);
      }
      seenTypes.add(name);
      named.push(resolveNamed(name, def, filename));
    }
    const ext = doc['$dd:redis'];
    if (ext) {
      const service = ext.service;
      const ks = ext.keys ?? [];
      for (const key of ks) {
        if (seenKeys.has(key.name)) {
          throw new Error(`Duplicate redis key name across schemas: ${key.name}`);
        }
        seenKeys.add(key.name);
        if (!key.pattern) {
          throw new Error(`redis key ${key.name} missing pattern`);
        }
        if (!Array.isArray(key.params) || key.params.length === 0) {
          throw new Error(`redis key ${key.name} must declare params`);
        }
        for (const param of key.params) {
          if (!param.name || !param.type) {
            throw new Error(`redis key ${key.name} param ${JSON.stringify(param)} missing name/type`);
          }
          if (param.type !== 'string') {
            throw new Error(
              `redis key ${key.name}.${param.name}: only string params are supported in v1`,
            );
          }
        }
        const placeholders = [...key.pattern.matchAll(/\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g)].map((m) => m[1]);
        const paramNames = new Set(key.params.map((p) => p.name));
        for (const ph of placeholders) {
          if (!paramNames.has(ph)) {
            throw new Error(`redis key ${key.name} pattern references {${ph}} but it's not in params`);
          }
        }
        if (!VALUE_TYPES.has(key.valueType)) {
          throw new Error(`redis key ${key.name} has unsupported valueType: ${key.valueType}`);
        }
        keys.push({
          name: key.name,
          description: key.description,
          pattern: key.pattern,
          defaultPrefix: key.defaultPrefix,
          params: key.params.map((p) => ({
            name: p.name,
            description: p.description,
          })),
          valueType: key.valueType,
          valueRef: key.valueRef,
          valueDescription: key.valueDescription,
          service: service,
        });
      }
    }
  }
  named.sort((a, b) => a.name.localeCompare(b.name));
  keys.sort((a, b) => a.name.localeCompare(b.name));
  return { named, keys };
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
    if (others.includes('array') || others.includes('object')) {
      return { typeRef: { tag: 'primitive', name: 'any' }, nullable };
    }
    const members = others.map((entry) => {
      const single = { ...def, type: entry };
      return resolveTypeRef(single, label).typeRef;
    });
    return { typeRef: { tag: 'union', members }, nullable };
  }

  if (def.type === 'string') return { typeRef: { tag: 'primitive', name: 'string' }, nullable: false };
  if (def.type === 'integer') return { typeRef: { tag: 'primitive', name: 'integer' }, nullable: false };
  if (def.type === 'number') return { typeRef: { tag: 'primitive', name: 'number' }, nullable: false };
  if (def.type === 'boolean') return { typeRef: { tag: 'primitive', name: 'boolean' }, nullable: false };
  if (def.type === 'null') return { typeRef: { tag: 'primitive', name: 'any' }, nullable: true };
  if (def.type === 'array') {
    if (!def.items) throw new Error(`array type missing items at ${label}`);
    const { typeRef } = resolveTypeRef(def.items, `${label}[]`);
    return { typeRef: { tag: 'array', items: typeRef }, nullable: false };
  }
  if (def.type === 'object' && (!def.properties || Object.keys(def.properties).length === 0)) {
    return { typeRef: { tag: 'primitive', name: 'any' }, nullable: false };
  }
  if (def.type === undefined) {
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
  add('generated/python/dd_redis_interfaces.py', renderPython(model));
  add('generated/python/__init__.py', 'from .dd_redis_interfaces import *  # noqa: F401,F403\n');
  add('generated/gleam/gleam.toml', renderGleamToml());
  add('generated/gleam/src/dd_redis_interfaces.gleam', renderGleam(model));
  add('generated/haskell/src/DdRedisInterfaces.hs', renderHaskellRedis(model));
  add('generated/haskell/dd-redis-interfaces.cabal', renderHaskellRedisCabal());
  add('generated/ocaml/lib/dd_redis_interfaces.ml', renderOcamlRedis(model));
  add('generated/ocaml/lib/dune', renderOcamlRedisDune());
  add('generated/ocaml/dune-project', renderOcamlDuneProject());
  add('generated/fsharp/DdRedisInterfaces.fs', renderFSharpRedis(model));
  add('generated/fsharp/DdRedisInterfaces.fsproj', renderFSharpRedisProject());
  add('generated/cpp/dd_redis_interfaces.hpp', renderCppRedis(model));
  add('generated/cpp/CMakeLists.txt', renderCppRedisCMake());
  add('generated/zig/dd_redis_interfaces.zig', renderZigRedis(model));
  add('generated/zig/build.zig', renderZigRedisBuild());
  add('generated/elixir/lib/dd_redis_interfaces.ex', renderElixirRedis(model));
  add('generated/elixir/mix.exs', renderElixirRedisMix());
  add('generated/go/interfaces.go', renderGoRedis(model));
  add('generated/go/go.mod', renderGoRedisMod());
  add('generated/jvm/src/main/java/dd/redis/DdRedisInterfaces.java', renderJavaRedis(model));
  add('generated/jvm/pom.xml', renderJavaRedisPom());
  add('generated/dart/lib/dd_redis_interfaces.dart', renderDartRedis(model));
  add('generated/dart/pubspec.yaml', renderDartRedisPubspec());
  add('generated/erlang/include/dd_redis_interfaces.hrl', renderErlangRedisHrl(model));
  add('generated/erlang/src/dd_redis_interfaces.erl', renderErlangRedis(model));
  add('generated/erlang/rebar.config', renderErlangRedisRebar());
  add('generated/erlang/src/dd_redis_interfaces.app.src', renderErlangRedisApp());
  add('generated/javascript/index.mjs', renderJavaScriptRedis(model));
  add('generated/javascript/package.json', renderJavaScriptRedisPackageJson());
  return outputs;
}

// ---------- TypeScript ----------

function renderTypeScript(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs');
  lines.push('// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.');
  lines.push('// Source schemas: remote/libs/interfaces/redis/schema/*.schema.json');
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

  if (model.keys.length > 0) {
    lines.push('// ---------- Redis key formatters ----------');
    lines.push('');
    for (const key of model.keys) {
      const params = key.params.map((p) => `${p.name}: string`).join(', ');
      const fnName = lowerCamel(key.name);
      if (key.description) {
        for (const line of splitDoc(key.description)) lines.push(`/** ${line} */`);
      } else {
        lines.push(`/** Format the redis key '${key.pattern}' for ${key.name}. */`);
      }
      const tmpl = key.pattern.replace(/\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g, '${$1}');
      lines.push(`export function ${fnName}(${params}): string {`);
      lines.push(`  return \`${tmpl}\`;`);
      lines.push('}');
      lines.push('');
      if (key.defaultPrefix) {
        const upper = upperSnake(key.name) + '_DEFAULT_PREFIX';
        lines.push(`export const ${upper} = ${JSON.stringify(key.defaultPrefix)};`);
        lines.push('');
      }
    }
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
    'name = "dd-redis-interfaces"',
    'version = "0.1.0"',
    'edition = "2021"',
    'description = "Generated Rust types and redis key formatters for dd cross-runtime redis usage. Do not edit by hand."',
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
  lines.push('// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs');
  lines.push('// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.');
  lines.push('');
  if (model.named.length > 0) {
    lines.push('use serde::{Deserialize, Serialize};');
    lines.push('use serde_json::Value;');
    lines.push('');
  }
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

  if (model.keys.length > 0) {
    lines.push('// ---------- Redis key formatters ----------');
    lines.push('');
    for (const key of model.keys) {
      const fnName = camelToSnake(key.name);
      const params = key.params.map((p) => `${camelToSnake(p.name)}: &str`).join(', ');
      if (key.description) {
        for (const line of splitDoc(key.description)) lines.push(`/// ${line}`);
      } else {
        lines.push(`/// Format the redis key '${key.pattern}' for ${key.name}.`);
      }
      lines.push(`pub fn ${fnName}(${params}) -> String {`);
      const fmtPattern = key.pattern.replace(/\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g, '{}');
      const args = key.params.map((p) => camelToSnake(p.name)).join(', ');
      lines.push(`    format!(${JSON.stringify(fmtPattern)}, ${args})`);
      lines.push('}');
      lines.push('');
      if (key.defaultPrefix) {
        const upper = upperSnake(key.name) + '_DEFAULT_PREFIX';
        lines.push(`pub const ${upper}: &str = ${JSON.stringify(key.defaultPrefix)};`);
        lines.push('');
      }
    }
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
  lines.push('"""AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs.');
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

  if (model.keys.length > 0) {
    lines.push('# ---------- Redis key formatters ----------');
    lines.push('');
    for (const key of model.keys) {
      const fnName = camelToSnake(key.name);
      const params = key.params.map((p) => `${camelToSnake(p.name)}: str`).join(', ');
      if (key.description) {
        lines.push(`def ${fnName}(${params}) -> str:`);
        lines.push(`    """${key.description.replace(/"/g, "'").split(/\n+/)[0]}"""`);
      } else {
        lines.push(`def ${fnName}(${params}) -> str:`);
      }
      const fmt = '"' + key.pattern.replace(/\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g, (_match, name) => `{${camelToSnake(name)}}`) + '"';
      const fmtArgs = key.params
        .map((p) => `${camelToSnake(p.name)}=${camelToSnake(p.name)}`)
        .join(', ');
      lines.push(`    return ${fmt}.format(${fmtArgs})`);
      lines.push('');
      if (key.defaultPrefix) {
        const upper = upperSnake(key.name) + '_DEFAULT_PREFIX';
        lines.push(`${upper} = ${JSON.stringify(key.defaultPrefix)}`);
        lines.push('');
      }
    }
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
    'name = "dd_redis_interfaces"',
    'version = "0.1.0"',
    'description = "Generated Gleam types and redis key formatters for dd cross-runtime redis usage. Do not edit by hand."',
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
  lines.push('// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs');
  lines.push('// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.');
  lines.push('');
  lines.push('import gleam/dynamic.{type Dynamic}');
  lines.push('import gleam/option.{type Option}');
  lines.push('import gleam/string');
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

  if (model.keys.length > 0) {
    lines.push('// ---------- Redis key formatters ----------');
    lines.push('');
    for (const key of model.keys) {
      const fnName = camelToSnake(key.name);
      const params = key.params.map((p) => `${camelToSnake(p.name)} ${camelToSnake(p.name)}: String`).join(', ');
      if (key.description) {
        for (const line of splitDoc(key.description)) lines.push(`/// ${line}`);
      } else {
        lines.push(`/// Format the redis key '${key.pattern}' for ${key.name}.`);
      }
      lines.push(`pub fn ${fnName}(${params}) -> String {`);
      // Build expression: literal segments joined with `<>`, with placeholders becoming the params.
      const segments = parsePattern(key.pattern);
      const exprParts = segments.map((seg) => {
        if (seg.kind === 'literal') return JSON.stringify(seg.text);
        return camelToSnake(seg.name);
      });
      lines.push('  ' + exprParts.join(' <> '));
      lines.push('}');
      lines.push('');
      if (key.defaultPrefix) {
        const upper = upperSnake(key.name) + '_DEFAULT_PREFIX';
        lines.push(`pub const ${upper.toLowerCase()} = ${JSON.stringify(key.defaultPrefix)}`);
        lines.push('');
      }
    }
    // Suppress unused-import warning for `string` in case no key uses it.
    lines.push('@internal');
    lines.push('pub fn _force_use_string_module(value: String) -> String {');
    lines.push('  string.lowercase(value)');
    lines.push('}');
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

function renderHaskellRedis(model) {
  const lines = [
    '{-# LANGUAGE OverloadedStrings #-}',
    '-- AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '-- Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    '',
    'module DdRedisInterfaces where',
    '',
    'import Data.Text (Text)',
    'import qualified Data.Text as T',
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
        let type = haskellRedisType(fieldDef.type);
        if (!fieldDef.required || fieldDef.nullable) type = `(Maybe ${type})`;
        lines.push(`${prefix} ${haskellRecordField(named.name, fieldDef.name)} :: ${type}`);
      });
      lines.push('  } deriving (Eq, Show)');
    }
    lines.push('');
  }

  if (model.keys.length > 0) {
    lines.push('-- Redis key formatters');
    lines.push('');
    for (const key of model.keys) {
      const fnName = lowerCamel(key.name);
      const paramNames = key.params.map((param) => lowerCamel(param.name));
      const signature = key.params.map(() => 'Text').concat('Text').join(' -> ');
      if (key.description) {
        for (const line of splitDoc(key.description)) lines.push(`-- ${line}`);
      }
      lines.push(`${fnName} :: ${signature}`);
      const segments = parsePattern(key.pattern).map((seg) =>
        seg.kind === 'literal' ? JSON.stringify(seg.text) : lowerCamel(seg.name),
      );
      lines.push(`${fnName} ${paramNames.join(' ')} = T.concat [${segments.join(', ')}]`);
      lines.push('');
      if (key.defaultPrefix) {
        const constName = `${lowerCamel(key.name)}DefaultPrefix`;
        lines.push(`${constName} :: Text`);
        lines.push(`${constName} = ${JSON.stringify(key.defaultPrefix)}`);
        lines.push('');
      }
    }
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function haskellRecordField(typeName, fieldName) {
  return `${lowerCamel(typeName)}${fieldName.charAt(0).toUpperCase()}${fieldName.slice(1)}`;
}

function haskellRedisType(type) {
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
    case 'array': return `[${haskellRedisType(type.items)}]`;
    case 'ref': return type.name;
    case 'union': return 'Text';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderHaskellRedisCabal() {
  return `${[
    '-- AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs. Do not edit by hand.',
    'cabal-version: 2.4',
    'name: dd-redis-interfaces',
    'version: 0.1.0.0',
    'synopsis: Generated Haskell types and redis key formatters for dd cross-runtime redis usage.',
    'build-type: Simple',
    '',
    'library',
    '  exposed-modules: DdRedisInterfaces',
    '  hs-source-dirs: src',
    '  build-depends: base >=4.14 && <5, text',
    '  default-language: Haskell2010',
  ].join('\n')}\n`;
}

// ---------- OCaml ----------

function renderOcamlRedis(model) {
  const lines = [
    '(* AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs *)',
    '(* Do not edit by hand; edit the JSON Schema under schema/ and regenerate. *)',
    '',
  ];

  // Emit every named type in a single mutually-recursive block so cross-references resolve
  // regardless of declaration order (OCaml types must be in scope before use).
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
        let type = ocamlRedisType(fieldDef.type);
        if (!fieldDef.required || fieldDef.nullable) type = `${type} option`;
        lines.push(`  ${typeName}_${camelToSnake(fieldDef.name)} : ${type};`);
      }
      lines.push('}');
    }
  });
  if (model.named.length > 0) {
    lines.push('');
  }

  if (model.keys.length > 0) {
    lines.push('(* Redis key formatters *)');
    lines.push('');
    for (const key of model.keys) {
      const fnName = camelToSnake(key.name);
      const segments = parsePattern(key.pattern);
      const usedParams = new Set(
        segments.filter((seg) => seg.kind === 'param').map((seg) => seg.name),
      );
      // OCaml errors on unused bindings under dune's dev profile; underscore any param the pattern
      // does not actually reference so the formatter still takes every declared argument.
      const paramBindings = key.params.map((param) =>
        usedParams.has(param.name) ? camelToSnake(param.name) : `_${camelToSnake(param.name)}`,
      );
      const expr = segments
        .map((seg) => (seg.kind === 'literal' ? JSON.stringify(seg.text) : camelToSnake(seg.name)))
        .join(' ^ ');
      lines.push(`let ${fnName} ${paramBindings.join(' ')} =`);
      lines.push(`  ${expr}`);
      lines.push('');
      if (key.defaultPrefix) {
        lines.push(`let ${fnName}_default_prefix = ${JSON.stringify(key.defaultPrefix)}`);
        lines.push('');
      }
    }
  }

  return `${lines.join('\n').trimEnd()}\n`;
}

function ocamlRedisType(type) {
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
    case 'array': return `${ocamlRedisType(type.items)} list`;
    case 'ref': return camelToSnake(type.name);
    case 'union': return 'string';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderOcamlRedisDune() {
  return `${['(library', ' (name dd_redis_interfaces))'].join('\n')}\n`;
}

function renderOcamlDuneProject() {
  return '(lang dune 3.0)\n';
}

// ---------- shared codegen helpers for F#/C++/Zig ----------

function collectTypeRefs(type) {
  if (type.tag === 'ref') return [type.name];
  if (type.tag === 'array') return collectTypeRefs(type.items);
  if (type.tag === 'union') return type.members.flatMap(collectTypeRefs);
  return [];
}

// Deps-first ordering so value-typed struct members resolve in languages (C++) that require a
// referenced type to be complete before use. Cycles are not expected in JSON-schema value types.
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

// ---------- F# ----------

function renderFSharpRedis(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'module DdRedisInterfaces',
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
        let type = fsharpRedisType(field.type);
        if (!field.required || field.nullable) type = `${type} option`;
        lines.push(`${open}${named.name}${pascalField(field.name)}: ${type}${close}`);
      });
    }
  });

  if (model.keys.length > 0) {
    lines.push('');
    for (const key of model.keys) {
      const params = key.params.map((p) => `(${camelToSnake(p.name)}: string)`).join(' ');
      const expr = parsePattern(key.pattern)
        .map((seg) => (seg.kind === 'literal' ? JSON.stringify(seg.text) : camelToSnake(seg.name)))
        .join(' + ');
      lines.push(`let ${lowerCamel(key.name)} ${params} : string = ${expr}`);
      if (key.defaultPrefix) {
        lines.push(`let ${lowerCamel(key.name)}DefaultPrefix : string = ${JSON.stringify(key.defaultPrefix)}`);
      }
    }
  }

  return `${lines.join('\n')}\n`;
}

function fsharpRedisType(type) {
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
    case 'array': return `${fsharpRedisType(type.items)} list`;
    case 'ref': return type.name;
    case 'union': return 'string';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderFSharpRedisProject() {
  return `${[
    '<!-- AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs. Do not edit by hand. -->',
    '<Project Sdk="Microsoft.NET.Sdk">',
    '  <PropertyGroup>',
    '    <TargetFramework>net8.0</TargetFramework>',
    '  </PropertyGroup>',
    '  <ItemGroup>',
    '    <Compile Include="DdRedisInterfaces.fs" />',
    '  </ItemGroup>',
    '</Project>',
  ].join('\n')}\n`;
}

// ---------- C++ ----------

function renderCppRedis(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    '#pragma once',
    '',
    '#include <cstdint>',
    '#include <optional>',
    '#include <string>',
    '#include <vector>',
    '',
    'namespace dd_redis_interfaces {',
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
      const type = optional ? `std::optional<${cppRedisType(field.type)}>` : cppRedisType(field.type);
      lines.push(`    ${type} ${camelToSnake(field.name)};`);
    }
    lines.push('};');
  }

  if (model.keys.length > 0) {
    lines.push('');
    for (const key of model.keys) {
      const params = key.params.map((p) => `const std::string& ${camelToSnake(p.name)}`).join(', ');
      const expr = parsePattern(key.pattern)
        .map((seg) => (seg.kind === 'literal' ? `std::string(${JSON.stringify(seg.text)})` : camelToSnake(seg.name)))
        .join(' + ');
      lines.push(`inline std::string ${camelToSnake(key.name)}(${params}) { return ${expr}; }`);
      if (key.defaultPrefix) {
        lines.push(`inline const char* ${upperSnake(key.name)}_DEFAULT_PREFIX = ${JSON.stringify(key.defaultPrefix)};`);
      }
    }
  }

  lines.push('');
  lines.push('}  // namespace dd_redis_interfaces');
  return `${lines.join('\n')}\n`;
}

function cppRedisType(type) {
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
    case 'array': return `std::vector<${cppRedisType(type.items)}>`;
    case 'ref': return type.name;
    case 'union': return 'std::string';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderCppRedisCMake() {
  return `${[
    '# AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs. Do not edit by hand.',
    'cmake_minimum_required(VERSION 3.14)',
    'project(dd_redis_interfaces CXX)',
    'add_library(dd_redis_interfaces INTERFACE)',
    'target_include_directories(dd_redis_interfaces INTERFACE ${CMAKE_CURRENT_SOURCE_DIR})',
    'target_compile_features(dd_redis_interfaces INTERFACE cxx_std_17)',
  ].join('\n')}\n`;
}

// ---------- Zig ----------

function renderZigRedis(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'const std = @import("std");',
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
        const type = optional ? `?${zigRedisType(field.type)}` : zigRedisType(field.type);
        lines.push(`    ${zigIdentIface(camelToSnake(field.name))}: ${type},`);
      }
      lines.push('};');
    }
  }

  for (const key of model.keys) {
    lines.push('');
    const segments = parsePattern(key.pattern);
    const used = new Set(segments.filter((s) => s.kind === 'param').map((s) => s.name));
    const params = key.params.map((p) => `${camelToSnake(p.name)}: []const u8`).join(', ');
    const fmt = segments
      .map((seg) => (seg.kind === 'literal' ? seg.text.replace(/\{/g, '{{').replace(/\}/g, '}}') : '{s}'))
      .join('');
    const args = segments.filter((s) => s.kind === 'param').map((s) => camelToSnake(s.name)).join(', ');
    lines.push(`pub fn ${camelToSnake(key.name)}(allocator: std.mem.Allocator, ${params}) ![]u8 {`);
    for (const param of key.params.filter((p) => !used.has(p.name))) {
      lines.push(`    _ = ${camelToSnake(param.name)};`);
    }
    lines.push(`    return std.fmt.allocPrint(allocator, ${JSON.stringify(fmt)}, .{ ${args} });`);
    lines.push('}');
    if (key.defaultPrefix) {
      lines.push(`pub const ${camelToSnake(key.name)}_default_prefix: []const u8 = ${JSON.stringify(key.defaultPrefix)};`);
    }
  }

  return `${lines.join('\n')}\n`;
}

function zigRedisType(type) {
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
    case 'array': return `[]const ${zigRedisType(type.items)}`;
    case 'ref': return type.name;
    case 'union': return '[]const u8';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderZigRedisBuild() {
  return `${[
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs. Do not edit by hand.',
    'const std = @import("std");',
    '',
    'pub fn build(b: *std.Build) void {',
    '    const target = b.standardTargetOptions(.{});',
    '    const optimize = b.standardOptimizeOption(.{});',
    '    _ = b.addModule("dd_redis_interfaces", .{',
    '        .root_source_file = b.path("dd_redis_interfaces.zig"),',
    '        .target = target,',
    '        .optimize = optimize,',
    '    });',
    '}',
  ].join('\n')}\n`;
}

// ---------- Elixir ----------

function renderElixirRedis(model) {
  const lines = [
    '# AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '# Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'defmodule DdRedisInterfaces do',
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

  for (const key of model.keys) {
    lines.push('');
    const segments = parsePattern(key.pattern);
    const used = new Set(segments.filter((s) => s.kind === 'param').map((s) => s.name));
    const params = key.params.map((p) => (used.has(p.name) ? camelToSnake(p.name) : `_${camelToSnake(p.name)}`));
    const expr = segments
      .map((seg) => (seg.kind === 'literal' ? JSON.stringify(seg.text) : camelToSnake(seg.name)))
      .join(' <> ');
    lines.push(`  def ${camelToSnake(key.name)}(${params.join(', ')}), do: ${expr}`);
    if (key.defaultPrefix) {
      lines.push(`  def ${camelToSnake(key.name)}_default_prefix, do: ${JSON.stringify(key.defaultPrefix)}`);
    }
  }

  lines.push('end');
  return `${lines.join('\n')}\n`;
}

function renderElixirRedisMix() {
  return `${[
    '# AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs. Do not edit by hand.',
    'defmodule DdRedisInterfaces.MixProject do',
    '  use Mix.Project',
    '',
    '  def project do',
    '    [app: :dd_redis_interfaces, version: "0.1.0", elixir: "~> 1.14"]',
    '  end',
    'end',
  ].join('\n')}\n`;
}

function toCamel(name) {
  return name.replace(/_([a-z0-9])/g, (_match, ch) => ch.toUpperCase());
}

function capitalize(name) {
  return name ? name.charAt(0).toUpperCase() + name.slice(1) : name;
}

// ---------- Go ----------

function renderGoRedis(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'package ddredisinterfaces',
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
      lines.push(`\t${pascalField(field.name)} ${goRedisFieldType(field)} \`json:"${tag}"\``);
    }
    lines.push('}');
  }
  for (const key of model.keys) {
    lines.push('');
    const params = key.params.map((p) => `${toCamel(p.name)} string`).join(', ');
    const expr = parsePattern(key.pattern)
      .map((seg) => (seg.kind === 'literal' ? JSON.stringify(seg.text) : toCamel(seg.name)))
      .join(' + ');
    lines.push(`func ${pascalField(toCamel(key.name))}(${params}) string {`);
    lines.push(`\treturn ${expr}`);
    lines.push('}');
    if (key.defaultPrefix) {
      lines.push(`const ${pascalField(toCamel(key.name))}DefaultPrefix = ${JSON.stringify(key.defaultPrefix)}`);
    }
  }
  return `${lines.join('\n')}\n`;
}

function goRedisBaseType(type) {
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
    case 'array': return `[]${goRedisBaseType(type.items)}`;
    case 'ref': return type.name;
    case 'union': return 'any';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function goRedisFieldType(field) {
  const base = goRedisBaseType(field.type);
  const optional = !field.required || field.nullable;
  if (optional && field.type.tag !== 'array') {
    return `*${base}`;
  }
  return base;
}

function renderGoRedisMod() {
  return `${['module dd/redis/interfaces', '', 'go 1.21'].join('\n')}\n`;
}

// ---------- Java ----------

function renderJavaRedis(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    'package dd.redis;',
    '',
    'import java.util.List;',
    '',
    'public final class DdRedisInterfaces {',
    '    private DdRedisInterfaces() {}',
  ];
  for (const named of model.named.filter((n) => n.kind === 'enum')) {
    lines.push('');
    lines.push(`    public enum ${named.name} { ${named.values.map((v) => gleamEnumVariant(v)).join(', ')} }`);
  }
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    lines.push('');
    const components = named.fields.map((f) => `${javaRedisComponentType(f)} ${toCamel(f.name)}`).join(', ');
    lines.push(`    public record ${named.name}(${components}) {}`);
  }
  for (const key of model.keys) {
    lines.push('');
    const params = key.params.map((p) => `String ${toCamel(p.name)}`).join(', ');
    const expr = parsePattern(key.pattern)
      .map((seg) => (seg.kind === 'literal' ? JSON.stringify(seg.text) : toCamel(seg.name)))
      .join(' + ');
    lines.push(`    public static String ${toCamel(key.name)}(${params}) {`);
    lines.push(`        return ${expr};`);
    lines.push('    }');
    if (key.defaultPrefix) {
      lines.push(`    public static final String ${upperSnake(key.name)}_DEFAULT_PREFIX = ${JSON.stringify(key.defaultPrefix)};`);
    }
  }
  lines.push('}');
  return `${lines.join('\n')}\n`;
}

function javaRedisComponentType(field) {
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
    case 'array': return `List<${javaRedisBoxedType(field.type.items)}>`;
    case 'ref': return field.type.name;
    case 'union': return 'Object';
    default: throw new Error(`unhandled type tag ${field.type.tag}`);
  }
}

function javaRedisBoxedType(type) {
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
    case 'array': return `List<${javaRedisBoxedType(type.items)}>`;
    case 'ref': return type.name;
    case 'union': return 'Object';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderJavaRedisPom() {
  return `${[
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<project xmlns="http://maven.apache.org/POM/4.0.0">',
    '    <modelVersion>4.0.0</modelVersion>',
    '    <groupId>dev.dd.redis</groupId>',
    '    <artifactId>dd-redis-interfaces</artifactId>',
    '    <version>0.1.0</version>',
    '    <packaging>jar</packaging>',
    '    <description>Generated Java redis interfaces - do not edit by hand.</description>',
    '    <properties>',
    '        <maven.compiler.source>17</maven.compiler.source>',
    '        <maven.compiler.target>17</maven.compiler.target>',
    '        <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>',
    '    </properties>',
    '</project>',
  ].join('\n')}\n`;
}

// ---------- Dart ----------

function renderDartRedis(model) {
  // Enum value types are string-valued, so a ref to one decodes as a plain String; object refs go
  // through the generated class's fromJson.
  const enumNames = new Set(model.named.filter((n) => n.kind === 'enum').map((n) => n.name));
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
  ];
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    lines.push('');
    lines.push(`class ${named.name} {`);
    for (const field of named.fields) {
      lines.push(`  final ${dartRedisType(field, enumNames)} ${toCamel(field.name)};`);
    }
    const ctorParams = named.fields
      .map((f) => (!f.required || f.nullable ? `this.${toCamel(f.name)}` : `required this.${toCamel(f.name)}`))
      .join(', ');
    lines.push(`  const ${named.name}({${ctorParams}});`);
    lines.push(`  factory ${named.name}.fromJson(Map<String, dynamic> json) => ${named.name}(`);
    for (const field of named.fields) {
      lines.push(`        ${toCamel(field.name)}: ${dartRedisFromJson(field.type, `json['${field.name}']`, !field.required || field.nullable, enumNames)},`);
    }
    lines.push('      );');
    lines.push('}');
  }
  for (const key of model.keys) {
    lines.push('');
    const params = key.params.map((p) => `String ${toCamel(p.name)}`).join(', ');
    const expr = parsePattern(key.pattern)
      .map((seg) => (seg.kind === 'literal' ? `'${seg.text}'` : toCamel(seg.name)))
      .join(' + ');
    lines.push(`String ${toCamel(key.name)}(${params}) => ${expr};`);
    if (key.defaultPrefix) {
      lines.push(`const ${toCamel(key.name)}DefaultPrefix = ${JSON.stringify(key.defaultPrefix)};`);
    }
  }
  return `${lines.join('\n')}\n`;
}

function dartRedisBaseType(type, enumNames) {
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
    case 'array': return `List<${dartRedisBaseType(type.items, enumNames)}>`;
    case 'ref': return enumNames.has(type.name) ? 'String' : type.name;
    case 'union': return 'dynamic';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function dartRedisType(field, enumNames) {
  const base = dartRedisBaseType(field.type, enumNames);
  // `dynamic` is already nullable, so an extra `?` is a lint warning.
  if ((!field.required || field.nullable) && base !== 'dynamic') {
    return `${base}?`;
  }
  return base;
}

function dartRedisFromJson(type, access, optional, enumNames) {
  const decode = (innerType, innerAccess) => {
    if (innerType.tag === 'ref' && !enumNames.has(innerType.name)) {
      return `${innerType.name}.fromJson(${innerAccess} as Map<String, dynamic>)`;
    }
    if (innerType.tag === 'array') {
      return `(${innerAccess} as List).map((e) => ${decode(innerType.items, 'e')}).toList()`;
    }
    return `${innerAccess} as ${dartRedisBaseType(innerType, enumNames)}`;
  };
  if (optional) {
    return `${access} == null ? null : ${decode(type, access)}`;
  }
  return decode(type, access);
}

function renderDartRedisPubspec() {
  return `${[
    'name: dd_redis_interfaces',
    'description: Generated Dart types and redis key formatters. Do not edit by hand.',
    'version: 0.1.0',
    'publish_to: none',
    '',
    'environment:',
    "  sdk: '>=3.0.0 <4.0.0'",
  ].join('\n')}\n`;
}

// ---------- Erlang ----------

function renderErlangRedisHrl(model) {
  const lines = [
    '%% AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '%% Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
  ];
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    const fields = named.fields.map((f) => camelToSnake(f.name)).join(', ');
    lines.push(`-record(${camelToSnake(named.name)}, {${fields}}).`);
  }
  return `${lines.join('\n')}\n`;
}

function renderErlangRedis(model) {
  const fns = [];
  const exports = [];
  for (const key of model.keys) {
    const fnName = camelToSnake(key.name);
    exports.push(`${fnName}/${key.params.length}`);
    const params = key.params.map((p) => capitalize(toCamel(p.name))).join(', ');
    const expr = parsePattern(key.pattern)
      .map((seg) => (seg.kind === 'literal' ? JSON.stringify(seg.text) : `${capitalize(toCamel(seg.name))}/binary`))
      .join(', ');
    fns.push(`${fnName}(${params}) ->`);
    fns.push(`    <<${expr}>>.`);
    fns.push('');
    if (key.defaultPrefix) {
      exports.push(`${fnName}_default_prefix/0`);
      fns.push(`${fnName}_default_prefix() -> ${JSON.stringify(key.defaultPrefix)}.`);
      fns.push('');
    }
  }
  for (const named of model.named.filter((n) => n.kind === 'enum')) {
    exports.push(`${camelToSnake(named.name)}_values/0`);
    fns.push(`${camelToSnake(named.name)}_values() -> [${named.values.map((v) => JSON.stringify(v)).join(', ')}].`);
    fns.push('');
  }
  return `${[
    '%% AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '%% Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
    '-module(dd_redis_interfaces).',
    `-export([${exports.join(', ')}]).`,
    '',
    ...fns,
  ].join('\n').trimEnd()}\n`;
}

function renderErlangRedisRebar() {
  return `${['{erl_opts, [debug_info]}.', '{deps, []}.'].join('\n')}\n`;
}

function renderErlangRedisApp() {
  return `${[
    '{application, dd_redis_interfaces,',
    ' [{description, "Generated Erlang redis interfaces"},',
    '  {vsn, "0.1.0"},',
    '  {registered, []},',
    '  {applications, [kernel, stdlib]},',
    '  {env, []},',
    '  {modules, [dd_redis_interfaces]}]}.',
  ].join('\n')}\n`;
}

// ---------- JavaScript ----------

function renderJavaScriptRedis(model) {
  const lines = [
    '// AUTOGENERATED by remote/libs/interfaces/redis/src/generate.mjs',
    '// Do not edit by hand; edit the JSON Schema under schema/ and regenerate.',
  ];
  for (const named of model.named.filter((n) => n.kind === 'object')) {
    lines.push('');
    lines.push('/**');
    lines.push(` * @typedef {object} ${named.name}`);
    for (const field of named.fields) {
      const optional = !field.required || field.nullable;
      const name = optional ? `[${toCamel(field.name)}]` : toCamel(field.name);
      lines.push(` * @property {${jsRedisType(field.type)}} ${name}`);
    }
    lines.push(' */');
  }
  if (model.keys.length > 0) {
    lines.push('');
    for (const key of model.keys) {
      const params = key.params.map((p) => toCamel(p.name)).join(', ');
      const expr = parsePattern(key.pattern)
        .map((seg) => (seg.kind === 'literal' ? seg.text.replace(/`/g, '\\`').replace(/\$\{/g, '\\${') : `\${${toCamel(seg.name)}}`))
        .join('');
      lines.push(`export function ${toCamel(key.name)}(${params}) {`);
      lines.push(`  return \`${expr}\`;`);
      lines.push('}');
      if (key.defaultPrefix) {
        lines.push(`export const ${upperSnake(key.name)}_DEFAULT_PREFIX = ${JSON.stringify(key.defaultPrefix)};`);
      }
    }
  }
  return `${lines.join('\n')}\n`;
}

function jsRedisType(type) {
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
    case 'array': return `${jsRedisType(type.items)}[]`;
    case 'ref': return type.name;
    case 'union': return '*';
    default: throw new Error(`unhandled type tag ${type.tag}`);
  }
}

function renderJavaScriptRedisPackageJson() {
  return `${JSON.stringify(
    {
      name: '@dd/redis-interfaces-js',
      version: '0.1.0',
      private: true,
      type: 'module',
      description: 'AUTOGENERATED pure-ESM JavaScript redis interfaces. Do not edit by hand.',
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

function lowerCamel(name) {
  if (!name) return name;
  return name.charAt(0).toLowerCase() + name.slice(1);
}

function upperSnake(name) {
  return camelToSnake(name).toUpperCase();
}

function splitDoc(text) {
  return text.split(/\n+/).map((line) => line.trim()).filter((line) => line.length > 0);
}

function parsePattern(pattern) {
  const segments = [];
  const regex = /\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g;
  let cursor = 0;
  let match;
  while ((match = regex.exec(pattern)) !== null) {
    if (match.index > cursor) {
      segments.push({ kind: 'literal', text: pattern.slice(cursor, match.index) });
    }
    segments.push({ kind: 'param', name: match[1] });
    cursor = match.index + match[0].length;
  }
  if (cursor < pattern.length) {
    segments.push({ kind: 'literal', text: pattern.slice(cursor) });
  }
  return segments;
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
