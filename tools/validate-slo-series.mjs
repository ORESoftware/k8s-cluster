#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..');
const requirePass = process.argv.includes('--require-pass');

const catalogPath = resolve(
  repoRoot,
  'docs/production/managed-public-beta-slos.json',
);
const derivedPath = resolve(
  repoRoot,
  'docs/production/managed-public-beta-slo-derived-series.json',
);

const [catalogText, derivedText] = await Promise.all([
  readFile(catalogPath, 'utf8'),
  readFile(derivedPath, 'utf8'),
]);

const errors = [];
const warnings = [];
const fail = (message) => errors.push(message);

function parseJson(name, text) {
  try {
    return JSON.parse(text);
  } catch (error) {
    fail(`${name} is not valid JSON: ${error.message}`);
    return {};
  }
}

function requireString(object, field, label) {
  if (typeof object?.[field] !== 'string' || object[field].trim() === '') {
    fail(`${label}.${field} must be a non-empty string.`);
  }
}

const catalog = parseJson('SLO catalog', catalogText);
const derived = parseJson('Derived-series registry', derivedText);

if (derived.schema_version !== 1) {
  fail(
    `Expected derived-series schema_version 1, received ${String(derived.schema_version)}.`,
  );
}
if (derived.contract_issue !== 'DEN-1390') {
  fail(
    `Expected derived-series contract_issue DEN-1390, received ${String(derived.contract_issue)}.`,
  );
}

const allowedStatuses = new Set(derived.allowed_source_statuses ?? []);
for (const status of ['specified', 'instrumented', 'queryable', 'measured']) {
  if (!allowedStatuses.has(status)) {
    fail(`Derived-series allowed_source_statuses is missing ${status}.`);
  }
}

const allowedLabels = new Set(
  catalog.label_policy?.allowed_grouping_labels ?? [],
);
const forbiddenLabels = new Set(
  catalog.label_policy?.forbidden_customer_labels ?? [],
);

const declaredSeries = new Map();
const sourceStatusCounts = new Map();

function registerSeries(series, label, sourceKind) {
  requireString(series, 'name', label);
  requireString(series, 'type', label);

  if (!Array.isArray(series.required_labels) || series.required_labels.length === 0) {
    fail(`${label}.required_labels must be a non-empty array.`);
  } else {
    const uniqueLabels = new Set();
    for (const metricLabel of series.required_labels) {
      if (typeof metricLabel !== 'string' || metricLabel.trim() === '') {
        fail(`${label}.required_labels contains an empty/non-string label.`);
        continue;
      }
      if (uniqueLabels.has(metricLabel)) {
        fail(`${label}.required_labels repeats ${metricLabel}.`);
      }
      uniqueLabels.add(metricLabel);
      if (!allowedLabels.has(metricLabel)) {
        fail(`${label} uses undeclared/high-cardinality label ${metricLabel}.`);
      }
      if (forbiddenLabels.has(metricLabel)) {
        fail(`${label} uses forbidden customer label ${metricLabel}.`);
      }
    }
  }

  const signature = JSON.stringify({
    type: series.type,
    required_labels: [...(series.required_labels ?? [])].sort(),
  });
  const prior = declaredSeries.get(series.name);
  if (prior && prior.signature !== signature) {
    fail(
      `${series.name} is declared inconsistently by ${prior.label} and ${label}.`,
    );
  } else if (!prior) {
    declaredSeries.set(series.name, {
      name: series.name,
      type: series.type,
      signature,
      label,
      sourceKind,
    });
  }
}

for (const [sloIndex, slo] of (catalog.slos ?? []).entries()) {
  const sloLabel = slo?.id || `catalog.slos[${sloIndex}]`;
  for (const [seriesIndex, series] of (slo.source_series ?? []).entries()) {
    registerSeries(
      series,
      `${sloLabel}.source_series[${seriesIndex}]`,
      'raw',
    );
  }
}

for (const [seriesIndex, series] of (derived.series ?? []).entries()) {
  const label = `derived.series[${seriesIndex}]`;
  registerSeries(series, label, 'derived');
  requireString(series, 'producer', label);
  requireString(series, 'semantics', label);
  requireString(series, 'source_status', label);

  if (!allowedStatuses.has(series.source_status)) {
    fail(`${label} uses unsupported source_status ${String(series.source_status)}.`);
  }
  sourceStatusCounts.set(
    series.source_status,
    (sourceStatusCounts.get(series.source_status) ?? 0) + 1,
  );

  if (!Array.isArray(series.blocking_issues) || series.blocking_issues.length === 0) {
    fail(`${label}.blocking_issues must be a non-empty array.`);
  } else {
    for (const issue of series.blocking_issues) {
      if (!/^DEN-\d+$/.test(issue)) {
        fail(`${label} has malformed blocking issue ${String(issue)}.`);
      }
    }
  }

  if (!Array.isArray(series.evidence)) {
    fail(`${label}.evidence must be an array.`);
  }
  if (series.source_status === 'measured' && series.evidence?.length === 0) {
    fail(`${label} is measured but has no exact-candidate evidence.`);
  }
  if (requirePass && series.source_status !== 'measured') {
    fail(
      `${label} source_status is ${series.source_status}; certification requires measured.`,
    );
  }
  if (requirePass && series.evidence?.length === 0) {
    fail(`${label} has no exact-candidate measurement evidence.`);
  }
}

if (!Array.isArray(derived.series) || derived.series.length === 0) {
  fail('Derived-series registry must contain at least one series.');
}

function resolveReference(reference) {
  if (declaredSeries.has(reference)) {
    return declaredSeries.get(reference);
  }

  for (const suffix of ['_bucket', '_sum', '_count', '_created']) {
    if (!reference.endsWith(suffix)) {
      continue;
    }
    const base = reference.slice(0, -suffix.length);
    const declaration = declaredSeries.get(base);
    if (declaration?.type === 'histogram') {
      return declaration;
    }
  }

  return null;
}

const referencePattern = /\bfiducia_[A-Za-z0-9_:]+\b/g;
const referencedSeries = new Set();

for (const [sloIndex, slo] of (catalog.slos ?? []).entries()) {
  const sloLabel = slo?.id || `catalog.slos[${sloIndex}]`;
  const queries = [
    ...(Array.isArray(slo.objective_queries) ? slo.objective_queries : []),
    ...(Array.isArray(slo.alert_queries) ? slo.alert_queries : []),
  ];

  for (const [queryIndex, query] of queries.entries()) {
    const expression = query?.expression;
    if (typeof expression !== 'string' || expression.trim() === '') {
      continue;
    }
    const references = expression.match(referencePattern) ?? [];
    for (const reference of references) {
      referencedSeries.add(reference);
      if (!resolveReference(reference)) {
        fail(
          `${sloLabel} query ${query?.name || queryIndex} references undeclared series/table ${reference}.`,
        );
      }
    }
  }
}

for (const [name, declaration] of declaredSeries.entries()) {
  const directlyReferenced = referencedSeries.has(name);
  const histogramReferenced =
    declaration.type === 'histogram' &&
    ['_bucket', '_sum', '_count', '_created'].some((suffix) =>
      referencedSeries.has(`${name}${suffix}`),
    );
  if (!directlyReferenced && !histogramReferenced) {
    warnings.push(
      `${name} (${declaration.sourceKind}) is declared but is not referenced by an objective or alert query.`,
    );
  }
}

if (!requirePass && (sourceStatusCounts.get('measured') ?? 0) === 0) {
  warnings.push(
    'No derived SLO series is measured yet; exact-candidate evidence is still required.',
  );
}

console.log(
  `Validated ${declaredSeries.size} unique raw/derived SLO series and ${referencedSeries.size} query references.`,
);
console.log(
  `Derived source states: ${[...sourceStatusCounts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([status, count]) => `${status}=${count}`)
    .join(', ') || 'none'}.`,
);

for (const warning of warnings) {
  console.warn(`WARN: ${warning}`);
}

if (errors.length > 0) {
  console.error(`\nSLO series validation failed with ${errors.length} error(s):`);
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exitCode = 1;
} else {
  console.log(
    requirePass
      ? 'All raw and derived SLO series are structurally valid and evidence-complete.'
      : 'All SLO query references resolve to declared low-cardinality series.',
  );
}
