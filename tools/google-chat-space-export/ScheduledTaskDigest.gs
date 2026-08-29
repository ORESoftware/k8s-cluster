/*
 * One-email daily digest for the portfolio's registered scheduled tasks.
 *
 * The fixed recipient and America/Chicago trigger are intentional. The digest
 * reads public GitHub Actions evidence, verifies execution jobs for critical
 * workflows, and leaves non-GitHub tasks visibly unverified until a durable
 * runtime feed is connected. No credential is required for public repositories;
 * an optional GitHub token may be stored manually as a Script Property under
 * SCHEDULED_TASK_DIGEST_GITHUB_TOKEN, but is never emitted or persisted elsewhere.
 */

'use strict';

const SCHEDULED_TASK_DIGEST = Object.freeze({
  version: 1,
  recipient: 'alexander.d.mills@gmail.com',
  senderName: 'Scheduled Task Digest',
  timezone: 'America/Chicago',
  triggerHandler: 'runScheduledTaskDigest',
  triggerHour: 7,
  triggerMinute: 0,
  lookbackHours: 24,
  githubApiBase: 'https://api.github.com',
  githubTokenProperty: 'SCHEDULED_TASK_DIGEST_GITHUB_TOKEN',
  bootstrapProperty: 'SCHEDULED_TASK_DIGEST_BOOTSTRAP_V1',
  deliveryProperty: 'SCHEDULED_TASK_DIGEST_DELIVERY_V1',
  maxPagesPerRepository: 5,
  maxPagesPerJobList: 3,
  maxResponseChars: 1500000,
  maxObservedWorkflowGroups: 100,
  repositories: Object.freeze([
    'ORESoftware/ai-agent-coordinator.rs',
    'ORESoftware/k8s-cluster',
    'ORESoftware/project-registry',
  ]),
  criticalTasks: Object.freeze([
    Object.freeze({
      id: 'recent-chat-reconciliation',
      name: 'Recent 96-hour ChatGPT reconciliation',
      repository: 'ORESoftware/ai-agent-coordinator.rs',
      workflowPath: '.github/workflows/last-30-hours-introspection.yml',
      schedule: '00:30 America/Lima, daily',
      dueEveryDigest: true,
      executionJobNames: Object.freeze(['enqueue-and-verify']),
      maxCandidateRuns: 3,
    }),
    Object.freeze({
      id: 'nightly-org-maintenance',
      name: 'Nightly organization maintenance',
      repository: 'ORESoftware/ai-agent-coordinator.rs',
      workflowPath: '.github/workflows/nightly-org-maintenance.yml',
      schedule: '01:00 America/Lima, daily',
      dueEveryDigest: true,
      executionJobPrefixes: Object.freeze(['maintain (']),
      maxCandidateRuns: 2,
    }),
    Object.freeze({
      id: 'nightly-artifact-recovery',
      name: 'Nightly artifact recovery',
      repository: 'ORESoftware/ai-agent-coordinator.rs',
      workflowPath: '.github/workflows/nightly-artifact-recovery.yml',
      schedule: '02:17 and 03:47 America/Chicago, daily',
      dueEveryDigest: true,
      executionJobNames: Object.freeze(['enqueue', 'enqueue-and-verify']),
      maxCandidateRuns: 4,
    }),
    Object.freeze({
      id: 'daily-portfolio-briefing',
      name: 'Daily portfolio briefing',
      repository: 'ORESoftware/ai-agent-coordinator.rs',
      workflowPath: '.github/workflows/daily-portfolio-briefing.yml',
      schedule: '08:00 America/Chicago, daily',
      dueEveryDigest: true,
      candidateTimezone: 'America/Chicago',
      candidateLocalHours: Object.freeze([7, 8, 9, 10]),
      executionJobNames: Object.freeze(['enqueue']),
      maxCandidateRuns: 4,
    }),
    Object.freeze({
      id: 'weekly-job-opportunity-digest',
      name: 'Weekly job opportunity digest',
      repository: 'ORESoftware/ai-agent-coordinator.rs',
      workflowPath: '.github/workflows/weekly-job-opportunity-digest.yml',
      schedule: '09:17 America/New_York, Monday',
      dueOnDigestWeekdays: Object.freeze(['Tue']),
      executionJobNames: Object.freeze(['enqueue']),
      maxCandidateRuns: 3,
    }),
  ]),
  externalCoverage: Object.freeze([
    Object.freeze({
      id: 'governed-pr-linear-reconciliation',
      name: 'Governed pull-request and Linear reconciliation',
      schedule: '01:00 America/Chicago, daily',
      status: 'UNVERIFIED',
      reason: 'Kubernetes Job history and the finalizer ledger are not exposed to this mailer yet.',
    }),
    Object.freeze({
      id: 'fleet-interdependency',
      name: 'Fleet interdependency reconciliation',
      schedule: '02:00 America/Chicago, daily',
      status: 'UNVERIFIED',
      reason: 'The Kubernetes CronJob exists in GitOps, but no live Job-result feed is connected.',
    }),
    Object.freeze({
      id: 'messaging-intel-discovery',
      name: 'Messaging-Intel contact discovery',
      schedule: '03:00 America/Lima, daily',
      status: 'UNVERIFIED',
      reason: 'No durable scheduler ledger or runtime evidence endpoint is registered.',
    }),
    Object.freeze({
      id: 'clients-repositories-audit',
      name: 'Clients and repositories audit',
      schedule: 'nightly',
      status: 'UNVERIFIED',
      reason: 'No current machine-readable run feed is registered.',
    }),
    Object.freeze({
      id: 'benefactor-outreach',
      name: 'Benefactor outreach',
      schedule: '06:00 America/Chicago, daily',
      status: 'UNVERIFIED',
      reason: 'The digest can inspect GitOps configuration but not authoritative live CronJob completion evidence.',
    }),
  ]),
});

// Apps Script initializes all project globals before dispatching doGet/doPost or
// a trigger handler. The deployment workflow calls the public health endpoint,
// so a healthy deployment proves the trigger exists and a bounded GitHub API
// probe succeeded. Any bootstrap failure prevents the health response.
const SCHEDULED_TASK_DIGEST_BOOTSTRAP_RESULT = bootstrapScheduledTaskDigest_();

/** Install or repair the one fixed daily trigger and verify runtime dependencies. */
function bootstrapScheduledTaskDigest_() {
  const lock = LockService.getScriptLock();
  lock.waitLock(30000);
  try {
    const trigger = ensureScheduledTaskDigestTrigger_();
    const properties = PropertiesService.getScriptProperties();
    const expectedVersion = String(SCHEDULED_TASK_DIGEST.version);
    const current = properties.getProperty(SCHEDULED_TASK_DIGEST.bootstrapProperty);
    let cached = false;

    if (current) {
      try {
        const parsed = JSON.parse(current);
        cached = parsed && parsed.version === expectedVersion;
      } catch (error) {
        cached = false;
      }
    }

    if (!cached) {
      const probe = githubRequestJson_(
        SCHEDULED_TASK_DIGEST.githubApiBase +
          '/repos/ORESoftware/ai-agent-coordinator.rs',
      );
      if (!probe.json || probe.json.full_name !== 'ORESoftware/ai-agent-coordinator.rs') {
        throw new Error('Scheduled-task digest GitHub bootstrap probe returned an unexpected repository.');
      }
      const quota = Number(MailApp.getRemainingDailyQuota());
      if (!Number.isFinite(quota) || quota < 1) {
        throw new Error('Scheduled-task digest has no remaining mail quota.');
      }
      properties.setProperty(
        SCHEDULED_TASK_DIGEST.bootstrapProperty,
        JSON.stringify({
          version: expectedVersion,
          verifiedAt: new Date().toISOString(),
          triggerHandler: SCHEDULED_TASK_DIGEST.triggerHandler,
        }),
      );
    }

    return {
      ok: true,
      version: expectedVersion,
      triggerHandler: trigger.handler,
      triggerCount: trigger.count,
      timezone: SCHEDULED_TASK_DIGEST.timezone,
      hour: SCHEDULED_TASK_DIGEST.triggerHour,
      minute: SCHEDULED_TASK_DIGEST.triggerMinute,
      verificationCached: cached,
    };
  } finally {
    lock.releaseLock();
  }
}

/** Idempotently maintain exactly one digest trigger without touching other triggers. */
function ensureScheduledTaskDigestTrigger_() {
  const handler = SCHEDULED_TASK_DIGEST.triggerHandler;
  let matching = ScriptApp.getProjectTriggers().filter(function (trigger) {
    return trigger.getHandlerFunction() === handler;
  });

  if (matching.length === 0) {
    ScriptApp.newTrigger(handler)
      .timeBased()
      .atHour(SCHEDULED_TASK_DIGEST.triggerHour)
      .nearMinute(SCHEDULED_TASK_DIGEST.triggerMinute)
      .everyDays(1)
      .inTimezone(SCHEDULED_TASK_DIGEST.timezone)
      .create();
    matching = ScriptApp.getProjectTriggers().filter(function (trigger) {
      return trigger.getHandlerFunction() === handler;
    });
  }

  while (matching.length > 1) {
    ScriptApp.deleteTrigger(matching.pop());
  }

  const verified = ScriptApp.getProjectTriggers().filter(function (trigger) {
    return trigger.getHandlerFunction() === handler;
  });
  if (verified.length !== 1) {
    throw new Error('Scheduled-task digest trigger installation did not converge to exactly one trigger.');
  }
  return { handler: handler, count: verified.length };
}

/** Time-driven entry point. */
function runScheduledTaskDigest() {
  return runScheduledTaskDigestAt_(new Date(), false, 'scheduled');
}

/** Explicit editor-only canary. It does not advance the scheduled delivery key. */
function sendScheduledTaskDigestNow() {
  return runScheduledTaskDigestAt_(new Date(), true, 'manual-canary');
}

function runScheduledTaskDigestAt_(now, force, mode) {
  const instant = now instanceof Date ? now : new Date(now);
  if (Number.isNaN(instant.getTime())) {
    throw new Error('Scheduled-task digest requires a valid execution instant.');
  }

  const logicalDate = Utilities.formatDate(
    instant,
    SCHEDULED_TASK_DIGEST.timezone,
    'yyyy-MM-dd',
  );
  const lock = LockService.getScriptLock();
  if (!lock.tryLock(30000)) {
    throw new Error('Another scheduled-task digest invocation is running.');
  }

  const properties = PropertiesService.getScriptProperties();
  let attemptId = null;
  try {
    if (!force) {
      const existing = parseDeliveryState_(
        properties.getProperty(SCHEDULED_TASK_DIGEST.deliveryProperty),
      );
      if (existing && existing.logicalDate === logicalDate) {
        return {
          status: 'duplicate_suppressed',
          logicalDate: logicalDate,
          priorState: existing.state,
          sentAt: existing.sentAt || null,
        };
      }
      attemptId = Utilities.getUuid();
      properties.setProperty(
        SCHEDULED_TASK_DIGEST.deliveryProperty,
        JSON.stringify({
          state: 'pending',
          logicalDate: logicalDate,
          attemptId: attemptId,
          startedAt: instant.toISOString(),
        }),
      );
    }

    const report = collectScheduledTaskDigest_(instant);
    const rendered = renderScheduledTaskDigest_(report, mode || 'scheduled');
    MailApp.sendEmail({
      to: SCHEDULED_TASK_DIGEST.recipient,
      subject: rendered.subject,
      body: rendered.text,
      htmlBody: rendered.html,
      name: SCHEDULED_TASK_DIGEST.senderName,
    });

    if (!force) {
      properties.setProperty(
        SCHEDULED_TASK_DIGEST.deliveryProperty,
        JSON.stringify({
          state: 'sent',
          logicalDate: logicalDate,
          attemptId: attemptId,
          sentAt: new Date().toISOString(),
          subject: rendered.subject,
          digestSha256: sha256Hex_(rendered.text),
        }),
      );
    }

    return {
      status: force ? 'manual_canary_sent' : 'sent',
      logicalDate: logicalDate,
      recipient: SCHEDULED_TASK_DIGEST.recipient,
      subject: rendered.subject,
      summary: report.summary,
    };
  } catch (error) {
    if (!force && attemptId) {
      const current = parseDeliveryState_(
        properties.getProperty(SCHEDULED_TASK_DIGEST.deliveryProperty),
      );
      if (current && current.state === 'pending' && current.attemptId === attemptId) {
        properties.deleteProperty(SCHEDULED_TASK_DIGEST.deliveryProperty);
      }
    }
    throw error;
  } finally {
    lock.releaseLock();
  }
}

function parseDeliveryState_(raw) {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === 'object' ? parsed : null;
  } catch (error) {
    return null;
  }
}

function collectScheduledTaskDigest_(now) {
  const windowEnd = new Date(now.getTime());
  const windowStart = new Date(
    windowEnd.getTime() - SCHEDULED_TASK_DIGEST.lookbackHours * 60 * 60 * 1000,
  );
  const repositoryEvidence = {};
  const sourceErrors = [];
  let allRuns = [];

  SCHEDULED_TASK_DIGEST.repositories.forEach(function (repository) {
    try {
      const evidence = fetchRepositoryScheduleRuns_(repository, windowStart);
      repositoryEvidence[repository] = evidence;
      allRuns = allRuns.concat(evidence.runs);
      if (evidence.incomplete) {
        sourceErrors.push({
          source: repository,
          error: 'GitHub schedule-run pagination exceeded the configured safety bound.',
        });
      }
    } catch (error) {
      repositoryEvidence[repository] = { runs: [], incomplete: true };
      sourceErrors.push({
        source: repository,
        error: boundedError_(error),
      });
    }
  });

  const jobsCache = {};
  const critical = SCHEDULED_TASK_DIGEST.criticalTasks.map(function (task) {
    return evaluateCriticalTask_(task, allRuns, windowEnd, jobsCache);
  });
  const criticalKeys = {};
  SCHEDULED_TASK_DIGEST.criticalTasks.forEach(function (task) {
    criticalKeys[task.repository + '|' + task.workflowPath] = true;
  });

  const observed = groupObservedScheduleRuns_(allRuns).filter(function (group) {
    return !criticalKeys[group.repository + '|' + group.workflowPath];
  });
  const observedOverflow = Math.max(
    0,
    observed.length - SCHEDULED_TASK_DIGEST.maxObservedWorkflowGroups,
  );
  const boundedObserved = observed.slice(0, SCHEDULED_TASK_DIGEST.maxObservedWorkflowGroups);

  const external = SCHEDULED_TASK_DIGEST.externalCoverage.map(function (entry) {
    return {
      id: entry.id,
      name: entry.name,
      schedule: entry.schedule,
      status: entry.status,
      reason: entry.reason,
    };
  });

  const summary = summarizeTaskOutcomes_(critical, boundedObserved, external, sourceErrors);
  return {
    schemaVersion: 'scheduled_task_digest.v1',
    generatedAt: windowEnd.toISOString(),
    timezone: SCHEDULED_TASK_DIGEST.timezone,
    windowStart: windowStart.toISOString(),
    windowEnd: windowEnd.toISOString(),
    critical: critical,
    observed: boundedObserved,
    observedOverflow: observedOverflow,
    external: external,
    sourceErrors: sourceErrors,
    repositories: repositoryEvidence,
    summary: summary,
  };
}

function fetchRepositoryScheduleRuns_(repository, since) {
  const encodedRepository = repository
    .split('/')
    .map(encodeURIComponent)
    .join('/');
  let url =
    SCHEDULED_TASK_DIGEST.githubApiBase +
    '/repos/' +
    encodedRepository +
    '/actions/runs?event=schedule&per_page=100&created=' +
    encodeURIComponent('>=' + since.toISOString());
  let page = 0;
  let incomplete = false;
  const runs = [];

  while (url && page < SCHEDULED_TASK_DIGEST.maxPagesPerRepository) {
    page += 1;
    const response = githubRequestJson_(url);
    const payload = response.json;
    if (!payload || !Array.isArray(payload.workflow_runs)) {
      throw new Error('GitHub schedule-run response was missing workflow_runs.');
    }
    payload.workflow_runs.forEach(function (run) {
      runs.push(normalizeWorkflowRun_(repository, run));
    });
    url = nextGithubLink_(response.headers);
  }
  if (url) incomplete = true;

  runs.sort(function (left, right) {
    return String(right.createdAt).localeCompare(String(left.createdAt));
  });
  return {
    repository: repository,
    runs: runs,
    pages: page,
    incomplete: incomplete,
  };
}

function normalizeWorkflowRun_(repository, run) {
  const id = Number(run && run.id);
  return {
    repository: repository,
    id: Number.isFinite(id) ? id : null,
    name: publicText_(run && run.name, 160) || 'Unnamed workflow',
    workflowPath: publicText_(run && run.path, 300) || 'unknown-workflow',
    status: publicText_(run && run.status, 40) || 'unknown',
    conclusion: publicText_(run && run.conclusion, 40) || null,
    createdAt: publicText_(run && run.created_at, 64) || null,
    updatedAt: publicText_(run && run.updated_at, 64) || null,
    htmlUrl: safeGithubUrl_(run && run.html_url),
    runNumber: Number(run && run.run_number) || null,
    runAttempt: Number(run && run.run_attempt) || 1,
    headBranch: publicText_(run && run.head_branch, 160) || null,
  };
}

function evaluateCriticalTask_(task, allRuns, windowEnd, jobsCache) {
  const due = criticalTaskDue_(task, windowEnd);
  let candidates = allRuns.filter(function (run) {
    return run.repository === task.repository && run.workflowPath === task.workflowPath;
  });

  if (task.candidateLocalHours && task.candidateLocalHours.length) {
    candidates = candidates.filter(function (run) {
      if (!run.createdAt) return false;
      const hour = Number(
        Utilities.formatDate(
          new Date(run.createdAt),
          task.candidateTimezone || SCHEDULED_TASK_DIGEST.timezone,
          'H',
        ),
      );
      return task.candidateLocalHours.indexOf(hour) !== -1;
    });
  }

  candidates.sort(function (left, right) {
    return String(right.createdAt).localeCompare(String(left.createdAt));
  });
  candidates = candidates.slice(0, task.maxCandidateRuns || 3);

  if (candidates.length === 0) {
    return {
      id: task.id,
      name: task.name,
      schedule: task.schedule,
      status: due ? 'MISSED' : 'NOT_DUE',
      reason: due
        ? 'No schedule-event workflow run was observed in the 24-hour window.'
        : 'This task was not due during the digest window.',
      attempts: [],
      selected: null,
    };
  }

  const attempts = candidates.map(function (run) {
    let jobs = null;
    if (run.id && taskHasExecutionContract_(task)) {
      const cacheKey = task.repository + '#' + run.id;
      if (!Object.prototype.hasOwnProperty.call(jobsCache, cacheKey)) {
        try {
          jobsCache[cacheKey] = fetchWorkflowRunJobs_(task.repository, run.id);
        } catch (error) {
          jobsCache[cacheKey] = {
            jobs: [],
            incomplete: true,
            error: boundedError_(error),
          };
        }
      }
      jobs = jobsCache[cacheKey];
    }
    return classifyCriticalRun_(task, run, jobs);
  });

  const selected = chooseCriticalAttempt_(attempts);
  return {
    id: task.id,
    name: task.name,
    schedule: task.schedule,
    status: selected.status,
    reason: selected.reason,
    attempts: attempts,
    selected: selected,
  };
}

function criticalTaskDue_(task, windowEnd) {
  if (task.dueEveryDigest) return true;
  if (task.dueOnDigestWeekdays && task.dueOnDigestWeekdays.length) {
    const weekday = Utilities.formatDate(
      windowEnd,
      SCHEDULED_TASK_DIGEST.timezone,
      'EEE',
    );
    return task.dueOnDigestWeekdays.indexOf(weekday) !== -1;
  }
  return false;
}

function taskHasExecutionContract_(task) {
  return Boolean(
    (task.executionJobNames && task.executionJobNames.length) ||
      (task.executionJobPrefixes && task.executionJobPrefixes.length),
  );
}

function fetchWorkflowRunJobs_(repository, runId) {
  const encodedRepository = repository
    .split('/')
    .map(encodeURIComponent)
    .join('/');
  let url =
    SCHEDULED_TASK_DIGEST.githubApiBase +
    '/repos/' +
    encodedRepository +
    '/actions/runs/' +
    encodeURIComponent(String(runId)) +
    '/jobs?per_page=100';
  let page = 0;
  let incomplete = false;
  const jobs = [];

  while (url && page < SCHEDULED_TASK_DIGEST.maxPagesPerJobList) {
    page += 1;
    const response = githubRequestJson_(url);
    const payload = response.json;
    if (!payload || !Array.isArray(payload.jobs)) {
      throw new Error('GitHub workflow-job response was missing jobs.');
    }
    payload.jobs.forEach(function (job) {
      jobs.push({
        id: Number(job && job.id) || null,
        name: publicText_(job && job.name, 240) || 'Unnamed job',
        status: publicText_(job && job.status, 40) || 'unknown',
        conclusion: publicText_(job && job.conclusion, 40) || null,
        startedAt: publicText_(job && job.started_at, 64) || null,
        completedAt: publicText_(job && job.completed_at, 64) || null,
      });
    });
    url = nextGithubLink_(response.headers);
  }
  if (url) incomplete = true;
  return { jobs: jobs, incomplete: incomplete, error: null };
}

function classifyCriticalRun_(task, run, jobsResult) {
  const conclusion = String(run.conclusion || '').toLowerCase();
  const status = String(run.status || '').toLowerCase();
  const evidence = {
    runId: run.id,
    runNumber: run.runNumber,
    runAttempt: run.runAttempt,
    createdAt: run.createdAt,
    updatedAt: run.updatedAt,
    htmlUrl: run.htmlUrl,
    workflowConclusion: conclusion || null,
    executionJobs: [],
  };

  if (status !== 'completed') {
    return {
      status: 'RUNNING',
      reason: 'The workflow has not reached a terminal conclusion.',
      evidence: evidence,
    };
  }
  if (
    ['failure', 'cancelled', 'timed_out', 'action_required', 'stale'].indexOf(conclusion) !== -1
  ) {
    if (jobsResult && jobsResult.jobs) {
      evidence.executionJobs = matchingExecutionJobs_(task, jobsResult.jobs);
    }
    return {
      status: 'FAILED',
      reason: 'The scheduled workflow concluded ' + (conclusion || 'failure') + '.',
      evidence: evidence,
    };
  }
  if (conclusion === 'skipped') {
    return {
      status: 'MISSED',
      reason: 'The scheduled workflow was skipped before completing its execution contract.',
      evidence: evidence,
    };
  }
  if (conclusion !== 'success') {
    return {
      status: 'UNVERIFIED',
      reason: 'The workflow conclusion was not a recognized terminal outcome.',
      evidence: evidence,
    };
  }
  if (!taskHasExecutionContract_(task)) {
    return {
      status: 'SUCCESS',
      reason: 'The workflow completed successfully.',
      evidence: evidence,
    };
  }
  if (!jobsResult || jobsResult.error) {
    return {
      status: 'UNVERIFIED',
      reason: jobsResult && jobsResult.error
        ? 'The workflow was green, but execution-job evidence could not be fetched.'
        : 'The workflow was green, but execution-job evidence was unavailable.',
      evidence: evidence,
    };
  }

  const executionJobs = matchingExecutionJobs_(task, jobsResult.jobs || []);
  evidence.executionJobs = executionJobs;
  if (executionJobs.length === 0) {
    return {
      status: jobsResult.incomplete ? 'UNVERIFIED' : 'FALSE_GREEN',
      reason: jobsResult.incomplete
        ? 'The workflow was green, but the bounded job listing was incomplete.'
        : 'The workflow was green, but no required execution job was present.',
      evidence: evidence,
    };
  }
  if (executionJobs.some(function (job) { return job.conclusion === 'success'; })) {
    return {
      status: 'SUCCESS',
      reason: 'A required execution job completed successfully.',
      evidence: evidence,
    };
  }
  if (
    executionJobs.some(function (job) {
      return ['failure', 'cancelled', 'timed_out', 'action_required', 'stale'].indexOf(job.conclusion) !== -1;
    })
  ) {
    return {
      status: 'FAILED',
      reason: 'A required execution job failed.',
      evidence: evidence,
    };
  }
  if (executionJobs.every(function (job) { return job.conclusion === 'skipped'; })) {
    return {
      status: 'FALSE_GREEN',
      reason: 'The workflow was green while every required execution job was skipped.',
      evidence: evidence,
    };
  }
  return {
    status: 'RUNNING',
    reason: 'A required execution job has not reached a terminal conclusion.',
    evidence: evidence,
  };
}

function matchingExecutionJobs_(task, jobs) {
  return jobs.filter(function (job) {
    if (task.executionJobNames && task.executionJobNames.indexOf(job.name) !== -1) {
      return true;
    }
    return Boolean(
      task.executionJobPrefixes &&
        task.executionJobPrefixes.some(function (prefix) {
          return job.name.indexOf(prefix) === 0;
        }),
    );
  });
}

function chooseCriticalAttempt_(attempts) {
  const priority = {
    SUCCESS: 0,
    RUNNING: 1,
    FAILED: 2,
    FALSE_GREEN: 3,
    MISSED: 4,
    UNVERIFIED: 5,
    NOT_DUE: 6,
  };
  return attempts.slice().sort(function (left, right) {
    const leftPriority = Object.prototype.hasOwnProperty.call(priority, left.status)
      ? priority[left.status]
      : 99;
    const rightPriority = Object.prototype.hasOwnProperty.call(priority, right.status)
      ? priority[right.status]
      : 99;
    if (leftPriority !== rightPriority) return leftPriority - rightPriority;
    return String(right.evidence && right.evidence.createdAt).localeCompare(
      String(left.evidence && left.evidence.createdAt),
    );
  })[0];
}

function groupObservedScheduleRuns_(runs) {
  const groups = {};
  runs.forEach(function (run) {
    const key = run.repository + '|' + run.workflowPath;
    if (!groups[key]) {
      groups[key] = {
        repository: run.repository,
        workflowPath: run.workflowPath,
        name: run.name,
        attempts: [],
      };
    }
    groups[key].attempts.push(run);
  });

  return Object.keys(groups)
    .map(function (key) {
      const group = groups[key];
      group.attempts.sort(function (left, right) {
        return String(right.createdAt).localeCompare(String(left.createdAt));
      });
      const counts = {};
      group.attempts.forEach(function (run) {
        const outcome = publicText_(run.conclusion || run.status || 'unknown', 40).toUpperCase();
        counts[outcome] = (counts[outcome] || 0) + 1;
      });
      const latest = group.attempts[0];
      return {
        repository: group.repository,
        workflowPath: group.workflowPath,
        name: group.name,
        status: observedWorkflowStatus_(latest),
        attemptCount: group.attempts.length,
        counts: counts,
        latest: latest,
      };
    })
    .sort(function (left, right) {
      return (left.repository + '|' + left.workflowPath).localeCompare(
        right.repository + '|' + right.workflowPath,
      );
    });
}

function observedWorkflowStatus_(run) {
  if (!run || run.status !== 'completed') return 'RUNNING';
  const conclusion = String(run.conclusion || '').toLowerCase();
  if (conclusion === 'success') return 'OBSERVED_SUCCESS';
  if (conclusion === 'skipped') return 'SKIPPED';
  if (['failure', 'cancelled', 'timed_out', 'action_required', 'stale'].indexOf(conclusion) !== -1) {
    return 'FAILED';
  }
  return 'UNVERIFIED';
}

function summarizeTaskOutcomes_(critical, observed, external, sourceErrors) {
  const counts = {
    certifiedSuccess: 0,
    failed: 0,
    falseGreen: 0,
    missed: 0,
    running: 0,
    unverified: 0,
    notDue: 0,
    observedSuccess: 0,
    sourceErrors: sourceErrors.length,
  };

  critical.forEach(function (task) {
    if (task.status === 'SUCCESS') counts.certifiedSuccess += 1;
    else if (task.status === 'FAILED') counts.failed += 1;
    else if (task.status === 'FALSE_GREEN') counts.falseGreen += 1;
    else if (task.status === 'MISSED') counts.missed += 1;
    else if (task.status === 'RUNNING') counts.running += 1;
    else if (task.status === 'NOT_DUE') counts.notDue += 1;
    else counts.unverified += 1;
  });
  observed.forEach(function (group) {
    if (group.status === 'OBSERVED_SUCCESS') counts.observedSuccess += 1;
    else if (group.status === 'FAILED') counts.failed += 1;
    else if (group.status === 'RUNNING') counts.running += 1;
    else counts.unverified += 1;
  });
  external.forEach(function () {
    counts.unverified += 1;
  });
  return counts;
}

function renderScheduledTaskDigest_(report, mode) {
  const manual = mode === 'manual-canary';
  const prefix = manual ? '[MANUAL CANARY] ' : '';
  const localDate = Utilities.formatDate(
    new Date(report.windowEnd),
    report.timezone,
    'yyyy-MM-dd',
  );
  const summary = report.summary;
  const subject =
    prefix +
    '[Scheduled task digest] ' +
    localDate +
    ' — ' +
    summary.certifiedSuccess +
    ' certified, ' +
    summary.failed +
    ' failed, ' +
    summary.falseGreen +
    ' false-green, ' +
    summary.missed +
    ' missed';

  const lines = [
    'Scheduled task digest',
    '',
    'Window: ' + formatCentralInstant_(report.windowStart) + ' to ' + formatCentralInstant_(report.windowEnd),
    'Generated: ' + formatCentralInstant_(report.generatedAt),
    'Mode: ' + (manual ? 'manual canary' : 'scheduled'),
    '',
    'Summary',
    '  Certified success: ' + summary.certifiedSuccess,
    '  Failed: ' + summary.failed,
    '  False green: ' + summary.falseGreen,
    '  Missed/skipped: ' + summary.missed,
    '  Running: ' + summary.running,
    '  Unverified: ' + summary.unverified,
    '  Not due: ' + summary.notDue,
    '  Other observed-success workflows: ' + summary.observedSuccess,
    '  Source errors: ' + summary.sourceErrors,
    '',
    'Critical registered tasks',
  ];

  report.critical.forEach(function (task) {
    lines.push(
      '- [' + task.status + '] ' + task.name + ' — ' + task.schedule + ' — ' + task.reason,
    );
    if (task.selected && task.selected.evidence) {
      const evidence = task.selected.evidence;
      lines.push(
        '    Evidence: run ' +
          (evidence.runId || 'unknown') +
          ', created ' +
          formatCentralInstant_(evidence.createdAt) +
          (evidence.htmlUrl ? ', ' + evidence.htmlUrl : ''),
      );
      if (evidence.executionJobs && evidence.executionJobs.length) {
        lines.push(
          '    Execution jobs: ' +
            evidence.executionJobs
              .map(function (job) {
                return job.name + '=' + (job.conclusion || job.status || 'unknown');
              })
              .join(', '),
        );
      }
    }
    if (task.attempts && task.attempts.length > 1) {
      lines.push('    Attempts reviewed: ' + task.attempts.length);
    }
  });

  lines.push('', 'Other observed GitHub schedule workflows');
  if (report.observed.length === 0) {
    lines.push('- None beyond the critical registry.');
  } else {
    report.observed.forEach(function (group) {
      const latest = group.latest || {};
      lines.push(
        '- [' +
          group.status +
          '] ' +
          group.repository +
          ' / ' +
          group.name +
          ' — attempts=' +
          group.attemptCount +
          ', latest=' +
          formatCentralInstant_(latest.createdAt) +
          (latest.htmlUrl ? ', ' + latest.htmlUrl : ''),
      );
    });
  }
  if (report.observedOverflow) {
    lines.push(
      '- Coverage warning: ' +
        report.observedOverflow +
        ' additional workflow groups exceeded the email safety bound.',
    );
  }

  lines.push('', 'Registered schedules without authoritative runtime feeds');
  report.external.forEach(function (entry) {
    lines.push(
      '- [' + entry.status + '] ' + entry.name + ' — ' + entry.schedule + ' — ' + entry.reason,
    );
  });

  if (report.sourceErrors.length) {
    lines.push('', 'Source errors');
    report.sourceErrors.forEach(function (entry) {
      lines.push('- ' + entry.source + ': ' + entry.error);
    });
  }

  lines.push(
    '',
    'Interpretation',
    'A GitHub workflow conclusion of success is counted as certified only when its required execution job also succeeded. Missing runtime feeds remain UNVERIFIED and are never treated as success.',
    '',
    'Tracking: DEN-3562',
  );

  const text = lines.join('\n');
  const htmlRows = report.critical
    .map(function (task) {
      const evidence = task.selected && task.selected.evidence ? task.selected.evidence : null;
      const evidenceHtml = evidence
        ? 'Run ' +
          escapeHtml_(String(evidence.runId || 'unknown')) +
          ' at ' +
          escapeHtml_(formatCentralInstant_(evidence.createdAt)) +
          (evidence.htmlUrl
            ? ' — <a href="' + escapeHtml_(evidence.htmlUrl) + '">evidence</a>'
            : '')
        : 'No run evidence';
      return (
        '<tr><td><strong>' +
        escapeHtml_(task.status) +
        '</strong></td><td>' +
        escapeHtml_(task.name) +
        '<br><small>' +
        escapeHtml_(task.schedule) +
        '</small></td><td>' +
        escapeHtml_(task.reason) +
        '<br><small>' +
        evidenceHtml +
        '</small></td></tr>'
      );
    })
    .join('');

  const observedHtml = report.observed
    .map(function (group) {
      const latest = group.latest || {};
      return (
        '<li><strong>' +
        escapeHtml_(group.status) +
        '</strong> — ' +
        escapeHtml_(group.repository + ' / ' + group.name) +
        ' — ' +
        escapeHtml_(String(group.attemptCount)) +
        ' attempt(s), latest ' +
        escapeHtml_(formatCentralInstant_(latest.createdAt)) +
        (latest.htmlUrl
          ? ' — <a href="' + escapeHtml_(latest.htmlUrl) + '">run</a>'
          : '') +
        '</li>'
      );
    })
    .join('');

  const externalHtml = report.external
    .map(function (entry) {
      return (
        '<li><strong>' +
        escapeHtml_(entry.status) +
        '</strong> — ' +
        escapeHtml_(entry.name + ' — ' + entry.schedule + ' — ' + entry.reason) +
        '</li>'
      );
    })
    .join('');

  const sourceErrorHtml = report.sourceErrors.length
    ? '<h2>Source errors</h2><ul>' +
      report.sourceErrors
        .map(function (entry) {
          return '<li>' + escapeHtml_(entry.source + ': ' + entry.error) + '</li>';
        })
        .join('') +
      '</ul>'
    : '';

  const html =
    '<div style="font-family:Arial,sans-serif;line-height:1.45">' +
    '<h1>Scheduled task digest</h1>' +
    '<p><strong>Window:</strong> ' +
    escapeHtml_(formatCentralInstant_(report.windowStart)) +
    ' to ' +
    escapeHtml_(formatCentralInstant_(report.windowEnd)) +
    '</p>' +
    '<p><strong>Summary:</strong> ' +
    escapeHtml_(
      summary.certifiedSuccess +
        ' certified, ' +
        summary.failed +
        ' failed, ' +
        summary.falseGreen +
        ' false-green, ' +
        summary.missed +
        ' missed, ' +
        summary.unverified +
        ' unverified',
    ) +
    '</p>' +
    '<h2>Critical registered tasks</h2>' +
    '<table border="1" cellpadding="6" cellspacing="0" style="border-collapse:collapse">' +
    '<thead><tr><th>Outcome</th><th>Task</th><th>Evidence</th></tr></thead><tbody>' +
    htmlRows +
    '</tbody></table>' +
    '<h2>Other observed GitHub schedule workflows</h2><ul>' +
    (observedHtml || '<li>None beyond the critical registry.</li>') +
    '</ul>' +
    '<h2>Registered schedules without authoritative runtime feeds</h2><ul>' +
    externalHtml +
    '</ul>' +
    sourceErrorHtml +
    '<p><strong>Interpretation:</strong> a green workflow is certified only when its required execution job also succeeded. Missing runtime feeds remain unverified.</p>' +
    '<p>Tracking: DEN-3562</p>' +
    '</div>';

  return { subject: subject, text: text, html: html };
}

function githubRequestJson_(url) {
  if (String(url).indexOf(SCHEDULED_TASK_DIGEST.githubApiBase + '/') !== 0) {
    throw new Error('Scheduled-task digest refused a non-GitHub API URL.');
  }
  const headers = {
    Accept: 'application/vnd.github+json',
    'X-GitHub-Api-Version': '2022-11-28',
    'User-Agent': 'oresoftware-scheduled-task-digest/1',
  };
  const token = PropertiesService.getScriptProperties().getProperty(
    SCHEDULED_TASK_DIGEST.githubTokenProperty,
  );
  if (token && String(token).trim()) {
    headers.Authorization = 'Bearer ' + String(token).trim();
  }

  const response = UrlFetchApp.fetch(url, {
    method: 'get',
    headers: headers,
    followRedirects: false,
    muteHttpExceptions: true,
  });
  const status = Number(response.getResponseCode());
  const body = String(response.getContentText() || '');
  if (body.length > SCHEDULED_TASK_DIGEST.maxResponseChars) {
    throw new Error('GitHub API response exceeded the configured size limit.');
  }
  if (status < 200 || status >= 300) {
    throw new Error('GitHub API returned HTTP ' + status + '.');
  }

  let parsed;
  try {
    parsed = body ? JSON.parse(body) : {};
  } catch (error) {
    throw new Error('GitHub API response was not valid JSON.');
  }
  return {
    json: parsed,
    headers: response.getAllHeaders ? response.getAllHeaders() : response.getHeaders(),
  };
}

function nextGithubLink_(headers) {
  if (!headers) return null;
  let link = null;
  Object.keys(headers).some(function (key) {
    if (String(key).toLowerCase() === 'link') {
      link = headers[key];
      return true;
    }
    return false;
  });
  if (Array.isArray(link)) link = link.join(',');
  const match = String(link || '').match(/<([^>]+)>;\s*rel="next"/);
  if (!match) return null;
  const next = match[1];
  if (next.indexOf(SCHEDULED_TASK_DIGEST.githubApiBase + '/') !== 0) {
    throw new Error('GitHub pagination attempted to leave the API origin.');
  }
  return next;
}

function safeGithubUrl_(value) {
  const text = String(value || '').trim();
  return /^https:\/\/github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/actions\/runs\/\d+$/.test(text)
    ? text
    : null;
}

function publicText_(value, maxLength) {
  let text = String(value == null ? '' : value);
  text = text.replace(/ghp_[A-Za-z0-9]{20,}/g, '[REDACTED]');
  text = text.replace(/lin_api_[A-Za-z0-9]{20,}/g, '[REDACTED]');
  text = text.replace(/SG\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}/g, '[REDACTED]');
  text = text.replace(/Bearer\s+[A-Za-z0-9._-]{16,}/gi, 'Bearer [REDACTED]');
  text = text.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, ' ');
  return text.trim().slice(0, maxLength || 500);
}

function boundedError_(error) {
  const message = publicText_(error && error.message ? error.message : error, 240);
  return message || 'Unknown bounded source error.';
}

function formatCentralInstant_(value) {
  if (!value) return 'unknown';
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return 'unknown';
  return Utilities.formatDate(
    date,
    SCHEDULED_TASK_DIGEST.timezone,
    'yyyy-MM-dd HH:mm z',
  );
}

function sha256Hex_(value) {
  const bytes = Utilities.computeDigest(
    Utilities.DigestAlgorithm.SHA_256,
    String(value),
    Utilities.Charset.UTF_8,
  );
  return bytes
    .map(function (byte) {
      const unsigned = byte < 0 ? byte + 256 : byte;
      return ('0' + unsigned.toString(16)).slice(-2);
    })
    .join('');
}

function escapeHtml_(value) {
  return String(value == null ? '' : value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}
