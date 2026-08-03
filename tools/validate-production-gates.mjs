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
  matrixText,
  gateDoc,
  incidentRunbook,
  communicationTemplates,
  evidenceTemplate,
] = await Promise.all([
  readFile(paths.contract, 'utf8'),
  readFile(paths.matrix, 'utf8'),
  readFile(paths.gateDoc, 'utf8'),
  readFile(paths.incidentRunbook, 'utf8'),
  readFile(paths.communicationTemplates, 'utf8'),
  readFile(paths.evidenceTemplate, 'utf8'),
]);

const errors = [];
const warnings = [];
const fail = (message) => errors.push(message);

let matrix;
try {
  matrix = JSON.parse(matrixText);
} catch (error) {
  fail(`Gate matrix is not valid JSON: ${error.message}`);
  matrix = {};
}

function requireText(documentName, text, requiredValues) {
  for (const required of requiredValues) {
    if (!text.includes(required)) {
      fail(`${documentName} is missing required text: ${required}`);
    }
  }
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

if (matrix.schema_version !== 1) {
  fail(`Expected schema_version 1, received ${String(matrix.schema_version)}.`);
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

  if (!automaticNoGo.has(row.invariant)) {
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
    'No rows are passed yet. Structural validation is green, but the release gate remains open and --require-pass will fail.',
  );
}

console.log(
  `Validated ${tests.length} production-gate rows across ${coveredRoutes.size}/${requiredRouteClasses.size} required route classes.`,
);
console.log(
  `Validated ${requiredSloIds.length} SLO IDs plus the incident, communication, and evidence controls.`,
);
console.log(
  `Statuses: ${[...statusCounts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([status, count]) => `${status}=${count}`)
    .join(', ') || 'none'}`,
);

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
      ? 'Production release gate is evidence-complete.'
      : 'Production contract, operations controls, and gate schema are structurally valid.',
  );
}
