import { readFile } from 'node:fs/promises';

import {
  CONFIG,
  boundedError,
  fetchJson,
  redact,
  zonedParts,
} from './shared.mjs';

const MONTH_NAMES = Object.freeze({ jan: 1, feb: 2, mar: 3, apr: 4, may: 5, jun: 6, jul: 7, aug: 8, sep: 9, oct: 10, nov: 11, dec: 12 });
const DOW_NAMES = Object.freeze({ sun: 0, mon: 1, tue: 2, wed: 3, thu: 4, fri: 5, sat: 6 });
const DOW_FROM_SHORT = Object.freeze({ Sun: 0, Mon: 1, Tue: 2, Wed: 3, Thu: 4, Fri: 5, Sat: 6 });

function parseCronAtom(raw, names, min, max) {
  const lower = String(raw).toLowerCase();
  const value = names && Object.hasOwn(names, lower) ? names[lower] : Number(lower);
  if (!Number.isInteger(value) || value < min || value > max) throw new Error(`Unsupported cron value: ${raw}`);
  return value;
}

function expandCronField(raw, min, max, names = null, mapValue = (value) => value) {
  const values = new Set();
  for (const segment of String(raw).split(',')) {
    if (!segment) throw new Error('Empty cron field segment.');
    const [base, stepRaw] = segment.split('/');
    if (segment.split('/').length > 2) throw new Error(`Unsupported cron field: ${raw}`);
    const step = stepRaw === undefined ? 1 : Number(stepRaw);
    if (!Number.isInteger(step) || step < 1) throw new Error(`Invalid cron step: ${segment}`);
    let start;
    let end;
    if (base === '*') {
      start = min;
      end = max;
    } else if (base.includes('-')) {
      const parts = base.split('-');
      if (parts.length !== 2) throw new Error(`Invalid cron range: ${base}`);
      start = parseCronAtom(parts[0], names, min, max);
      end = parseCronAtom(parts[1], names, min, max);
      if (start > end) throw new Error(`Descending cron range is unsupported: ${base}`);
    } else {
      start = parseCronAtom(base, names, min, max);
      end = start;
    }
    for (let value = start; value <= end; value += step) values.add(mapValue(value));
  }
  return values;
}

export function parseCronExpression(expression) {
  const macros = {
    '@hourly': '0 * * * *',
    '@daily': '0 0 * * *',
    '@midnight': '0 0 * * *',
    '@weekly': '0 0 * * 0',
    '@monthly': '0 0 1 * *',
    '@yearly': '0 0 1 1 *',
    '@annually': '0 0 1 1 *',
  };
  const normalized = macros[String(expression).trim().toLowerCase()] || String(expression).trim();
  const fields = normalized.split(/\s+/);
  if (fields.length !== 5) throw new Error(`Expected five cron fields, got ${fields.length}.`);
  const [minuteRaw, hourRaw, domRaw, monthRaw, dowRaw] = fields;
  return {
    expression: normalized,
    minute: expandCronField(minuteRaw, 0, 59),
    hour: expandCronField(hourRaw, 0, 23),
    dayOfMonth: expandCronField(domRaw, 1, 31),
    month: expandCronField(monthRaw, 1, 12, MONTH_NAMES),
    dayOfWeek: expandCronField(dowRaw, 0, 7, DOW_NAMES, (value) => (value === 7 ? 0 : value)),
    domWildcard: domRaw === '*',
    dowWildcard: dowRaw === '*',
  };
}

export function cronMatches(parsed, instant, timeZone) {
  const parts = zonedParts(instant, timeZone);
  const minute = Number(parts.minute);
  const hour = Number(parts.hour);
  const day = Number(parts.day);
  const month = Number(parts.month);
  const dow = DOW_FROM_SHORT[parts.weekday];
  if (!parsed.minute.has(minute) || !parsed.hour.has(hour) || !parsed.month.has(month)) return false;
  const domMatch = parsed.dayOfMonth.has(day);
  const dowMatch = parsed.dayOfWeek.has(dow);
  if (!parsed.domWildcard && !parsed.dowWildcard) return domMatch || dowMatch;
  return domMatch && dowMatch;
}

export function dueInstants(expression, timeZone, windowStart, windowEnd) {
  const parsed = parseCronExpression(expression);
  // Validate the IANA timezone before iterating.
  new Intl.DateTimeFormat('en-US', { timeZone }).format(windowEnd);
  // Use the half-open interval (windowStart, windowEnd] so an exact 24-hour
  // boundary is counted once rather than at both ends.
  const startMinute = Math.floor(windowStart.getTime() / 60_000) * 60_000 + 60_000;
  const endMinute = Math.floor(windowEnd.getTime() / 60_000) * 60_000;
  const due = [];
  for (let timestamp = startMinute; timestamp <= endMinute; timestamp += 60_000) {
    const instant = new Date(timestamp);
    if (cronMatches(parsed, instant, timeZone)) due.push(instant.toISOString());
  }
  return due;
}

function kubernetesApiOrigin() {
  const host = process.env.KUBERNETES_SERVICE_HOST || 'kubernetes.default.svc';
  const port = process.env.KUBERNETES_SERVICE_PORT_HTTPS || '443';
  return `https://${host}:${port}`;
}

export async function serviceAccountToken() {
  const token = await readFile('/var/run/secrets/kubernetes.io/serviceaccount/token', 'utf8');
  if (!token.trim()) throw new Error('Kubernetes service-account token was empty.');
  return token.trim();
}

export async function kubernetesRequest(path, {
  method = 'GET',
  body,
  expectedStatuses = null,
  fetchImpl = globalThis.fetch,
  token = null,
} = {}) {
  const origin = kubernetesApiOrigin();
  const bearer = token || (await serviceAccountToken());
  return fetchJson(`${origin}${path}`, {
    method,
    body,
    expectedStatuses,
    fetchImpl,
    allowedOrigins: [origin],
    headers: {
      Accept: 'application/json',
      Authorization: `Bearer ${bearer}`,
      ...(body === undefined ? {} : { 'Content-Type': method === 'PATCH' ? 'application/merge-patch+json' : 'application/json' }),
    },
  });
}

export async function listKubernetesCollection(path, { fetchImpl = globalThis.fetch, token = null } = {}) {
  const items = [];
  let continuation = '';
  let pages = 0;
  do {
    pages += 1;
    const separator = path.includes('?') ? '&' : '?';
    const requestPath = `${path}${separator}limit=500${continuation ? `&continue=${encodeURIComponent(continuation)}` : ''}`;
    const response = await kubernetesRequest(requestPath, { fetchImpl, token });
    if (!Array.isArray(response.json.items)) throw new Error('Kubernetes list response omitted items.');
    items.push(...response.json.items);
    continuation = response.json.metadata?.continue || '';
  } while (continuation && pages < CONFIG.maxKubernetesPages);
  return { items, pages, incomplete: Boolean(continuation) };
}

function cronJobOwner(job) {
  return job.metadata?.ownerReferences?.find((owner) => owner.kind === 'CronJob' && owner.name)?.name || null;
}

function kubernetesJobState(job) {
  const conditions = job.status?.conditions || [];
  if (conditions.some((condition) => condition.type === 'Failed' && condition.status === 'True')) return 'FAILED';
  if (conditions.some((condition) => condition.type === 'Complete' && condition.status === 'True')) return 'SUCCESS';
  if (Number(job.status?.active || 0) > 0) return 'RUNNING';
  if (Number(job.status?.failed || 0) > 0) return 'FAILED';
  if (Number(job.status?.succeeded || 0) > 0) return 'SUCCESS';
  return 'UNVERIFIED';
}

function historyCapacity(cronJob) {
  const successes = Number(cronJob.spec?.successfulJobsHistoryLimit ?? 3);
  const failures = Number(cronJob.spec?.failedJobsHistoryLimit ?? 1);
  return Math.max(0, successes) + Math.max(0, failures);
}

export function evaluateKubernetesCronJob(cronJob, allJobs, windowStart, windowEnd) {
  const namespace = cronJob.metadata?.namespace || 'default';
  const name = cronJob.metadata?.name || 'unnamed-cronjob';
  const schedule = String(cronJob.spec?.schedule || '');
  const timeZone = String(cronJob.spec?.timeZone || 'Etc/UTC');
  const base = { namespace, name, schedule, timeZone, expectedRuns: 0, observedRuns: 0, jobs: [] };
  if (cronJob.metadata?.annotations?.['dd.dev/digest.exclude'] === 'true') {
    return { ...base, status: 'NOT_DUE', reason: 'Excluded from its own digest to avoid self-referential evidence.' };
  }
  if (cronJob.spec?.suspend === true) {
    return { ...base, status: 'SUSPENDED', reason: 'The CronJob is suspended.' };
  }
  let due;
  try {
    due = dueInstants(schedule, timeZone, windowStart, windowEnd);
  } catch (error) {
    return { ...base, status: 'UNVERIFIED', reason: `Cron schedule could not be evaluated: ${boundedError(error)}` };
  }
  const jobs = allJobs
    .filter((job) => {
      if ((job.metadata?.namespace || 'default') !== namespace) return false;
      if (cronJobOwner(job) !== name) return false;
      if (job.metadata?.labels?.['dd.dev/manual-canary'] === 'true') return false;
      const timestamp = Date.parse(job.metadata?.creationTimestamp || job.status?.startTime || '');
      return Number.isFinite(timestamp) && timestamp >= windowStart.getTime() && timestamp <= windowEnd.getTime();
    })
    .map((job) => ({
      name: redact(job.metadata?.name || 'unnamed-job', 180),
      state: kubernetesJobState(job),
      createdAt: job.metadata?.creationTimestamp || null,
      startedAt: job.status?.startTime || null,
      completedAt: job.status?.completionTime || null,
    }))
    .sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)));
  const result = { ...base, expectedRuns: due.length, observedRuns: jobs.length, jobs: jobs.slice(0, 12), latestDueAt: due.at(-1) || null };
  if (due.length === 0) return { ...result, status: 'NOT_DUE', reason: 'The CronJob was not due in the 24-hour window.' };
  if (jobs.some((job) => job.state === 'FAILED')) return { ...result, status: 'FAILED', reason: 'At least one retained Job failed.' };
  if (jobs.some((job) => job.state === 'RUNNING')) return { ...result, status: 'RUNNING', reason: 'At least one Job is still running.' };
  if (jobs.length === 0) return { ...result, status: 'MISSED', reason: `No Job evidence was retained for ${due.length} due execution(s).` };
  if (jobs.length >= due.length && jobs.every((job) => job.state === 'SUCCESS')) {
    return { ...result, status: 'SUCCESS', reason: `All ${due.length} due execution(s) have successful retained Job evidence.` };
  }
  if (jobs.every((job) => job.state === 'SUCCESS') && due.length > historyCapacity(cronJob)) {
    return {
      ...result,
      status: 'OBSERVED_SUCCESS',
      reason: `The latest retained Jobs succeeded, but history limits cannot certify all ${due.length} due execution(s).`,
    };
  }
  return { ...result, status: 'UNVERIFIED', reason: `Observed ${jobs.length} retained Job(s) for ${due.length} due execution(s).` };
}
