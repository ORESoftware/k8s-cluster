// Shared constants, bounded HTTP, redaction, and timezone helpers.

export const CONFIG = Object.freeze({
  recipient: 'alexander.d.mills@gmail.com',
  timezone: 'America/Chicago',
  lookbackHours: 24,
  githubApiBase: 'https://api.github.com',
  githubRepositories: Object.freeze([
    'ORESoftware/ai-agent-coordinator.rs',
    'ORESoftware/k8s-cluster',
    'ORESoftware/project-registry',
  ]),
  emailServiceUrl:
    process.env.EMAIL_SERVICE_URL ||
    'http://dd-email-sms-contact-rs.default.svc.cluster.local:8120',
  leaseNamespace: process.env.LEASE_NAMESPACE || 'default',
  leaseName: process.env.LEASE_NAME || 'dd-scheduled-task-digest-delivery',
  maxGithubPages: 5,
  maxGithubJobPages: 3,
  maxKubernetesPages: 20,
  maxBodyBytes: 2 * 1024 * 1024,
  maxObservedGithubGroups: 100,
  maxCronJobsInEmail: 180,
  requestTimeoutMs: 30_000,
});

export const CRITICAL_GITHUB_TASKS = Object.freeze([
  Object.freeze({
    id: 'recent-chat-reconciliation',
    name: 'Recent 96-hour ChatGPT reconciliation',
    repository: 'ORESoftware/ai-agent-coordinator.rs',
    workflowPath: '.github/workflows/last-30-hours-introspection.yml',
    schedule: '00:30 America/Lima, daily',
    expectedRuns: 1,
    executionJobNames: Object.freeze(['enqueue-and-verify']),
  }),
  Object.freeze({
    id: 'nightly-org-maintenance',
    name: 'Nightly organization maintenance',
    repository: 'ORESoftware/ai-agent-coordinator.rs',
    workflowPath: '.github/workflows/nightly-org-maintenance.yml',
    schedule: '01:00 America/Lima, daily',
    expectedRuns: 1,
    executionJobPrefixes: Object.freeze(['maintain (']),
    requireAllExecutionJobs: true,
  }),
  Object.freeze({
    id: 'nightly-artifact-recovery',
    name: 'Nightly artifact recovery',
    repository: 'ORESoftware/ai-agent-coordinator.rs',
    workflowPath: '.github/workflows/nightly-artifact-recovery.yml',
    schedule: '02:17 and 03:47 America/Chicago, daily',
    expectedRuns: 2,
    executionJobNames: Object.freeze(['enqueue', 'enqueue-and-verify']),
  }),
  Object.freeze({
    id: 'daily-portfolio-briefing',
    name: 'Daily portfolio briefing',
    repository: 'ORESoftware/ai-agent-coordinator.rs',
    workflowPath: '.github/workflows/daily-portfolio-briefing.yml',
    schedule: '08:00 America/Chicago, daily',
    expectedRuns: 1,
    candidateTimezone: 'America/Chicago',
    candidateLocalHours: Object.freeze([7, 8, 9, 10]),
    executionJobNames: Object.freeze(['enqueue']),
  }),
  Object.freeze({
    id: 'weekly-job-opportunity-digest',
    name: 'Weekly job opportunity digest',
    repository: 'ORESoftware/ai-agent-coordinator.rs',
    workflowPath: '.github/workflows/weekly-job-opportunity-digest.yml',
    schedule: '09:17 America/New_York, Monday',
    expectedRuns: 1,
    dueOnDigestWeekdays: Object.freeze(['Tue']),
    executionJobNames: Object.freeze(['enqueue']),
  }),
]);

export const EXTERNAL_COVERAGE = Object.freeze([
  Object.freeze({
    name: 'ChatGPT-native scheduled tasks',
    status: 'UNVERIFIED',
    reason: 'No authoritative ChatGPT task-run ledger is connected to the cluster digest yet.',
  }),
  Object.freeze({
    name: 'Messaging-Intel contact discovery',
    status: 'UNVERIFIED',
    reason: 'No durable runtime ledger or evidence endpoint is registered.',
  }),
  Object.freeze({
    name: 'Clients and repositories audit',
    status: 'UNVERIFIED',
    reason: 'No current machine-readable scheduler feed is registered.',
  }),
]);

export const STATUS_PRIORITY = Object.freeze({
  FAILED: 0,
  FALSE_GREEN: 1,
  MISSED: 2,
  RUNNING: 3,
  UNVERIFIED: 4,
  SUSPENDED: 5,
  SUCCESS: 6,
  OBSERVED_SUCCESS: 7,
  NOT_DUE: 8,
});

const SECRET_PATTERNS = Object.freeze([
  /ghp_[A-Za-z0-9]{20,}/g,
  /github_pat_[A-Za-z0-9_]{20,}/g,
  /lin_api_[A-Za-z0-9]{20,}/g,
  /SG\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/g,
  /Bearer\s+[A-Za-z0-9._-]{16,}/gi,
  /-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----/g,
]);

export function redact(value, maxLength = 800) {
  let text = String(value ?? '');
  for (const pattern of SECRET_PATTERNS) text = text.replace(pattern, '[REDACTED]');
  text = text.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, ' ');
  return text.trim().slice(0, maxLength);
}

export function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

export function boundedError(error) {
  return redact(error instanceof Error ? error.message : error, 320) || 'Unknown bounded error.';
}

export function ensureAllowedUrl(rawUrl, allowedOrigins) {
  const parsed = new URL(rawUrl);
  if (!allowedOrigins.includes(parsed.origin)) {
    throw new Error(`Refused URL outside allowed origins: ${parsed.origin}`);
  }
  if (parsed.username || parsed.password) throw new Error('Refused URL containing credentials.');
  return parsed;
}

export async function fetchJson(rawUrl, {
  method = 'GET',
  headers = {},
  body,
  allowedOrigins,
  expectedStatuses = null,
  fetchImpl = globalThis.fetch,
} = {}) {
  const parsed = ensureAllowedUrl(rawUrl, allowedOrigins);
  const response = await fetchImpl(parsed, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    redirect: 'error',
    signal: AbortSignal.timeout(CONFIG.requestTimeoutMs),
  });
  const contentLength = Number(response.headers.get('content-length') || 0);
  if (contentLength > CONFIG.maxBodyBytes) throw new Error('Response exceeded the configured size limit.');
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength > CONFIG.maxBodyBytes) throw new Error('Response exceeded the configured size limit.');
  const text = new TextDecoder().decode(bytes);
  const accepted = expectedStatuses ? expectedStatuses.includes(response.status) : response.ok;
  if (!accepted) throw new Error(`HTTP ${response.status} from ${parsed.origin}.`);
  let json = {};
  if (text) {
    try {
      json = JSON.parse(text);
    } catch {
      throw new Error(`Non-JSON response from ${parsed.origin}.`);
    }
  }
  return { status: response.status, headers: response.headers, json };
}


export function zonedParts(date, timeZone) {
  const formatter = new Intl.DateTimeFormat('en-US', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    weekday: 'short',
    hourCycle: 'h23',
  });
  return Object.fromEntries(formatter.formatToParts(date).map((part) => [part.type, part.value]));
}
