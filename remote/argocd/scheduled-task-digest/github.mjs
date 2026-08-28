import {
  CONFIG,
  boundedError,
  fetchJson,
  ensureAllowedUrl,
  redact,
  zonedParts,
} from './shared.mjs';

function nextGithubLink(headers) {
  const link = headers.get('link') || '';
  const match = link.match(/<([^>]+)>;\s*rel="next"/);
  if (!match) return null;
  const next = ensureAllowedUrl(match[1], [CONFIG.githubApiBase]);
  return next.toString();
}

function githubHeaders() {
  const headers = {
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28',
    'User-Agent': 'oresoftware-scheduled-task-digest/2',
  };
  const token = String(process.env.GITHUB_TOKEN || '').trim();
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

export function normalizeGithubRun(repository, run) {
  const id = Number(run?.id);
  return {
    repository,
    id: Number.isFinite(id) ? id : null,
    name: redact(run?.name || 'Unnamed workflow', 160),
    workflowPath: redact(run?.path || 'unknown-workflow', 300),
    status: redact(run?.status || 'unknown', 40).toLowerCase(),
    conclusion: run?.conclusion ? redact(run.conclusion, 40).toLowerCase() : null,
    createdAt: run?.created_at || null,
    updatedAt: run?.updated_at || null,
    htmlUrl: safeGithubRunUrl(run?.html_url),
    runNumber: Number(run?.run_number) || null,
    runAttempt: Number(run?.run_attempt) || 1,
  };
}

function safeGithubRunUrl(value) {
  const text = String(value || '');
  return /^https:\/\/github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/actions\/runs\/\d+$/.test(text)
    ? text
    : null;
}

export async function fetchRepositoryScheduleRuns(repository, since, { fetchImpl = globalThis.fetch } = {}) {
  const encoded = repository.split('/').map(encodeURIComponent).join('/');
  let url = `${CONFIG.githubApiBase}/repos/${encoded}/actions/runs?event=schedule&per_page=100&created=${encodeURIComponent(`>=${since.toISOString()}`)}`;
  const runs = [];
  let page = 0;
  while (url && page < CONFIG.maxGithubPages) {
    page += 1;
    const response = await fetchJson(url, {
      headers: githubHeaders(),
      allowedOrigins: [CONFIG.githubApiBase],
      fetchImpl,
    });
    if (!Array.isArray(response.json.workflow_runs)) {
      throw new Error('GitHub schedule-run response omitted workflow_runs.');
    }
    for (const run of response.json.workflow_runs) runs.push(normalizeGithubRun(repository, run));
    url = nextGithubLink(response.headers);
  }
  runs.sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)));
  return { repository, runs, pages: page, incomplete: Boolean(url) };
}

export async function fetchGithubRunJobs(repository, runId, { fetchImpl = globalThis.fetch } = {}) {
  const encoded = repository.split('/').map(encodeURIComponent).join('/');
  let url = `${CONFIG.githubApiBase}/repos/${encoded}/actions/runs/${encodeURIComponent(String(runId))}/jobs?per_page=100`;
  const jobs = [];
  let page = 0;
  while (url && page < CONFIG.maxGithubJobPages) {
    page += 1;
    const response = await fetchJson(url, {
      headers: githubHeaders(),
      allowedOrigins: [CONFIG.githubApiBase],
      fetchImpl,
    });
    if (!Array.isArray(response.json.jobs)) throw new Error('GitHub workflow-job response omitted jobs.');
    for (const job of response.json.jobs) {
      jobs.push({
        id: Number(job?.id) || null,
        name: redact(job?.name || 'Unnamed job', 240),
        status: redact(job?.status || 'unknown', 40).toLowerCase(),
        conclusion: job?.conclusion ? redact(job.conclusion, 40).toLowerCase() : null,
        startedAt: job?.started_at || null,
        completedAt: job?.completed_at || null,
      });
    }
    url = nextGithubLink(response.headers);
  }
  return { jobs, pages: page, incomplete: Boolean(url) };
}

function executionJobsFor(task, jobs) {
  return jobs.filter((job) =>
    task.executionJobNames?.includes(job.name) ||
    task.executionJobPrefixes?.some((prefix) => job.name.startsWith(prefix)),
  );
}

export function classifyGithubRun(task, run, jobsResult = null) {
  const evidence = {
    runId: run.id,
    createdAt: run.createdAt,
    updatedAt: run.updatedAt,
    htmlUrl: run.htmlUrl,
    workflowConclusion: run.conclusion,
    executionJobs: [],
  };
  if (run.status !== 'completed') {
    return { status: 'RUNNING', reason: 'The workflow has not reached a terminal state.', evidence };
  }
  if (['failure', 'cancelled', 'timed_out', 'action_required', 'stale'].includes(run.conclusion)) {
    if (jobsResult?.jobs) evidence.executionJobs = executionJobsFor(task, jobsResult.jobs);
    return { status: 'FAILED', reason: `The workflow concluded ${run.conclusion}.`, evidence };
  }
  if (run.conclusion === 'skipped') {
    return { status: 'MISSED', reason: 'The scheduled workflow was skipped.', evidence };
  }
  if (run.conclusion !== 'success') {
    return { status: 'UNVERIFIED', reason: 'The workflow conclusion was not recognized.', evidence };
  }
  const hasContract = Boolean(task.executionJobNames?.length || task.executionJobPrefixes?.length);
  if (!hasContract) return { status: 'SUCCESS', reason: 'The workflow completed successfully.', evidence };
  if (!jobsResult || jobsResult.error) {
    return { status: 'UNVERIFIED', reason: 'The workflow was green, but execution-job evidence was unavailable.', evidence };
  }
  const matching = executionJobsFor(task, jobsResult.jobs || []);
  evidence.executionJobs = matching;
  if (matching.length === 0) {
    return {
      status: jobsResult.incomplete ? 'UNVERIFIED' : 'FALSE_GREEN',
      reason: jobsResult.incomplete
        ? 'The workflow was green, but the bounded job list was incomplete.'
        : 'The workflow was green, but no required execution job was present.',
      evidence,
    };
  }
  if (matching.some((job) => ['failure', 'cancelled', 'timed_out', 'action_required', 'stale'].includes(job.conclusion))) {
    return { status: 'FAILED', reason: 'A required execution job failed.', evidence };
  }
  if (matching.every((job) => job.conclusion === 'skipped')) {
    return { status: 'FALSE_GREEN', reason: 'Every required execution job was skipped.', evidence };
  }
  if (task.requireAllExecutionJobs) {
    if (matching.every((job) => job.conclusion === 'success')) {
      return { status: 'SUCCESS', reason: 'All required execution jobs succeeded.', evidence };
    }
    if (matching.some((job) => job.conclusion === 'skipped')) {
      return { status: 'FALSE_GREEN', reason: 'The workflow was green, but required execution was only partial.', evidence };
    }
  } else if (matching.some((job) => job.conclusion === 'success')) {
    return { status: 'SUCCESS', reason: 'A required execution job succeeded.', evidence };
  }
  return { status: 'RUNNING', reason: 'Required execution has not reached a terminal state.', evidence };
}


function taskIsDue(task, windowEnd) {
  if (!task.dueOnDigestWeekdays) return true;
  return task.dueOnDigestWeekdays.includes(zonedParts(windowEnd, CONFIG.timezone).weekday);
}

function aggregateAttemptStatus(attempts, expectedRuns) {
  if (attempts.some((attempt) => attempt.status === 'FAILED')) return 'FAILED';
  if (attempts.some((attempt) => attempt.status === 'FALSE_GREEN')) return 'FALSE_GREEN';
  if (attempts.some((attempt) => attempt.status === 'RUNNING')) return 'RUNNING';
  if (attempts.length < expectedRuns) return 'MISSED';
  if (attempts.every((attempt) => attempt.status === 'SUCCESS')) return 'SUCCESS';
  if (attempts.some((attempt) => attempt.status === 'MISSED')) return 'MISSED';
  return 'UNVERIFIED';
}

export async function evaluateCriticalGithubTask(
  task,
  allRuns,
  windowEnd,
  { fetchImpl = globalThis.fetch, jobsCache = new Map() } = {},
) {
  if (!taskIsDue(task, windowEnd)) {
    return {
      id: task.id,
      name: task.name,
      schedule: task.schedule,
      status: 'NOT_DUE',
      reason: 'This task was not due during the digest window.',
      expectedRuns: 0,
      attempts: [],
    };
  }
  let candidates = allRuns.filter(
    (run) => run.repository === task.repository && run.workflowPath === task.workflowPath,
  );
  if (task.candidateLocalHours?.length) {
    candidates = candidates.filter((run) => {
      if (!run.createdAt) return false;
      const hour = Number(zonedParts(new Date(run.createdAt), task.candidateTimezone || CONFIG.timezone).hour);
      return task.candidateLocalHours.includes(hour);
    });
  }
  candidates.sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)));
  candidates = candidates.slice(0, Math.max(task.expectedRuns + 2, 4));

  const attempts = [];
  for (const run of candidates) {
    let jobsResult = null;
    if (run.id && (task.executionJobNames?.length || task.executionJobPrefixes?.length)) {
      const cacheKey = `${task.repository}#${run.id}`;
      if (!jobsCache.has(cacheKey)) {
        try {
          jobsCache.set(cacheKey, await fetchGithubRunJobs(task.repository, run.id, { fetchImpl }));
        } catch (error) {
          jobsCache.set(cacheKey, { jobs: [], incomplete: true, error: boundedError(error) });
        }
      }
      jobsResult = jobsCache.get(cacheKey);
    }
    attempts.push(classifyGithubRun(task, run, jobsResult));
  }

  if (attempts.length === 0) {
    return {
      id: task.id,
      name: task.name,
      schedule: task.schedule,
      status: 'MISSED',
      reason: 'No schedule-event run was observed in the 24-hour window.',
      expectedRuns: task.expectedRuns,
      attempts: [],
    };
  }

  const status = aggregateAttemptStatus(attempts, task.expectedRuns);
  const reason =
    status === 'MISSED' && attempts.length < task.expectedRuns
      ? `Observed ${attempts.length} of ${task.expectedRuns} expected schedule-event runs.`
      : attempts.find((attempt) => attempt.status === status)?.reason || 'Outcome could not be certified.';
  return {
    id: task.id,
    name: task.name,
    schedule: task.schedule,
    status,
    reason,
    expectedRuns: task.expectedRuns,
    attempts,
  };
}

export function groupObservedGithubRuns(runs, criticalKeys) {
  const groups = new Map();
  for (const run of runs) {
    const key = `${run.repository}|${run.workflowPath}`;
    if (criticalKeys.has(key)) continue;
    if (!groups.has(key)) groups.set(key, { repository: run.repository, workflowPath: run.workflowPath, name: run.name, attempts: [] });
    groups.get(key).attempts.push(run);
  }
  return [...groups.values()]
    .map((group) => {
      group.attempts.sort((a, b) => String(b.createdAt).localeCompare(String(a.createdAt)));
      const latest = group.attempts[0];
      let status = 'UNVERIFIED';
      if (latest.status !== 'completed') status = 'RUNNING';
      else if (latest.conclusion === 'success') status = 'OBSERVED_SUCCESS';
      else if (latest.conclusion === 'skipped') status = 'MISSED';
      else if (['failure', 'cancelled', 'timed_out', 'action_required', 'stale'].includes(latest.conclusion)) status = 'FAILED';
      return { ...group, status, latest, attemptCount: group.attempts.length };
    })
    .sort((a, b) => `${a.repository}|${a.workflowPath}`.localeCompare(`${b.repository}|${b.workflowPath}`));
}

