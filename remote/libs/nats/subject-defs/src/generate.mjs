// JSON Schema -> per-language NATS subject constants + formatters + parsers.
//
// Source of truth: every *.schema.json file listed in schema/index.json.
// Each schema document MAY declare value types under `$defs` (same conventions
// as @dd/shared-interfaces) AND subject conventions under the `$dd:nats`
// extension block. Cross-schema refs are not supported in this first pass.
//
// `$dd:nats` shape:
//   {
//     "service": "dd-remote-rest-api",
//     "summary": "...",
//     "subjects": [
//       {
//         "name": "ThreadTasks",
//         "description": "...",
//         "kind": "parameterized" | "static",
//         // static:
//         "subject": "dd.remote.events",
//         // parameterized:
//         "pattern": "dd.remote.thread.{thread_id}.tasks",
//         "wildcard": "dd.remote.thread.*.tasks",
//         "params": [
//           { "name": "thread_id", "type": "string", "description": "..." }
//         ],
//         "direction": "publish" | "subscribe" | "both",
//         "queueGroup": "dd-remote-thread-preparer",
//         "stream": "DD_REMOTE_TASKS"
//       }
//     ],
//     "queueGroups": [
//       { "name": "ThreadPreparerQueueGroup", "value": "dd-remote-thread-preparer", "description": "..." }
//     ],
//     "streams": [
//       {
//         "name": "DD_REMOTE_TASKS",
//         "description": "...",
//         "subjects": ["dd.remote.thread.*.tasks"],
//         "retention": "limits",
//         "storage": "file",
//         "ack": "explicit"
//       }
//     ]
//   }
//
// Outputs (kept idiomatic per-language, free of external runtime deps where
// possible; the Java target additionally produces a Maven build file):
//   generated/typescript/index.ts
//   generated/rust/Cargo.toml + src/lib.rs
//   generated/python/dd_nats_subject_defs.py + __init__.py
//   generated/gleam/gleam.toml + src/dd_nats_subject_defs.gleam
//   generated/erlang/rebar.config + src/dd_nats_subject_defs.{erl,app.src}
//   generated/dart/pubspec.yaml + lib/dd_nats_subject_defs.dart
//   generated/go/go.mod + ddnats.go
//   generated/jvm/pom.xml + src/main/java/dd/nats/DdNatsSubjects.java
//
// Run `pnpm --filter @dd/nats-subject-defs generate` to write files;
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
        `nats-subject-defs generated outputs are stale:\n${stale.map((file) => `  - ${file}`).join('\n')}`,
      );
      process.exitCode = 1;
      return;
    }

    console.log('nats-subject-defs generated outputs are up to date.');
    return;
  }

  for (const [relativePath, contents] of outputs) {
    const absolutePath = path.join(packageRoot, relativePath);
    await mkdir(path.dirname(absolutePath), { recursive: true });
    await writeFile(absolutePath, contents);
  }
  console.log(`Generated ${outputs.size} nats-subject-defs files.`);
}

// ---------- Model ----------

const DIRECTIONS = new Set(['publish', 'subscribe', 'both']);

/**
 * @typedef {{ name: string, description?: string }} ParamDef
 * @typedef {{
 *   name: string,
 *   description?: string,
 *   service?: string,
 *   kind: 'static',
 *   subject: string,
 *   direction: string,
 *   queueGroup?: string,
 *   stream?: string,
 * }} StaticSubject
 *
 * @typedef {{
 *   name: string,
 *   description?: string,
 *   service?: string,
 *   kind: 'parameterized',
 *   pattern: string,
 *   wildcard: string,
 *   params: ParamDef[],
 *   tailParam?: string,
 *   direction: string,
 *   queueGroup?: string,
 *   stream?: string,
 * }} ParameterizedSubject
 *
 * @typedef {StaticSubject | ParameterizedSubject} Subject
 *
 * @typedef {{
 *   name: string,
 *   value: string,
 *   description?: string,
 *   service?: string,
 * }} QueueGroup
 *
 * @typedef {{
 *   name: string,
 *   description?: string,
 *   service?: string,
 *   subjects: string[],
 *   retention?: string,
 *   storage?: string,
 *   ack?: string,
 * }} Stream
 */

function buildModel(schemaFiles) {
  /** @type {Subject[]} */ const subjects = [];
  /** @type {QueueGroup[]} */ const queueGroups = [];
  /** @type {Stream[]} */ const streams = [];
  const seenSubjects = new Set();
  const seenQueueGroupNames = new Set();
  const seenStreams = new Set();

  for (const { filename, doc } of schemaFiles) {
    const ext = doc['$dd:nats'];
    if (!ext) continue;
    const service = ext.service;

    for (const subj of ext.subjects ?? []) {
      if (!subj.name) throw new Error(`${filename}: subject is missing 'name'`);
      if (seenSubjects.has(subj.name)) {
        throw new Error(`Duplicate subject name across schemas: ${subj.name}`);
      }
      seenSubjects.add(subj.name);
      const direction = subj.direction ?? 'both';
      if (!DIRECTIONS.has(direction)) {
        throw new Error(`${subj.name}: invalid direction ${direction}`);
      }

      if (subj.kind === 'static') {
        if (!subj.subject || typeof subj.subject !== 'string') {
          throw new Error(`${subj.name}: static subject requires 'subject' string`);
        }
        if (/[{}*>]/.test(subj.subject)) {
          throw new Error(`${subj.name}: static subject must not contain wildcards or placeholders`);
        }
        subjects.push({
          name: subj.name,
          description: subj.description,
          service,
          kind: 'static',
          subject: subj.subject,
          direction,
          queueGroup: subj.queueGroup,
          stream: subj.stream,
        });
      } else if (subj.kind === 'parameterized') {
        if (!subj.pattern) throw new Error(`${subj.name}: parameterized subject requires 'pattern'`);
        if (!subj.wildcard) throw new Error(`${subj.name}: parameterized subject requires 'wildcard'`);
        if (!Array.isArray(subj.params) || subj.params.length === 0) {
          throw new Error(`${subj.name}: parameterized subject requires non-empty 'params'`);
        }
        const placeholders = [
          ...subj.pattern.matchAll(/\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g),
        ].map((m) => m[1]);
        const paramNames = subj.params.map((p) => {
          if (!p.name) throw new Error(`${subj.name}: param missing name`);
          if (p.type && p.type !== 'string') {
            throw new Error(`${subj.name}.${p.name}: only string params supported in v1`);
          }
          return p.name;
        });
        for (const ph of placeholders) {
          if (!paramNames.includes(ph)) {
            throw new Error(`${subj.name}: pattern references {${ph}} not declared in params`);
          }
        }
        for (const p of paramNames) {
          if (!placeholders.includes(p)) {
            throw new Error(`${subj.name}: param '${p}' declared but not used in pattern`);
          }
        }
        subjects.push({
          name: subj.name,
          description: subj.description,
          service,
          kind: 'parameterized',
          pattern: subj.pattern,
          wildcard: subj.wildcard,
          params: subj.params.map((p) => ({ name: p.name, description: p.description })),
          direction,
          queueGroup: subj.queueGroup,
          stream: subj.stream,
        });
      } else {
        throw new Error(`${subj.name}: unsupported subject kind ${subj.kind}`);
      }
    }

    for (const qg of ext.queueGroups ?? []) {
      if (!qg.name || !qg.value) {
        throw new Error(`${filename}: queueGroup needs 'name' and 'value'`);
      }
      if (seenQueueGroupNames.has(qg.name)) {
        throw new Error(`Duplicate queueGroup name across schemas: ${qg.name}`);
      }
      seenQueueGroupNames.add(qg.name);
      queueGroups.push({
        name: qg.name,
        value: qg.value,
        description: qg.description,
        service,
      });
    }

    for (const st of ext.streams ?? []) {
      if (!st.name) throw new Error(`${filename}: stream needs 'name'`);
      if (seenStreams.has(st.name)) {
        throw new Error(`Duplicate stream name across schemas: ${st.name}`);
      }
      seenStreams.add(st.name);
      if (!Array.isArray(st.subjects) || st.subjects.length === 0) {
        throw new Error(`${st.name}: stream needs non-empty 'subjects'`);
      }
      streams.push({
        name: st.name,
        description: st.description,
        service,
        subjects: st.subjects,
        retention: st.retention,
        storage: st.storage,
        ack: st.ack,
      });
    }
  }

  subjects.sort((a, b) => a.name.localeCompare(b.name));
  queueGroups.sort((a, b) => a.name.localeCompare(b.name));
  streams.sort((a, b) => a.name.localeCompare(b.name));

  return { subjects, queueGroups, streams };
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

  add('generated/python/dd_nats_subject_defs.py', renderPython(model));
  add(
    'generated/python/__init__.py',
    'from .dd_nats_subject_defs import *  # noqa: F401,F403\n',
  );

  add('generated/gleam/gleam.toml', renderGleamToml());
  add('generated/gleam/src/dd_nats_subject_defs.gleam', renderGleam(model));

  add('generated/erlang/rebar.config', renderErlangRebar());
  add('generated/erlang/src/dd_nats_subject_defs.app.src', renderErlangApp());
  add('generated/erlang/src/dd_nats_subject_defs.erl', renderErlang(model));

  add('generated/dart/pubspec.yaml', renderDartPubspec());
  add('generated/dart/lib/dd_nats_subject_defs.dart', renderDart(model));

  add('generated/go/go.mod', renderGoMod());
  add('generated/go/ddnats.go', renderGo(model));

  add('generated/jvm/pom.xml', renderJavaPom());
  add(
    'generated/jvm/src/main/java/dd/nats/DdNatsSubjects.java',
    renderJava(model),
  );

  return outputs;
}

// ============================================================
// TypeScript
// ============================================================

function renderTypeScript(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs');
  lines.push('// Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('// Source schemas: remote/libs/nats/subject-defs/schema/*.schema.json');
  lines.push('');
  lines.push('// ---------- Static subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockTs(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    const constName = `${upperSnake(subj.name)}_SUBJECT`;
    lines.push(`export const ${constName} = ${JSON.stringify(subj.subject)};`);
    if (subj.queueGroup) {
      const qgConst = `${upperSnake(subj.name)}_QUEUE_GROUP`;
      lines.push(`export const ${qgConst} = ${JSON.stringify(subj.queueGroup)};`);
    }
    if (subj.stream) {
      const streamConst = `${upperSnake(subj.name)}_STREAM`;
      lines.push(`export const ${streamConst} = ${JSON.stringify(subj.stream)};`);
    }
    lines.push('');
  }

  lines.push('// ---------- Parameterized subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockTs(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    const fnName = lowerCamel(subj.name) + 'Subject';
    const parseFnName = 'parse' + pascal(subj.name) + 'Subject';
    const wildcardConst = `${upperSnake(subj.name)}_WILDCARD`;
    const patternConst = `${upperSnake(subj.name)}_PATTERN`;
    lines.push(`export const ${patternConst} = ${JSON.stringify(subj.pattern)};`);
    lines.push(`export const ${wildcardConst} = ${JSON.stringify(subj.wildcard)};`);
    if (subj.queueGroup) {
      const qgConst = `${upperSnake(subj.name)}_QUEUE_GROUP`;
      lines.push(`export const ${qgConst} = ${JSON.stringify(subj.queueGroup)};`);
    }
    if (subj.stream) {
      const streamConst = `${upperSnake(subj.name)}_STREAM`;
      lines.push(`export const ${streamConst} = ${JSON.stringify(subj.stream)};`);
    }
    const params = subj.params.map((p) => `${camelToLowerCamel(p.name)}: string`).join(', ');
    const tmpl = subj.pattern.replace(
      /\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g,
      (_m, n) => '${' + camelToLowerCamel(n) + '}',
    );
    lines.push(`export function ${fnName}(${params}): string {`);
    lines.push(`  return \`${tmpl}\`;`);
    lines.push('}');

    // Parser: return type
    const typeName = pascal(subj.name) + 'SubjectParts';
    lines.push(`export type ${typeName} = {`);
    for (const p of subj.params) {
      lines.push(`  ${camelToLowerCamel(p.name)}: string;`);
    }
    lines.push('};');
    lines.push(
      `export function ${parseFnName}(subject: string): ${typeName} | null {`,
    );
    lines.push(`  const patternTokens = ${JSON.stringify(subj.pattern.split('.'))};`);
    lines.push('  const subjectTokens = subject.split(".");');
    lines.push('  const result: Record<string, string> = {};');
    lines.push('  let si = 0;');
    lines.push('  for (let pi = 0; pi < patternTokens.length; pi += 1) {');
    lines.push('    const tok = patternTokens[pi];');
    lines.push('    const phMatch = /^\\{([a-zA-Z_][a-zA-Z0-9_]*)\\}$/.exec(tok);');
    lines.push('    if (phMatch) {');
    lines.push('      if (si >= subjectTokens.length) return null;');
    lines.push('      result[phMatch[1]] = subjectTokens[si];');
    lines.push('      si += 1;');
    lines.push('      continue;');
    lines.push('    }');
    lines.push('    if (si >= subjectTokens.length || subjectTokens[si] !== tok) return null;');
    lines.push('    si += 1;');
    lines.push('  }');
    lines.push('  if (si !== subjectTokens.length) return null;');
    lines.push(`  return result as ${typeName};`);
    lines.push('}');
    lines.push('');
  }

  if (model.queueGroups.length > 0) {
    lines.push('// ---------- Standalone queue groups ----------');
    lines.push('');
    for (const qg of model.queueGroups) {
      emitDocBlockTs(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
      lines.push(`export const ${upperSnake(qg.name)} = ${JSON.stringify(qg.value)};`);
      lines.push('');
    }
  }

  if (model.streams.length > 0) {
    lines.push('// ---------- JetStream streams ----------');
    lines.push('');
    for (const st of model.streams) {
      emitDocBlockTs(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
      lines.push(`export const ${upperSnake(st.name)}_STREAM_NAME = ${JSON.stringify(st.name)};`);
      lines.push(
        `export const ${upperSnake(st.name)}_STREAM_SUBJECTS: readonly string[] = ${JSON.stringify(st.subjects)};`,
      );
      if (st.retention) {
        lines.push(
          `export const ${upperSnake(st.name)}_STREAM_RETENTION = ${JSON.stringify(st.retention)};`,
        );
      }
      if (st.storage) {
        lines.push(
          `export const ${upperSnake(st.name)}_STREAM_STORAGE = ${JSON.stringify(st.storage)};`,
        );
      }
      if (st.ack) {
        lines.push(
          `export const ${upperSnake(st.name)}_STREAM_ACK = ${JSON.stringify(st.ack)};`,
        );
      }
      lines.push('');
    }
  }

  return lines.join('\n');
}

function emitDocBlockTs(lines, parts) {
  const filtered = parts.filter(Boolean);
  if (filtered.length === 0) return;
  lines.push('/**');
  for (const part of filtered) {
    for (const line of splitDoc(part)) {
      lines.push(` * ${line}`);
    }
  }
  lines.push(' */');
}

// ============================================================
// Rust
// ============================================================

function renderRustCargo() {
  return [
    '[package]',
    'name = "dd-nats-subject-defs"',
    'version = "0.1.0"',
    'edition = "2021"',
    'description = "Generated Rust constants, formatters and parsers for dd NATS subject conventions. Do not edit by hand."',
    '',
    '[lib]',
    'path = "src/lib.rs"',
    '',
    '[dependencies]',
    '',
  ].join('\n');
}

function renderRust(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs');
  lines.push('// Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('');
  lines.push('#![allow(clippy::needless_return)]');
  lines.push('');
  lines.push('// ---------- Static subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockRust(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    const constName = `${upperSnake(subj.name)}_SUBJECT`;
    lines.push(`pub const ${constName}: &str = ${JSON.stringify(subj.subject)};`);
    if (subj.queueGroup) {
      lines.push(
        `pub const ${upperSnake(subj.name)}_QUEUE_GROUP: &str = ${JSON.stringify(subj.queueGroup)};`,
      );
    }
    if (subj.stream) {
      lines.push(
        `pub const ${upperSnake(subj.name)}_STREAM: &str = ${JSON.stringify(subj.stream)};`,
      );
    }
    lines.push('');
  }

  lines.push('// ---------- Parameterized subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockRust(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(
      `pub const ${upperSnake(subj.name)}_PATTERN: &str = ${JSON.stringify(subj.pattern)};`,
    );
    lines.push(
      `pub const ${upperSnake(subj.name)}_WILDCARD: &str = ${JSON.stringify(subj.wildcard)};`,
    );
    if (subj.queueGroup) {
      lines.push(
        `pub const ${upperSnake(subj.name)}_QUEUE_GROUP: &str = ${JSON.stringify(subj.queueGroup)};`,
      );
    }
    if (subj.stream) {
      lines.push(
        `pub const ${upperSnake(subj.name)}_STREAM: &str = ${JSON.stringify(subj.stream)};`,
      );
    }

    const fnName = `${camelToSnake(subj.name)}_subject`;
    const parseFnName = `parse_${camelToSnake(subj.name)}_subject`;
    const params = subj.params.map((p) => `${camelToSnake(p.name)}: &str`).join(', ');
    const fmtPattern = subj.pattern.replace(/\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g, '{}');
    const args = subj.params.map((p) => camelToSnake(p.name)).join(', ');
    lines.push(`pub fn ${fnName}(${params}) -> String {`);
    lines.push(`    format!(${JSON.stringify(fmtPattern)}, ${args})`);
    lines.push('}');
    lines.push('');

    const structName = `${pascal(subj.name)}SubjectParts`;
    lines.push('#[derive(Debug, Clone, PartialEq, Eq)]');
    lines.push(`pub struct ${structName} {`);
    for (const p of subj.params) {
      lines.push(`    pub ${camelToSnake(p.name)}: String,`);
    }
    lines.push('}');
    lines.push('');

    const patternTokensLit = `&[${subj.pattern
      .split('.')
      .map((tok) => JSON.stringify(tok))
      .join(', ')}]`;
    lines.push(`pub fn ${parseFnName}(subject: &str) -> Option<${structName}> {`);
    lines.push(`    let pattern_tokens: &[&str] = ${patternTokensLit};`);
    lines.push('    let subject_tokens: Vec<&str> = subject.split(\'.\').collect();');
    for (const p of subj.params) {
      lines.push(`    let mut ${camelToSnake(p.name)}: Option<String> = None;`);
    }
    lines.push('    let mut si: usize = 0;');
    lines.push('    for tok in pattern_tokens.iter() {');
    lines.push('        if tok.starts_with(\'{\') && tok.ends_with(\'}\') {');
    lines.push('            if si >= subject_tokens.len() { return None; }');
    lines.push('            let name = &tok[1..tok.len()-1];');
    lines.push('            match name {');
    for (const p of subj.params) {
      lines.push(`                ${JSON.stringify(p.name)} => { ${camelToSnake(p.name)} = Some(subject_tokens[si].to_string()); }`);
    }
    lines.push('                _ => return None,');
    lines.push('            }');
    lines.push('            si += 1;');
    lines.push('            continue;');
    lines.push('        }');
    lines.push('        if si >= subject_tokens.len() || subject_tokens[si] != *tok { return None; }');
    lines.push('        si += 1;');
    lines.push('    }');
    lines.push('    if si != subject_tokens.len() { return None; }');
    lines.push(`    Some(${structName} {`);
    for (const p of subj.params) {
      lines.push(`        ${camelToSnake(p.name)}: ${camelToSnake(p.name)}?,`);
    }
    lines.push('    })');
    lines.push('}');
    lines.push('');
  }

  if (model.queueGroups.length > 0) {
    lines.push('// ---------- Standalone queue groups ----------');
    lines.push('');
    for (const qg of model.queueGroups) {
      emitDocBlockRust(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
      lines.push(`pub const ${upperSnake(qg.name)}: &str = ${JSON.stringify(qg.value)};`);
      lines.push('');
    }
  }

  if (model.streams.length > 0) {
    lines.push('// ---------- JetStream streams ----------');
    lines.push('');
    for (const st of model.streams) {
      emitDocBlockRust(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
      lines.push(
        `pub const ${upperSnake(st.name)}_STREAM_NAME: &str = ${JSON.stringify(st.name)};`,
      );
      const subjArr = `&[${st.subjects.map((s) => JSON.stringify(s)).join(', ')}]`;
      lines.push(
        `pub const ${upperSnake(st.name)}_STREAM_SUBJECTS: &[&str] = ${subjArr};`,
      );
      if (st.retention) {
        lines.push(
          `pub const ${upperSnake(st.name)}_STREAM_RETENTION: &str = ${JSON.stringify(st.retention)};`,
        );
      }
      if (st.storage) {
        lines.push(
          `pub const ${upperSnake(st.name)}_STREAM_STORAGE: &str = ${JSON.stringify(st.storage)};`,
        );
      }
      if (st.ack) {
        lines.push(
          `pub const ${upperSnake(st.name)}_STREAM_ACK: &str = ${JSON.stringify(st.ack)};`,
        );
      }
      lines.push('');
    }
  }

  return lines.join('\n');
}

function emitDocBlockRust(lines, parts) {
  for (const part of parts.filter(Boolean)) {
    for (const line of splitDoc(part)) lines.push(`/// ${line}`);
  }
}

// ============================================================
// Python
// ============================================================

function renderPython(model) {
  const lines = [];
  lines.push('"""AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs.');
  lines.push('');
  lines.push('Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('"""');
  lines.push('');
  lines.push('from __future__ import annotations');
  lines.push('');
  lines.push('from dataclasses import dataclass');
  lines.push('from typing import Optional');
  lines.push('');
  lines.push('# ---------- Static subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockPy(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`${upperSnake(subj.name)}_SUBJECT = ${JSON.stringify(subj.subject)}`);
    if (subj.queueGroup) {
      lines.push(`${upperSnake(subj.name)}_QUEUE_GROUP = ${JSON.stringify(subj.queueGroup)}`);
    }
    if (subj.stream) {
      lines.push(`${upperSnake(subj.name)}_STREAM = ${JSON.stringify(subj.stream)}`);
    }
    lines.push('');
  }

  lines.push('# ---------- Parameterized subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockPy(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`${upperSnake(subj.name)}_PATTERN = ${JSON.stringify(subj.pattern)}`);
    lines.push(`${upperSnake(subj.name)}_WILDCARD = ${JSON.stringify(subj.wildcard)}`);
    if (subj.queueGroup) {
      lines.push(`${upperSnake(subj.name)}_QUEUE_GROUP = ${JSON.stringify(subj.queueGroup)}`);
    }
    if (subj.stream) {
      lines.push(`${upperSnake(subj.name)}_STREAM = ${JSON.stringify(subj.stream)}`);
    }

    const className = `${pascal(subj.name)}SubjectParts`;
    lines.push('@dataclass(frozen=True)');
    lines.push(`class ${className}:`);
    for (const p of subj.params) {
      lines.push(`    ${camelToSnake(p.name)}: str`);
    }
    lines.push('');

    const fnName = `${camelToSnake(subj.name)}_subject`;
    const parseFnName = `parse_${camelToSnake(subj.name)}_subject`;
    const sig = subj.params.map((p) => `${camelToSnake(p.name)}: str`).join(', ');
    lines.push(`def ${fnName}(${sig}) -> str:`);
    if (subj.description) {
      lines.push(`    """${escapePyDoc(subj.description)}"""`);
    }
    const fmt = subj.pattern.replace(
      /\{([a-zA-Z_][a-zA-Z0-9_]*)\}/g,
      (_m, n) => `{${camelToSnake(n)}}`,
    );
    const fmtArgs = subj.params
      .map((p) => `${camelToSnake(p.name)}=${camelToSnake(p.name)}`)
      .join(', ');
    lines.push(`    return ${JSON.stringify(fmt)}.format(${fmtArgs})`);
    lines.push('');

    lines.push(`def ${parseFnName}(subject: str) -> Optional[${className}]:`);
    lines.push(`    """Parse a resolved ${subj.name} subject; returns None on mismatch."""`);
    lines.push(`    pattern_tokens = ${JSON.stringify(subj.pattern.split('.'))}`);
    lines.push('    subject_tokens = subject.split(".")');
    lines.push('    result: dict[str, str] = {}');
    lines.push('    si = 0');
    lines.push('    for tok in pattern_tokens:');
    lines.push('        if tok.startswith("{") and tok.endswith("}"):');
    lines.push('            if si >= len(subject_tokens):');
    lines.push('                return None');
    lines.push('            result[tok[1:-1]] = subject_tokens[si]');
    lines.push('            si += 1');
    lines.push('            continue');
    lines.push('        if si >= len(subject_tokens) or subject_tokens[si] != tok:');
    lines.push('            return None');
    lines.push('        si += 1');
    lines.push('    if si != len(subject_tokens):');
    lines.push('        return None');
    const ctorArgs = subj.params
      .map((p) => `${camelToSnake(p.name)}=result[${JSON.stringify(p.name)}]`)
      .join(', ');
    lines.push(`    return ${className}(${ctorArgs})`);
    lines.push('');
  }

  if (model.queueGroups.length > 0) {
    lines.push('# ---------- Standalone queue groups ----------');
    lines.push('');
    for (const qg of model.queueGroups) {
      emitDocBlockPy(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
      lines.push(`${upperSnake(qg.name)} = ${JSON.stringify(qg.value)}`);
      lines.push('');
    }
  }

  if (model.streams.length > 0) {
    lines.push('# ---------- JetStream streams ----------');
    lines.push('');
    for (const st of model.streams) {
      emitDocBlockPy(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
      lines.push(
        `${upperSnake(st.name)}_STREAM_NAME = ${JSON.stringify(st.name)}`,
      );
      lines.push(
        `${upperSnake(st.name)}_STREAM_SUBJECTS = (${st.subjects.map((s) => JSON.stringify(s)).join(', ')}${st.subjects.length === 1 ? ',' : ''})`,
      );
      if (st.retention) {
        lines.push(
          `${upperSnake(st.name)}_STREAM_RETENTION = ${JSON.stringify(st.retention)}`,
        );
      }
      if (st.storage) {
        lines.push(
          `${upperSnake(st.name)}_STREAM_STORAGE = ${JSON.stringify(st.storage)}`,
        );
      }
      if (st.ack) {
        lines.push(
          `${upperSnake(st.name)}_STREAM_ACK = ${JSON.stringify(st.ack)}`,
        );
      }
      lines.push('');
    }
  }

  return lines.join('\n');
}

function emitDocBlockPy(lines, parts) {
  for (const part of parts.filter(Boolean)) {
    for (const line of splitDoc(part)) lines.push(`# ${line}`);
  }
}

function escapePyDoc(text) {
  return text.replace(/\\/g, '\\\\').replace(/"/g, "'").replace(/\n+/g, ' ');
}

// ============================================================
// Gleam
// ============================================================

function renderGleamToml() {
  return [
    'name = "dd_nats_subject_defs"',
    'version = "0.1.0"',
    'description = "Generated Gleam constants, formatters and parsers for dd NATS subject conventions. Do not edit by hand."',
    'target = "erlang"',
    '',
    '[dependencies]',
    'gleam_stdlib = ">= 0.40.0 and < 2.0.0"',
    '',
  ].join('\n');
}

function renderGleam(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs');
  lines.push('// Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('');
  lines.push('import gleam/list');
  lines.push('import gleam/option.{type Option, None, Some}');
  lines.push('import gleam/string');
  lines.push('');

  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockGleam(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`pub const ${camelToSnake(subj.name)}_subject = ${JSON.stringify(subj.subject)}`);
    if (subj.queueGroup) {
      lines.push(
        `pub const ${camelToSnake(subj.name)}_queue_group = ${JSON.stringify(subj.queueGroup)}`,
      );
    }
    if (subj.stream) {
      lines.push(
        `pub const ${camelToSnake(subj.name)}_stream = ${JSON.stringify(subj.stream)}`,
      );
    }
    lines.push('');
  }

  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockGleam(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`pub const ${camelToSnake(subj.name)}_pattern = ${JSON.stringify(subj.pattern)}`);
    lines.push(`pub const ${camelToSnake(subj.name)}_wildcard = ${JSON.stringify(subj.wildcard)}`);
    if (subj.queueGroup) {
      lines.push(
        `pub const ${camelToSnake(subj.name)}_queue_group = ${JSON.stringify(subj.queueGroup)}`,
      );
    }
    if (subj.stream) {
      lines.push(
        `pub const ${camelToSnake(subj.name)}_stream = ${JSON.stringify(subj.stream)}`,
      );
    }

    const typeName = `${pascal(subj.name)}SubjectParts`;
    lines.push(`pub type ${typeName} {`);
    lines.push(`  ${typeName}(`);
    subj.params.forEach((p, i) => {
      const comma = i === subj.params.length - 1 ? '' : ',';
      lines.push(`    ${camelToSnake(p.name)}: String${comma}`);
    });
    lines.push('  )');
    lines.push('}');

    const fnName = `${camelToSnake(subj.name)}_subject`;
    const fnParams = subj.params
      .map((p) => {
        const n = camelToSnake(p.name);
        return `${n} ${n}: String`;
      })
      .join(', ');
    const segs = parsePattern(subj.pattern);
    const expr = segs
      .map((s) => (s.kind === 'literal' ? JSON.stringify(s.text) : camelToSnake(s.name)))
      .join(' <> ');
    lines.push(`pub fn ${fnName}(${fnParams}) -> String {`);
    lines.push(`  ${expr}`);
    lines.push('}');
    lines.push('');

    const parseFn = `parse_${camelToSnake(subj.name)}_subject`;
    lines.push(`pub fn ${parseFn}(subject: String) -> Option(${typeName}) {`);
    lines.push(
      `  let pattern_tokens = string.split(${JSON.stringify(subj.pattern)}, on: ".")`,
    );
    lines.push('  let subject_tokens = string.split(subject, on: ".")');
    lines.push('  case list.length(pattern_tokens) == list.length(subject_tokens) {');
    lines.push('    False -> None');
    lines.push('    True -> {');
    // Use index-based assignment via zip
    lines.push('      let pairs = list.zip(pattern_tokens, subject_tokens)');
    for (const p of subj.params) {
      lines.push(
        `      let ${camelToSnake(p.name)} = lookup_param(pairs, ${JSON.stringify('{' + p.name + '}')})`,
      );
    }
    // Verify literal segments match
    lines.push('      let literals_ok = list.all(pairs, fn(pair) {');
    lines.push('        let #(p, s) = pair');
    lines.push('        case string.starts_with(p, "{") && string.ends_with(p, "}") {');
    lines.push('          True -> True');
    lines.push('          False -> p == s');
    lines.push('        }');
    lines.push('      })');
    lines.push('      case literals_ok {');
    lines.push('        False -> None');
    const guards = subj.params
      .map((p) => `${camelToSnake(p.name)}_opt`)
      .join(', ');
    void guards;
    // Build the result by pattern-matching on each Option
    let chain = subj.params
      .map((p) => `        use ${camelToSnake(p.name)} <- with_some(${camelToSnake(p.name)})`)
      .join('\n');
    void chain;
    lines.push('        True -> {');
    // use nested unwraps
    for (const p of subj.params) {
      lines.push(`          use ${camelToSnake(p.name)} <- with_some(${camelToSnake(p.name)})`);
    }
    const ctorFields = subj.params
      .map((p) => `${camelToSnake(p.name)}: ${camelToSnake(p.name)}`)
      .join(', ');
    lines.push(`          Some(${typeName}(${ctorFields}))`);
    lines.push('        }');
    lines.push('      }');
    lines.push('    }');
    lines.push('  }');
    lines.push('}');
    lines.push('');
  }

  if (model.queueGroups.length > 0) {
    for (const qg of model.queueGroups) {
      emitDocBlockGleam(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
      lines.push(`pub const ${camelToSnake(qg.name)} = ${JSON.stringify(qg.value)}`);
      lines.push('');
    }
  }

  if (model.streams.length > 0) {
    for (const st of model.streams) {
      emitDocBlockGleam(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
      lines.push(
        `pub const ${camelToSnake(st.name)}_stream_name = ${JSON.stringify(st.name)}`,
      );
      lines.push(
        `pub fn ${camelToSnake(st.name)}_stream_subjects() -> List(String) {`,
      );
      lines.push(`  [${st.subjects.map((s) => JSON.stringify(s)).join(', ')}]`);
      lines.push('}');
      if (st.retention) {
        lines.push(
          `pub const ${camelToSnake(st.name)}_stream_retention = ${JSON.stringify(st.retention)}`,
        );
      }
      if (st.storage) {
        lines.push(
          `pub const ${camelToSnake(st.name)}_stream_storage = ${JSON.stringify(st.storage)}`,
        );
      }
      if (st.ack) {
        lines.push(
          `pub const ${camelToSnake(st.name)}_stream_ack = ${JSON.stringify(st.ack)}`,
        );
      }
      lines.push('');
    }
  }

  // Helpers
  lines.push('// ---------- helpers ----------');
  lines.push('');
  lines.push('fn lookup_param(pairs: List(#(String, String)), key: String) -> Option(String) {');
  lines.push('  case list.find(pairs, fn(pair) { pair.0 == key }) {');
  lines.push('    Ok(pair) -> Some(pair.1)');
  lines.push('    Error(_) -> None');
  lines.push('  }');
  lines.push('}');
  lines.push('');
  lines.push('fn with_some(opt: Option(a), fun: fn(a) -> Option(b)) -> Option(b) {');
  lines.push('  case opt {');
  lines.push('    Some(value) -> fun(value)');
  lines.push('    None -> None');
  lines.push('  }');
  lines.push('}');
  lines.push('');

  return lines.join('\n');
}

function emitDocBlockGleam(lines, parts) {
  for (const part of parts.filter(Boolean)) {
    for (const line of splitDoc(part)) lines.push(`/// ${line}`);
  }
}

// ============================================================
// Erlang
// ============================================================

function renderErlangRebar() {
  return ['{erl_opts, [debug_info]}.', '{deps, []}.', ''].join('\n');
}

function renderErlangApp() {
  return [
    '{application, dd_nats_subject_defs,',
    ' [{description, "Generated Erlang NATS subject defs"},',
    '  {vsn, "0.1.0"},',
    '  {registered, []},',
    '  {applications, [kernel, stdlib]},',
    '  {env, []},',
    '  {modules, [dd_nats_subject_defs]}]}.',
    '',
  ].join('\n');
}

function renderErlang(model) {
  const lines = [];
  lines.push('%% AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs');
  lines.push('%% Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('-module(dd_nats_subject_defs).');
  lines.push('');

  const exports = [];
  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    exports.push(`${camelToSnake(subj.name)}_subject/0`);
    if (subj.queueGroup) exports.push(`${camelToSnake(subj.name)}_queue_group/0`);
    if (subj.stream) exports.push(`${camelToSnake(subj.name)}_stream/0`);
  }
  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    exports.push(`${camelToSnake(subj.name)}_pattern/0`);
    exports.push(`${camelToSnake(subj.name)}_wildcard/0`);
    exports.push(`${camelToSnake(subj.name)}_subject/${subj.params.length}`);
    exports.push(`parse_${camelToSnake(subj.name)}_subject/1`);
    if (subj.queueGroup) exports.push(`${camelToSnake(subj.name)}_queue_group/0`);
    if (subj.stream) exports.push(`${camelToSnake(subj.name)}_stream/0`);
  }
  for (const qg of model.queueGroups) exports.push(`${camelToSnake(qg.name)}/0`);
  for (const st of model.streams) {
    exports.push(`${camelToSnake(st.name)}_stream_name/0`);
    exports.push(`${camelToSnake(st.name)}_stream_subjects/0`);
    if (st.retention) exports.push(`${camelToSnake(st.name)}_stream_retention/0`);
    if (st.storage) exports.push(`${camelToSnake(st.name)}_stream_storage/0`);
    if (st.ack) exports.push(`${camelToSnake(st.name)}_stream_ack/0`);
  }

  if (exports.length > 0) {
    lines.push('-export([');
    lines.push('    ' + exports.join(',\n    '));
    lines.push(']).');
    lines.push('');
  }

  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockErl(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`${camelToSnake(subj.name)}_subject() -> ${erlangBinaryLiteral(subj.subject)}.`);
    if (subj.queueGroup) {
      lines.push(
        `${camelToSnake(subj.name)}_queue_group() -> ${erlangBinaryLiteral(subj.queueGroup)}.`,
      );
    }
    if (subj.stream) {
      lines.push(
        `${camelToSnake(subj.name)}_stream() -> ${erlangBinaryLiteral(subj.stream)}.`,
      );
    }
    lines.push('');
  }

  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockErl(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(
      `${camelToSnake(subj.name)}_pattern() -> ${erlangBinaryLiteral(subj.pattern)}.`,
    );
    lines.push(
      `${camelToSnake(subj.name)}_wildcard() -> ${erlangBinaryLiteral(subj.wildcard)}.`,
    );
    if (subj.queueGroup) {
      lines.push(
        `${camelToSnake(subj.name)}_queue_group() -> ${erlangBinaryLiteral(subj.queueGroup)}.`,
      );
    }
    if (subj.stream) {
      lines.push(
        `${camelToSnake(subj.name)}_stream() -> ${erlangBinaryLiteral(subj.stream)}.`,
      );
    }

    const varNames = subj.params.map((p) => pascalSnakeToVar(p.name));
    const sigArgs = varNames.join(', ');
    const segs = parsePattern(subj.pattern);
    const exprParts = segs.map((s) => {
      if (s.kind === 'literal') return erlangBinaryLiteral(s.text);
      const v = pascalSnakeToVar(s.name);
      return `to_bin(${v})`;
    });
    lines.push(`${camelToSnake(subj.name)}_subject(${sigArgs}) ->`);
    lines.push(`    iolist_to_binary([${exprParts.join(', ')}]).`);
    lines.push('');

    const tokens = subj.pattern.split('.');
    lines.push(`parse_${camelToSnake(subj.name)}_subject(Subject) ->`);
    lines.push('    SubjectBin = to_bin(Subject),');
    lines.push('    Tokens = binary:split(SubjectBin, <<".">>, [global]),');
    lines.push(`    PatternTokens = [${tokens.map((t) => erlangBinaryLiteral(t)).join(', ')}],`);
    lines.push('    case length(Tokens) =:= length(PatternTokens) of');
    lines.push('        false -> error;');
    lines.push('        true ->');
    lines.push('            Pairs = lists:zip(PatternTokens, Tokens),');
    lines.push('            LiteralsOk = lists:all(fun({P, S}) ->');
    lines.push('                case is_placeholder(P) of');
    lines.push('                    true -> true;');
    lines.push('                    false -> P =:= S');
    lines.push('                end');
    lines.push('            end, Pairs),');
    lines.push('            case LiteralsOk of');
    lines.push('                false -> error;');
    lines.push('                true ->');
    const mapFields = subj.params
      .map(
        (p) =>
          `${camelToSnake(p.name)} => extract_param(Pairs, ${erlangBinaryLiteral('{' + p.name + '}')})`,
      )
      .join(', ');
    lines.push(`                    {ok, #{${mapFields}}}`);
    lines.push('            end');
    lines.push('    end.');
    lines.push('');
  }

  for (const qg of model.queueGroups) {
    emitDocBlockErl(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
    lines.push(`${camelToSnake(qg.name)}() -> ${erlangBinaryLiteral(qg.value)}.`);
    lines.push('');
  }

  for (const st of model.streams) {
    emitDocBlockErl(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
    lines.push(
      `${camelToSnake(st.name)}_stream_name() -> ${erlangBinaryLiteral(st.name)}.`,
    );
    lines.push(`${camelToSnake(st.name)}_stream_subjects() ->`);
    lines.push(
      `    [${st.subjects.map((s) => erlangBinaryLiteral(s)).join(', ')}].`,
    );
    if (st.retention) {
      lines.push(
        `${camelToSnake(st.name)}_stream_retention() -> ${erlangBinaryLiteral(st.retention)}.`,
      );
    }
    if (st.storage) {
      lines.push(
        `${camelToSnake(st.name)}_stream_storage() -> ${erlangBinaryLiteral(st.storage)}.`,
      );
    }
    if (st.ack) {
      lines.push(
        `${camelToSnake(st.name)}_stream_ack() -> ${erlangBinaryLiteral(st.ack)}.`,
      );
    }
    lines.push('');
  }

  // Helpers
  lines.push('%% ---------- helpers ----------');
  lines.push('');
  lines.push('to_bin(B) when is_binary(B) -> B;');
  lines.push('to_bin(L) when is_list(L) -> list_to_binary(L);');
  lines.push('to_bin(A) when is_atom(A) -> atom_to_binary(A, utf8).');
  lines.push('');
  lines.push('is_placeholder(<<"{", Rest/binary>>) ->');
  lines.push('    case binary:last(Rest) of');
  lines.push('        $} -> true;');
  lines.push('        _ -> false');
  lines.push('    end;');
  lines.push('is_placeholder(_) -> false.');
  lines.push('');
  lines.push('extract_param(Pairs, Key) ->');
  lines.push('    case lists:keyfind(Key, 1, Pairs) of');
  lines.push('        {_, V} -> V;');
  lines.push('        false -> <<>>');
  lines.push('    end.');
  lines.push('');

  return lines.join('\n');
}

function emitDocBlockErl(lines, parts) {
  for (const part of parts.filter(Boolean)) {
    for (const line of splitDoc(part)) lines.push(`%% ${line}`);
  }
}

function erlangBinaryLiteral(text) {
  return `<<${JSON.stringify(text)}/utf8>>`;
}

function pascalSnakeToVar(name) {
  return name
    .split(/[_\s-]+/)
    .map((part) => (part ? part.charAt(0).toUpperCase() + part.slice(1) : ''))
    .join('');
}

// ============================================================
// Dart
// ============================================================

function renderDartPubspec() {
  return [
    'name: dd_nats_subject_defs',
    'description: Generated Dart constants, formatters and parsers for dd NATS subject conventions. Do not edit by hand.',
    'version: 0.1.0',
    'publish_to: none',
    '',
    'environment:',
    "  sdk: '>=3.0.0 <4.0.0'",
    '',
  ].join('\n');
}

function renderDart(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs');
  lines.push('// Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('library dd_nats_subject_defs;');
  lines.push('');

  lines.push('// ---------- Static subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockDart(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`const String ${lowerCamel(subj.name)}Subject = ${JSON.stringify(subj.subject)};`);
    if (subj.queueGroup) {
      lines.push(
        `const String ${lowerCamel(subj.name)}QueueGroup = ${JSON.stringify(subj.queueGroup)};`,
      );
    }
    if (subj.stream) {
      lines.push(
        `const String ${lowerCamel(subj.name)}Stream = ${JSON.stringify(subj.stream)};`,
      );
    }
    lines.push('');
  }

  lines.push('// ---------- Parameterized subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockDart(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`const String ${lowerCamel(subj.name)}Pattern = ${JSON.stringify(subj.pattern)};`);
    lines.push(`const String ${lowerCamel(subj.name)}Wildcard = ${JSON.stringify(subj.wildcard)};`);
    if (subj.queueGroup) {
      lines.push(
        `const String ${lowerCamel(subj.name)}QueueGroup = ${JSON.stringify(subj.queueGroup)};`,
      );
    }
    if (subj.stream) {
      lines.push(
        `const String ${lowerCamel(subj.name)}Stream = ${JSON.stringify(subj.stream)};`,
      );
    }

    const className = `${pascal(subj.name)}SubjectParts`;
    lines.push(`class ${className} {`);
    for (const p of subj.params) {
      lines.push(`  final String ${camelToLowerCamel(p.name)};`);
    }
    const ctorArgs = subj.params.map((p) => `required this.${camelToLowerCamel(p.name)}`).join(', ');
    lines.push(`  const ${className}({${ctorArgs}});`);
    lines.push('}');
    lines.push('');

    const fnName = `${lowerCamel(subj.name)}Subject`;
    const params = subj.params.map((p) => `String ${camelToLowerCamel(p.name)}`).join(', ');
    const segs = parsePattern(subj.pattern);
    const expr = segs
      .map((s) => (s.kind === 'literal' ? JSON.stringify(s.text) : `$${camelToLowerCamel(s.name)}`))
      .map((s) => (s.startsWith('"') ? s.slice(1, -1) : s))
      .join('');
    lines.push(`String ${fnName}(${params}) {`);
    lines.push(`  return '${expr.replace(/'/g, "\\'")}';`);
    lines.push('}');
    lines.push('');

    const parseFn = `parse${pascal(subj.name)}Subject`;
    lines.push(`${className}? ${parseFn}(String subject) {`);
    lines.push(`  final patternTokens = ${JSON.stringify(subj.pattern.split('.'))};`);
    lines.push("  final subjectTokens = subject.split('.');");
    lines.push('  if (patternTokens.length != subjectTokens.length) return null;');
    lines.push('  final result = <String, String>{};');
    lines.push('  for (var i = 0; i < patternTokens.length; i += 1) {');
    lines.push('    final p = patternTokens[i];');
    lines.push('    final s = subjectTokens[i];');
    lines.push("    if (p.startsWith('{') && p.endsWith('}')) {");
    lines.push('      result[p.substring(1, p.length - 1)] = s;');
    lines.push('    } else if (p != s) {');
    lines.push('      return null;');
    lines.push('    }');
    lines.push('  }');
    const dartCtorArgs = subj.params
      .map(
        (p) =>
          `${camelToLowerCamel(p.name)}: result[${JSON.stringify(p.name)}]!`,
      )
      .join(', ');
    lines.push(`  return ${className}(${dartCtorArgs});`);
    lines.push('}');
    lines.push('');
  }

  if (model.queueGroups.length > 0) {
    lines.push('// ---------- Standalone queue groups ----------');
    lines.push('');
    for (const qg of model.queueGroups) {
      emitDocBlockDart(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
      lines.push(`const String ${lowerCamel(qg.name)} = ${JSON.stringify(qg.value)};`);
      lines.push('');
    }
  }

  if (model.streams.length > 0) {
    lines.push('// ---------- JetStream streams ----------');
    lines.push('');
    for (const st of model.streams) {
      emitDocBlockDart(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
      lines.push(`const String ${lowerCamel(st.name)}StreamName = ${JSON.stringify(st.name)};`);
      const subjArr = st.subjects.map((s) => JSON.stringify(s)).join(', ');
      lines.push(`const List<String> ${lowerCamel(st.name)}StreamSubjects = [${subjArr}];`);
      if (st.retention) {
        lines.push(
          `const String ${lowerCamel(st.name)}StreamRetention = ${JSON.stringify(st.retention)};`,
        );
      }
      if (st.storage) {
        lines.push(
          `const String ${lowerCamel(st.name)}StreamStorage = ${JSON.stringify(st.storage)};`,
        );
      }
      if (st.ack) {
        lines.push(
          `const String ${lowerCamel(st.name)}StreamAck = ${JSON.stringify(st.ack)};`,
        );
      }
      lines.push('');
    }
  }

  return lines.join('\n');
}

function emitDocBlockDart(lines, parts) {
  for (const part of parts.filter(Boolean)) {
    for (const line of splitDoc(part)) lines.push(`/// ${line}`);
  }
}

// ============================================================
// Go
// ============================================================

function renderGoMod() {
  return ['module dd/nats/subjectdefs', '', 'go 1.21', ''].join('\n');
}

function renderGo(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs');
  lines.push('// Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('');
  lines.push('package ddnats');
  lines.push('');
  lines.push('import (');
  lines.push('\t"fmt"');
  lines.push('\t"strings"');
  lines.push(')');
  lines.push('');
  lines.push('// silence imports when no parameterized subjects need them.');
  lines.push('var _ = fmt.Sprintf');
  lines.push('var _ = strings.Split');
  lines.push('');

  // Constants
  lines.push('// ---------- Static subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockGo(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`const ${pascal(subj.name)}Subject = ${JSON.stringify(subj.subject)}`);
    if (subj.queueGroup) {
      lines.push(`const ${pascal(subj.name)}QueueGroup = ${JSON.stringify(subj.queueGroup)}`);
    }
    if (subj.stream) {
      lines.push(`const ${pascal(subj.name)}Stream = ${JSON.stringify(subj.stream)}`);
    }
    lines.push('');
  }

  lines.push('// ---------- Parameterized subjects ----------');
  lines.push('');
  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockGo(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(`const ${pascal(subj.name)}Pattern = ${JSON.stringify(subj.pattern)}`);
    lines.push(`const ${pascal(subj.name)}Wildcard = ${JSON.stringify(subj.wildcard)}`);
    if (subj.queueGroup) {
      lines.push(`const ${pascal(subj.name)}QueueGroup = ${JSON.stringify(subj.queueGroup)}`);
    }
    if (subj.stream) {
      lines.push(`const ${pascal(subj.name)}Stream = ${JSON.stringify(subj.stream)}`);
    }

    const structName = `${pascal(subj.name)}SubjectParts`;
    lines.push(`type ${structName} struct {`);
    for (const p of subj.params) {
      lines.push(`\t${pascal(p.name)} string`);
    }
    lines.push('}');
    lines.push('');

    const fnName = `${pascal(subj.name)}Subject`;
    const fnParams = subj.params.map((p) => `${camelToLowerCamel(p.name)} string`).join(', ');
    const fmtPattern = subj.pattern.replace(/\{[a-zA-Z_][a-zA-Z0-9_]*\}/g, '%s');
    const fmtArgs = subj.params.map((p) => camelToLowerCamel(p.name)).join(', ');
    lines.push(`func ${fnName}(${fnParams}) string {`);
    lines.push(`\treturn fmt.Sprintf(${JSON.stringify(fmtPattern)}, ${fmtArgs})`);
    lines.push('}');
    lines.push('');

    const parseFn = `Parse${pascal(subj.name)}Subject`;
    const patternStrLit = JSON.stringify(subj.pattern);
    lines.push(`func ${parseFn}(subject string) (*${structName}, bool) {`);
    lines.push(`\tpatternTokens := strings.Split(${patternStrLit}, ".")`);
    lines.push('\tsubjectTokens := strings.Split(subject, ".")');
    lines.push('\tif len(patternTokens) != len(subjectTokens) {');
    lines.push('\t\treturn nil, false');
    lines.push('\t}');
    lines.push(`\tparts := &${structName}{}`);
    lines.push('\tfor i, p := range patternTokens {');
    lines.push('\t\ts := subjectTokens[i]');
    lines.push('\t\tif strings.HasPrefix(p, "{") && strings.HasSuffix(p, "}") {');
    lines.push('\t\t\tname := p[1 : len(p)-1]');
    lines.push('\t\t\tswitch name {');
    for (const p of subj.params) {
      lines.push(`\t\t\tcase ${JSON.stringify(p.name)}:`);
      lines.push(`\t\t\t\tparts.${pascal(p.name)} = s`);
    }
    lines.push('\t\t\tdefault:');
    lines.push('\t\t\t\treturn nil, false');
    lines.push('\t\t\t}');
    lines.push('\t\t\tcontinue');
    lines.push('\t\t}');
    lines.push('\t\tif p != s {');
    lines.push('\t\t\treturn nil, false');
    lines.push('\t\t}');
    lines.push('\t}');
    lines.push('\treturn parts, true');
    lines.push('}');
    lines.push('');
  }

  if (model.queueGroups.length > 0) {
    lines.push('// ---------- Standalone queue groups ----------');
    lines.push('');
    for (const qg of model.queueGroups) {
      emitDocBlockGo(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
      lines.push(`const ${pascal(qg.name)} = ${JSON.stringify(qg.value)}`);
      lines.push('');
    }
  }

  if (model.streams.length > 0) {
    lines.push('// ---------- JetStream streams ----------');
    lines.push('');
    for (const st of model.streams) {
      emitDocBlockGo(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
      lines.push(`const ${pascal(st.name)}StreamName = ${JSON.stringify(st.name)}`);
      const subjArr = st.subjects.map((s) => JSON.stringify(s)).join(', ');
      lines.push(`var ${pascal(st.name)}StreamSubjects = []string{${subjArr}}`);
      if (st.retention) {
        lines.push(`const ${pascal(st.name)}StreamRetention = ${JSON.stringify(st.retention)}`);
      }
      if (st.storage) {
        lines.push(`const ${pascal(st.name)}StreamStorage = ${JSON.stringify(st.storage)}`);
      }
      if (st.ack) {
        lines.push(`const ${pascal(st.name)}StreamAck = ${JSON.stringify(st.ack)}`);
      }
      lines.push('');
    }
  }

  return lines.join('\n');
}

function emitDocBlockGo(lines, parts) {
  for (const part of parts.filter(Boolean)) {
    for (const line of splitDoc(part)) lines.push(`// ${line}`);
  }
}

// ============================================================
// Java 17+
// ============================================================

function renderJavaPom() {
  return [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<project xmlns="http://maven.apache.org/POM/4.0.0">',
    '    <modelVersion>4.0.0</modelVersion>',
    '    <groupId>dev.dd.nats</groupId>',
    '    <artifactId>dd-nats-subject-defs</artifactId>',
    '    <version>0.1.0</version>',
    '    <packaging>jar</packaging>',
    '    <description>Generated Java NATS subject defs - do not edit by hand.</description>',
    '    <properties>',
    '        <maven.compiler.source>17</maven.compiler.source>',
    '        <maven.compiler.target>17</maven.compiler.target>',
    '        <project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>',
    '    </properties>',
    '</project>',
    '',
  ].join('\n');
}

function renderJava(model) {
  const lines = [];
  lines.push('// AUTOGENERATED by remote/libs/nats/subject-defs/src/generate.mjs');
  lines.push('// Do not edit by hand; edit JSON Schema under schema/ and regenerate.');
  lines.push('package dd.nats;');
  lines.push('');
  lines.push('import java.util.List;');
  lines.push('import java.util.Optional;');
  lines.push('');
  lines.push('/**');
  lines.push(' * Generated NATS subject constants, formatters, parsers, queue group names,');
  lines.push(' * and JetStream stream definitions. Source of truth lives under');
  lines.push(' * remote/libs/nats/subject-defs/schema/.');
  lines.push(' */');
  lines.push('public final class DdNatsSubjects {');
  lines.push('    private DdNatsSubjects() {}');
  lines.push('');

  // Static subjects
  for (const subj of model.subjects.filter((s) => s.kind === 'static')) {
    emitDocBlockJava(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(
      `    public static final String ${upperSnake(subj.name)}_SUBJECT = ${JSON.stringify(subj.subject)};`,
    );
    if (subj.queueGroup) {
      lines.push(
        `    public static final String ${upperSnake(subj.name)}_QUEUE_GROUP = ${JSON.stringify(subj.queueGroup)};`,
      );
    }
    if (subj.stream) {
      lines.push(
        `    public static final String ${upperSnake(subj.name)}_STREAM = ${JSON.stringify(subj.stream)};`,
      );
    }
    lines.push('');
  }

  // Parameterized subjects
  for (const subj of model.subjects.filter((s) => s.kind === 'parameterized')) {
    emitDocBlockJava(lines, [subj.description, subj.service ? `Service: ${subj.service}` : null]);
    lines.push(
      `    public static final String ${upperSnake(subj.name)}_PATTERN = ${JSON.stringify(subj.pattern)};`,
    );
    lines.push(
      `    public static final String ${upperSnake(subj.name)}_WILDCARD = ${JSON.stringify(subj.wildcard)};`,
    );
    if (subj.queueGroup) {
      lines.push(
        `    public static final String ${upperSnake(subj.name)}_QUEUE_GROUP = ${JSON.stringify(subj.queueGroup)};`,
      );
    }
    if (subj.stream) {
      lines.push(
        `    public static final String ${upperSnake(subj.name)}_STREAM = ${JSON.stringify(subj.stream)};`,
      );
    }

    // Record type for the parsed parts
    const recordName = `${pascal(subj.name)}SubjectParts`;
    const recordFields = subj.params.map((p) => `String ${camelToLowerCamel(p.name)}`).join(', ');
    lines.push(`    public record ${recordName}(${recordFields}) {}`);
    lines.push('');

    // Formatter
    const fnName = `${lowerCamel(subj.name)}Subject`;
    const fnParams = subj.params.map((p) => `String ${camelToLowerCamel(p.name)}`).join(', ');
    const segs = parsePattern(subj.pattern);
    const concatParts = segs.map((s) => {
      if (s.kind === 'literal') return JSON.stringify(s.text);
      return camelToLowerCamel(s.name);
    });
    lines.push(`    public static String ${fnName}(${fnParams}) {`);
    lines.push(`        return ${concatParts.join(' + ')};`);
    lines.push('    }');
    lines.push('');

    // Parser
    const parseFn = `parse${pascal(subj.name)}Subject`;
    const tokens = subj.pattern.split('.');
    lines.push(`    public static Optional<${recordName}> ${parseFn}(String subject) {`);
    lines.push(
      `        List<String> patternTokens = List.of(${tokens.map((t) => JSON.stringify(t)).join(', ')});`,
    );
    lines.push('        String[] subjectTokens = subject.split("\\\\.");');
    lines.push('        if (patternTokens.size() != subjectTokens.length) return Optional.empty();');
    for (const p of subj.params) {
      lines.push(`        String ${camelToLowerCamel(p.name)} = null;`);
    }
    lines.push('        for (int i = 0; i < patternTokens.size(); i += 1) {');
    lines.push('            String p = patternTokens.get(i);');
    lines.push('            String s = subjectTokens[i];');
    lines.push('            if (p.startsWith("{") && p.endsWith("}")) {');
    lines.push('                String name = p.substring(1, p.length() - 1);');
    lines.push('                switch (name) {');
    for (const p of subj.params) {
      lines.push(`                    case ${JSON.stringify(p.name)} -> ${camelToLowerCamel(p.name)} = s;`);
    }
    lines.push('                    default -> { return Optional.empty(); }');
    lines.push('                }');
    lines.push('            } else if (!p.equals(s)) {');
    lines.push('                return Optional.empty();');
    lines.push('            }');
    lines.push('        }');
    const ctor = subj.params.map((p) => camelToLowerCamel(p.name)).join(', ');
    lines.push(`        return Optional.of(new ${recordName}(${ctor}));`);
    lines.push('    }');
    lines.push('');
  }

  if (model.queueGroups.length > 0) {
    for (const qg of model.queueGroups) {
      emitDocBlockJava(lines, [qg.description, qg.service ? `Service: ${qg.service}` : null]);
      lines.push(
        `    public static final String ${upperSnake(qg.name)} = ${JSON.stringify(qg.value)};`,
      );
      lines.push('');
    }
  }

  if (model.streams.length > 0) {
    for (const st of model.streams) {
      emitDocBlockJava(lines, [st.description, st.service ? `Service: ${st.service}` : null]);
      lines.push(
        `    public static final String ${upperSnake(st.name)}_STREAM_NAME = ${JSON.stringify(st.name)};`,
      );
      const subjArr = st.subjects.map((s) => JSON.stringify(s)).join(', ');
      lines.push(
        `    public static final List<String> ${upperSnake(st.name)}_STREAM_SUBJECTS = List.of(${subjArr});`,
      );
      if (st.retention) {
        lines.push(
          `    public static final String ${upperSnake(st.name)}_STREAM_RETENTION = ${JSON.stringify(st.retention)};`,
        );
      }
      if (st.storage) {
        lines.push(
          `    public static final String ${upperSnake(st.name)}_STREAM_STORAGE = ${JSON.stringify(st.storage)};`,
        );
      }
      if (st.ack) {
        lines.push(
          `    public static final String ${upperSnake(st.name)}_STREAM_ACK = ${JSON.stringify(st.ack)};`,
        );
      }
      lines.push('');
    }
  }

  lines.push('}');
  lines.push('');
  return lines.join('\n');
}

function emitDocBlockJava(lines, parts) {
  const filtered = parts.filter(Boolean);
  if (filtered.length === 0) return;
  lines.push('    /**');
  for (const part of filtered) {
    for (const line of splitDoc(part)) lines.push(`     * ${line}`);
  }
  lines.push('     */');
}

// ============================================================
// helpers
// ============================================================

function camelToSnake(name) {
  // Already snake_case (lower or mixed-case with underscores) — preserve.
  // Already UPPER_SNAKE (e.g. "DD_REMOTE_TASKS") — just lower-case it.
  if (/^[A-Z][A-Z0-9_]*$/.test(name)) return name.toLowerCase();
  return name.replace(/[A-Z]/g, (ch, index) => (index === 0 ? ch.toLowerCase() : `_${ch.toLowerCase()}`));
}

function camelToLowerCamel(name) {
  // Used to turn snake_case (e.g. "thread_id") or PascalCase
  // (e.g. "ThreadId") into lowerCamel ("threadId").
  if (name.includes('_')) {
    const parts = name.split('_').filter(Boolean);
    if (parts.length === 0) return name;
    return (
      parts[0].toLowerCase() +
      parts
        .slice(1)
        .map((p) => (p ? p.charAt(0).toUpperCase() + p.slice(1).toLowerCase() : ''))
        .join('')
    );
  }
  return name.charAt(0).toLowerCase() + name.slice(1);
}

function lowerCamel(name) {
  if (!name) return name;
  const p = pascal(name);
  return p.charAt(0).toLowerCase() + p.slice(1);
}

function pascal(name) {
  // Normalises mixed inputs:
  //   "ThreadTasks"      -> "ThreadTasks"
  //   "thread_id"        -> "ThreadId"
  //   "DD_REMOTE_TASKS"  -> "DdRemoteTasks"
  return name
    .split(/[_\s-]+/)
    .map((part) => {
      if (!part) return '';
      const isAllUpper = /^[A-Z0-9]+$/.test(part);
      const head = part.charAt(0).toUpperCase();
      const tail = isAllUpper ? part.slice(1).toLowerCase() : part.slice(1);
      return head + tail;
    })
    .join('');
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
