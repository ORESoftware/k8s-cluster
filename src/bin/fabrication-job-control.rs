//! Operational entry point for the durable fabrication job-control plane.
//!
//! This binary is intentionally separate from the HTTP server lifecycle so
//! outbox dispatch and stalled-job reaping can run as independently scalable
//! Kubernetes workloads.

#[path = "../job_control.rs"]
mod job_control;

use std::{collections::BTreeMap, env, error::Error, fmt, io, time::Duration};

use job_control::{
    dispatch_once, reap_once, run_lease_drill, EnqueueRequest, JobControlResult, JobStore,
    NatsPublisher, REQUEST_SUBJECT, RESULT_SUBJECT,
};
use serde::Serialize;
use serde_json::{json, Value as JsonValue};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("fabrication-job-control failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> JobControlResult<()> {
    let command = CommandLine::parse()?;
    match command.name.as_str() {
        "schema-check" => {
            let store = connected_store().await?;
            store.ensure_schema().await?;
            print_json(&json!({"ok": true, "schema": "durable-job-control"}))?;
        }
        "enqueue" => {
            let store = connected_store().await?;
            let request = EnqueueRequest {
                tenant_id: command.required("tenant")?,
                request_id: command.required("request-id")?,
                idempotency_key: command.required("idempotency-key")?,
                kind: command.required("kind")?,
                request_payload: parse_json("payload", &command.required("payload")?)?,
                max_attempts: command.parse_or("max-attempts", 5_i32)?,
                priority: command.parse_or("priority", 0_i16)?,
                subject: command
                    .optional("subject")
                    .unwrap_or_else(|| REQUEST_SUBJECT.to_owned()),
            };
            let job = store.enqueue(&request).await?;
            print_json(&job)?;
        }
        "show" => {
            let store = connected_store().await?;
            let tenant = command.required("tenant")?;
            let job_id = command.required("job-id")?;
            let job = store.get_job(&tenant, &job_id).await?;
            print_json(&job)?;
        }
        "dispatch-once" => {
            let store = connected_store().await?;
            let owner = command.optional("owner").unwrap_or_else(default_owner);
            let limit = command.parse_or("limit", 100_u64)?;
            let publisher =
                NatsPublisher::connect_from_env("dd-fabrication-job-outbox-dispatcher").await?;
            let report = dispatch_once(&store, &publisher, &owner, limit).await?;
            print_json(&report)?;
        }
        "dispatch-loop" => {
            let store = connected_store().await?;
            let owner = command.optional("owner").unwrap_or_else(default_owner);
            let limit = command.parse_or("limit", 100_u64)?;
            let interval_secs = command.parse_or("interval-secs", 2_u64)?;
            let publisher =
                NatsPublisher::connect_from_env("dd-fabrication-job-outbox-dispatcher").await?;
            run_dispatch_loop(store, publisher, owner, limit, interval_secs).await?;
        }
        "reap-once" => {
            let store = connected_store().await?;
            let limit = command.parse_or("limit", 100_u64)?;
            let request_subject = command
                .optional("request-subject")
                .unwrap_or_else(|| REQUEST_SUBJECT.to_owned());
            let result_subject = command
                .optional("result-subject")
                .unwrap_or_else(|| RESULT_SUBJECT.to_owned());
            let report = reap_once(&store, limit, &request_subject, &result_subject).await?;
            print_json(&report)?;
        }
        "reap-loop" => {
            let store = connected_store().await?;
            let limit = command.parse_or("limit", 100_u64)?;
            let interval_secs = command.parse_or("interval-secs", 15_u64)?;
            let request_subject = command
                .optional("request-subject")
                .unwrap_or_else(|| REQUEST_SUBJECT.to_owned());
            let result_subject = command
                .optional("result-subject")
                .unwrap_or_else(|| RESULT_SUBJECT.to_owned());
            run_reaper_loop(store, limit, interval_secs, request_subject, result_subject).await?;
        }
        "lease-drill" => {
            let store = connected_store().await?;
            let tenant = command.required("tenant")?;
            let job_id = command.required("job-id")?;
            let owner = command.optional("owner").unwrap_or_else(default_owner);
            let seconds = command.parse_or("seconds", 30_u64)?;
            let complete = command.flag("complete");
            let job = run_lease_drill(
                store,
                &tenant,
                &job_id,
                &owner,
                Duration::from_secs(seconds),
                complete,
            )
            .await?;
            print_json(&job)?;
        }
        "help" | "--help" | "-h" => {
            println!("{}", usage());
        }
        other => {
            return Err(invalid(format!("unknown command {other:?}\n\n{}", usage())));
        }
    }
    Ok(())
}

async fn connected_store() -> JobControlResult<JobStore> {
    let store = JobStore::connect_from_env().await?;
    store.ensure_schema().await?;
    Ok(store)
}

async fn run_dispatch_loop(
    store: JobStore,
    publisher: NatsPublisher,
    owner: String,
    limit: u64,
    interval_secs: u64,
) -> JobControlResult<()> {
    validate_interval(interval_secs)?;
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = interval.tick() => {
                let report = dispatch_once(&store, &publisher, &owner, limit).await?;
                emit_loop_report("outbox_dispatch", &report)?;
            }
        }
    }
}

async fn run_reaper_loop(
    store: JobStore,
    limit: u64,
    interval_secs: u64,
    request_subject: String,
    result_subject: String,
) -> JobControlResult<()> {
    validate_interval(interval_secs)?;
    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = interval.tick() => {
                let report = reap_once(
                    &store,
                    limit,
                    &request_subject,
                    &result_subject,
                ).await?;
                emit_loop_report("job_reaper", &report)?;
            }
        }
    }
}

fn emit_loop_report<T: Serialize>(kind: &str, report: &T) -> JobControlResult<()> {
    let value = json!({
        "kind": kind,
        "observedAt": chrono::Utc::now().to_rfc3339(),
        "report": report
    });
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

fn validate_interval(interval_secs: u64) -> JobControlResult<()> {
    if interval_secs == 0 || interval_secs > 3_600 {
        return Err(invalid("interval-secs must be between 1 and 3600"));
    }
    Ok(())
}

fn parse_json(name: &str, value: &str) -> JobControlResult<JsonValue> {
    serde_json::from_str(value)
        .map_err(|error| invalid(format!("--{name} must be valid JSON: {error}")))
}

fn default_owner() -> String {
    let hostname = env::var("HOSTNAME").unwrap_or_else(|_| "unknown-host".to_owned());
    format!(
        "{}:{hostname}:{}",
        env!("CARGO_PKG_NAME"),
        std::process::id()
    )
}

fn print_json<T: Serialize>(value: &T) -> JobControlResult<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn invalid(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[derive(Debug)]
struct CommandLine {
    name: String,
    values: BTreeMap<String, String>,
    flags: BTreeMap<String, bool>,
}

impl CommandLine {
    fn parse() -> JobControlResult<Self> {
        let mut args = env::args().skip(1);
        let Some(name) = args.next() else {
            return Ok(Self {
                name: "help".to_owned(),
                values: BTreeMap::new(),
                flags: BTreeMap::new(),
            });
        };
        let mut values = BTreeMap::new();
        let mut flags = BTreeMap::new();
        let mut pending: Option<String> = None;

        for argument in args {
            if let Some(key) = pending.take() {
                values.insert(key, argument);
                continue;
            }
            let Some(key) = argument.strip_prefix("--") else {
                return Err(invalid(format!(
                    "unexpected positional argument {argument:?}"
                )));
            };
            if let Some((name, value)) = key.split_once('=') {
                if name.is_empty() || value.is_empty() {
                    return Err(invalid("flags in --name=value form need both parts"));
                }
                values.insert(name.to_owned(), value.to_owned());
            } else if matches!(key, "complete") {
                flags.insert(key.to_owned(), true);
            } else {
                pending = Some(key.to_owned());
            }
        }
        if let Some(key) = pending {
            return Err(invalid(format!("missing value for --{key}")));
        }
        Ok(Self {
            name,
            values,
            flags,
        })
    }

    fn required(&self, name: &str) -> JobControlResult<String> {
        self.values
            .get(name)
            .cloned()
            .ok_or_else(|| invalid(format!("--{name} is required")))
    }

    fn optional(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }

    fn parse_or<T>(&self, name: &str, default: T) -> JobControlResult<T>
    where
        T: std::str::FromStr,
        T::Err: fmt::Display,
    {
        match self.values.get(name) {
            Some(value) => value
                .parse::<T>()
                .map_err(|error| invalid(format!("--{name} is invalid: {error}"))),
            None => Ok(default),
        }
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
}

fn usage() -> &'static str {
    r#"fabrication-job-control

PostgreSQL is the durable job ledger; NATS JetStream carries wakeups. Worker
claims require both a Fiducia fencing lease and a PostgreSQL transaction
advisory lock.

Commands:
  schema-check
  enqueue --tenant T --request-id R --idempotency-key K --kind KIND
          --payload JSON [--max-attempts N] [--priority N] [--subject SUBJECT]
  show --tenant T --job-id UUID
  dispatch-once [--owner ID] [--limit N]
  dispatch-loop [--owner ID] [--limit N] [--interval-secs N]
  reap-once [--limit N] [--request-subject S] [--result-subject S]
  reap-loop [--limit N] [--interval-secs N]
            [--request-subject S] [--result-subject S]
  lease-drill --tenant T --job-id UUID [--owner ID] [--seconds N] [--complete]

Environment:
  FABRICATION_DATABASE_URL | RDS_DATABASE_URL | DATABASE_URL
  NATS_URL, NATS_REQUIRE_TLS, NATS_CREDENTIALS_FILE, NATS_TOKEN, NATS_NKEY
  FIDUCIA_BASE_URL, FIDUCIA_AUTH_TOKEN, FIDUCIA_TLS_CA_PEM,
  FIDUCIA_TLS_CA_PATH, FIDUCIA_LEASE_SECS, FIDUCIA_WAIT_SECS
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_are_bounded() {
        assert!(validate_interval(0).is_err());
        assert!(validate_interval(1).is_ok());
        assert!(validate_interval(3_600).is_ok());
        assert!(validate_interval(3_601).is_err());
    }

    #[test]
    fn json_parser_rejects_invalid_payloads() {
        assert!(parse_json("payload", "{").is_err());
        assert_eq!(
            parse_json("payload", r#"{"ok":true}"#).unwrap(),
            json!({"ok": true})
        );
    }

    #[test]
    fn usage_describes_both_lock_layers() {
        assert!(usage().contains("Fiducia"));
        assert!(usage().contains("PostgreSQL transaction"));
        assert!(usage().contains("dispatch-loop"));
        assert!(usage().contains("reap-loop"));
    }
}
