#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, '..');
const requirePass = process.argv.includes('--require-pass');

const paths = {
  contract: resolve(
    repoRoot,
    'docs/production/managed-public-beta-service-contract-v0.1.md',
  ),
  sloCatalog: resolve(
    repoRoot,
    'docs/production/managed-public-beta-slos.json',
  ),
  matrix: resolve(
    repoRoot,
    'docs/security/production-safety-release-gate.json',
  ),
  gateDoc: resolve(
    repoRoot,
    'docs/security/production-safety-release-gate.md',
  ),
  incidentRunbook: resolve(
    repoRoot,
    'docs/operations/managed-beta-incident-runbook.md',
  ),
  communicationTemplates: resolve(
    repoRoot,
    'docs/operations/managed-beta-communication-templates.md',
  ),
  evidenceTemplate: resolve(
    repoRoot,
    'docs/security/production-gate-evidence-template.md',
  ),
};

const [
  contract,
  sloCatalogText,
  matrixText,
  gateDoc,
  incidentRunbook,
  communicationTemplates,
  evidenceTemplate,
] = await Promise.all([
  readFile(paths.contract, 'utf8'),
  readFile(paths.sloCatalog, 'utf8'),
  readFile(paths.matrix, 'utf8'),
  readFile(paths.gateDoc, 'utf8'),
  readFile(paths.incidentRunbook, 'utf8'),
  readFile(paths.communicationTemplates, 'utf8'),
  readFile(paths.evidenceTemplate, 'utf8'),
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

const sloCatalog = parseJson('SLO catalog', sloCatalogText);
const matrix = parseJson('Gate matrix', matrixText);

function requireText(documentName, text, requiredValues) {
  for (const required of requiredValues) {
    if (!text.includes(required)) {
      fail(`${documentName} is missing required text: ${required}`);
    }
  }
}

function requireNonEmptyString(object, field, label) {
  if (typeof object?.[field] !== 'string' || object[field].trim().length === 0) {
    fail(`${label}.${field} must be a non-empty string.`);
    return false;
  }
  return true;
}

requireText('Service contract', contract, [
  '# Managed Fiducia Public Beta Service Contract v0.1',
  '## 1. Product boundary',
  '## 2. Deployment and availability truth',
  '## 3. Tenant and data boundaries',
  '## 4. Consistency and operation semantics',
  '## 5. Launch SLOs and indicators',
  '## 6. Recovery objectives',
  '## 8. Support and incident model',
  '## 9. Maintenance and change policy',
  '## 11. Launch exceptions',
  '## 12. Approval and evidence',
  'not a contractual SLA',
  'synthetic provider names are placement/test labels only',
  'fencing token',
  'clean-room',
  'independently of Fiducia',
  'Sev-0',
  '99.5%',
  '60-second positive introspection cache',
]);

requireText('Human-readable gate document', gateDoc, [
  'Fail closed: no real-user launch',
  '## 3. Trust boundaries and route classes',
  '## 5. Non-negotiable invariants',
  '## 6. Evidence requirements',
  'node tools/validate-production-gates.mjs --require-pass',
]);

requireText('Incident runbook', incidentRunbook, [
  '# Managed Fiducia Beta Incident Runbook',
  '## 2. Automatic stop and containment conditions',
  'Safety takes priority over availability.',
  '## 3. First 15 minutes',
  '## 5. Incident-specific containment playbooks',
  'one healthy follower at a time',
  '## 8. Required pre-launch tabletop',
]);

requireText('Communication templates', communicationTemplates, [
  '# Managed Fiducia Beta Communication Templates',
  '## 1. Initial incident notice',
  '## 4. Resolution notice',
  '## 5. Planned maintenance notice',
  '## 6. Emergency maintenance notice',
  '**Next update:**',
  'calling engineering SLO targets contractual SLAs',
]);

requireText('Evidence bundle template', evidenceTemplate, [
  '# Fiducia Production Gate Evidence Bundle Template',
  '## 1. Candidate identity',
  '## 3. Service contract measurements',
  '## 4. Safety matrix execution',
  '## 8. Backup, clean-room restore, and rollback proof',
  '## 11. Final decision',
  'node tools/validate-production-gates.mjs --require-pass',
]);

const requiredSloIds = [
  'SLO-AVAIL-01',
  'SLO-SUCCESS-01',
  'SLO-READ-01',
  'SLO-WRITE-01',
  'SLO-RENEW-01',
  'SLO-FAILOVER-01',
  'SLO-REVOKE-01',
  'SLO-SECRET-01',
  'SLO-WATCH-01',
  'SLO-SUPPORT-01',
];
for (const sloId of requiredSloIds) {
  if (!contract.includes(`\`${sloId}\``)) {
    fail(`Service contract is missing required SLO ID ${sloId}.`);
  }
  if (!evidenceTemplate.includes(`\`${sloId}\``)) {
    fail(`Evidence bundle template is missing required SLO ID ${sloId}.`);
  }
}

// ---------------------------------------------------------------------------
// Machine-readable SLO contract.
// ---------------------------------------------------------------------------

if (sloCatalog.schema_version !== 1) {
  fail(
    `Expected SLO catalog schema_version 1, received ${String(sloCatalog.schema_version)}.`,
  );
}
if (sloCatalog.contract_issue !== 'DEN-1390') {
  fail(
    `Expected SLO catalog contract_issue DEN-1390, received ${String(sloCatalog.contract_issue)}.`,
  );
}

const allowedSourceStatuses = new Set(sloCatalog.allowed_source_statuses ?? []);
for (const status of ['specified', 'instrumented', 'queryable', 'measured']) {
  if (!allowedSourceStatuses.has(status)) {
    fail(`SLO allowed_source_statuses is missing ${status}.`);
  }
}

const allowedGroupingLabels = new Set(
  sloCatalog.label_policy?.allowed_grouping_labels ?? [],
);
const forbiddenCustomerLabels = new Set(
  sloCatalog.label_policy?.forbidden_customer_labels ?? [],
);
if (allowedGroupingLabels.size === 0) {
  fail('SLO label policy must declare allowed_grouping_labels.');
}
if (forbiddenCustomerLabels.size === 0) {
  fail('SLO label policy must declare forbidden_customer_labels.');
}
for (const label of allowedGroupingLabels) {
  if (forbiddenCustomerLabels.has(label)) {
    fail(`SLO label ${label} cannot be both allowed and forbidden.`);
  }
}

const slos = Array.isArray(sloCatalog.slos) ? sloCatalog.slos : [];
if (slos.length !== requiredSloIds.length) {
  fail(
    `Expected exactly ${requiredSloIds.length} SLO catalog entries; found ${slos.length}.`,
  );
}

const seenSloIds = new Set();
const sloStatusCounts = new Map();
const requiredSloFields = [
  'id',
  'name',
  'objective',
  'source_type',
  'source_status',
  'source_series',
  'objective_queries',
  'alert_queries',
  'owner',
  'review_cadence',
  'blocking_issues',
  'evidence',
];

for (const [index, slo] of slos.entries()) {
  const label = slo?.id || `SLO row ${index + 1}`;
  for (const field of requiredSloFields) {
    if (!(field in (slo ?? {}))) {
      fail(`${label} is missing field ${field}.`);
    }
  }
  for (const field of [
    'id',
    'name',
    'objective',
    'source_type',
    'source_status',
    'owner',
    'review_cadence',
  ]) {
    requireNonEmptyString(slo, field, label);
  }

  if (!requiredSloIds.includes(slo.id)) {
    fail(`${label} is not one of the contract's required SLO IDs.`);
  }
  if (seenSloIds.has(slo.id)) {
    fail(`Duplicate SLO catalog id ${slo.id}.`);
  }
  seenSloIds.add(slo.id);

  if (!allowedSourceStatuses.has(slo.source_status)) {
    fail(`${label} uses unsupported source_status ${String(slo.source_status)}.`);
  }
  sloStatusCounts.set(
    slo.source_status,
    (sloStatusCounts.get(slo.source_status) ?? 0) + 1,
  );

  if (!Array.isArray(slo.source_series) || slo.source_series.length === 0) {
    fail(`${label}.source_series must be a non-empty array.`);
  } else {
    const seenSeries = new Set();
    for (const [seriesIndex, series] of slo.source_series.entries()) {
      const seriesLabel = `${label}.source_series[${seriesIndex}]`;
      for (const field of ['name', 'type', 'producer', 'eligibility']) {
        requireNonEmptyString(series, field, seriesLabel);
      }
      if (seenSeries.has(series.name)) {
        fail(`${label} repeats source series ${series.name}.`);
      }
      seenSeries.add(series.name);
      if (
        !Array.isArray(series.required_labels) ||
        series.required_labels.length === 0
      ) {
        fail(`${seriesLabel}.required_labels must be a non-empty array.`);
      } else {
        for (const metricLabel of series.required_labels) {
          if (!allowedGroupingLabels.has(metricLabel)) {
            fail(
              `${seriesLabel} uses undeclared/high-cardinality label ${String(metricLabel)}.`,
            );
          }
          if (forbiddenCustomerLabels.has(metricLabel)) {
            fail(`${seriesLabel} uses forbidden customer label ${metricLabel}.`);
          }
        }
      }
    }
  }

  if (!Array.isArray(slo.objective_queries) || slo.objective_queries.length === 0) {
    fail(`${label}.objective_queries must be a non-empty array.`);
  } else {
    for (const [queryIndex, query] of slo.objective_queries.entries()) {
      const queryLabel = `${label}.objective_queries[${queryIndex}]`;
      for (const field of ['name', 'language', 'expression', 'pass_condition']) {
        requireNonEmptyString(query, field, queryLabel);
      }
      if (!['promql', 'sql'].includes(query.language)) {
        fail(`${queryLabel}.language must be promql or sql.`);
      }
    }
  }

  if (!Array.isArray(slo.alert_queries) || slo.alert_queries.length === 0) {
    fail(`${label}.alert_queries must be a non-empty array.`);
  } else {
    for (const [alertIndex, alert] of slo.alert_queries.entries()) {
      const alertLabel = `${label}.alert_queries[${alertIndex}]`;
      for (const field of ['name', 'severity', 'for', 'expression']) {
        requireNonEmptyString(alert, field, alertLabel);
      }
      if (!['warning', 'critical'].includes(alert.severity)) {
        fail(`${alertLabel}.severity must be warning or critical.`);
      }
      if (!/^\d+(ms|s|m|h|d)$/.test(alert.for ?? '')) {
        fail(`${alertLabel}.for must be a duration such as 0m, 5m, or 1h.`);
      }
    }
  }

  if (!Array.isArray(slo.blocking_issues) || slo.blocking_issues.length === 0) {
    fail(`${label}.blocking_issues must be a non-empty array.`);
  } else {
    for (const issue of slo.blocking_issues) {
      if (!/^DEN-\d+$/.test(issue)) {
        fail(`${label} has malformed blocking issue ${String(issue)}.`);
      }
    }
  }

  if (!Array.isArray(slo.evidence)) {
    fail(`${label}.evidence must be an array.`);
  }

  if (slo.source_status === 'measured' && slo.evidence?.length === 0) {
    fail(`${label} is measured but has no exact-candidate evidence.`);
  }
  if (requirePass && slo.source_status !== 'measured') {
    fail(`${label} source_status is ${slo.source_status}; certification requires measured.`);
  }
  if (requirePass && slo.evidence?.length === 0) {
    fail(`${label} has no exact-candidate measurement evidence.`);
  }
}

for (const requiredSloId of requiredSloIds) {
  if (!seenSloIds.has(requiredSloId)) {
    fail(`SLO catalog is missing ${requiredSloId}.`);
  }
}

if (!requirePass && (sloStatusCounts.get('measured') ?? 0) === 0) {
  warnings.push(
    'No SLO sources are measured yet. The query contracts are defined, but exact-candidate telemetry evidence is still required.',
  );
}

// ---------------------------------------------------------------------------
// Machine-readable adversarial safety gate.
// ---------------------------------------------------------------------------

if (matrix.schema_version !== 1) {
  fail(`Expected gate schema_version 1, received ${String(matrix.schema_version)}.`);
}
if (matrix.gate_issue !== 'DEN-1391') {
  fail(`Expected gate_issue DEN-1391, received ${String(matrix.gate_issue)}.`);
}
if (matrix.service_contract_issue !== 'DEN-1390') {
  fail(
    `Expected service_contract_issue DEN-1390, received ${String(matrix.service_contract_issue)}.`,
  );
}

const requiredRouteClasses = new Set(matrix.required_route_classes ?? []);
const allowedStatuses = new Set(matrix.allowed_statuses ?? []);
const expectedStatuses = new Set([
  'not_started',
  'automated',
  'live_evidence_pending',
  'passed',
  'accepted_risk',
  'failed',
]);

for (const status of expectedStatuses) {
  if (!allowedStatuses.has(status)) {
    fail(`allowed_statuses is missing ${status}.`);
  }
}

const automaticNoGo = new Set(matrix.automatic_no_go_invariants ?? []);
for (const invariant of [
  'cross_tenant_access',
  'credential_plane_bypass',
  'secret_disclosure',
  'stale_fencing_accepted',
  'committed_state_regression',
  'unrecoverable_authoritative_state',
  'mutable_or_unverified_production_artifact',
  'missing_accountable_operator',
]) {
  if (!automaticNoGo.has(invariant)) {
    fail(`automatic_no_go_invariants is missing ${invariant}.`);
  }
}
const declaredInvariants = new Set([
  ...automaticNoGo,
  ...(matrix.waivable_invariants ?? []),
]);

const tests = Array.isArray(matrix.tests) ? matrix.tests : [];
if (tests.length < 20) {
  fail(`Expected at least 20 adversarial test rows; found ${tests.length}.`);
}

const requiredFields = [
  'id',
  'area',
  'route_class',
  'surface',
  'threat',
  'invariant',
  'test',
  'automation_target',
  'evidence_required',
  'owner',
  'blockers',
  'status',
  'evidence',
];
const seenIds = new Set();
const coveredRoutes = new Set();
const statusCounts = new Map();

for (const [index, row] of tests.entries()) {
  const label = row?.id || `row ${index + 1}`;

  for (const field of requiredFields) {
    if (!(field in row)) {
      fail(`${label} is missing field ${field}.`);
      continue;
    }
    if (typeof row[field] === 'string' && row[field].trim().length === 0) {
      fail(`${label}.${field} must not be empty.`);
    }
  }

  if (!/^[A-Z]+-\d{3}$/.test(row.id ?? '')) {
    fail(`${label} must use a stable ID such as AUTH-001.`);
  }
  if (seenIds.has(row.id)) {
    fail(`Duplicate test id ${row.id}.`);
  }
  seenIds.add(row.id);

  if (!requiredRouteClasses.has(row.route_class)) {
    fail(`${label} uses undeclared route_class ${row.route_class}.`);
  } else {
    coveredRoutes.add(row.route_class);
  }

  if (!allowedStatuses.has(row.status)) {
    fail(`${label} uses unsupported status ${row.status}.`);
  }
  statusCounts.set(row.status, (statusCounts.get(row.status) ?? 0) + 1);

  if (!declaredInvariants.has(row.invariant)) {
    fail(`${label} uses undeclared invariant ${row.invariant}.`);
  }

  if (!Array.isArray(row.blockers)) {
    fail(`${label}.blockers must be an array.`);
  } else {
    for (const blocker of row.blockers) {
      if (!/^DEN-\d+$/.test(blocker)) {
        fail(`${label} has malformed Linear blocker ${String(blocker)}.`);
      }
    }
  }

  if (!Array.isArray(row.evidence)) {
    fail(`${label}.evidence must be an array.`);
  }

  if (row.status === 'passed' && (!row.evidence || row.evidence.length === 0)) {
    fail(`${label} is passed but has no durable evidence reference.`);
  }

  if (row.status === 'failed') {
    fail(`${label} is failed; the production gate is closed.`);
  }

  if (row.status === 'accepted_risk') {
    const riskFields = [
      'risk_owner',
      'risk_rationale',
      'risk_expires_on',
      'containment_plan',
      'remediation_issue',
      'risk_reviewer',
    ];
    for (const field of riskFields) {
      if (typeof row[field] !== 'string' || row[field].trim().length === 0) {
        fail(`${label} accepted_risk is missing ${field}.`);
      }
    }
    if (!/^\d{4}-\d{2}-\d{2}$/.test(row.risk_expires_on ?? '')) {
      fail(`${label}.risk_expires_on must be YYYY-MM-DD.`);
    }
    if (!/^DEN-\d+$/.test(row.remediation_issue ?? '')) {
      fail(`${label}.remediation_issue must be a Linear issue identifier.`);
    }
    if (automaticNoGo.has(row.invariant)) {
      fail(
        `${label} covers automatic no-go invariant ${row.invariant} and cannot be accepted_risk.`,
      );
    }
  }

  if (requirePass && row.status !== 'passed') {
    fail(`${label} is ${row.status}; release certification requires passed.`);
  }
}

for (const routeClass of requiredRouteClasses) {
  if (!coveredRoutes.has(routeClass)) {
    fail(`Required route class ${routeClass} has no adversarial test row.`);
  }
}

const referencedIssues = new Set(
  tests.flatMap((row) => (Array.isArray(row.blockers) ? row.blockers : [])),
);
for (const mustReference of [
  'DEN-78',
  'DEN-252',
  'DEN-253',
  'DEN-254',
  'DEN-433',
  'DEN-437',
  'DEN-438',
  'DEN-1241',
  'DEN-1243',
  'DEN-1244',
  'DEN-332',
  'DEN-373',
  'DEN-946',
]) {
  if (!referencedIssues.has(mustReference)) {
    fail(`Gate matrix does not consume required blocker ${mustReference}.`);
  }
}

if (!requirePass && (statusCounts.get('passed') ?? 0) === 0) {
  warnings.push(
    'No safety rows are passed yet. Structural validation is green, but the release gate remains open and --require-pass will fail.',
  );
}

console.log(
  `Validated ${slos.length} SLO definitions; source states: ${[...sloStatusCounts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([status, count]) => `${status}=${count}`)
    .join(', ') || 'none'}.`,
);
console.log(
  `Validated ${tests.length} production-gate rows across ${coveredRoutes.size}/${requiredRouteClasses.size} required route classes.`,
);
console.log(
  `Safety statuses: ${[...statusCounts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([status, count]) => `${status}=${count}`)
    .join(', ') || 'none'}`,
);
console.log('Validated the incident, communication, and evidence controls.');

for (const warning of warnings) {
  console.warn(`WARN: ${warning}`);
}

if (errors.length > 0) {
  console.error(`\nProduction gate validation failed with ${errors.length} error(s):`);
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exitCode = 1;
} else {
  console.log(
    requirePass
      ? 'Production release gate and exact-candidate SLO evidence are complete.'
      : 'Production contract, SLO catalog, operations controls, and gate schema are structurally valid.',
  );
}
