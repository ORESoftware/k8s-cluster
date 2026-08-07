
fn optional_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn parse_bool_env(name: &str, default: bool) -> Result<bool, String> {
    let Ok(raw) = env::var(name) else {
        return Ok(default);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("{name} must be true or false")),
    }
}

fn validate_http_url(value: &str, name: &str) -> Result<(), String> {
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("{name} must use http:// or https://"))
    }
}

fn parse_rules(raw: &str) -> Result<Vec<Rule>, String> {
    let rules: Vec<Rule> =
        serde_json::from_str(raw).map_err(|error| format!("invalid rules JSON: {error}"))?;
    if rules.is_empty() {
        return Err("at least one fallback rule is required".to_string());
    }
    for rule in &rules {
        validate_rule(rule)?;
    }
    Ok(rules)
}

fn validate_rule(rule: &Rule) -> Result<(), String> {
    if !valid_repo(&rule.repo) {
        return Err(format!("invalid repo in fallback rule: {}", rule.repo));
    }
    if rule.workflow.trim().is_empty() || rule.workflow.len() > 200 {
        return Err(format!("invalid workflow name for {}", rule.repo));
    }
    if rule.branches.iter().any(|value| !valid_ref_component(value)) {
        return Err(format!("invalid branch allowlist for {}", rule.repo));
    }
    if rule.source_events.is_empty()
        || rule
            .source_events
            .iter()
            .any(|value| value != "push" && value != "workflow_dispatch")
    {
        return Err(format!(
            "sourceEvents for {} may contain only push or workflow_dispatch",
            rule.repo
        ));
    }
    if rule.conclusions.is_empty()
        || rule
            .conclusions
            .iter()
            .any(|value| !FAILURE_CONCLUSIONS.contains(&value.as_str()))
    {
        return Err(format!(
            "conclusions for {} may contain only failure-like outcomes",
            rule.repo
        ));
    }
    match &rule.action {
        RuleAction::WorkflowDispatch {
            workflow_file,
            workflow_name,
            dispatch_ref,
            runner,
            extra_inputs,
        } => {
            if !valid_workflow_file(workflow_file) {
                return Err(format!("invalid fallback workflow file for {}", rule.repo));
            }
            if workflow_name.eq_ignore_ascii_case(&rule.workflow) || workflow_name.trim().is_empty() {
                return Err(format!(
                    "fallback workflow name for {} must be non-empty and differ from source workflow",
                    rule.repo
                ));
            }
            if !valid_ref_component(dispatch_ref) || !valid_label(runner) {
                return Err(format!("invalid dispatch ref or runner for {}", rule.repo));
            }
            if extra_inputs.len() > 20
                || extra_inputs.iter().any(|(key, value)| {
                    !valid_input_key(key) || value.len() > 1024 || value.contains("${{")
                })
            {
                return Err(format!("invalid extraInputs for {}", rule.repo));
            }
        }
        RuleAction::BuildServerProfile { profile, executor } => {
            if !valid_profile(profile)
                || executor
                    .as_deref()
                    .is_some_and(|value| value != "local" && value != "lambda")
            {
                return Err(format!("invalid build-server profile rule for {}", rule.repo));
            }
        }
    }
    Ok(())
}

fn valid_repo(value: &str) -> bool {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    parts.next().is_none()
        && !owner.is_empty()
        && !repo.is_empty()
        && owner.chars().all(valid_repo_char)
        && repo.chars().all(valid_repo_char)
}

fn valid_repo_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.')
}

fn valid_ref_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.contains("@{")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
}

fn valid_workflow_file(value: &str) -> bool {
    value.len() <= 128
        && (value.ends_with(".yml") || value.ends_with(".yaml"))
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn valid_input_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn valid_profile(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn valid_delivery(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn valid_commit_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn verify_github_signature(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(hex_signature) = header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(provided) = hex::decode(hex_signature) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    expected.as_slice().ct_eq(provided.as_slice()).into()
}

fn select_rule<'a>(event: &WorkflowRunEvent, rules: &'a [Rule]) -> Option<&'a Rule> {
    let conclusion = event.workflow_run.conclusion.as_deref()?;
    let branch = event.workflow_run.head_branch.as_deref()?;
    rules.iter().find(|rule| {
        rule.repo.eq_ignore_ascii_case(&event.repository.full_name)
            && rule.workflow.eq_ignore_ascii_case(&event.workflow_run.name)
            && (rule.branches.is_empty() || rule.branches.iter().any(|value| value == branch))
            && rule
                .source_events
                .iter()
                .any(|value| value == &event.workflow_run.event)
            && rule.conclusions.iter().any(|value| value == conclusion)
            && !matches!(
                &rule.action,
                RuleAction::WorkflowDispatch { workflow_name, .. }
                    if workflow_name.eq_ignore_ascii_case(&event.workflow_run.name)
            )
    })
}

async fn descriptor(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "service": SERVICE_NAME,
        "description": "Signed GitHub workflow failure bridge for official ARC runners and controlled dd-build-server profiles.",
        "dryRun": state.config.dry_run,
        "rules": state.config.rules.len(),
        "endpoints": {
            "githubWebhook": "POST /webhooks/github",
            "healthz": "GET /healthz",
            "readyz": "GET /readyz",
            "metrics": "GET /metrics"
        }
    }))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true, "service": SERVICE_NAME }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let ready = !state.config.rules.is_empty() && state.config.webhook_secret.len() >= 32;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(json!({ "ok": ready, "service": SERVICE_NAME }))).into_response()
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let counters = &state.counters;
    let body = format!(
        "# TYPE gha_clone_webhooks_received_total counter\n\
         gha_clone_webhooks_received_total {}\n\
         # TYPE gha_clone_webhooks_rejected_total counter\n\
         gha_clone_webhooks_rejected_total {}\n\
         # TYPE gha_clone_webhooks_ignored_total counter\n\
         gha_clone_webhooks_ignored_total {}\n\
         # TYPE gha_clone_webhooks_duplicate_total counter\n\
         gha_clone_webhooks_duplicate_total {}\n\
         # TYPE gha_clone_fallbacks_dispatched_total counter\n\
         gha_clone_fallbacks_dispatched_total {}\n\
         # TYPE gha_clone_fallbacks_failed_total counter\n\
         gha_clone_fallbacks_failed_total {}\n",
        counters.received.load(Ordering::Relaxed),
        counters.rejected.load(Ordering::Relaxed),
        counters.ignored.load(Ordering::Relaxed),
        counters.duplicates.load(Ordering::Relaxed),
        counters.dispatched.load(Ordering::Relaxed),
        counters.failed.load(Ordering::Relaxed),
    );
    ([("content-type", "text/plain; version=0.0.4; charset=utf-8")], body)
}
