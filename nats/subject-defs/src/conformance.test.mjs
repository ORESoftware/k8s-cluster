import { execFileSync, spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
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
  return !spawnSync(command, ['--version'], { stdio: 'ignore' }).error;
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
});

test('generated Rust target compiles when Cargo is available', {
  skip: !commandAvailable('cargo'),
}, () => {
  execFileSync('cargo', ['check', '--manifest-path', path.join(packageRoot, 'generated', 'rust', 'Cargo.toml')], {
    cwd: packageRoot,
    stdio: 'pipe',
    env: {
      ...process.env,
      CARGO_TARGET_DIR: '/tmp/dd-nats-subject-defs-cargo-target',
    },
  });
});

test('generated Go target compiles when Go is available', {
  skip: !commandAvailable('go'),
}, () => {
  execFileSync('go', ['test'], {
    cwd: path.join(packageRoot, 'generated', 'go'),
    stdio: 'pipe',
  });
});

test('generated C++ header parses when Clang is available', {
  skip: !commandAvailable('clang++'),
}, () => {
  execFileSync('clang++', ['-std=c++17', '-fsyntax-only', '-x', 'c++', path.join(packageRoot, ...targetFiles.cpp)], {
    cwd: packageRoot,
    stdio: 'pipe',
  });
});
