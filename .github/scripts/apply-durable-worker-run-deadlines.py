#!/usr/bin/env python3
"""Apply the durable run-deadline feature to the worker runtime.

This temporary, fail-closed applicator is used by the PR builder. Exact source
shape checks make drift fail before any commit or push.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path("remote/deployments/durable-worker-server-rs")
MODEL = ROOT / "src/model.rs"
ENGINE = ROOT / "src/engine/mod.rs"
STATE_TESTS = ROOT / "tests/state_machine.rs"
SMOKE = ROOT / "tests/gha_smoke.mjs"
PROTOCOL = ROOT / "PROTOCOL.md"
README = ROOT / "README.md"
OPERATIONS = ROOT / "OPERATIONS.md"
CONTRACT = Path("remote/tests/general/durable-worker-server-config.test.mjs")


def replace_once(path: Path, old: str, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(
            f"{path}: expected exactly one occurrence, found {count}: {old[:120]!r}"
        )
    path.write_text(text.replace(old, new, 1), encoding="utf-8")


def append_once(path: Path, marker: str, addition: str) -> None:
    text = path.read_text(encoding="utf-8")
    if marker in text:
        raise SystemExit(f"{path}: marker already present: {marker}")
    path.write_text(text.rstrip() + "\n\n" + addition.strip() + "\n", encoding="utf-8")


def add_field_to_literals(path: Path, type_name: str, field_line: str) -> None:
    text = path.read_text(encoding="utf-8")
    pattern = re.compile(rf"\b{re.escape(type_name)}\s*\{{")
    cursor = 0
    changed = False
    while True:
        match = pattern.search(text, cursor)
        if not match:
            break
        prefix = text[max(0, match.start() - 48) : match.start()]
        if re.search(r"\bstruct\s*$", prefix):
            cursor = match.end()
            continue
        brace = text.find("{", match.start(), match.end())
        if brace < 0:
            raise SystemExit(f"{path}: malformed {type_name} occurrence")
        preview = text[brace + 1 : brace + 600]
        if "deadline_ms:" in preview:
            cursor = brace + 1
            continue
        newline = text.find("\n", brace)
        if newline < 0 or text[brace + 1 : newline].strip():
            raise SystemExit(f"{path}: unsupported one-line {type_name} literal")
        indent_match = re.match(r"[ \t]*", text[newline + 1 :])
        assert indent_match is not None
        indent = indent_match.group(0)
        insertion = f"\n{indent}{field_line},"
        text = text[: brace + 1] + insertion + text[brace + 1 :]
        cursor = brace + 1 + len(insertion)
        changed = True
    if changed:
        path.write_text(text, encoding="utf-8")


def patch_model() -> None:
    replace_once(
        MODEL,
        """pub struct SubmitRunRequest {
    pub idempotency_key: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub metadata: JsonObject,
    pub steps: Vec<StepDefinition>,
}""",
        """pub struct SubmitRunRequest {
    pub idempotency_key: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub metadata: JsonObject,
    /// Absolute Unix epoch deadline in milliseconds.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    pub steps: Vec<StepDefinition>,
}""",
    )
    replace_once(
        MODEL,
        """    #[serde(default)]
    #[schema(value_type = Object)]
    pub metadata: JsonObject,
    #[serde(default)]
    pub priority: i32,""",
        """    #[serde(default)]
    #[schema(value_type = Object)]
    pub metadata: JsonObject,
    /// Absolute Unix epoch deadline in milliseconds.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    #[serde(default)]
    pub priority: i32,""",
    )
    replace_once(
        MODEL,
        """            metadata: self.metadata,
            steps: vec![StepDefinition {""",
        """            metadata: self.metadata,
            deadline_ms: self.deadline_ms,
            steps: vec![StepDefinition {""",
    )
    replace_once(
        MODEL,
        """    pub step_ids: BTreeMap<String, String>,
    pub counts: RunCounts,
    pub created_at_ms: u64,""",
        """    pub step_ids: BTreeMap<String, String>,
    pub counts: RunCounts,
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    pub created_at_ms: u64,""",
    )


def patch_metrics() -> None:
    replace_once(
        ENGINE,
        """    pub runs_submitted_total: AtomicU64,
    pub idempotent_replays_total: AtomicU64,""",
        """    pub runs_submitted_total: AtomicU64,
    pub run_deadlines_exceeded_total: AtomicU64,
    pub idempotent_replays_total: AtomicU64,""",
    )
    replace_once(
        ENGINE,
        """            (
                "dd_durable_idempotent_replays_total",
                &self.idempotent_replays_total,
            ),""",
        """            (
                "dd_durable_run_deadlines_exceeded_total",
                &self.run_deadlines_exceeded_total,
            ),
            (
                "dd_durable_idempotent_replays_total",
                &self.idempotent_replays_total,
            ),""",
    )


def patch_submission() -> None:
    replace_once(
        ENGINE,
        """        validate_run_request(&request)?;
        let now = self.now_ms();
        let request_hash = stable_hash(&request)?;

        let (run_id, idempotent_replay) =""",
        """        validate_run_request(&request)?;
        let now = self.now_ms();
        let request_hash = stable_hash(&request)?;

        if request
            .deadline_ms
            .is_some_and(|deadline_ms| deadline_ms <= now)
        {
            let Some(idempotency_key) = request.idempotency_key.as_deref() else {
                return Err(EngineError::InvalidRequest(
                    "deadlineMs must be greater than the current server time".to_string(),
                ));
            };
            let record_key = idempotency_key_key(idempotency_key);
            let Some(existing) = self.load::<IdempotencyRecord>(&record_key).await? else {
                return Err(EngineError::InvalidRequest(
                    "deadlineMs must be greater than the current server time".to_string(),
                ));
            };
            if existing.value.request_hash != request_hash {
                return Err(EngineError::IdempotencyMismatch);
            }
            if self
                .load::<RunRecord>(&run_key(&existing.value.run_id))
                .await?
                .is_some()
            {
                self.expire_run_if_due(&existing.value.run_id, now).await?;
                self.metrics
                    .idempotent_replays_total
                    .fetch_add(1, Ordering::Relaxed);
                return self
                    .idempotent_submit_response(&existing.value.run_id)
                    .await;
            }
        }

        let (run_id, idempotent_replay) =""",
    )
    replace_once(
        ENGINE,
        """            step_ids,
            counts: counts_for_steps(&materialized),
            created_at_ms: now,""",
        """            step_ids,
            counts: counts_for_steps(&materialized),
            deadline_ms: request.deadline_ms,
            created_at_ms: now,""",
    )
    replace_once(
        ENGINE,
        """                "name": run.name,
                "stepCount": run.counts.total,
                "idempotent": idempotent_replay,""",
        """                "name": run.name,
                "stepCount": run.counts.total,
                "deadlineMs": run.deadline_ms,
                "idempotent": idempotent_replay,""",
    )
    replace_once(
        ENGINE,
        """        .await;
        Ok(SubmitRunResponse {
            run_id,
            status: RunStatus::Pending,
            idempotent_replay,
        })
    }

    pub async fn get_run_snapshot""",
        """        .await;
        self.expire_run_if_due(&run_id, now).await?;
        let status = self
            .load::<RunRecord>(&run_key(&run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.clone(),
            })?
            .value
            .status;
        Ok(SubmitRunResponse {
            run_id,
            status,
            idempotent_replay,
        })
    }

    pub async fn get_run_snapshot""",
    )


def patch_mutation_boundaries() -> None:
    replace_once(
        ENGINE,
        """        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            if current.value.status == StepStatus::Running
                && current_lease_matches(&current.value, &command)
            {""",
        """        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            self.ensure_run_open_for_mutation(&current.value.run_id, now)
                .await?;
            if current.value.status == StepStatus::Running
                && current_lease_matches(&current.value, &command)
            {""",
    )
    replace_once(
        ENGINE,
        """        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            validate_active_lease(&current.value, &command, now)?;
            let lease = current""",
        """        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            self.ensure_run_open_for_mutation(&current.value.run_id, now)
                .await?;
            validate_active_lease(&current.value, &command, now)?;
            let lease = current""",
    )
    replace_once(
        ENGINE,
        """        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            validate_active_lease(&current.value, &command, now)?;
            let sequence = current.value.output_sequence.saturating_add(1);""",
        """        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self.load_step_versioned(step_id).await?;
            self.ensure_run_open_for_mutation(&current.value.run_id, now)
                .await?;
            validate_active_lease(&current.value, &command, now)?;
            let sequence = current.value.output_sequence.saturating_add(1);""",
    )
    replace_once(
        ENGINE,
        """            if current.value.status == StepStatus::Succeeded
                && last_lease_matches(&current.value, &expected)
            {
                return Ok(step_mutation(&current.value));
            }
            validate_expected_lease(&current.value, &expected, now)?;""",
        """            if current.value.status == StepStatus::Succeeded
                && last_lease_matches(&current.value, &expected)
            {
                return Ok(step_mutation(&current.value));
            }
            self.ensure_run_open_for_mutation(&current.value.run_id, now)
                .await?;
            validate_expected_lease(&current.value, &expected, now)?;""",
    )
    replace_once(
        ENGINE,
        """            if matches!(
                current.value.status,
                StepStatus::WaitingRetry | StepStatus::Failed
            ) && last_lease_matches(&current.value, &expected)
            {
                return Ok(step_mutation(&current.value));
            }
            match metric {""",
        """            if matches!(
                current.value.status,
                StepStatus::WaitingRetry | StepStatus::Failed
            ) && last_lease_matches(&current.value, &expected)
            {
                return Ok(step_mutation(&current.value));
            }
            if let FailureMetric::Worker = metric {
                self.ensure_run_open_for_mutation(&current.value.run_id, now)
                    .await?;
            }
            match metric {""",
    )
    replace_once(
        ENGINE,
        """        if run.status.is_terminal() {
            return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
        }

        let now = self.now_ms();
        self.store""",
        """        if run.status.is_terminal() {
            return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
        }

        let now = self.now_ms();
        self.ensure_run_open_for_mutation(run_id, now).await?;
        self.store""",
    )
    replace_once(
        ENGINE,
        """            if current.value.status.is_terminal() {
                return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
            }
            if paused && current.value.status == RunStatus::Paused {""",
        """            if current.value.status.is_terminal() {
                return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
            }
            self.ensure_run_open_for_mutation(run_id, self.now_ms())
                .await?;
            if paused && current.value.status == RunStatus::Paused {""",
    )


def patch_scheduler() -> None:
    helpers = r'''    async fn ensure_run_open_for_mutation(
        &self,
        run_id: &str,
        now: u64,
    ) -> Result<(), EngineError> {
        let run = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.to_string(),
            })?
            .value;
        if run.status.is_terminal() {
            return Err(EngineError::Conflict(format!("run {run_id} is terminal")));
        }
        if run
            .deadline_ms
            .is_some_and(|deadline_ms| now >= deadline_ms)
        {
            self.expire_run_if_due(run_id, now).await?;
            return Err(EngineError::Conflict(format!(
                "run {run_id} deadline exceeded"
            )));
        }
        Ok(())
    }

    async fn expire_overdue_runs(&self, now: u64) -> Result<(), EngineError> {
        for key in self.store.keys().await? {
            if !key.starts_with("run.") {
                continue;
            }
            let Some(run) = self.load::<RunRecord>(&key).await? else {
                continue;
            };
            if run.value.status.is_terminal()
                || !run
                    .value
                    .deadline_ms
                    .is_some_and(|deadline_ms| now >= deadline_ms)
            {
                continue;
            }
            self.expire_run_if_due(&run.value.id, now).await?;
        }
        Ok(())
    }

    async fn expire_run_if_due(&self, run_id: &str, now: u64) -> Result<bool, EngineError> {
        let initial = self
            .load::<RunRecord>(&run_key(run_id))
            .await?
            .ok_or_else(|| EngineError::NotFound {
                resource: "run",
                id: run_id.to_string(),
            })?
            .value;
        let Some(deadline_ms) = initial.deadline_ms else {
            return Ok(false);
        };
        if initial.status.is_terminal() || now < deadline_ms {
            return Ok(false);
        }

        self.cancel_nonterminal_steps(run_id, None, "run_deadline_exceeded")
            .await?;

        for _ in 0..MAX_CAS_RETRIES {
            let mut current = self
                .load::<RunRecord>(&run_key(run_id))
                .await?
                .ok_or_else(|| EngineError::NotFound {
                    resource: "run",
                    id: run_id.to_string(),
                })?;
            if current.value.status.is_terminal() {
                return Ok(false);
            }
            if current.value.deadline_ms != Some(deadline_ms) || now < deadline_ms {
                return Ok(false);
            }
            let snapshot = self.get_run_snapshot(run_id).await?;
            current.value.counts = counts_for_steps(&snapshot.steps);
            current.value.status = RunStatus::Failed;
            current.value.updated_at_ms = now;
            current.value.completed_at_ms = Some(now);
            match self
                .update_value(&run_key(run_id), current.revision, &current.value)
                .await
            {
                Ok(_) => {
                    self.metrics
                        .run_deadlines_exceeded_total
                        .fetch_add(1, Ordering::Relaxed);
                    self.publish_best_effort(self.event(
                        "run.deadline_exceeded",
                        run_id,
                        None,
                        None,
                        &format!("deadline-{deadline_ms}"),
                        object(json!({
                            "deadlineMs": deadline_ms,
                            "expiredAtMs": now,
                        })),
                    ))
                    .await;
                    return Ok(true);
                }
                Err(EngineError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(EngineError::Conflict(
            "run deadline transition exceeded CAS retry budget".to_string(),
        ))
    }

    async fn tick_inner(&self) -> Result<(), EngineError> {'''
    replace_once(
        ENGINE,
        "    async fn tick_inner(&self) -> Result<(), EngineError> {",
        helpers,
    )
    replace_once(
        ENGINE,
        """        let now = self.now_ms();
        let steps = self.scan_steps().await?;""",
        """        let now = self.now_ms();
        self.expire_overdue_runs(now).await?;
        let steps = self.scan_steps().await?;""",
    )
    replace_once(
        ENGINE,
        """            if run.status.is_terminal() || run.status == RunStatus::Paused {
                return Ok(None);
            }

            let now = self.now_ms();""",
        """            let now = self.now_ms();
            if run.status.is_terminal()
                || run.status == RunStatus::Paused
                || run
                    .deadline_ms
                    .is_some_and(|deadline_ms| now >= deadline_ms)
            {
                self.expire_run_if_due(&run.id, now).await?;
                return Ok(None);
            }""",
    )
    replace_once(
        ENGINE,
        """        if run.status.is_terminal() {
            return Ok(());
        }

        for step_id in run.step_ids.values() {""",
        """        if run.status.is_terminal() {
            return Ok(());
        }
        let now = self.now_ms();
        if run
            .deadline_ms
            .is_some_and(|deadline_ms| now >= deadline_ms)
        {
            self.expire_run_if_due(run_id, now).await?;
            return Ok(());
        }

        for step_id in run.step_ids.values() {""",
    )
    replace_once(
        ENGINE,
        """fn desired_run_status(previous: RunStatus, counts: &RunCounts) -> RunStatus {
    if counts.failed > 0 {""",
        """fn desired_run_status(previous: RunStatus, counts: &RunCounts) -> RunStatus {
    if previous.is_terminal() {
        return previous;
    }
    if counts.failed > 0 {""",
    )


def patch_literals() -> None:
    for path in ROOT.rglob("*.rs"):
        add_field_to_literals(path, "SubmitRunRequest", "deadline_ms: None")
        add_field_to_literals(path, "SubmitTaskRequest", "deadline_ms: None")
        add_field_to_literals(path, "RunRecord", "deadline_ms: None")


def patch_tests() -> None:
    append_once(
        STATE_TESTS,
        "run_deadline_fences_active_work_and_remains_terminal",
        r'''
#[tokio::test]
async fn rejects_expired_deadline_without_poisoning_idempotency() {
    let (engine, _, _) = engine(8_000_000);
    let error = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: Some(8_000_000),
            idempotency_key: Some("deadline-not-poisoned".to_string()),
            name: Some("expired".to_string()),
            metadata: JsonObject::new(),
            steps: vec![step("expired", &[])],
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        EngineError::InvalidRequest(message) if message.contains("deadlineMs")
    ));

    let accepted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: Some(8_001_000),
            idempotency_key: Some("deadline-not-poisoned".to_string()),
            name: Some("valid retry".to_string()),
            metadata: JsonObject::new(),
            steps: vec![step("expired", &[])],
        })
        .await
        .unwrap();
    assert_eq!(accepted.status, RunStatus::Pending);
}

#[tokio::test]
async fn deadline_is_part_of_the_idempotency_binding() {
    let (engine, _, _) = engine(9_000_000);
    let request = SubmitRunRequest {
        deadline_ms: Some(9_100_000),
        idempotency_key: Some("deadline-idempotency".to_string()),
        name: Some("deadline binding".to_string()),
        metadata: JsonObject::new(),
        steps: vec![step("bound", &[])],
    };
    engine.submit_run(request.clone()).await.unwrap();
    let mut changed = request;
    changed.deadline_ms = Some(9_200_000);
    let error = engine.submit_run(changed).await.unwrap_err();
    assert!(matches!(error, EngineError::IdempotencyMismatch));
}

#[tokio::test]
async fn idempotent_retry_after_deadline_returns_the_failed_run() {
    let (engine, clock, _) = engine(9_500_000);
    let request = SubmitRunRequest {
        deadline_ms: Some(9_500_100),
        idempotency_key: Some("deadline-replay".to_string()),
        name: Some("deadline replay".to_string()),
        metadata: JsonObject::new(),
        steps: vec![step("queued", &[])],
    };
    let submitted = engine.submit_run(request.clone()).await.unwrap();
    clock.advance(101);
    let replay = engine.submit_run(request).await.unwrap();
    assert_eq!(replay.run_id, submitted.run_id);
    assert!(replay.idempotent_replay);
    assert_eq!(replay.status, RunStatus::Failed);
    let snapshot = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(snapshot.run.status, RunStatus::Failed);
    assert_eq!(snapshot.steps[0].status, StepStatus::Cancelled);
}

#[tokio::test]
async fn run_deadline_fences_active_work_and_remains_terminal() {
    let (engine, clock, events) = engine(10_000_000);
    engine
        .register_worker(worker("deadline-worker", 1))
        .await
        .unwrap();
    let submitted = engine
        .submit_run(SubmitRunRequest {
            deadline_ms: Some(10_000_500),
            idempotency_key: Some("deadline-fencing".to_string()),
            name: Some("deadline fencing".to_string()),
            metadata: JsonObject::new(),
            steps: vec![step("active", &[]), step("queued", &[])],
        })
        .await
        .unwrap();

    let assignment = engine
        .poll_once("deadline-worker")
        .await
        .unwrap()
        .assignment
        .unwrap();
    engine
        .start_step(
            &assignment.step_id,
            LeaseCommand {
                worker_id: "deadline-worker".to_string(),
                lease_token: assignment.lease_token.clone(),
                lease_generation: assignment.lease_generation,
            },
        )
        .await
        .unwrap();

    clock.advance(501);
    engine.tick().await.unwrap();
    let expired = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(expired.run.status, RunStatus::Failed);
    assert_eq!(expired.run.deadline_ms, Some(10_000_500));
    assert_eq!(expired.run.counts.cancelled, 2);
    assert!(expired
        .steps
        .iter()
        .all(|step| step.status == StepStatus::Cancelled));

    let stale = engine
        .complete_step(
            &assignment.step_id,
            CompleteStepRequest {
                worker_id: "deadline-worker".to_string(),
                lease_token: assignment.lease_token,
                lease_generation: assignment.lease_generation,
                result: JsonObject::new(),
            },
        )
        .await;
    assert!(matches!(stale, Err(EngineError::Conflict(_))));

    engine.tick().await.unwrap();
    let still_failed = engine.get_run_snapshot(&submitted.run_id).await.unwrap();
    assert_eq!(still_failed.run.status, RunStatus::Failed);
    assert!(engine
        .metrics()
        .render_prometheus()
        .contains("dd_durable_run_deadlines_exceeded_total 1"));
    let event_types = events
        .snapshot()
        .await
        .into_iter()
        .map(|event| event.event_type)
        .collect::<Vec<_>>();
    assert!(event_types.contains(&"run.deadline_exceeded".to_string()));
    assert!(event_types.contains(&"step.cancelled".to_string()));
}
''',
    )


def patch_smoke() -> None:
    marker = (
        "console.log(JSON.stringify({ ok: true, runId: submitted.runId, "
        "status: snapshot.run.status }));\n"
    )
    text = SMOKE.read_text(encoding="utf-8")
    if text.count(marker) != 1:
        raise SystemExit(f"{SMOKE}: smoke ending changed")
    addition = r'''
const deadlineSubmitted = await request('/api/v1/tasks', {
  method: 'POST',
  body: JSON.stringify({
    idempotencyKey: 'gha-deadline-run-v2',
    name: 'deadline fencing smoke',
    deadlineMs: Date.now() + 2500,
    taskType: 'agent:deadline-smoke',
    queue: 'agents',
    input: { purpose: 'verify hard run deadline' },
    requiredCapabilities: ['llm'],
    retry: { maxAttempts: 1, initialBackoffMs: 100, maxBackoffMs: 100, multiplier: 1 },
    timeoutMs: 60000,
    leaseMs: 5000,
  }),
});
const deadlineAssignment = await poll();
const deadlineLease = {
  workerId: 'gha-node-worker',
  leaseToken: deadlineAssignment.leaseToken,
  leaseGeneration: deadlineAssignment.leaseGeneration,
};
await request(`/api/v1/steps/${deadlineAssignment.stepId}/start`, {
  method: 'POST',
  body: JSON.stringify(deadlineLease),
});

let deadlineSnapshot;
for (let attempt = 0; attempt < 40; attempt += 1) {
  deadlineSnapshot = await request(`/api/v1/runs/${deadlineSubmitted.runId}`);
  if (deadlineSnapshot.run.status === 'failed') break;
  await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
}
assert.equal(deadlineSnapshot.run.status, 'failed');
assert.equal(deadlineSnapshot.run.counts.cancelled, 1);
assert.equal(deadlineSnapshot.steps[0].status, 'cancelled');

const staleCompletion = await fetch(`${baseUrl}/api/v1/steps/${deadlineAssignment.stepId}/complete`, {
  method: 'POST',
  headers: { 'content-type': 'application/json', 'x-worker-auth': secret },
  body: JSON.stringify({ ...deadlineLease, result: { shouldNotCommit: true } }),
});
assert.equal(staleCompletion.status, 409);
const metrics = await request('/metrics', { headers: {} });
assert.match(metrics, /dd_durable_run_deadlines_exceeded_total [1-9][0-9]*/);

console.log(JSON.stringify({
  ok: true,
  runId: submitted.runId,
  status: snapshot.run.status,
  deadlineRunId: deadlineSubmitted.runId,
  deadlineStatus: deadlineSnapshot.run.status,
}));
'''
    SMOKE.write_text(text.replace(marker, addition, 1), encoding="utf-8")


def patch_docs_and_contract() -> None:
    append_once(
        PROTOCOL,
        "### Absolute run deadline",
        """
### Absolute run deadline

A task or DAG submission may set `deadlineMs` to an absolute Unix epoch time in
milliseconds. A new submission must use a value later than the server time.
The deadline is part of the idempotency binding; an exact replay still returns
the original run after expiry.

Once reached, the scheduler cancels every non-terminal step, releases worker
and keyed-concurrency lanes, records the run as irreversibly failed, emits
`run.deadline_exceeded`, and increments
`dd_durable_run_deadlines_exceeded_total`. New leases and lease-scoped
mutations are rejected at or after the deadline, including late completions.
""",
    )
    append_once(
        README,
        "## Run deadlines",
        """
## Run deadlines

Set `deadlineMs` on a task or DAG submission to an absolute Unix epoch
timestamp in milliseconds. At expiry, active and queued steps are cancelled,
their leases are fenced, and the run is durably failed. A late worker
completion cannot resurrect it. Exact idempotent retries return the original
terminal run.
""",
    )
    append_once(
        OPERATIONS,
        "## Deadline operations",
        """
## Deadline operations

Alert on increases in `dd_durable_run_deadlines_exceeded_total` and correlate
`run.deadline_exceeded` with queue latency, worker capacity, and downstream
dependency traces. A deadline failure is terminal; retry as a new run with a
new idempotency key and an explicit later deadline.
""",
    )
    replace_once(
        CONTRACT,
        """const operationsPath = 'remote/deployments/durable-worker-server-rs/OPERATIONS.md';
""",
        """const operationsPath = 'remote/deployments/durable-worker-server-rs/OPERATIONS.md';
const enginePath = 'remote/deployments/durable-worker-server-rs/src/engine/mod.rs';
const smokePath = 'remote/deployments/durable-worker-server-rs/tests/gha_smoke.mjs';
""",
    )
    replace_once(
        CONTRACT,
        """const operations = read(operationsPath);
""",
        """const operations = read(operationsPath);
const engine = read(enginePath);
const smoke = read(smokePath);
""",
    )
    append_once(
        CONTRACT,
        "run deadlines are durable, observable, and fenced end to end",
        r'''
test('run deadlines are durable, observable, and fenced end to end', () => {
  assert.match(protocol, /deadlineMs/);
  assert.match(protocol, /run\.deadline_exceeded/);
  assert.match(engine, /dd_durable_run_deadlines_exceeded_total/);
  assert.match(engine, /ensure_run_open_for_mutation/);
  assert.match(smoke, /deadlineSubmitted/);
  assert.match(smoke, /staleCompletion\.status, 409/);
});
''',
    )


def main() -> None:
    if "pub deadline_ms: Option<u64>" in MODEL.read_text(encoding="utf-8"):
        raise SystemExit("deadline feature already present")
    patch_model()
    patch_metrics()
    patch_submission()
    patch_mutation_boundaries()
    patch_scheduler()
    patch_literals()
    patch_tests()
    patch_smoke()
    patch_docs_and_contract()


if __name__ == "__main__":
    main()
