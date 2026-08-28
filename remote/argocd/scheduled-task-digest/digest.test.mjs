import assert from 'node:assert/strict';
import test from 'node:test';

import {
  CRITICAL_GITHUB_TASKS,
  classifyGithubRun,
  cronMatches,
  dueInstants,
  evaluateCriticalGithubTask,
  evaluateKubernetesCronJob,
  fetchRepositoryScheduleRuns,
  parseCronExpression,
  redact,
  renderDigest,
} from './digest.mjs';
import { deliveryDecision } from './main.mjs';

function githubRun(overrides = {}) {
  return {
    repository: 'ORESoftware/ai-agent-coordinator.rs',
    id: 1,
    name: 'Workflow',
    workflowPath: '.github/workflows/nightly-artifact-recovery.yml',
    status: 'completed',
    conclusion: 'success',
    createdAt: '2026-08-12T08:14:33Z',
    updatedAt: '2026-08-12T08:15:00Z',
    htmlUrl: 'https://github.com/ORESoftware/ai-agent-coordinator.rs/actions/runs/1',
    runNumber: 1,
    runAttempt: 1,
    ...overrides,
  };
}

function githubJob(name, conclusion) {
  return { id: 1, name, status: 'completed', conclusion, startedAt: null, completedAt: null };
}

function cronJob({ name = 'example', schedule = '0 1 * * *', timeZone = 'America/Chicago', suspend = false, successHistory = 3, failedHistory = 1 } = {}) {
  return {
    metadata: { namespace: 'default', name, annotations: {} },
    spec: {
      schedule,
      timeZone,
      suspend,
      successfulJobsHistoryLimit: successHistory,
      failedJobsHistoryLimit: failedHistory,
    },
  };
}

function job({ owner = 'example', name = 'example-1', createdAt = '2026-08-12T06:00:05Z', state = 'SUCCESS' } = {}) {
  const conditions = state === 'SUCCESS'
    ? [{ type: 'Complete', status: 'True' }]
    : state === 'FAILED'
      ? [{ type: 'Failed', status: 'True' }]
      : [];
  return {
    metadata: {
      namespace: 'default',
      name,
      creationTimestamp: createdAt,
      ownerReferences: [{ kind: 'CronJob', name: owner }],
      labels: {},
    },
    status: {
      active: state === 'RUNNING' ? 1 : 0,
      conditions,
      startTime: createdAt,
      completionTime: state === 'RUNNING' ? null : createdAt,
    },
  };
}

test('07:00 America/Chicago remains local-time correct across DST transitions', () => {
  const parsed = parseCronExpression('0 7 * * *');
  assert.equal(cronMatches(parsed, new Date('2026-03-07T13:00:00Z'), 'America/Chicago'), true);
  assert.equal(cronMatches(parsed, new Date('2026-03-08T12:00:00Z'), 'America/Chicago'), true);
  assert.equal(cronMatches(parsed, new Date('2026-03-08T13:00:00Z'), 'America/Chicago'), false);
  assert.equal(cronMatches(parsed, new Date('2026-10-31T12:00:00Z'), 'America/Chicago'), true);
  assert.equal(cronMatches(parsed, new Date('2026-11-01T13:00:00Z'), 'America/Chicago'), true);
});

test('24-hour cron windows are half-open and do not double-count the boundary', () => {
  const due = dueInstants(
    '0 * * * *',
    'America/Chicago',
    new Date('2026-08-11T12:00:00Z'),
    new Date('2026-08-12T12:00:00Z'),
  );
  assert.equal(due.length, 24);
  assert.equal(due.at(-1), '2026-08-12T12:00:00.000Z');
});



test('GitHub pagination remains on api.github.com and includes all pages', async () => {
  const page2 = 'https://api.github.com/repos/ORESoftware/ai-agent-coordinator.rs/actions/runs?event=schedule&per_page=100&page=2';
  let calls = 0;
  const fetchImpl = async (url) => {
    calls += 1;
    const first = !String(url).includes('page=2');
    return new Response(JSON.stringify({
      workflow_runs: [{
        id: first ? 1 : 2,
        name: 'Workflow',
        path: '.github/workflows/example.yml',
        status: 'completed',
        conclusion: 'success',
        created_at: first ? '2026-08-12T08:00:00Z' : '2026-08-12T09:00:00Z',
        updated_at: first ? '2026-08-12T08:01:00Z' : '2026-08-12T09:01:00Z',
        html_url: `https://github.com/ORESoftware/ai-agent-coordinator.rs/actions/runs/${first ? 1 : 2}`,
      }],
    }), {
      status: 200,
      headers: first ? { Link: `<${page2}>; rel="next"` } : {},
    });
  };
  const result = await fetchRepositoryScheduleRuns(
    'ORESoftware/ai-agent-coordinator.rs',
    new Date('2026-08-11T12:00:00Z'),
    { fetchImpl },
  );
  assert.equal(calls, 2);
  assert.equal(result.runs.length, 2);
  assert.equal(result.runs[0].id, 2);
});

test('green GitHub workflow with skipped execution is classified false green', () => {
  const task = CRITICAL_GITHUB_TASKS.find((item) => item.id === 'nightly-artifact-recovery');
  const result = classifyGithubRun(task, githubRun(), {
    jobs: [githubJob('validate', 'success'), githubJob('enqueue', 'skipped')],
    incomplete: false,
  });
  assert.equal(result.status, 'FALSE_GREEN');
  assert.match(result.reason, /skipped/i);
});

test('organization maintenance requires every maintain matrix job to succeed', () => {
  const task = CRITICAL_GITHUB_TASKS.find((item) => item.id === 'nightly-org-maintenance');
  const run = githubRun({ workflowPath: task.workflowPath });
  const result = classifyGithubRun(task, run, {
    jobs: [githubJob('maintain (ORESoftware)', 'success'), githubJob('maintain (shared-auth)', 'failure')],
    incomplete: false,
  });
  assert.equal(result.status, 'FAILED');
});

test('due critical workflow with no schedule-event evidence is missed', async () => {
  const task = CRITICAL_GITHUB_TASKS.find((item) => item.id === 'recent-chat-reconciliation');
  const result = await evaluateCriticalGithubTask(task, [], new Date('2026-08-12T12:00:00Z'));
  assert.equal(result.status, 'MISSED');
  assert.equal(result.expectedRuns, 1);
});

test('Kubernetes CronJob outcomes distinguish success, failure, missed, and history-limited evidence', () => {
  const start = new Date('2026-08-11T12:05:00Z');
  const end = new Date('2026-08-12T12:05:00Z');

  assert.equal(evaluateKubernetesCronJob(cronJob(), [job()], start, end).status, 'SUCCESS');
  assert.equal(evaluateKubernetesCronJob(cronJob(), [job({ state: 'FAILED' })], start, end).status, 'FAILED');
  assert.equal(evaluateKubernetesCronJob(cronJob(), [], start, end).status, 'MISSED');

  const hourly = cronJob({ schedule: '0 * * * *', successHistory: 3, failedHistory: 1 });
  const retained = [
    job({ name: 'example-1', createdAt: '2026-08-12T10:00:05Z' }),
    job({ name: 'example-2', createdAt: '2026-08-12T11:00:05Z' }),
    job({ name: 'example-3', createdAt: '2026-08-12T12:00:05Z' }),
  ];
  const historyLimited = evaluateKubernetesCronJob(hourly, retained, start, end);
  assert.equal(historyLimited.status, 'OBSERVED_SUCCESS');
  assert.match(historyLimited.reason, /history limits/i);
});

test('delivery decision suppresses both claimed and sent logical dates', () => {
  for (const state of ['claimed', 'sent']) {
    const lease = { metadata: { annotations: { 'dd.dev/logical-date': '2026-08-12', 'dd.dev/state': state } } };
    assert.equal(deliveryDecision(lease, '2026-08-12').action, 'suppress');
  }
  assert.equal(deliveryDecision(null, '2026-08-12').action, 'claim');
  assert.equal(deliveryDecision(null, '2026-08-12', true).action, 'send');
});

test('rendering creates one consolidated text and HTML email without credential material', () => {
  const secret = `ghp_${'A'.repeat(40)}`;
  const report = {
    generatedAt: '2026-08-12T12:00:00Z',
    windowStart: '2026-08-11T12:00:00Z',
    windowEnd: '2026-08-12T12:00:00Z',
    criticalGithub: [{
      id: 'x', name: `task ${secret}`, schedule: 'daily', status: 'FAILED', reason: `failure ${secret}`,
      expectedRuns: 1, attempts: [{ status: 'FAILED', evidence: { runId: 1, createdAt: '2026-08-12T07:00:00Z', executionJobs: [], htmlUrl: null } }],
    }],
    observedGithub: [],
    observedGithubOverflow: 0,
    kubernetes: [{ namespace: 'default', name: 'nightly', schedule: '0 1 * * *', timeZone: 'America/Chicago', status: 'SUCCESS', expectedRuns: 1, observedRuns: 1, reason: 'success', jobs: [] }],
    kubernetesOverflow: 0,
    externalCoverage: [],
    sourceErrors: [],
    summary: {
      githubCritical: { FAILED: 1 },
      githubObserved: {},
      kubernetes: { SUCCESS: 1 },
      externalUnverified: 0,
      sourceErrors: 0,
    },
  };
  const rendered = renderDigest(report, 'scheduled');
  assert.match(rendered.subject, /^\[Scheduled task digest\]/);
  assert.match(rendered.text, /Critical GitHub scheduled workflows/);
  assert.match(rendered.text, /Kubernetes CronJobs/);
  assert.match(rendered.html, /<table/);
  assert.doesNotMatch(rendered.text, new RegExp(secret));
  assert.doesNotMatch(rendered.html, new RegExp(secret));
  assert.match(redact(secret), /\[REDACTED\]/);
});
