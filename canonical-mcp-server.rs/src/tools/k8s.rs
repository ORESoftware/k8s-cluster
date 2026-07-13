//! `k8s_status`: read-only Kubernetes inspection by shelling out to
//! `kubectl`.
//!
//! Strict allowlist: only `kubectl get <resource> -o json` (and, if ever
//! needed, `kubectl config get-contexts`) are permitted — never
//! exec/delete/apply. Missing kubectl or an unreachable cluster is a
//! structured error, not a panic.

use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

/// Wall-clock budget for one kubectl invocation.
pub const KUBECTL_TIMEOUT: Duration = Duration::from_secs(20);

/// Resources `k8s_status` can inspect (all read-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum K8sResource {
    Nodes,
    Pods,
    Deployments,
    Services,
    Ingresses,
}

impl K8sResource {
    pub fn kubectl_name(self) -> &'static str {
        match self {
            K8sResource::Nodes => "nodes",
            K8sResource::Pods => "pods",
            K8sResource::Deployments => "deployments",
            K8sResource::Services => "services",
            K8sResource::Ingresses => "ingresses",
        }
    }

    fn namespaced(self) -> bool {
        !matches!(self, K8sResource::Nodes)
    }
}

/// Validate a namespace or context argument so nothing flag-like or
/// shell-ish reaches kubectl. (Arguments are passed exec-style, never
/// through a shell; this is defense in depth.)
pub fn validate_k8s_name<'a>(value: &'a str, what: &str) -> Result<&'a str, String> {
    let valid = !value.is_empty()
        && value.len() <= 253
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':' | '@' | '/'));
    if valid {
        Ok(value)
    } else {
        Err(format!("invalid {what}: {value:?}"))
    }
}

/// Human-readable age between two RFC 3339 timestamps (e.g. "3d4h").
pub fn age_between(created: &str, now: DateTime<Utc>) -> Option<String> {
    let created = DateTime::parse_from_rfc3339(created).ok()?;
    let seconds = (now - created.with_timezone(&Utc)).num_seconds().max(0);
    let (days, hours, minutes) = (
        seconds / 86_400,
        (seconds % 86_400) / 3_600,
        (seconds % 3_600) / 60,
    );
    Some(if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else {
        format!("{minutes}m")
    })
}

fn meta_str<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    item.get("metadata")?.get(key)?.as_str()
}

fn node_status(item: &Value) -> Value {
    let ready = item
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .and_then(|conditions| {
            conditions
                .iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some("Ready"))
        })
        .and_then(|c| c.get("status").and_then(Value::as_str))
        .unwrap_or("Unknown");
    let version = item
        .pointer("/status/nodeInfo/kubeletVersion")
        .and_then(Value::as_str);
    json!({ "ready": ready, "version": version })
}

fn pod_status(item: &Value) -> Value {
    let phase = item
        .pointer("/status/phase")
        .and_then(Value::as_str)
        .unwrap_or("Unknown");
    let (ready, total) = item
        .pointer("/status/containerStatuses")
        .and_then(Value::as_array)
        .map(|containers| {
            let ready = containers
                .iter()
                .filter(|c| c.get("ready").and_then(Value::as_bool) == Some(true))
                .count();
            (ready, containers.len())
        })
        .unwrap_or((0, 0));
    json!({ "phase": phase, "ready": format!("{ready}/{total}") })
}

fn deployment_status(item: &Value) -> Value {
    let desired = item
        .pointer("/spec/replicas")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let ready = item
        .pointer("/status/readyReplicas")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({ "ready": format!("{ready}/{desired}") })
}

fn service_status(item: &Value) -> Value {
    json!({
        "type": item.pointer("/spec/type").and_then(Value::as_str),
        "cluster_ip": item.pointer("/spec/clusterIP").and_then(Value::as_str),
    })
}

fn ingress_status(item: &Value) -> Value {
    let hosts: Vec<&str> = item
        .pointer("/spec/rules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter_map(|rule| rule.get("host").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    json!({ "hosts": hosts })
}

/// Summarize a `kubectl get <resource> -o json` list into compact rows.
pub fn summarize_items(resource: K8sResource, body: &Value, now: DateTime<Utc>) -> Vec<Value> {
    let Some(items) = body.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            let status = match resource {
                K8sResource::Nodes => node_status(item),
                K8sResource::Pods => pod_status(item),
                K8sResource::Deployments => deployment_status(item),
                K8sResource::Services => service_status(item),
                K8sResource::Ingresses => ingress_status(item),
            };
            json!({
                "name": meta_str(item, "name"),
                "namespace": meta_str(item, "namespace"),
                "status": status,
                "age": meta_str(item, "creationTimestamp")
                    .and_then(|created| age_between(created, now)),
            })
        })
        .collect()
}

/// Run the allowlisted `kubectl get` and summarize its JSON output.
pub async fn k8s_status_report(
    resource: K8sResource,
    namespace: Option<&str>,
    context: Option<&str>,
) -> Result<Value, String> {
    // Allowlisted, exec-style argument vector: `kubectl get <resource>`
    // only. Nothing here may ever grow into exec/delete/apply.
    let mut args: Vec<String> = vec![
        "get".to_string(),
        resource.kubectl_name().to_string(),
        "-o".to_string(),
        "json".to_string(),
        "--request-timeout=15s".to_string(),
    ];
    if resource.namespaced() {
        match namespace {
            Some(namespace) => {
                let namespace = validate_k8s_name(namespace, "namespace")?;
                args.push("--namespace".to_string());
                args.push(namespace.to_string());
            }
            None => args.push("--all-namespaces".to_string()),
        }
    }
    if let Some(context) = context {
        let context = validate_k8s_name(context, "context")?;
        args.push("--context".to_string());
        args.push(context.to_string());
    }

    let child = tokio::process::Command::new("kubectl")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(KUBECTL_TIMEOUT, child).await {
        Err(_) => {
            return Err(format!(
                "kubectl get {} timed out after {KUBECTL_TIMEOUT:?}",
                resource.kubectl_name()
            ))
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("kubectl is not installed or not on PATH".to_string())
        }
        Ok(Err(error)) => return Err(format!("failed to run kubectl: {error}")),
        Ok(Ok(output)) => output,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "kubectl get {} failed ({}): {}",
            resource.kubectl_name(),
            output.status,
            stderr.trim()
        ));
    }

    let body: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("kubectl returned invalid JSON: {error}"))?;
    Ok(json!({
        "resource": resource.kubectl_name(),
        "items": summarize_items(resource, &body, Utc::now()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 13, 12, 0, 0).unwrap()
    }

    #[test]
    fn summarize_pods_happy_path() {
        let body = json!({
            "items": [{
                "metadata": {
                    "name": "web-abc123",
                    "namespace": "prod",
                    "creationTimestamp": "2026-07-10T08:00:00Z"
                },
                "status": {
                    "phase": "Running",
                    "containerStatuses": [
                        { "ready": true },
                        { "ready": false }
                    ]
                }
            }]
        });
        let rows = summarize_items(K8sResource::Pods, &body, now());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "web-abc123");
        assert_eq!(rows[0]["namespace"], "prod");
        assert_eq!(rows[0]["status"]["phase"], "Running");
        assert_eq!(rows[0]["status"]["ready"], "1/2");
        assert_eq!(rows[0]["age"], "3d4h");
    }

    #[test]
    fn summarize_nodes_happy_path() {
        let body = json!({
            "items": [{
                "metadata": { "name": "node-1", "creationTimestamp": "2026-07-13T11:30:00Z" },
                "status": {
                    "conditions": [
                        { "type": "MemoryPressure", "status": "False" },
                        { "type": "Ready", "status": "True" }
                    ],
                    "nodeInfo": { "kubeletVersion": "v1.31.2" }
                }
            }]
        });
        let rows = summarize_items(K8sResource::Nodes, &body, now());
        assert_eq!(rows[0]["status"]["ready"], "True");
        assert_eq!(rows[0]["status"]["version"], "v1.31.2");
        assert_eq!(rows[0]["namespace"], Value::Null);
        assert_eq!(rows[0]["age"], "30m");
    }

    #[test]
    fn summarize_deployments_services_ingresses() {
        let deployments = json!({
            "items": [{
                "metadata": { "name": "web", "namespace": "prod" },
                "spec": { "replicas": 3 },
                "status": { "readyReplicas": 2 }
            }]
        });
        let rows = summarize_items(K8sResource::Deployments, &deployments, now());
        assert_eq!(rows[0]["status"]["ready"], "2/3");

        let services = json!({
            "items": [{
                "metadata": { "name": "web", "namespace": "prod" },
                "spec": { "type": "ClusterIP", "clusterIP": "10.0.0.7" }
            }]
        });
        let rows = summarize_items(K8sResource::Services, &services, now());
        assert_eq!(rows[0]["status"]["type"], "ClusterIP");
        assert_eq!(rows[0]["status"]["cluster_ip"], "10.0.0.7");

        let ingresses = json!({
            "items": [{
                "metadata": { "name": "web", "namespace": "prod" },
                "spec": { "rules": [ { "host": "canonical.cloud" }, {} ] }
            }]
        });
        let rows = summarize_items(K8sResource::Ingresses, &ingresses, now());
        assert_eq!(rows[0]["status"]["hosts"], json!(["canonical.cloud"]));
    }

    #[test]
    fn summarize_tolerates_missing_fields() {
        let body = json!({ "items": [ {} ] });
        let rows = summarize_items(K8sResource::Pods, &body, now());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], Value::Null);
        assert_eq!(rows[0]["status"]["phase"], "Unknown");
        assert_eq!(rows[0]["status"]["ready"], "0/0");
        assert_eq!(rows[0]["age"], Value::Null);
        assert!(summarize_items(K8sResource::Pods, &json!({}), now()).is_empty());
    }

    #[test]
    fn age_between_formats_and_rejects_garbage() {
        assert_eq!(
            age_between("2026-07-13T11:59:00Z", now()).as_deref(),
            Some("1m")
        );
        assert_eq!(
            age_between("2026-07-13T09:30:00Z", now()).as_deref(),
            Some("2h30m")
        );
        assert_eq!(
            age_between("2026-07-01T00:00:00Z", now()).as_deref(),
            Some("12d12h")
        );
        assert_eq!(age_between("not-a-date", now()), None);
        // Future timestamps clamp to zero rather than going negative.
        assert_eq!(
            age_between("2026-07-14T00:00:00Z", now()).as_deref(),
            Some("0m")
        );
    }

    #[test]
    fn validate_k8s_name_blocks_flags_and_junk() {
        assert_eq!(validate_k8s_name("prod", "namespace"), Ok("prod"));
        assert_eq!(
            validate_k8s_name("gke_proj_us-east1_cluster", "context"),
            Ok("gke_proj_us-east1_cluster")
        );
        assert!(validate_k8s_name("--kubeconfig=/tmp/x", "context").is_err());
        assert!(validate_k8s_name("", "namespace").is_err());
        assert!(validate_k8s_name("a b", "namespace").is_err());
        assert!(validate_k8s_name("x;rm", "namespace").is_err());
    }

    #[test]
    fn resource_names_and_namespacing() {
        assert_eq!(K8sResource::Ingresses.kubectl_name(), "ingresses");
        assert!(!K8sResource::Nodes.namespaced());
        assert!(K8sResource::Pods.namespaced());
        assert_eq!(
            serde_json::from_str::<K8sResource>("\"deployments\"").unwrap(),
            K8sResource::Deployments
        );
        assert!(serde_json::from_str::<K8sResource>("\"secrets\"").is_err());
    }
}
