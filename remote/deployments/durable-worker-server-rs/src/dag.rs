use std::collections::{BTreeMap, BTreeSet, VecDeque};

use thiserror::Error;

use crate::model::SubmitRunRequest;

pub const MAX_STEPS_PER_RUN: usize = 256;
pub const MAX_INPUT_BYTES_PER_STEP: usize = 1024 * 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DagError {
    #[error("a run must contain at least one step")]
    Empty,
    #[error("a run may contain at most {MAX_STEPS_PER_RUN} steps")]
    TooManySteps,
    #[error("invalid {field}: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
    #[error("duplicate step key: {0}")]
    DuplicateStep(String),
    #[error("step {step} depends on missing step {dependency}")]
    MissingDependency { step: String, dependency: String },
    #[error("step {0} cannot depend on itself")]
    SelfDependency(String),
    #[error("workflow graph contains a dependency cycle")]
    Cycle,
}

pub fn validate_identifier(value: &str, field: &'static str, max_len: usize) -> Result<(), DagError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DagError::InvalidField {
            field,
            message: "must not be empty".to_string(),
        });
    }
    if value.len() > max_len {
        return Err(DagError::InvalidField {
            field,
            message: format!("must be no more than {max_len} bytes"),
        });
    }
    if let Some(character) = value
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')))
    {
        return Err(DagError::InvalidField {
            field,
            message: format!(
                "must contain only ASCII alphanumerics, '-', '_', or ':' (found {character:?})"
            ),
        });
    }
    Ok(())
}

pub fn validate_run_request(request: &SubmitRunRequest) -> Result<Vec<String>, DagError> {
    if request.steps.is_empty() {
        return Err(DagError::Empty);
    }
    if request.steps.len() > MAX_STEPS_PER_RUN {
        return Err(DagError::TooManySteps);
    }
    if let Some(key) = request.idempotency_key.as_deref() {
        validate_identifier(key, "idempotencyKey", 200)?;
    }
    if let Some(name) = request.name.as_deref() {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.len() > 200 {
            return Err(DagError::InvalidField {
                field: "name",
                message: "must contain 1 to 200 bytes".to_string(),
            });
        }
    }

    let mut by_key = BTreeMap::new();
    for step in &request.steps {
        validate_identifier(&step.key, "steps[].key", 128)?;
        validate_identifier(&step.task_type, "steps[].taskType", 160)?;
        validate_identifier(&step.queue, "steps[].queue", 128)?;
        if by_key.insert(step.key.clone(), step).is_some() {
            return Err(DagError::DuplicateStep(step.key.clone()));
        }
        if step.retry.max_attempts == 0 || step.retry.max_attempts > 100 {
            return Err(DagError::InvalidField {
                field: "steps[].retry.maxAttempts",
                message: "must be between 1 and 100".to_string(),
            });
        }
        if !(1.0..=100.0).contains(&step.retry.multiplier) {
            return Err(DagError::InvalidField {
                field: "steps[].retry.multiplier",
                message: "must be between 1 and 100".to_string(),
            });
        }
        if step.retry.initial_backoff_ms > step.retry.max_backoff_ms {
            return Err(DagError::InvalidField {
                field: "steps[].retry",
                message: "initialBackoffMs must not exceed maxBackoffMs".to_string(),
            });
        }
        if !(1_000..=24 * 60 * 60 * 1_000).contains(&step.lease_ms) {
            return Err(DagError::InvalidField {
                field: "steps[].leaseMs",
                message: "must be between 1000 and 86400000".to_string(),
            });
        }
        if !(1_000..=7 * 24 * 60 * 60 * 1_000).contains(&step.timeout_ms) {
            return Err(DagError::InvalidField {
                field: "steps[].timeoutMs",
                message: "must be between 1000 and 604800000".to_string(),
            });
        }
        if let Some(signal) = step.wait_for_signal.as_deref() {
            validate_identifier(signal, "steps[].waitForSignal", 128)?;
        }
        if let Some(concurrency) = &step.concurrency {
            validate_identifier(&concurrency.key, "steps[].concurrency.key", 200)?;
            if concurrency.limit == 0 || concurrency.limit > 10_000 {
                return Err(DagError::InvalidField {
                    field: "steps[].concurrency.limit",
                    message: "must be between 1 and 10000".to_string(),
                });
            }
        }
        if let Some(affinity) = step.affinity_key.as_deref() {
            validate_identifier(affinity, "steps[].affinityKey", 200)?;
        }
        for capability in &step.required_capabilities {
            validate_identifier(capability, "steps[].requiredCapabilities[]", 128)?;
        }
        let input_bytes = serde_json::to_vec(&step.input)
            .map_err(|error| DagError::InvalidField {
                field: "steps[].input",
                message: error.to_string(),
            })?
            .len();
        if input_bytes > MAX_INPUT_BYTES_PER_STEP {
            return Err(DagError::InvalidField {
                field: "steps[].input",
                message: format!("must serialize to no more than {MAX_INPUT_BYTES_PER_STEP} bytes"),
            });
        }
    }

    let mut indegree = BTreeMap::<String, usize>::new();
    let mut outgoing = BTreeMap::<String, BTreeSet<String>>::new();
    for step in &request.steps {
        indegree.insert(step.key.clone(), step.depends_on.len());
        for dependency in &step.depends_on {
            if dependency == &step.key {
                return Err(DagError::SelfDependency(step.key.clone()));
            }
            if !by_key.contains_key(dependency) {
                return Err(DagError::MissingDependency {
                    step: step.key.clone(),
                    dependency: dependency.clone(),
                });
            }
            outgoing
                .entry(dependency.clone())
                .or_default()
                .insert(step.key.clone());
        }
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(key, degree)| (*degree == 0).then_some(key.clone()))
        .collect::<VecDeque<_>>();
    let mut ordered = Vec::with_capacity(request.steps.len());
    while let Some(key) = ready.pop_front() {
        ordered.push(key.clone());
        if let Some(children) = outgoing.get(&key) {
            for child in children {
                let degree = indegree
                    .get_mut(child)
                    .expect("validated child must have an indegree entry");
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(child.clone());
                }
            }
        }
    }
    if ordered.len() != request.steps.len() {
        return Err(DagError::Cycle);
    }
    Ok(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{JsonObject, RetryPolicy, StepDefinition};

    fn step(key: &str, depends_on: &[&str]) -> StepDefinition {
        StepDefinition {
            key: key.to_string(),
            task_type: "test".to_string(),
            queue: "default".to_string(),
            input: JsonObject::new(),
            depends_on: depends_on.iter().map(|value| (*value).to_string()).collect(),
            priority: 0,
            required_capabilities: BTreeSet::new(),
            retry: RetryPolicy::default(),
            timeout_ms: 60_000,
            lease_ms: 10_000,
            not_before_ms: None,
            wait_for_signal: None,
            concurrency: None,
            affinity_key: None,
        }
    }

    #[test]
    fn orders_a_valid_graph() {
        let request = SubmitRunRequest {
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![step("a", &[]), step("b", &["a"]), step("c", &["a", "b"])],
        };
        assert_eq!(validate_run_request(&request).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn rejects_cycles() {
        let request = SubmitRunRequest {
            idempotency_key: None,
            name: None,
            metadata: JsonObject::new(),
            steps: vec![step("a", &["b"]), step("b", &["a"])],
        };
        assert_eq!(validate_run_request(&request), Err(DagError::Cycle));
    }
}
