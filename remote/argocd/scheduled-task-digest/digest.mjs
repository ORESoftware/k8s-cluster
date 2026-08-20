import {
  CONFIG,
  CRITICAL_GITHUB_TASKS,
  EXTERNAL_COVERAGE,
  STATUS_PRIORITY,
  escapeHtml,
  redact,
  zonedParts,
} from './shared.mjs';
import {
  evaluateCriticalGithubTask,
  fetchRepositoryScheduleRuns,
  groupObservedGithubRuns,
} from './github.mjs';
import {
  evaluateKubernetesCronJob,
  listKubernetesCollection,
  serviceAccountToken,
} from './kubernetes.mjs';

function countStatuses(entries) {
  const counts = {};
  for (const entry of entries) counts[entry.status] = (counts[entry.status] || 0) + 1;
  return counts;
}

export async function collectDigest(windowEnd, { fetchImpl = globalThis.fetch } = {}) {
  const windowStart = new Date(windowEnd.getTime() - CONFIG.lookbackHours * 60 * 60 * 1000);
  const sourceErrors = [];
  const repositoryEvidence = {};
  let allGithubRuns = [];

  for (const repository of CONFIG.githubRepositories) {
    try {
      const evidence = await fetchRepositoryScheduleRuns(repository, windowStart, { fetchImpl });
      repositoryEvidence[repository] = evidence;
      allGithubRuns.push(...evidence.runs);
      if (evidence.incomplete) sourceErrors.push({ source: repository, error: 'GitHub pagination exceeded the safety bound.' });
    } catch (error) {
      repositoryEvidence[repository] = { repository, runs: [], incomplete: true };
      sourceErrors.push({ source: repository, error: boundedError(error) });
    }
  }

  const jobsCache = new Map();
  const criticalGithub = [];
  for (const task of CRITICAL_GITHUB_TASKS) {
    criticalGithub.push(await evaluateCriticalGithubTask(task, allGithubRuns, windowEnd, { fetchImpl, jobsCache }));
  }
  const criticalKeys = new Set(CRITICAL_GITHUB_TASKS.map((task) => `${task.repository}|${task.workflowPath}`));
  const allObservedGithub = groupObservedGithubRuns(allGithubRuns, criticalKeys);
  const observedGithub = allObservedGithub.slice(0, CONFIG.maxObservedGithubGroups);

  let kubernetes = [];
  let kubernetesIncomplete = false;
  try {
    const token = await serviceAccountToken();
    const [cronJobsResult, jobsResult] = await Promise.all([
      listKubernetesCollection('/apis/batch/v1/cronjobs', { fetchImpl, token }),
      listKubernetesCollection('/apis/batch/v1/jobs', { fetchImpl, token }),
    ]);
    kubernetesIncomplete = cronJobsResult.incomplete || jobsResult.incomplete;
    if (kubernetesIncomplete) sourceErrors.push({ source: 'kubernetes', error: 'Kubernetes pagination exceeded the safety bound.' });
    kubernetes = cronJobsResult.items
      .map((cronJob) => evaluateKubernetesCronJob(cronJob, jobsResult.items, windowStart, windowEnd))
      .sort((a, b) => {
        const severity = (STATUS_PRIORITY[a.status] ?? 99) - (STATUS_PRIORITY[b.status] ?? 99);
        return severity || `${a.namespace}/${a.name}`.localeCompare(`${b.namespace}/${b.name}`);
      });
  } catch (error) {
    sourceErrors.push({ source: 'kubernetes', error: boundedError(error) });
  }

  const boundedKubernetes = kubernetes.slice(0, CONFIG.maxCronJobsInEmail);
  const summary = {
    githubCritical: countStatuses(criticalGithub),
    githubObserved: countStatuses(observedGithub),
    kubernetes: countStatuses(boundedKubernetes),
    externalUnverified: EXTERNAL_COVERAGE.length,
    sourceErrors: sourceErrors.length,
  };
  return {
    schemaVersion: 'scheduled_task_digest.v2',
    generatedAt: windowEnd.toISOString(),
    windowStart: windowStart.toISOString(),
    windowEnd: windowEnd.toISOString(),
    timezone: CONFIG.timezone,
    criticalGithub,
    observedGithub,
    observedGithubOverflow: Math.max(0, allObservedGithub.length - observedGithub.length),
    kubernetes: boundedKubernetes,
    kubernetesOverflow: Math.max(0, kubernetes.length - boundedKubernetes.length),
    kubernetesIncomplete,
    externalCoverage: EXTERNAL_COVERAGE,
    sourceErrors,
    repositoryEvidence,
    summary,
  };
}

export function formatInstant(value, timeZone = CONFIG.timezone) {
  if (!value) return 'unknown';
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return 'unknown';
  return new Intl.DateTimeFormat('en-CA', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    timeZoneName: 'short',
    hourCycle: 'h23',
  }).format(date);
}

export function logicalDateAt(instant, timeZone = CONFIG.timezone) {
  const parts = zonedParts(instant, timeZone);
  return `${parts.year}-${parts.month}-${parts.day}`;
}

function flattenedSummary(report) {
  const github = report.summary.githubCritical;
  const kube = report.summary.kubernetes;
  return {
    certified: (github.SUCCESS || 0) + (kube.SUCCESS || 0),
    failed: (github.FAILED || 0) + (kube.FAILED || 0),
    falseGreen: github.FALSE_GREEN || 0,
    missed: (github.MISSED || 0) + (kube.MISSED || 0),
    running: (github.RUNNING || 0) + (kube.RUNNING || 0),
    unverified:
      (github.UNVERIFIED || 0) +
      (kube.UNVERIFIED || 0) +
      (kube.SUSPENDED || 0) +
      report.summary.externalUnverified +
      report.summary.sourceErrors,
    observedSuccess: (report.summary.githubObserved.OBSERVED_SUCCESS || 0) + (kube.OBSERVED_SUCCESS || 0),
  };
}

function attemptEvidenceLine(attempt) {
  const evidence = attempt.evidence || {};
  const jobs = evidence.executionJobs?.length
    ? `; execution jobs ${evidence.executionJobs.map((job) => `${job.name}=${job.conclusion || job.status}`).join(', ')}`
    : '';
  return `run ${evidence.runId || 'unknown'} at ${formatInstant(evidence.createdAt)}${jobs}${evidence.htmlUrl ? `; ${evidence.htmlUrl}` : ''}`;
}

export function renderDigest(report, mode = 'scheduled') {
  const manual = mode === 'manual-canary';
  const summary = flattenedSummary(report);
  const date = logicalDateAt(new Date(report.windowEnd));
  const subject = `${manual ? '[MANUAL CANARY] ' : ''}[Scheduled task digest] ${date} — ${summary.certified} certified, ${summary.failed} failed, ${summary.falseGreen} false-green, ${summary.missed} missed`;
  const lines = [
    'Scheduled task digest',
    '',
    `Window: ${formatInstant(report.windowStart)} to ${formatInstant(report.windowEnd)}`,
    `Generated: ${formatInstant(report.generatedAt)}`,
    `Mode: ${manual ? 'manual canary' : 'scheduled'}`,
    '',
    'Summary',
    `  Certified success: ${summary.certified}`,
    `  Failed: ${summary.failed}`,
    `  False green: ${summary.falseGreen}`,
    `  Missed: ${summary.missed}`,
    `  Running: ${summary.running}`,
    `  Unverified/suspended/source-error: ${summary.unverified}`,
    `  Observed success with incomplete certification: ${summary.observedSuccess}`,
    '',
    'Critical GitHub scheduled workflows',
  ];
  for (const task of report.criticalGithub) {
    lines.push(`- [${task.status}] ${task.name} — ${task.schedule} — ${task.reason}`);
    for (const attempt of task.attempts.slice(0, 4)) lines.push(`    ${attemptEvidenceLine(attempt)}`);
  }
  lines.push('', 'Other observed GitHub schedule-event workflows');
  if (!report.observedGithub.length) lines.push('- None beyond the critical registry.');
  for (const group of report.observedGithub) {
    lines.push(`- [${group.status}] ${group.repository} / ${group.name} — attempts=${group.attemptCount}; latest=${formatInstant(group.latest?.createdAt)}${group.latest?.htmlUrl ? `; ${group.latest.htmlUrl}` : ''}`);
  }
  if (report.observedGithubOverflow) lines.push(`- Coverage warning: ${report.observedGithubOverflow} additional workflow groups exceeded the email bound.`);

  lines.push('', 'Kubernetes CronJobs (all namespaces)');
  if (!report.kubernetes.length) lines.push('- No CronJob evidence was available.');
  for (const cron of report.kubernetes) {
    lines.push(`- [${cron.status}] ${cron.namespace}/${cron.name} — ${cron.schedule} ${cron.timeZone}; expected=${cron.expectedRuns}; retained=${cron.observedRuns}; ${cron.reason}`);
    for (const job of cron.jobs.slice(0, 3)) lines.push(`    ${job.name}=${job.state} at ${formatInstant(job.createdAt)}`);
  }
  if (report.kubernetesOverflow) lines.push(`- Coverage warning: ${report.kubernetesOverflow} additional CronJobs exceeded the email bound.`);

  lines.push('', 'Registered schedules without authoritative runtime feeds');
  for (const item of report.externalCoverage) lines.push(`- [${item.status}] ${item.name} — ${item.reason}`);
  if (report.sourceErrors.length) {
    lines.push('', 'Source errors');
    for (const error of report.sourceErrors) lines.push(`- ${error.source}: ${error.error}`);
  }
  lines.push(
    '',
    'Interpretation',
    'GitHub workflows are certified only when their required execution jobs succeeded. Kubernetes CronJobs are certified only when retained Job evidence covers every due execution; history-limited successes are labeled observed rather than certified. Missing feeds are never treated as success.',
    '',
    'Tracking: DEN-3562',
  );
  const text = redact(lines.join('\n'), 950_000);

  const criticalRows = report.criticalGithub.map((task) => {
    const evidence = task.attempts.slice(0, 4).map((attempt) => escapeHtml(attemptEvidenceLine(attempt))).join('<br>');
    return `<tr><td><strong>${escapeHtml(task.status)}</strong></td><td>${escapeHtml(task.name)}<br><small>${escapeHtml(task.schedule)}</small></td><td>${escapeHtml(task.reason)}${evidence ? `<br><small>${evidence}</small>` : ''}</td></tr>`;
  }).join('');
  const kubernetesRows = report.kubernetes.map((cron) =>
    `<tr><td><strong>${escapeHtml(cron.status)}</strong></td><td>${escapeHtml(`${cron.namespace}/${cron.name}`)}<br><small>${escapeHtml(`${cron.schedule} ${cron.timeZone}`)}</small></td><td>${escapeHtml(`expected=${cron.expectedRuns}; retained=${cron.observedRuns}; ${cron.reason}`)}</td></tr>`,
  ).join('');
  const observedItems = report.observedGithub.map((group) =>
    `<li><strong>${escapeHtml(group.status)}</strong> — ${escapeHtml(`${group.repository} / ${group.name}`)} — ${escapeHtml(String(group.attemptCount))} attempt(s)${group.latest?.htmlUrl ? ` — <a href="${escapeHtml(group.latest.htmlUrl)}">latest run</a>` : ''}</li>`,
  ).join('');
  const externalItems = report.externalCoverage.map((item) => `<li><strong>${escapeHtml(item.status)}</strong> — ${escapeHtml(`${item.name} — ${item.reason}`)}</li>`).join('');
  const errorItems = report.sourceErrors.map((item) => `<li>${escapeHtml(`${item.source}: ${item.error}`)}</li>`).join('');
  const html = redact(
    `<div style="font-family:Arial,sans-serif;line-height:1.45"><h1>Scheduled task digest</h1><p><strong>Window:</strong> ${escapeHtml(formatInstant(report.windowStart))} to ${escapeHtml(formatInstant(report.windowEnd))}</p><p><strong>Summary:</strong> ${escapeHtml(`${summary.certified} certified, ${summary.failed} failed, ${summary.falseGreen} false-green, ${summary.missed} missed, ${summary.unverified} unverified`)}</p><h2>Critical GitHub scheduled workflows</h2><table border="1" cellpadding="6" cellspacing="0" style="border-collapse:collapse"><thead><tr><th>Outcome</th><th>Task</th><th>Evidence</th></tr></thead><tbody>${criticalRows}</tbody></table><h2>Other observed GitHub schedule-event workflows</h2><ul>${observedItems || '<li>None beyond the critical registry.</li>'}</ul><h2>Kubernetes CronJobs (all namespaces)</h2><table border="1" cellpadding="6" cellspacing="0" style="border-collapse:collapse"><thead><tr><th>Outcome</th><th>CronJob</th><th>Evidence</th></tr></thead><tbody>${kubernetesRows}</tbody></table><h2>Registered schedules without authoritative runtime feeds</h2><ul>${externalItems}</ul>${errorItems ? `<h2>Source errors</h2><ul>${errorItems}</ul>` : ''}<p><strong>Interpretation:</strong> green configuration or workflow validation is not success unless execution evidence is present.</p><p>Tracking: DEN-3562</p></div>`,
    950_000,
  );
  return { subject: redact(subject, 998), text, html, summary };
}



export {
  CONFIG,
  CRITICAL_GITHUB_TASKS,
  EXTERNAL_COVERAGE,
  STATUS_PRIORITY,
  boundedError,
  escapeHtml,
  fetchJson,
  ensureAllowedUrl,
  redact,
  zonedParts,
} from './shared.mjs';
export {
  classifyGithubRun,
  evaluateCriticalGithubTask,
  fetchGithubRunJobs,
  fetchRepositoryScheduleRuns,
  groupObservedGithubRuns,
  normalizeGithubRun,
} from './github.mjs';
export {
  cronMatches,
  dueInstants,
  evaluateKubernetesCronJob,
  kubernetesRequest,
  listKubernetesCollection,
  parseCronExpression,
  serviceAccountToken,
} from './kubernetes.mjs';
