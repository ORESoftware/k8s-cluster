import { execFileSync, spawnSync } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { buildModel } from './generate.mjs';

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const schemaRoot = path.join(packageRoot, 'schema');

const targetFiles = {
  typescript: ['generated', 'typescript', 'index.ts'],
  javascript: ['generated', 'javascript', 'index.mjs'],
  rust: ['generated', 'rust', 'src', 'lib.rs'],
  python: ['generated', 'python', 'dd_nats_subject_defs.py'],
  gleam: ['generated', 'gleam', 'src', 'dd_nats_subject_defs.gleam'],
  erlang: ['generated', 'erlang', 'src', 'dd_nats_subject_defs.erl'],
  dart: ['generated', 'dart', 'lib', 'dd_nats_subject_defs.dart'],
  go: ['generated', 'go', 'ddnats.go'],
  java: ['generated', 'jvm', 'src', 'main', 'java', 'dd', 'nats', 'DdNatsSubjects.java'],
  haskell: ['generated', 'haskell', 'src', 'DdNatsSubjectDefs.hs'],
  ocaml: ['generated', 'ocaml', 'lib', 'dd_nats_subject_defs.ml'],
  fsharp: ['generated', 'fsharp', 'DdNatsSubjectDefs.fs'],
  cpp: ['generated', 'cpp', 'dd_nats_subject_defs.hpp'],
  zig: ['generated', 'zig', 'dd_nats_subject_defs.zig'],
  elixir: ['generated', 'elixir', 'lib', 'dd_nats_subject_defs.ex'],
};

function commandAvailable(command) {
  // macOS can expose compiler shims (notably `javac`) even when no matching
  // runtime is installed. A spawned process is usable only when its version
  // probe succeeds, not merely when the executable path resolves.
  return spawnSync(command, ['--version'], { stdio: 'ignore' }).status === 0;
}

function camelToSnake(name) {
  if (/^[A-Z][A-Z0-9_]*$/.test(name)) return name.toLowerCase();
  return name.replace(/[A-Z]/g, (character, index) => (
    index === 0 ? character.toLowerCase() : `_${character.toLowerCase()}`
  ));
}

function pascal(name) {
  return name
    .split(/[_\s-]+/)
    .map((part) => {
      if (!part) return '';
      const isAllUpper = /^[A-Z0-9]+$/.test(part);
      return part.charAt(0).toUpperCase() + (isAllUpper ? part.slice(1).toLowerCase() : part.slice(1));
    })
    .join('');
}

function lowerCamel(name) {
  const value = pascal(name);
  return value.charAt(0).toLowerCase() + value.slice(1);
}

function natsLowerCamel(name) {
  return name.charAt(0).toLowerCase() + name.slice(1);
}

function upperSnake(name) {
  return camelToSnake(name).toUpperCase();
}

function fixtureValue(paramName) {
  return paramName === 'prefix' ? 'cdc' : `fixture_${paramName}`;
}

function renderPattern(pattern, params) {
  const values = Object.fromEntries(params.map((param) => [param.name, fixtureValue(param.name)]));
  return pattern.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (_match, name) => values[name]);
}

async function temporaryDirectory(name) {
  return mkdtemp(path.join(tmpdir(), `${name}-`));
}

function executableSubjectSpecification(model) {
  return model.subjects.map((subject) => {
    if (subject.kind === 'static') {
      return {
        kind: 'static',
        name: subject.name,
        symbol: `${upperSnake(subject.name)}_SUBJECT`,
        expected: subject.subject,
      };
    }
    const values = Object.fromEntries(subject.params.map((param) => [param.name, fixtureValue(param.name)]));
    const wildcardParams = subject.params.filter((param) => subject.wildcard.includes(`{${param.name}}`));
    return {
      kind: 'parameterized',
      name: subject.name,
      formatter: `${lowerCamel(subject.name)}Subject`,
      parser: `parse${pascal(subject.name)}Subject`,
      args: subject.params.map((param) => values[param.name]),
      values,
      expected: renderPattern(subject.pattern, subject.params),
      wildcardFormatter: wildcardParams.length > 0 ? `format${pascal(subject.name)}Wildcard` : null,
      wildcardArgs: wildcardParams.map((param) => values[param.name]),
      expectedWildcard: renderPattern(subject.wildcard, subject.params),
    };
  });
}

function subscriptionMatchesSubject(subscription, subject) {
  const subscriptionTokens = subscription.split('.');
  const subjectTokens = subject.split('.');
  let subjectIndex = 0;
  for (let subscriptionIndex = 0; subscriptionIndex < subscriptionTokens.length; subscriptionIndex += 1) {
    const token = subscriptionTokens[subscriptionIndex];
    if (token === '>') return subscriptionIndex === subscriptionTokens.length - 1 && subjectIndex < subjectTokens.length;
    if (subjectIndex >= subjectTokens.length || subjectTokens[subjectIndex] === '>') return false;
    if (token !== '*' && token !== subjectTokens[subjectIndex]) return false;
    subjectIndex += 1;
  }
  return subjectIndex === subjectTokens.length;
}

async function loadSchemaModel() {
  const index = JSON.parse(await readFile(path.join(schemaRoot, 'index.json'), 'utf8'));
  const schemas = await Promise.all(index.schemas.map(async (filename) => ({
    filename,
    doc: JSON.parse(await readFile(path.join(schemaRoot, filename), 'utf8')),
  })));
  return buildModel(schemas);
}

test('schema index is sorted, unique, path-safe, and complete', async () => {
  const index = JSON.parse(await readFile(path.join(schemaRoot, 'index.json'), 'utf8'));
  assert.ok(Array.isArray(index.schemas) && index.schemas.length > 0, 'schema index must be non-empty');
  assert.equal(new Set(index.schemas).size, index.schemas.length, 'schema index contains duplicates');
  assert.deepEqual(index.schemas, [...index.schemas].sort(), 'schema index must stay alphabetized');
  for (const filename of index.schemas) {
    assert.match(filename, /^[A-Za-z0-9_-]+\.schema\.json$/, `unsafe schema index entry ${filename}`);
    assert.equal(path.basename(filename), filename, `schema index entry escapes schema/: ${filename}`);
  }

  const actualSchemas = (await readdir(schemaRoot))
    .filter((filename) => filename.endsWith('.schema.json'))
    .sort();
  assert.deepEqual(index.schemas, actualSchemas, 'schema/index.json must list every schema file exactly once');
});

function staticSymbol(target, name) {
  const snake = camelToSnake(name);
  const upper = upperSnake(name);
  const camel = lowerCamel(name);
  const p = pascal(name);
  switch (target) {
    case 'typescript':
    case 'javascript':
    case 'rust':
    case 'python':
    case 'java':
      return `${upper}_SUBJECT`;
    case 'gleam':
    case 'erlang':
    case 'ocaml':
    case 'cpp':
    case 'zig':
    case 'elixir':
      return `${snake}_subject`;
    case 'dart':
      return `${camel}Subject`;
    case 'go':
      return `${p}Subject`;
    case 'haskell':
    case 'fsharp':
      return `${natsLowerCamel(name)}Subject`;
    default:
      throw new Error(`Unknown target ${target}`);
  }
}

function parameterSymbol(target, name, suffix) {
  const snake = camelToSnake(name);
  const upper = upperSnake(name);
  const camel = lowerCamel(name);
  const p = pascal(name);
  const suffixPascal = pascal(suffix);
  switch (target) {
    case 'typescript':
    case 'javascript':
    case 'rust':
    case 'python':
    case 'java':
      return `${upper}_${suffix.toUpperCase()}`;
    case 'gleam':
    case 'erlang':
    case 'ocaml':
    case 'cpp':
    case 'zig':
    case 'elixir':
      return `${snake}_${suffix.toLowerCase()}`;
    case 'dart':
      return `${camel}${suffixPascal}`;
    case 'go':
      return `${p}${suffixPascal}`;
    case 'haskell':
    case 'fsharp':
      return `${natsLowerCamel(name)}${suffixPascal}`;
    default:
      throw new Error(`Unknown target ${target}`);
  }
}

function dynamicWildcardFormatterSymbol(target, name) {
  const snake = camelToSnake(name);
  const p = pascal(name);
  switch (target) {
    case 'typescript':
    case 'javascript':
    case 'dart':
    case 'java':
      return `format${p}Wildcard`;
    case 'go':
      return `Format${p}Wildcard`;
    case 'haskell':
    case 'fsharp':
      return `format${p}Wildcard`;
    case 'rust':
    case 'python':
    case 'gleam':
    case 'erlang':
    case 'ocaml':
    case 'cpp':
    case 'zig':
    case 'elixir':
      return `format_${snake}_wildcard`;
    default:
      throw new Error(`Unknown target ${target}`);
  }
}

test('schema model is NATS-valid and stream bindings are coherent', async () => {
  const model = await loadSchemaModel();
  assert.ok(model.subjects.length > 0, 'the schema index must produce subjects');

  const staticValues = new Set();
  for (const subject of model.subjects) {
    if (subject.kind === 'static') {
      assert.match(subject.subject, /^[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*$/);
      assert.ok(!staticValues.has(subject.subject), `duplicate static subject ${subject.subject}`);
      staticValues.add(subject.subject);
      continue;
    }

    const parameterNames = subject.params.map((param) => param.name);
    const placeholders = [...subject.pattern.matchAll(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g)].map((match) => match[1]);
    assert.deepEqual(placeholders.sort(), [...parameterNames].sort(), `${subject.name} parameter/pattern mismatch`);
    assert.ok(subscriptionMatchesSubject(
      renderPattern(subject.wildcard, subject.params),
      renderPattern(subject.pattern, subject.params),
    ), `${subject.name} wildcard does not cover its formatted subject`);
  }

  for (const stream of model.streams) {
    assert.ok(stream.subjects.length > 0, `${stream.name} has no subjects`);
    assert.ok(stream.subjects.some((subscription) => model.subjects.some((subject) => (
      subscriptionMatchesSubject(
        subscription,
        subject.kind === 'static' ? subject.subject : renderPattern(subject.pattern, subject.params),
      )
    ))), `${stream.name} does not cover any declared subject`);
  }

  const streams = new Map(model.streams.map((stream) => [stream.name, stream]));
  for (const subject of model.subjects.filter((entry) => entry.stream)) {
    const stream = streams.get(subject.stream);
    assert.ok(stream, `${subject.name} references unknown stream ${subject.stream}`);
    const resolved = subject.kind === 'static' ? subject.subject : renderPattern(subject.pattern, subject.params);
    assert.ok(stream.subjects.some((subscription) => subscriptionMatchesSubject(subscription, resolved)),
      `${subject.name} is not covered by its declared stream ${subject.stream}`);
  }
});

test('every generated language target contains the complete schema contract', async () => {
  const model = await loadSchemaModel();
  const sources = Object.fromEntries(await Promise.all(
    Object.entries(targetFiles).map(async ([target, file]) => [
      target,
      await readFile(path.join(packageRoot, ...file), 'utf8'),
    ]),
  ));

  for (const [target, source] of Object.entries(sources)) {
    assert.match(source, /AUTOGENERATED by remote\/libs\/nats\/subject-defs\/src\/generate\.mjs/,
      `${target} is not a generated NATS target`);
    for (const subject of model.subjects) {
      if (subject.kind === 'static') {
        assert.ok(source.includes(staticSymbol(target, subject.name)),
          `${target} omits static symbol for ${subject.name}`);
        assert.ok(source.includes(JSON.stringify(subject.subject)),
          `${target} omits static subject value for ${subject.name}`);
        continue;
      }

      assert.ok(source.includes(parameterSymbol(target, subject.name, 'pattern')),
        `${target} omits pattern symbol for ${subject.name}`);
      assert.ok(source.includes(parameterSymbol(target, subject.name, 'wildcard')),
        `${target} omits wildcard symbol for ${subject.name}`);
      assert.ok(source.includes(JSON.stringify(subject.pattern)),
        `${target} omits pattern value for ${subject.name}`);
      assert.ok(source.includes(JSON.stringify(subject.wildcard)),
        `${target} omits wildcard value for ${subject.name}`);
      if (/\{[A-Za-z_][A-Za-z0-9_]*\}/.test(subject.wildcard)) {
        assert.ok(source.includes(dynamicWildcardFormatterSymbol(target, subject.name)),
          `${target} omits dynamic wildcard formatter for ${subject.name}`);
      }
    }
    for (const queueGroup of model.queueGroups) {
      assert.ok(source.includes(JSON.stringify(queueGroup.value)),
        `${target} omits standalone queue group ${queueGroup.name}`);
    }
    for (const stream of model.streams) {
      assert.ok(source.includes(JSON.stringify(stream.name)), `${target} omits stream ${stream.name}`);
      for (const subscription of stream.subjects) {
        assert.ok(source.includes(JSON.stringify(subscription)),
          `${target} omits ${stream.name} subscription ${subscription}`);
      }
    }
  }
});

test('every exported generated language target is covered by conformance tests', async () => {
  const packageJson = JSON.parse(await readFile(path.join(packageRoot, 'package.json'), 'utf8'));
  const exportedTargets = Object.entries(packageJson.exports)
    .filter(([name]) => name !== './schema')
    .map(([, filename]) => filename.replace(/^\.\//, ''))
    .sort();
  const testedTargets = Object.values(targetFiles).map((parts) => path.join(...parts)).sort();
  assert.deepEqual(testedTargets, exportedTargets);
});

test('generated JavaScript formats and parses every parameterized subject', async () => {
  const model = await loadSchemaModel();
  const generated = await import(pathToFileURL(path.join(packageRoot, ...targetFiles.javascript)).href);

  for (const subject of model.subjects) {
    if (subject.kind === 'static') {
      assert.equal(generated[`${upperSnake(subject.name)}_SUBJECT`], subject.subject);
      continue;
    }

    const values = Object.fromEntries(subject.params.map((param) => [param.name, fixtureValue(param.name)]));
    const formatter = generated[`${lowerCamel(subject.name)}Subject`];
    const parser = generated[`parse${pascal(subject.name)}Subject`];
    assert.equal(typeof formatter, 'function', `missing JavaScript formatter for ${subject.name}`);
    assert.equal(typeof parser, 'function', `missing JavaScript parser for ${subject.name}`);
    const rendered = formatter(...subject.params.map((param) => values[param.name]));
    assert.equal(rendered, renderPattern(subject.pattern, subject.params), `${subject.name} formatter drifted`);
    assert.deepEqual(parser(rendered), values, `${subject.name} parser drifted`);
    assert.equal(parser(`${rendered}.unexpected`), null, `${subject.name} parser accepts an invalid subject`);
    assert.equal(generated[`${upperSnake(subject.name)}_PATTERN`], subject.pattern);
    assert.equal(generated[`${upperSnake(subject.name)}_WILDCARD`], subject.wildcard);

    if (/\{[A-Za-z_][A-Za-z0-9_]*\}/.test(subject.wildcard)) {
      const wildcardFormatter = generated[`format${pascal(subject.name)}Wildcard`];
      const wildcardParams = subject.params.filter((param) => subject.wildcard.includes(`{${param.name}}`));
      assert.equal(typeof wildcardFormatter, 'function', `missing JavaScript wildcard formatter for ${subject.name}`);
      assert.equal(
        wildcardFormatter(...wildcardParams.map((param) => values[param.name])),
        renderPattern(subject.wildcard, subject.params),
        `${subject.name} wildcard formatter drifted`,
      );
    }
  }

  for (const queueGroup of model.queueGroups) {
    assert.equal(generated[upperSnake(queueGroup.name)], queueGroup.value);
  }
  for (const stream of model.streams) {
    assert.equal(generated[`${upperSnake(stream.name)}_STREAM_NAME`], stream.name);
    assert.deepEqual(generated[`${upperSnake(stream.name)}_STREAM_SUBJECTS`], stream.subjects);
  }
});

test('generated TypeScript executes every formatter and parser on Node 22+', {
  skip: Number(process.versions.node.split('.')[0]) < 22,
}, async () => {
  const model = await loadSchemaModel();
  const specification = executableSubjectSpecification(model);
  const moduleUrl = pathToFileURL(path.join(packageRoot, ...targetFiles.typescript)).href;
  const script = `
import assert from 'node:assert/strict';
import * as generated from ${JSON.stringify(moduleUrl)};

for (const item of JSON.parse(process.env.DD_NATS_CONFORMANCE_SPEC)) {
  if (item.kind === 'static') {
    assert.equal(generated[item.symbol], item.expected, item.name);
    continue;
  }
  const rendered = generated[item.formatter](...item.args);
  assert.equal(rendered, item.expected, item.name);
  assert.deepEqual(generated[item.parser](rendered), item.values, item.name);
  assert.equal(generated[item.parser](rendered + '.unexpected'), null, item.name);
  if (item.wildcardFormatter) {
    assert.equal(generated[item.wildcardFormatter](...item.wildcardArgs), item.expectedWildcard, item.name);
  }
}
`;
  execFileSync(process.execPath, ['--experimental-strip-types', '--input-type=module', '-e', script], {
    cwd: packageRoot,
    stdio: 'pipe',
    env: {
      ...process.env,
      DD_NATS_CONFORMANCE_SPEC: JSON.stringify(specification),
      NODE_NO_WARNINGS: '1',
    },
  });
});

test('generated Python formats and parses every parameterized subject', async () => {
  const model = await loadSchemaModel();
  const specification = model.subjects.map((subject) => ({
    name: subject.name,
    kind: subject.kind,
    subject: subject.kind === 'static' ? subject.subject : undefined,
    pattern: subject.kind === 'parameterized' ? subject.pattern : undefined,
    wildcard: subject.kind === 'parameterized' ? subject.wildcard : undefined,
    params: subject.kind === 'parameterized' ? subject.params : [],
  }));
  const script = String.raw`
import json
import os
import sys

sys.path.insert(0, os.environ['DD_NATS_PYTHON_PATH'])
import dd_nats_subject_defs as defs

def snake(name):
    if name and name[0].isupper() and name.upper() == name:
        return name.lower()
    return ''.join((ch.lower() if i == 0 else '_' + ch.lower()) if ch.isupper() else ch for i, ch in enumerate(name))

def pascal(name):
    return ''.join(part[:1].upper() + (part[1:].lower() if part.isupper() else part[1:]) for part in name.replace('-', '_').replace(' ', '_').split('_') if part)

def upper(name):
    return snake(name).upper()

def fixture(name):
    return 'cdc' if name == 'prefix' else 'fixture_' + name

for item in json.loads(os.environ['DD_NATS_CONFORMANCE_SPEC']):
    if item['kind'] == 'static':
        assert getattr(defs, upper(item['name']) + '_SUBJECT') == item['subject']
        continue
    values = {param['name']: fixture(param['name']) for param in item['params']}
    rendered = getattr(defs, snake(item['name']) + '_subject')(**values)
    expected = item['pattern']
    for name, value in values.items():
        expected = expected.replace('{' + name + '}', value)
    assert rendered == expected, (item['name'], rendered, expected)
    parsed = getattr(defs, 'parse_' + snake(item['name']) + '_subject')(rendered)
    assert parsed is not None, item['name']
    for name, value in values.items():
        assert getattr(parsed, snake(name)) == value, (item['name'], name)
    assert getattr(defs, 'parse_' + snake(item['name']) + '_subject')(rendered + '.unexpected') is None
    if '{' in item['wildcard']:
        wildcard_params = {name: value for name, value in values.items() if '{' + name + '}' in item['wildcard']}
        wildcard = getattr(defs, 'format_' + snake(item['name']) + '_wildcard')(**wildcard_params)
        expected_wildcard = item['wildcard']
        for name, value in values.items():
            expected_wildcard = expected_wildcard.replace('{' + name + '}', value)
        assert wildcard == expected_wildcard, (item['name'], wildcard, expected_wildcard)
print('OK')
`;
  const output = execFileSync('python3', ['-c', script], {
    cwd: packageRoot,
    encoding: 'utf8',
    env: {
      ...process.env,
      PYTHONDONTWRITEBYTECODE: '1',
      DD_NATS_PYTHON_PATH: path.join(packageRoot, 'generated', 'python'),
      DD_NATS_CONFORMANCE_SPEC: JSON.stringify(specification),
    },
  });
  assert.match(output, /OK/);
});

test('schema validation rejects invalid or ambiguous NATS definitions', () => {
  const makeModel = (extension) => buildModel([{
    filename: 'fixture.schema.json',
    doc: { '$dd:nats': extension },
  }]);
  const parameterized = {
    name: 'Example',
    kind: 'parameterized',
    pattern: 'dd.example.{id}',
    wildcard: 'dd.example.*',
    params: [{ name: 'id', type: 'string' }],
  };

  assert.throws(() => makeModel({ subjects: [
    { name: 'First', kind: 'static', subject: 'dd.duplicate' },
    { name: 'Second', kind: 'static', subject: 'dd.duplicate' },
  ] }), /Duplicate static NATS subject/);
  assert.throws(() => makeModel({ subjects: [{
    ...parameterized,
    pattern: 'dd.example.prefix-{id}',
  }] }), /placeholders must occupy a complete NATS token/);
  assert.throws(() => makeModel({ subjects: [{
    ...parameterized,
    wildcard: 'dd.other.*',
  }] }), /does not cover pattern/);
  assert.throws(() => makeModel({ subjects: [{
    ...parameterized,
    pattern: 'dd.example.{id}.>',
    wildcard: 'dd.example.>',
  }] }), /subscribe-only/);
  assert.throws(() => makeModel({ subjects: [{
    ...parameterized,
    stream: 'MISSING_STREAM',
  }] }), /references unknown stream/);
  assert.throws(() => makeModel({ streams: [{
    name: 'UNUSED_STREAM',
    subjects: ['dd.unused.>'],
    retention: 'invalid',
  }] }), /invalid JetStream retention/);
  assert.throws(() => makeModel({ subjects: [{
    ...parameterized,
    pattern: 'dd.example.{id_value}.{idValue}',
    wildcard: 'dd.example.*.*',
    params: [
      { name: 'id_value', type: 'string' },
      { name: 'idValue', type: 'string' },
    ],
  }] }), /parameter names must remain unique after cross-language normalization/);
  assert.throws(() => makeModel({ subjects: [
    parameterized,
    {
      ...parameterized,
      name: 'EquivalentRoute',
      pattern: 'dd.example.{key}',
      params: [{ name: 'key', type: 'string' }],
    },
  ] }), /parameterized route duplicates Example/);
  assert.throws(() => makeModel({ queueGroups: [
    { name: 'WorkerGroup', value: 'worker-one' },
    { name: 'worker_group', value: 'worker-two' },
  ] }), /Duplicate generated queueGroup identifier/);
  assert.throws(() => makeModel({ streams: [
    { name: 'EVENT_STREAM', subjects: ['dd.example'] },
    { name: 'EventStream', subjects: ['dd.example'] },
  ], subjects: [
    { name: 'ExampleStatic', kind: 'static', subject: 'dd.example' },
  ] }), /Duplicate generated stream identifier/);
});

test('generated Rust target round-trips the complete subject contract when Cargo is available', {
  skip: !commandAvailable('cargo'),
}, async () => {
  const model = await loadSchemaModel();
  const directory = await temporaryDirectory('dd-nats-rust-conformance');
  const generatedCrate = path.join(packageRoot, 'generated', 'rust');
  await writeFile(path.join(directory, 'Cargo.toml'), `
[package]
name = "dd-nats-conformance"
version = "0.0.0"
edition = "2021"

[dependencies]
dd-nats-subject-defs = { path = ${JSON.stringify(generatedCrate)} }
`);
  const lines = ['use dd_nats_subject_defs as defs;', '', 'fn main() {'];
  for (const subject of model.subjects) {
    if (subject.kind === 'static') {
      lines.push(`    assert_eq!(defs::${upperSnake(subject.name)}_SUBJECT, ${JSON.stringify(subject.subject)});`);
      continue;
    }
    const base = camelToSnake(subject.name);
    const values = Object.fromEntries(subject.params.map((param) => [param.name, fixtureValue(param.name)]));
    const args = subject.params.map((param) => JSON.stringify(values[param.name])).join(', ');
    const rendered = `rendered_${base}`;
    const parsed = `parsed_${base}`;
    lines.push(`    let ${rendered} = defs::${base}_subject(${args});`);
    lines.push(`    assert_eq!(${rendered}, ${JSON.stringify(renderPattern(subject.pattern, subject.params))});`);
    lines.push(`    let ${parsed} = defs::parse_${base}_subject(&${rendered}).expect(${JSON.stringify(subject.name)});`);
    for (const param of subject.params) {
      lines.push(`    assert_eq!(${parsed}.${camelToSnake(param.name)}, ${JSON.stringify(values[param.name])});`);
    }
    lines.push(`    assert!(defs::parse_${base}_subject(&format!("{}.unexpected", ${rendered})).is_none());`);
    const wildcardParams = subject.params.filter((param) => subject.wildcard.includes(`{${param.name}}`));
    if (wildcardParams.length > 0) {
      const wildcardArgs = wildcardParams.map((param) => JSON.stringify(values[param.name])).join(', ');
      lines.push(`    assert_eq!(defs::format_${base}_wildcard(${wildcardArgs}), ${JSON.stringify(renderPattern(subject.wildcard, subject.params))});`);
    }
  }
  for (const queueGroup of model.queueGroups) {
    lines.push(`    assert_eq!(defs::${upperSnake(queueGroup.name)}, ${JSON.stringify(queueGroup.value)});`);
  }
  for (const stream of model.streams) {
    const subjects = `&[${stream.subjects.map((subject) => JSON.stringify(subject)).join(', ')}]`;
    lines.push(`    assert_eq!(defs::${upperSnake(stream.name)}_STREAM_NAME, ${JSON.stringify(stream.name)});`);
    lines.push(`    assert_eq!(defs::${upperSnake(stream.name)}_STREAM_SUBJECTS, ${subjects});`);
  }
  lines.push('}');
  await mkdir(path.join(directory, 'src'));
  await writeFile(path.join(directory, 'src', 'main.rs'), `${lines.join('\n')}\n`);

  execFileSync('cargo', ['run', '--quiet', '--offline', '--manifest-path', path.join(directory, 'Cargo.toml')], {
    cwd: directory,
    stdio: 'pipe',
    env: {
      ...process.env,
      CARGO_TARGET_DIR: path.join(directory, 'target'),
    },
  });
});

test('generated Go target round-trips the complete subject contract when Go is available', {
  skip: !commandAvailable('go'),
}, async () => {
  const model = await loadSchemaModel();
  const directory = await temporaryDirectory('dd-nats-go-conformance');
  const generatedModule = path.join(packageRoot, 'generated', 'go');
  await writeFile(path.join(directory, 'go.mod'), `module ddnatsconformance

go 1.21

require dd/nats/subjectdefs v0.0.0

replace dd/nats/subjectdefs => ${generatedModule}
`);
  const lines = [
    'package conformance',
    '',
    'import (',
    '\t"reflect"',
    '\t"testing"',
    '\tdefs "dd/nats/subjectdefs"',
    ')',
    '',
    'func TestCompleteContract(t *testing.T) {',
  ];
  for (const subject of model.subjects) {
    if (subject.kind === 'static') {
      lines.push(`\tif got := defs.${pascal(subject.name)}Subject; got != ${JSON.stringify(subject.subject)} { t.Fatalf(${JSON.stringify(subject.name + ': %q')}, got) }`);
      continue;
    }
    const base = pascal(subject.name);
    const variable = `rendered${base}`;
    const values = Object.fromEntries(subject.params.map((param) => [param.name, fixtureValue(param.name)]));
    const args = subject.params.map((param) => JSON.stringify(values[param.name])).join(', ');
    lines.push(`\t${variable} := defs.${base}Subject(${args})`);
    lines.push(`\tif ${variable} != ${JSON.stringify(renderPattern(subject.pattern, subject.params))} { t.Fatalf(${JSON.stringify(subject.name + ': %q')}, ${variable}) }`);
    lines.push(`\tparsed${base}, ok${base} := defs.Parse${base}Subject(${variable})`);
    lines.push(`\tif !ok${base} { t.Fatal(${JSON.stringify(subject.name + ': parser rejected formatted subject')}) }`);
    for (const param of subject.params) {
      lines.push(`\tif parsed${base}.${pascal(param.name)} != ${JSON.stringify(values[param.name])} { t.Fatal(${JSON.stringify(subject.name + ': parser drift')}) }`);
    }
    lines.push(`\tif _, ok := defs.Parse${base}Subject(${variable} + ".unexpected"); ok { t.Fatal(${JSON.stringify(subject.name + ': parser accepted invalid suffix')}) }`);
    const wildcardParams = subject.params.filter((param) => subject.wildcard.includes(`{${param.name}}`));
    if (wildcardParams.length > 0) {
      const wildcardArgs = wildcardParams.map((param) => JSON.stringify(values[param.name])).join(', ');
      lines.push(`\tif got := defs.Format${base}Wildcard(${wildcardArgs}); got != ${JSON.stringify(renderPattern(subject.wildcard, subject.params))} { t.Fatalf(${JSON.stringify(subject.name + ' wildcard: %q')}, got) }`);
    }
  }
  for (const queueGroup of model.queueGroups) {
    lines.push(`\tif defs.${pascal(queueGroup.name)} != ${JSON.stringify(queueGroup.value)} { t.Fatal(${JSON.stringify(queueGroup.name)}) }`);
  }
  for (const stream of model.streams) {
    const subjects = `[]string{${stream.subjects.map((subject) => JSON.stringify(subject)).join(', ')}}`;
    lines.push(`\tif defs.${pascal(stream.name)}StreamName != ${JSON.stringify(stream.name)} { t.Fatal(${JSON.stringify(stream.name)}) }`);
    lines.push(`\tif !reflect.DeepEqual(defs.${pascal(stream.name)}StreamSubjects, ${subjects}) { t.Fatal(${JSON.stringify(stream.name + ' subjects')}) }`);
  }
  lines.push('}');
  await writeFile(path.join(directory, 'conformance_test.go'), `${lines.join('\n')}\n`);

  execFileSync('go', ['test', './...'], {
    cwd: directory,
    stdio: 'pipe',
    env: { ...process.env, GOWORK: 'off' },
  });
});

test('generated Java target round-trips the complete subject contract when javac is available', {
  skip: !commandAvailable('javac') || !commandAvailable('java'),
}, async () => {
  const model = await loadSchemaModel();
  const directory = await temporaryDirectory('dd-nats-java-conformance');
  const lines = [
    'import dd.nats.DdNatsSubjects;',
    'import java.util.List;',
    'import java.util.Objects;',
    '',
    'public final class DdNatsConformance {',
    '    private static void equal(Object actual, Object expected, String label) {',
    '        if (!Objects.equals(actual, expected)) throw new AssertionError(label + ": " + actual);',
    '    }',
    '    public static void main(String[] args) {',
  ];
  for (const subject of model.subjects) {
    if (subject.kind === 'static') {
      lines.push(`        equal(DdNatsSubjects.${upperSnake(subject.name)}_SUBJECT, ${JSON.stringify(subject.subject)}, ${JSON.stringify(subject.name)});`);
      continue;
    }
    const base = pascal(subject.name);
    const variable = `rendered${base}`;
    const parsed = `parsed${base}`;
    const values = Object.fromEntries(subject.params.map((param) => [param.name, fixtureValue(param.name)]));
    const args = subject.params.map((param) => JSON.stringify(values[param.name])).join(', ');
    lines.push(`        String ${variable} = DdNatsSubjects.${lowerCamel(subject.name)}Subject(${args});`);
    lines.push(`        equal(${variable}, ${JSON.stringify(renderPattern(subject.pattern, subject.params))}, ${JSON.stringify(subject.name)});`);
    lines.push(`        var ${parsed} = DdNatsSubjects.parse${base}Subject(${variable}).orElseThrow();`);
    for (const param of subject.params) {
      lines.push(`        equal(${parsed}.${lowerCamel(param.name)}(), ${JSON.stringify(values[param.name])}, ${JSON.stringify(subject.name + '.' + param.name)});`);
    }
    lines.push(`        if (DdNatsSubjects.parse${base}Subject(${variable} + ".unexpected").isPresent()) throw new AssertionError(${JSON.stringify(subject.name + ': invalid suffix')});`);
    const wildcardParams = subject.params.filter((param) => subject.wildcard.includes(`{${param.name}}`));
    if (wildcardParams.length > 0) {
      const wildcardArgs = wildcardParams.map((param) => JSON.stringify(values[param.name])).join(', ');
      lines.push(`        equal(DdNatsSubjects.format${base}Wildcard(${wildcardArgs}), ${JSON.stringify(renderPattern(subject.wildcard, subject.params))}, ${JSON.stringify(subject.name + ' wildcard')});`);
    }
  }
  for (const queueGroup of model.queueGroups) {
    lines.push(`        equal(DdNatsSubjects.${upperSnake(queueGroup.name)}, ${JSON.stringify(queueGroup.value)}, ${JSON.stringify(queueGroup.name)});`);
  }
  for (const stream of model.streams) {
    const subjects = `List.of(${stream.subjects.map((subject) => JSON.stringify(subject)).join(', ')})`;
    lines.push(`        equal(DdNatsSubjects.${upperSnake(stream.name)}_STREAM_NAME, ${JSON.stringify(stream.name)}, ${JSON.stringify(stream.name)});`);
    lines.push(`        equal(DdNatsSubjects.${upperSnake(stream.name)}_STREAM_SUBJECTS, ${subjects}, ${JSON.stringify(stream.name + ' subjects')});`);
  }
  lines.push('    }', '}', '');
  const harness = path.join(directory, 'DdNatsConformance.java');
  await writeFile(harness, lines.join('\n'));
  const generatedJava = path.join(packageRoot, ...targetFiles.java);
  execFileSync('javac', ['-d', directory, generatedJava, harness], { cwd: directory, stdio: 'pipe' });
  execFileSync('java', ['-cp', directory, 'DdNatsConformance'], { cwd: directory, stdio: 'pipe' });
});

test('generated C++ target round-trips the complete subject contract when Clang is available', {
  skip: !commandAvailable('clang++'),
}, async () => {
  const model = await loadSchemaModel();
  const directory = await temporaryDirectory('dd-nats-cpp-conformance');
  const header = path.join(packageRoot, ...targetFiles.cpp);
  const lines = [
    `#include ${JSON.stringify(header)}`,
    '#include <stdexcept>',
    '',
    'using namespace dd_nats_subject_defs;',
    '',
    'void equal(const std::string& actual, const std::string& expected, const char* label) {',
    '    if (actual != expected) throw std::runtime_error(label);',
    '}',
    '',
    'int main() {',
  ];
  for (const subject of model.subjects) {
    if (subject.kind === 'static') {
      lines.push(`    equal(${camelToSnake(subject.name)}_subject, ${JSON.stringify(subject.subject)}, ${JSON.stringify(subject.name)});`);
      continue;
    }
    const base = camelToSnake(subject.name);
    const variable = `rendered_${base}`;
    const parsed = `parsed_${base}`;
    const values = Object.fromEntries(subject.params.map((param) => [param.name, fixtureValue(param.name)]));
    const args = subject.params.map((param) => JSON.stringify(values[param.name])).join(', ');
    lines.push(`    auto ${variable} = ${base}_subject(${args});`);
    lines.push(`    equal(${variable}, ${JSON.stringify(renderPattern(subject.pattern, subject.params))}, ${JSON.stringify(subject.name)});`);
    lines.push(`    auto ${parsed} = parse_${base}_subject(${variable});`);
    lines.push(`    if (!${parsed}.has_value()) throw std::runtime_error(${JSON.stringify(subject.name + ': parser rejected formatted subject')});`);
    for (const param of subject.params) {
      lines.push(`    equal(${parsed}->${camelToSnake(param.name)}, ${JSON.stringify(values[param.name])}, ${JSON.stringify(subject.name + '.' + param.name)});`);
    }
    lines.push(`    if (parse_${base}_subject(${variable} + ".unexpected").has_value()) throw std::runtime_error(${JSON.stringify(subject.name + ': invalid suffix')});`);
    const wildcardParams = subject.params.filter((param) => subject.wildcard.includes(`{${param.name}}`));
    if (wildcardParams.length > 0) {
      const wildcardArgs = wildcardParams.map((param) => JSON.stringify(values[param.name])).join(', ');
      lines.push(`    equal(format_${base}_wildcard(${wildcardArgs}), ${JSON.stringify(renderPattern(subject.wildcard, subject.params))}, ${JSON.stringify(subject.name + ' wildcard')});`);
    }
  }
  lines.push('    return 0;', '}', '');
  const source = path.join(directory, 'conformance.cpp');
  const binary = path.join(directory, 'conformance');
  await writeFile(source, lines.join('\n'));
  execFileSync('clang++', ['-std=c++17', source, '-o', binary], { cwd: directory, stdio: 'pipe' });
  execFileSync(binary, [], { cwd: directory, stdio: 'pipe' });
});

test('generated Erlang target compiles when Erlang is available', {
  skip: !commandAvailable('erlc'),
}, async () => {
  const directory = await temporaryDirectory('dd-nats-erlang-conformance');
  execFileSync('erlc', ['-o', directory, path.join(packageRoot, ...targetFiles.erlang)], {
    cwd: directory,
    stdio: 'pipe',
  });
});

test('generated Gleam target compiles when Gleam is available', {
  skip: !commandAvailable('gleam'),
}, async () => {
  const directory = await temporaryDirectory('dd-nats-gleam-conformance');
  await mkdir(path.join(directory, 'src'));
  await copyFile(
    path.join(packageRoot, 'generated', 'gleam', 'gleam.toml'),
    path.join(directory, 'gleam.toml'),
  );
  for (const filename of ['dd_nats_subject_defs.gleam', 'dd_nats_subject_consts.erl']) {
    await copyFile(
      path.join(packageRoot, 'generated', 'gleam', 'src', filename),
      path.join(directory, 'src', filename),
    );
  }
  execFileSync('gleam', ['check'], {
    cwd: directory,
    stdio: 'pipe',
    env: { ...process.env, ERL_COMPILER_OPTIONS: '[nowarn_deprecated_catch]' },
  });
});

test('generated Dart target analyzes cleanly when Dart is available', {
  skip: !commandAvailable('dart'),
}, () => {
  execFileSync('dart', ['analyze', path.join(packageRoot, ...targetFiles.dart)], {
    cwd: packageRoot,
    stdio: 'pipe',
  });
});

test('generated Haskell target type-checks when GHC is available', {
  skip: !commandAvailable('ghc'),
}, async () => {
  const directory = await temporaryDirectory('dd-nats-haskell-conformance');
  execFileSync('ghc', [
    '-fno-code',
    '-outputdir', directory,
    path.join(packageRoot, ...targetFiles.haskell),
  ], { cwd: directory, stdio: 'pipe' });
});

test('generated OCaml target compiles when OCaml is available', {
  skip: !commandAvailable('ocamlc'),
}, async () => {
  const directory = await temporaryDirectory('dd-nats-ocaml-conformance');
  execFileSync('ocamlc', [
    '-c',
    '-o', path.join(directory, 'dd_nats_subject_defs.cmo'),
    path.join(packageRoot, ...targetFiles.ocaml),
  ], { cwd: directory, stdio: 'pipe' });
});

test('generated F# target builds when dotnet is available', {
  skip: !commandAvailable('dotnet'),
}, async () => {
  const directory = await temporaryDirectory('dd-nats-fsharp-conformance');
  for (const filename of ['DdNatsSubjectDefs.fs', 'DdNatsSubjectDefs.fsproj']) {
    await copyFile(
      path.join(packageRoot, 'generated', 'fsharp', filename),
      path.join(directory, filename),
    );
  }
  execFileSync('dotnet', ['build', '--nologo', '--verbosity', 'quiet', 'DdNatsSubjectDefs.fsproj'], {
    cwd: directory,
    stdio: 'pipe',
  });
});

test('generated Zig target compiles when Zig is available', {
  skip: !commandAvailable('zig'),
}, async () => {
  const directory = await temporaryDirectory('dd-nats-zig-conformance');
  execFileSync('zig', [
    'test', path.join(packageRoot, ...targetFiles.zig),
    '--cache-dir', path.join(directory, 'cache'),
    '--global-cache-dir', path.join(directory, 'global-cache'),
  ], { cwd: directory, stdio: 'pipe' });
});

test('generated Elixir target compiles when Elixir is available', {
  skip: !commandAvailable('elixirc'),
}, async () => {
  const directory = await temporaryDirectory('dd-nats-elixir-conformance');
  execFileSync('elixirc', ['-o', directory, path.join(packageRoot, ...targetFiles.elixir)], {
    cwd: directory,
    stdio: 'pipe',
  });
});
