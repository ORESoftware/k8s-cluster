import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const SOURCE_URL = new URL('../ScheduledTaskDigest.gs', import.meta.url);
const API_BASE = 'https://api.github.com';
const CONTROL_REPO = 'ORESoftware/ai-agent-coordinator.rs';

function signedBytes(buffer) {
  return [...buffer].map((byte) => (byte > 127 ? byte - 256 : byte));
}

function dateParts(date, timeZone) {
  const formatter = new Intl.DateTimeFormat('en-US', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    weekday: 'short',
    timeZoneName: 'shortOffset',
    hourCycle: 'h23',
  });
  const parts = Object.fromEntries(
    formatter.formatToParts(date).map((part) => [part.type, part.value]),
  );
  return parts;
}

function formatDate(date, timeZone, pattern) {
  const parts = dateParts(date, timeZone);
  if (pattern === 'yyyy-MM-dd') return `${parts.year}-${parts.month}-${parts.day}`;
  if (pattern === 'yyyy-MM-dd HH:mm z') {
    return `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute} ${parts.timeZoneName}`;
  }
  if (pattern === 'EEE') return parts.weekday;
  if (pattern === 'H') return String(Number(parts.hour));
  if (pattern === 'yyyyMMdd-HHmmss') {
    return `${parts.year}${parts.month}${parts.day}-${parts.hour}${parts.minute}${parts.second}`;
  }
  throw new Error(`Unsupported test format pattern: ${pattern}`);
}

function response(body, { status = 200, headers = {} } = {}) {
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  return {
    getResponseCode() {
      return status;
    },
    getContentText() {
      return text;
    },
    getAllHeaders() {
      return structuredClone(headers);
    },
    getHeaders() {
      return structuredClone(headers);
    },
  };
}

function runFixture({
  id,
  name,
  path,
  conclusion,
  createdAt,
  status = 'completed',
  repository = CONTROL_REPO,
}) {
  return {
    id,
    name,
    path,
    status,
    conclusion,
    created_at: createdAt,
    updated_at: createdAt,
    html_url: `https://github.com/${repository}/actions/runs/${id}`,
    run_number: id,
    run_attempt: 1,
    head_branch: 'main',
  };
}

function jobFixture({ id, name, conclusion, status = 'completed' }) {
  return {
    id,
    name,
    status,
    conclusion,
    started_at: '2026-08-12T07:18:40Z',
    completed_at: '2026-08-12T07:19:00Z',
  };
}

function runsUrl(repository, since = null) {
  const prefix = `${API_BASE}/repos/${repository}/actions/runs?event=schedule&per_page=100&created=`;
  return since ? prefix + encodeURIComponent(`>=${since}`) : prefix;
}

function createHarness({ route = null, preexistingTriggers = [] } = {}) {
  const properties = new Map();
  const triggers = preexistingTriggers.map((handler, index) => ({
    id: `preexisting-${index}`,
    handler,
    getHandlerFunction() {
      return this.handler;
    },
  }));
  const triggerBuilds = [];
  const fetchCalls = [];
  const sentEmails = [];
  let uuidCounter = 0;

  function defaultRoute(url) {
    if (url === `${API_BASE}/repos/${CONTROL_REPO}`) {
      return response({ full_name: CONTROL_REPO });
    }
    if (url.includes('/actions/runs?')) {
      return response({ total_count: 0, workflow_runs: [] });
    }
    if (url.includes('/jobs?')) {
      return response({ total_count: 0, jobs: [] });
    }
    throw new Error(`No route fixture for ${url}`);
  }

  const scriptProperties = {
    getProperty(key) {
      return properties.has(key) ? properties.get(key) : null;
    },
    setProperty(key, value) {
      properties.set(key, String(value));
      return this;
    },
    deleteProperty(key) {
      properties.delete(key);
      return this;
    },
  };

  const context = vm.createContext({
    console: { log() {}, warn() {}, error() {} },
    structuredClone,
    Date,
    JSON,
    Object,
    Array,
    String,
    Number,
    Boolean,
    Math,
    RegExp,
    Error,
    encodeURIComponent,
    decodeURIComponent,
    PropertiesService: {
      getScriptProperties() {
        return scriptProperties;
      },
    },
    LockService: {
      getScriptLock() {
        return {
          waitLock() {},
          tryLock() {
            return true;
          },
          releaseLock() {},
        };
      },
    },
    ScriptApp: {
      getProjectTriggers() {
        return triggers.slice();
      },
      deleteTrigger(trigger) {
        const index = triggers.indexOf(trigger);
        if (index >= 0) triggers.splice(index, 1);
      },
      newTrigger(handler) {
        const build = {
          handler,
          hour: null,
          minute: null,
          days: null,
          timeZone: null,
        };
        triggerBuilds.push(build);
        const builder = {
          timeBased() {
            return this;
          },
          atHour(hour) {
            build.hour = hour;
            return this;
          },
          nearMinute(minute) {
            build.minute = minute;
            return this;
          },
          everyDays(days) {
            build.days = days;
            return this;
          },
          inTimezone(timeZone) {
            build.timeZone = timeZone;
            return this;
          },
          create() {
            const trigger = {
              id: `trigger-${triggers.length + 1}`,
              handler,
              getHandlerFunction() {
                return this.handler;
              },
            };
            triggers.push(trigger);
            return trigger;
          },
        };
        return builder;
      },
    },
    UrlFetchApp: {
      fetch(url, options) {
        fetchCalls.push({ url, options: structuredClone(options) });
        return (route || defaultRoute)(url, options, defaultRoute);
      },
    },
    MailApp: {
      getRemainingDailyQuota() {
        return 100;
      },
      sendEmail(message) {
        sentEmails.push(structuredClone(message));
      },
    },
    Utilities: {
      DigestAlgorithm: { SHA_256: 'SHA_256' },
      Charset: { UTF_8: 'UTF_8' },
      formatDate,
      getUuid() {
        uuidCounter += 1;
        return `uuid-${uuidCounter}`;
      },
      computeDigest(algorithm, value, charset) {
        assert.equal(algorithm, 'SHA_256');
        assert.equal(charset, 'UTF_8');
        return signedBytes(createHash('sha256').update(String(value), 'utf8').digest());
      },
    },
  });

  return {
    context,
    properties,
    triggers,
    triggerBuilds,
    fetchCalls,
    sentEmails,
    async load() {
      const source = await readFile(SOURCE_URL, 'utf8');
      vm.runInContext(source, context, { filename: 'ScheduledTaskDigest.gs' });
      return this;
    },
  };
}

test('bootstrap installs exactly one 07:00 America/Chicago trigger and proves GitHub/mail access', async () => {
  const harness = await createHarness().load();

  assert.equal(harness.context.bootstrapScheduledTaskDigest_().ok, true);
  assert.equal(harness.triggers.length, 1);
  assert.equal(harness.triggers[0].getHandlerFunction(), 'runScheduledTaskDigest');
  assert.deepEqual(harness.triggerBuilds, [
    {
      handler: 'runScheduledTaskDigest',
      hour: 7,
      minute: 0,
      days: 1,
      timeZone: 'America/Chicago',
    },
  ]);
  assert.equal(harness.fetchCalls[0].url, `${API_BASE}/repos/${CONTROL_REPO}`);
  assert.match(
    harness.properties.get('SCHEDULED_TASK_DIGEST_BOOTSTRAP_V1'),
    /"version":"1"/,
  );

  harness.context.ensureScheduledTaskDigestTrigger_();
  assert.equal(harness.triggers.length, 1);
  assert.equal(harness.triggerBuilds.length, 1);
});

test('duplicate preexisting digest triggers are reduced without touching unrelated triggers', async () => {
  const harness = await createHarness({
    preexistingTriggers: [
      'runScheduledTaskDigest',
      'runScheduledTaskDigest',
      'continueEmailGoogleChatExport',
    ],
  }).load();

  assert.deepEqual(
    harness.triggers.map((trigger) => trigger.getHandlerFunction()).sort(),
    ['continueEmailGoogleChatExport', 'runScheduledTaskDigest'],
  );
  assert.equal(harness.triggerBuilds.length, 0);
});

test('critical evidence distinguishes hard failure, false green, skipped work, missing work, and not-due weekly work', async () => {
  const windowEnd = new Date('2026-08-12T12:00:00.000Z'); // 07:00 Central
  const windowStart = new Date(windowEnd.getTime() - 24 * 60 * 60 * 1000).toISOString();
  const orgRun = runFixture({
    id: 31573435819,
    name: 'Nightly organization Codex maintenance',
    path: '.github/workflows/nightly-org-maintenance.yml',
    conclusion: 'failure',
    createdAt: '2026-08-12T07:18:37Z',
  });
  const artifactRun = runFixture({
    id: 31577452915,
    name: 'Nightly artifact recovery',
    path: '.github/workflows/nightly-artifact-recovery.yml',
    conclusion: 'success',
    createdAt: '2026-08-12T08:14:33Z',
  });
  const briefingRun = runFixture({
    id: 31493858395,
    name: 'Daily portfolio briefing',
    path: '.github/workflows/daily-portfolio-briefing.yml',
    conclusion: 'skipped',
    createdAt: '2026-08-11T12:58:57Z',
  });

  const harness = await createHarness({
    route(url, options, defaultRoute) {
      if (url === `${API_BASE}/repos/${CONTROL_REPO}`) return defaultRoute(url, options);
      if (url.startsWith(runsUrl(CONTROL_REPO))) {
        assert.ok(url.includes(encodeURIComponent(`>=${windowStart}`)));
        return response({
          total_count: 3,
          workflow_runs: [orgRun, artifactRun, briefingRun],
        });
      }
      if (url.startsWith(runsUrl('ORESoftware/k8s-cluster'))) {
        return response({ total_count: 0, workflow_runs: [] });
      }
      if (url.startsWith(runsUrl('ORESoftware/project-registry'))) {
        return response({ total_count: 0, workflow_runs: [] });
      }
      if (url.includes('/actions/runs/31573435819/jobs')) {
        return response({
          total_count: 2,
          jobs: [
            jobFixture({ id: 1, name: 'validate', conclusion: 'success' }),
            jobFixture({ id: 2, name: 'maintain (ORESoftware)', conclusion: 'failure' }),
          ],
        });
      }
      if (url.includes('/actions/runs/31577452915/jobs')) {
        return response({
          total_count: 2,
          jobs: [
            jobFixture({ id: 3, name: 'validate', conclusion: 'success' }),
            jobFixture({ id: 4, name: 'enqueue', conclusion: 'skipped' }),
          ],
        });
      }
      if (url.includes('/actions/runs/31493858395/jobs')) {
        return response({ total_count: 0, jobs: [] });
      }
      throw new Error(`Unexpected route ${url}`);
    },
  }).load();

  const report = harness.context.collectScheduledTaskDigest_(windowEnd);
  const byId = Object.fromEntries(report.critical.map((task) => [task.id, task]));

  assert.equal(byId['recent-chat-reconciliation'].status, 'MISSED');
  assert.equal(byId['nightly-org-maintenance'].status, 'FAILED');
  assert.equal(byId['nightly-artifact-recovery'].status, 'FALSE_GREEN');
  assert.equal(byId['daily-portfolio-briefing'].status, 'MISSED');
  assert.equal(byId['weekly-job-opportunity-digest'].status, 'NOT_DUE');
  assert.equal(report.summary.certifiedSuccess, 0);
  assert.equal(report.summary.failed, 1);
  assert.equal(report.summary.falseGreen, 1);
  assert.equal(report.summary.missed, 2);
  assert.equal(report.summary.unverified, 5);
  assert.equal(report.summary.notDue, 1);
});

test('one Central logical date sends exactly one consolidated email and redacts optional credentials', async () => {
  const harness = await createHarness().load();
  const secret = ['ghp', 'ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890'].join('_');
  harness.properties.set('SCHEDULED_TASK_DIGEST_GITHUB_TOKEN', secret);
  const now = new Date('2026-08-12T12:00:00.000Z');

  const first = harness.context.runScheduledTaskDigestAt_(now, false, 'scheduled');
  const second = harness.context.runScheduledTaskDigestAt_(now, false, 'scheduled');

  assert.equal(first.status, 'sent');
  assert.equal(second.status, 'duplicate_suppressed');
  assert.equal(harness.sentEmails.length, 1);
  const email = harness.sentEmails[0];
  assert.equal(email.to, 'alexander.d.mills@gmail.com');
  assert.match(email.subject, /^\[Scheduled task digest\] 2026-08-12/);
  assert.match(email.body, /Critical registered tasks/);
  assert.match(email.htmlBody, /<table/);
  assert.doesNotMatch(email.body, new RegExp(secret));
  assert.doesNotMatch(email.htmlBody, new RegExp(secret));
  assert.match(
    harness.properties.get('SCHEDULED_TASK_DIGEST_DELIVERY_V1'),
    /"state":"sent"/,
  );
});

test('manual canary is clearly labeled and does not consume the scheduled delivery key', async () => {
  const harness = await createHarness().load();
  const now = new Date('2026-08-12T12:00:00.000Z');

  const manual = harness.context.runScheduledTaskDigestAt_(now, true, 'manual-canary');
  assert.equal(manual.status, 'manual_canary_sent');
  assert.equal(harness.sentEmails.length, 1);
  assert.match(harness.sentEmails[0].subject, /^\[MANUAL CANARY\]/);
  assert.equal(harness.properties.has('SCHEDULED_TASK_DIGEST_DELIVERY_V1'), false);

  const scheduled = harness.context.runScheduledTaskDigestAt_(now, false, 'scheduled');
  assert.equal(scheduled.status, 'sent');
  assert.equal(harness.sentEmails.length, 2);
});

test('GitHub pagination is bounded, origin-locked, and includes every returned schedule run', async () => {
  const now = new Date('2026-08-12T12:00:00.000Z');
  const nextUrl = `${API_BASE}/repos/${CONTROL_REPO}/actions/runs?event=schedule&per_page=100&page=2`;
  let controlPages = 0;
  const harness = await createHarness({
    route(url, options, defaultRoute) {
      if (url === `${API_BASE}/repos/${CONTROL_REPO}`) return defaultRoute(url, options);
      if (url.startsWith(runsUrl(CONTROL_REPO))) {
        controlPages += 1;
        return response(
          {
            total_count: 2,
            workflow_runs: [
              runFixture({
                id: 10,
                name: 'A',
                path: '.github/workflows/a.yml',
                conclusion: 'success',
                createdAt: '2026-08-12T10:00:00Z',
              }),
            ],
          },
          { headers: { Link: `<${nextUrl}>; rel="next"` } },
        );
      }
      if (url === nextUrl) {
        controlPages += 1;
        return response({
          total_count: 2,
          workflow_runs: [
            runFixture({
              id: 9,
              name: 'B',
              path: '.github/workflows/b.yml',
              conclusion: 'failure',
              createdAt: '2026-08-12T09:00:00Z',
            }),
          ],
        });
      }
      if (url.startsWith(runsUrl('ORESoftware/k8s-cluster'))) {
        return response({ total_count: 0, workflow_runs: [] });
      }
      if (url.startsWith(runsUrl('ORESoftware/project-registry'))) {
        return response({ total_count: 0, workflow_runs: [] });
      }
      if (url.includes('/jobs?')) return response({ total_count: 0, jobs: [] });
      throw new Error(`Unexpected route ${url}`);
    },
  }).load();

  const report = harness.context.collectScheduledTaskDigest_(now);
  assert.equal(controlPages, 2);
  assert.equal(report.repositories[CONTROL_REPO].runs.length, 2);
  assert.equal(report.observed.length, 2);
  assert.equal(report.observed[0].attemptCount, 1);
  assert.equal(report.observed[1].attemptCount, 1);
});

test('upstream error bodies and credential-shaped workflow fields never enter the email', async () => {
  const leaked = ['lin', 'api', 'ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890'].join('_');
  const harness = await createHarness({
    route(url, options, defaultRoute) {
      if (url === `${API_BASE}/repos/${CONTROL_REPO}`) return defaultRoute(url, options);
      if (url.startsWith(runsUrl(CONTROL_REPO))) {
        return response({
          total_count: 1,
          workflow_runs: [
            runFixture({
              id: 88,
              name: `nightly ${leaked}`,
              path: '.github/workflows/untrusted.yml',
              conclusion: 'success',
              createdAt: '2026-08-12T09:00:00Z',
            }),
          ],
        });
      }
      if (url.startsWith(runsUrl('ORESoftware/k8s-cluster'))) {
        return response(`provider exploded token=${leaked}`, { status: 503 });
      }
      if (url.startsWith(runsUrl('ORESoftware/project-registry'))) {
        return response({ total_count: 0, workflow_runs: [] });
      }
      if (url.includes('/jobs?')) return response({ total_count: 0, jobs: [] });
      throw new Error(`Unexpected route ${url}`);
    },
  }).load();

  const report = harness.context.collectScheduledTaskDigest_(
    new Date('2026-08-12T12:00:00.000Z'),
  );
  const rendered = harness.context.renderScheduledTaskDigest_(report, 'scheduled');

  assert.doesNotMatch(rendered.text, new RegExp(leaked));
  assert.doesNotMatch(rendered.html, new RegExp(leaked));
  assert.match(rendered.text, /\[REDACTED\]/);
  assert.match(rendered.text, /GitHub API returned HTTP 503/);
  assert.doesNotMatch(rendered.text, /provider exploded/);
});
