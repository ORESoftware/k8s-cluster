import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, lstatSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import test from 'node:test';

type CanonicalSubject = {
  kind: 'static' | 'parameterized';
  name: string;
  pattern?: string;
  stream?: string;
  subject?: string;
  wildcard?: string;
};

type CanonicalStream = {
  name: string;
  subjects: string[];
};

type CanonicalModel = {
  subjects: CanonicalSubject[];
  streams: CanonicalStream[];
};

type SubjectUsage = {
  expression: string;
  file: string;
  line: number;
  source: string;
};

const TRACKED_WORKLOAD_ROOTS = [
  'remote/argocd',
  'remote/databases',
  'remote/deployments',
];

const SCANNED_FILE = /\.(?:ya?ml|json|toml|sql|rs|ts|js|mjs|py|go|java|fs|ex|erl|gleam|dart|hs|ml|zig|cpp|h|hpp)$/;
const CONFIG_FILE = /\.(?:ya?ml|json|toml|sql)$/;
const COMMENT_LINE = /^\s*(?:\/\/|#|--|\/\*|\*)/;
const SUBJECT_TOKEN = String.raw`(?:[A-Za-z0-9_-]+|\*|>|\{\}|\{[A-Za-z_][A-Za-z0-9_]*\}|\$\{[A-Za-z_][A-Za-z0-9_]*\}|<[A-Za-z_][A-Za-z0-9_]*>)`;
const SUBJECT_LITERAL = new RegExp(
  String.raw`\b(?:dd\.remote|cdc|presence\.(?:broadcast|member_change))(?:\.${SUBJECT_TOKEN})+`,
  'g',
);
const SUBJECT_PREFIX_CONTEXT = /(?:SUBJECT_PREFIX|subject[_-]?prefix|subjectPrefix)/i;

// Keep exceptions exact and temporary. The current pinned schema covers every
// observed cluster subject, so any future uncovered usage must fail immediately.
const KNOWN_PINNED_SCHEMA_DEBT = new Map<string, string>();

function findRepoRoot(): string {
  const candidates = [
    process.cwd(),
    resolve(process.cwd(), '..'),
    resolve(process.cwd(), '..', '..'),
  ];

  for (const candidate of candidates) {
    if (existsSync(resolve(candidate, 'remote/libs/nats/subject-defs/schema/index.json'))) {
      return candidate;
    }
  }

  throw new Error('Could not find k8s-cluster root with initialized remote/libs subject schemas.');
}

async function loadCanonicalModel(repoRoot: string): Promise<CanonicalModel> {
  const subjectDefsRoot = resolve(repoRoot, 'remote/libs/nats/subject-defs');
  const schemaRoot = resolve(subjectDefsRoot, 'schema');
  const index = JSON.parse(readFileSync(resolve(schemaRoot, 'index.json'), 'utf8')) as {
    schemas: string[];
  };
  const schemaFiles = index.schemas.map((filename) => ({
    filename,
    doc: JSON.parse(readFileSync(resolve(schemaRoot, filename), 'utf8')),
  }));
  const generatorUrl = pathToFileURL(resolve(subjectDefsRoot, 'src/generate.mjs')).href;
  const generator = await import(generatorUrl) as {
    buildModel(files: Array<{ filename: string; doc: unknown }>): CanonicalModel;
  };

  return generator.buildModel(schemaFiles);
}

function trackedWorkloadFiles(repoRoot: string): string[] {
  const output = execFileSync('git', ['ls-files', '--recurse-submodules', '--', ...TRACKED_WORKLOAD_ROOTS], {
    cwd: repoRoot,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });

  return output
    .split('\n')
    .filter(Boolean)
    .filter((file) => SCANNED_FILE.test(file))
    .filter((file) => !file.startsWith('remote/libs/'))
    .filter((file) => !file.split('/').some((part) => part === 'vendor'))
    .filter((file) => {
      try {
        return lstatSync(resolve(repoRoot, file)).isFile();
      } catch {
        return false;
      }
    });
}

function normalizePlaceholders(expression: string): string {
  return expression
    .replace(/<([A-Za-z_][A-Za-z0-9_]*)>/g, '{$1}')
    .replace(/\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g, '{$1}')
    .replace(/\{\}/g, '{value}');
}

function scanLiteralUsages(repoRoot: string): SubjectUsage[] {
  const usages: SubjectUsage[] = [];

  for (const file of trackedWorkloadFiles(repoRoot)) {
    const lines = readFileSync(resolve(repoRoot, file), 'utf8').split(/\r?\n/);

    for (let index = 0; index < lines.length; index += 1) {
      const source = lines[index];
      if (COMMENT_LINE.test(source)) continue;

      for (const match of source.matchAll(SUBJECT_LITERAL)) {
        const matchIndex = match.index ?? 0;
        if (source[matchIndex + match[0].length] === '.') continue;
        if (/\bassert(?:_eq|_ne)?!/.test(source)) continue;
        const context = lines.slice(Math.max(0, index - 2), index + 1).join('\n');
        if (!CONFIG_FILE.test(file) && !/subject/i.test(context)) continue;
        if (SUBJECT_PREFIX_CONTEXT.test(context)) continue;

        usages.push({
          expression: normalizePlaceholders(match[0]),
          file,
          line: index + 1,
          source: source.trim(),
        });
      }
    }
  }

  return usages;
}

function browserJobDerivedUsages(repoRoot: string): SubjectUsage[] {
  const file = 'remote/deployments/browser-job-runner-rs/src/main.rs';
  const lines = readFileSync(resolve(repoRoot, file), 'utf8').split(/\r?\n/);
  const contents = lines.join('\n');
  const prefixMatch = contents.match(
    /result_subject_prefix:\s*env_value\(\s*"[^"]*SUBJECT_PREFIX",\s*"([^"]+)"/,
  );

  assert.ok(prefixMatch, `${file} must expose the browser-job result subject prefix`);

  return ['events', 'result'].map((suffix) => {
    const sourceFragment = `format!("{}.{job_id}.${suffix}", state.config.result_subject_prefix)`;
    const index = lines.findIndex((line) => line.includes(sourceFragment));
    assert.notEqual(index, -1, `${file} must retain the expected ${suffix} subject construction`);

    return {
      expression: `${prefixMatch[1]}.{job_id}.${suffix}`,
      file,
      line: index + 1,
      source: lines[index].trim(),
    };
  });
}

function uniqueUsages(usages: SubjectUsage[]): SubjectUsage[] {
  return [...new Map(
    usages.map((usage) => [
      `${usage.expression}\0${usage.file}\0${usage.line}`,
      usage,
    ]),
  ).values()];
}

function isSingleTokenPattern(token: string): boolean {
  return token === '*' || /^\{[A-Za-z_][A-Za-z0-9_]*\}$/.test(token);
}

// Returns true when every subject selected by `usage` is also selected by
// `canonical`. This supports concrete subjects, parameterized patterns, '*',
// and terminal '>' without allowing a broad workload wildcard to hide drift.
function expressionIsContainedBy(usage: string, canonical: string): boolean {
  const usageTokens = usage.split('.');
  const canonicalTokens = canonical.split('.');

  for (let index = 0; index < canonicalTokens.length; index += 1) {
    const canonicalToken = canonicalTokens[index];
    const usageToken = usageTokens[index];

    if (canonicalToken === '>') {
      return index === canonicalTokens.length - 1 && index < usageTokens.length;
    }
    if (usageToken === undefined || usageToken === '>') return false;
    if (isSingleTokenPattern(canonicalToken)) continue;
    if (isSingleTokenPattern(usageToken) || usageToken !== canonicalToken) return false;
  }

  return usageTokens.length === canonicalTokens.length;
}

function subjectExpressions(subject: CanonicalSubject): string[] {
  if (subject.kind === 'static') return subject.subject ? [subject.subject] : [];
  return [subject.pattern, subject.wildcard].filter((value): value is string => Boolean(value));
}

function modelCoversExpression(model: CanonicalModel, usage: string): boolean {
  const streams = new Map(model.streams.map((stream) => [stream.name, stream]));

  return model.subjects.some((subject) => {
    if (!subjectExpressions(subject).some((canonical) => expressionIsContainedBy(usage, canonical))) {
      return false;
    }
    if (!subject.stream) return true;

    const stream = streams.get(subject.stream);
    assert.ok(stream, `Canonical subject ${subject.name} references missing stream ${subject.stream}`);
    return stream.subjects.some((filter) => expressionIsContainedBy(usage, filter));
  });
}

function formatUsages(usages: SubjectUsage[]): string {
  return usages
    .sort((left, right) => left.expression.localeCompare(right.expression)
      || left.file.localeCompare(right.file)
      || left.line - right.line)
    .map((usage) => `  ${usage.expression}\n    ${usage.file}:${usage.line}\n    ${usage.source}`)
    .join('\n');
}

test('subject containment respects parameters, wildcards, and stream bounds', async () => {
  const model = await loadCanonicalModel(findRepoRoot());

  assert.equal(modelCoversExpression(model, 'dd.remote.container_pool.rust.requests'), true);
  assert.equal(modelCoversExpression(model, 'dd.remote.container_pool.*.requests'), true);
  assert.equal(modelCoversExpression(model, 'cdc.public.app_config.>'), true);
  assert.equal(modelCoversExpression(model, 'dd.remote.not_declared.requests'), false);
  assert.equal(modelCoversExpression(model, 'dd.remote.>'), false);
});

test('tracked workload NATS subjects agree with the pinned remote/libs schema', async () => {
  const repoRoot = findRepoRoot();
  const model = await loadCanonicalModel(repoRoot);
  const usages = uniqueUsages([
    ...scanLiteralUsages(repoRoot),
    ...browserJobDerivedUsages(repoRoot),
  ]);

  assert.ok(model.subjects.length > 100, 'pinned remote/libs schema model is unexpectedly small');
  assert.ok(usages.length > 100, 'workload subject scan is unexpectedly small');

  const unexpected = usages.filter((usage) =>
    !modelCoversExpression(model, usage.expression)
    && !KNOWN_PINNED_SCHEMA_DEBT.has(usage.expression));

  assert.equal(
    unexpected.length,
    0,
    `Workload NATS subjects are absent from the pinned remote/libs schema:\n${formatUsages(unexpected)}`,
  );

  const observedExpressions = new Set(usages.map((usage) => usage.expression));
  const staleDebt = [...KNOWN_PINNED_SCHEMA_DEBT.entries()].filter(([expression]) =>
    !observedExpressions.has(expression) || modelCoversExpression(model, expression));

  assert.deepEqual(
    staleDebt,
    [],
    `Remove stale pinned-schema debt entries (usage disappeared or remote/libs now covers it):\n${staleDebt
      .map(([expression, reason]) => `  ${expression}: ${reason}`)
      .join('\n')}`,
  );
});
