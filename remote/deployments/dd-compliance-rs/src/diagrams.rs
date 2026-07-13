use std::collections::BTreeMap;

use crate::{
    config::SCHEMA_VERSION,
    models::{
        DiagramArtifact, DiagramReport, DiagramRequest, InfraEdge, InfraMatch, InfraNode,
    },
    util::{normalize_key, now_ms},
};

pub async fn generate_infrastructure_diagram(request: DiagramRequest) -> DiagramReport {
    let request_id = request
        .request_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("diagram-{}", now_ms()));
    let title = request
        .title
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Infrastructure parity diagram".to_string());

    let mut desired_nodes = Vec::new();
    desired_nodes.extend(parse_terraform_nodes(&request.terraform));
    desired_nodes.extend(parse_kubernetes_nodes(&request.gitops, "gitops"));
    let mut live_nodes = parse_kubernetes_nodes(&request.live, "live");
    for node in request.nodes {
        if normalize_key(&node.source).contains("live") {
            live_nodes.push(node);
        } else {
            desired_nodes.push(node);
        }
    }
    desired_nodes = dedupe_nodes(desired_nodes);
    live_nodes = dedupe_nodes(live_nodes);

    let (matches, missing_in_live, unexpected_live) = compare_nodes(&desired_nodes, &live_nodes);
    let mut edges = request.edges;
    edges.extend(match_edges(&matches));
    let mermaid = render_mermaid(
        &title,
        &desired_nodes,
        &live_nodes,
        &edges,
        &missing_in_live,
        &unexpected_live,
    );

    let include_local = request
        .options
        .as_ref()
        .and_then(|options| options.include_local_mermaid)
        .unwrap_or(true);
    let mut diagrams = Vec::new();
    if include_local {
        diagrams.push(DiagramArtifact {
            kind: "infrastructure-parity".to_string(),
            format: "mermaid".to_string(),
            renderer: "dd-compliance-rs".to_string(),
            content: mermaid.clone(),
        });
    }

    let ok = missing_in_live.is_empty() && unexpected_live.is_empty();
    let summary = format!(
        "{} desired node(s), {} live node(s), {} match(es), {} missing, {} unexpected.",
        desired_nodes.len(),
        live_nodes.len(),
        matches.len(),
        missing_in_live.len(),
        unexpected_live.len()
    );
    DiagramReport {
        ok,
        request_id,
        schema_version: SCHEMA_VERSION.to_string(),
        title,
        summary,
        desired_nodes,
        live_nodes,
        edges,
        matches,
        missing_in_live,
        unexpected_live,
        diagrams,
        generated_at_ms: now_ms(),
        notes: vec![
            "Diagram parity is based on supplied Terraform, GitOps, and live inventory evidence; provide fresh dd_cluster or kubectl inventory to compare against runtime state.".to_string(),
            "dd-compliance-rs emits Mermaid locally for infrastructure parity diagrams.".to_string(),
        ],
    }
}

fn parse_terraform_nodes(sources: &[crate::models::DiagramSource]) -> Vec<InfraNode> {
    let mut nodes = Vec::new();
    for source in sources {
        let source_name = source.name.as_deref().unwrap_or("terraform");
        for line in source.content.lines() {
            let trimmed = line.trim();
            let quoted = quoted_parts(trimmed);
            if trimmed.starts_with("resource ") && quoted.len() >= 2 {
                let kind = quoted[0].clone();
                let name = quoted[1].clone();
                nodes.push(InfraNode {
                    id: format!(
                        "terraform-{}-{}",
                        normalize_key(&kind),
                        normalize_key(&name)
                    ),
                    label: format!("{kind}.{name}"),
                    kind,
                    source: source_name.to_string(),
                    namespace: None,
                });
            } else if trimmed.starts_with("module ") && !quoted.is_empty() {
                let name = quoted[0].clone();
                nodes.push(InfraNode {
                    id: format!("terraform-module-{}", normalize_key(&name)),
                    label: format!("module.{name}"),
                    kind: "terraform_module".to_string(),
                    source: source_name.to_string(),
                    namespace: None,
                });
            }
        }
    }
    nodes
}

fn parse_kubernetes_nodes(
    sources: &[crate::models::DiagramSource],
    source_kind: &str,
) -> Vec<InfraNode> {
    let mut nodes = Vec::new();
    for source in sources {
        let source_name = source.name.as_deref().unwrap_or(source_kind);
        let mut kind: Option<String> = None;
        let mut name: Option<String> = None;
        let mut namespace: Option<String> = None;
        let mut in_metadata = false;
        let mut metadata_indent = 0usize;
        for raw_line in source.content.lines().chain(std::iter::once("---")) {
            let indent = raw_line.chars().take_while(|ch| ch.is_whitespace()).count();
            let line = raw_line.trim();
            if line == "---" || line.starts_with("kind:") {
                if let (Some(kind_value), Some(name_value)) = (kind.take(), name.take()) {
                    nodes.push(kubernetes_node(
                        source_name,
                        source_kind,
                        &kind_value,
                        &name_value,
                        namespace.take(),
                    ));
                }
                if let Some(value) = line.strip_prefix("kind:") {
                    kind = Some(clean_yaml_scalar(value));
                }
                in_metadata = false;
                continue;
            }
            if line == "metadata:" {
                in_metadata = true;
                metadata_indent = indent;
                continue;
            }
            if in_metadata && indent <= metadata_indent && !line.is_empty() {
                in_metadata = false;
            }
            if in_metadata {
                if let Some(value) = line.strip_prefix("name:") {
                    name = Some(clean_yaml_scalar(value));
                } else if let Some(value) = line.strip_prefix("namespace:") {
                    namespace = Some(clean_yaml_scalar(value));
                }
            }
        }
    }
    nodes
}

fn kubernetes_node(
    source_name: &str,
    source_kind: &str,
    kind: &str,
    name: &str,
    namespace: Option<String>,
) -> InfraNode {
    InfraNode {
        id: format!(
            "{}-{}-{}",
            normalize_key(source_kind),
            normalize_key(kind),
            normalize_key(name)
        ),
        label: format!("{kind}/{name}"),
        kind: kind.to_string(),
        source: source_name.to_string(),
        namespace,
    }
}

fn quoted_parts(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    for ch in value.chars() {
        if in_quote {
            if escaped {
                current.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                parts.push(current.clone());
                current.clear();
                in_quote = false;
            } else {
                current.push(ch);
            }
        } else if ch == '"' {
            in_quote = true;
        }
    }
    parts
}

fn clean_yaml_scalar(value: &str) -> String {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string()
}

fn dedupe_nodes(nodes: Vec<InfraNode>) -> Vec<InfraNode> {
    let mut by_id = BTreeMap::new();
    for node in nodes {
        by_id.entry(node.id.clone()).or_insert(node);
    }
    by_id.into_values().collect()
}

fn compare_nodes(
    desired_nodes: &[InfraNode],
    live_nodes: &[InfraNode],
) -> (Vec<InfraMatch>, Vec<InfraNode>, Vec<InfraNode>) {
    let mut live_by_key = BTreeMap::<String, &InfraNode>::new();
    for live in live_nodes {
        live_by_key.entry(match_key(live)).or_insert(live);
    }
    let mut matched_live = BTreeMap::<String, bool>::new();
    let mut matches = Vec::new();
    let mut missing = Vec::new();
    for desired in desired_nodes {
        let key = match_key(desired);
        if let Some(live) = live_by_key.get(&key) {
            matched_live.insert(live.id.clone(), true);
            matches.push(InfraMatch {
                desired_id: desired.id.clone(),
                live_id: live.id.clone(),
                normalized_name: key,
            });
        } else {
            missing.push(desired.clone());
        }
    }
    let unexpected = live_nodes
        .iter()
        .filter(|node| !matched_live.contains_key(&node.id))
        .cloned()
        .collect();
    (matches, missing, unexpected)
}

fn match_key(node: &InfraNode) -> String {
    let label = node.label.split('/').last().unwrap_or(&node.label);
    normalize_key(label.trim_start_matches("module."))
}

fn match_edges(matches: &[InfraMatch]) -> Vec<InfraEdge> {
    matches
        .iter()
        .map(|item| InfraEdge {
            from: item.desired_id.clone(),
            to: item.live_id.clone(),
            label: Some("matches live".to_string()),
        })
        .collect()
}

fn render_mermaid(
    title: &str,
    desired_nodes: &[InfraNode],
    live_nodes: &[InfraNode],
    edges: &[InfraEdge],
    missing: &[InfraNode],
    unexpected: &[InfraNode],
) -> String {
    let mut out = String::new();
    out.push_str("flowchart LR\n");
    out.push_str(&format!("  title[\"{}\"]\n", mermaid_label(title)));
    out.push_str("  subgraph desired[\"Terraform / GitOps desired\"]\n");
    if desired_nodes.is_empty() {
        out.push_str("    desired_empty[\"no desired evidence supplied\"]\n");
    }
    for node in desired_nodes {
        out.push_str(&format!(
            "    {}[\"{}\"]\n",
            mermaid_id(&node.id),
            mermaid_label(&format!("{}\\n{}", node.label, node.source))
        ));
    }
    out.push_str("  end\n");
    out.push_str("  subgraph live[\"Live infrastructure\"]\n");
    if live_nodes.is_empty() {
        out.push_str("    live_empty[\"no live inventory supplied\"]\n");
    }
    for node in live_nodes {
        out.push_str(&format!(
            "    {}[\"{}\"]\n",
            mermaid_id(&node.id),
            mermaid_label(&format!("{}\\n{}", node.label, node.source))
        ));
    }
    out.push_str("  end\n");
    for edge in edges {
        out.push_str(&format!(
            "  {} -->|{}| {}\n",
            mermaid_id(&edge.from),
            mermaid_label(edge.label.as_deref().unwrap_or("relates")),
            mermaid_id(&edge.to)
        ));
    }
    for node in missing {
        out.push_str(&format!(
            "  {} -. missing in live .-> missing_{}\n",
            mermaid_id(&node.id),
            mermaid_id(&node.id)
        ));
        out.push_str(&format!(
            "  missing_{}[\"missing: {}\"]\n",
            mermaid_id(&node.id),
            mermaid_label(&node.label)
        ));
    }
    for node in unexpected {
        out.push_str(&format!(
            "  {} -. unexpected live .-> review_{}\n",
            mermaid_id(&node.id),
            mermaid_id(&node.id)
        ));
        out.push_str(&format!(
            "  review_{}[\"review unexpected live node\"]\n",
            mermaid_id(&node.id)
        ));
    }
    out
}

fn mermaid_id(value: &str) -> String {
    let normalized = normalize_key(value).replace('-', "_");
    if normalized.is_empty() {
        "node".to_string()
    } else {
        normalized
    }
}

fn mermaid_label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DiagramSource;

    #[test]
    fn parses_terraform_and_kubernetes_nodes() {
        let terraform = vec![DiagramSource {
            name: Some("main.tf".to_string()),
            content: "resource \"aws_instance\" \"runtime\" {}\nmodule \"network\" {}".to_string(),
        }];
        let gitops = vec![DiagramSource {
            name: Some("deployment.yaml".to_string()),
            content:
                "kind: Deployment\nmetadata:\n  name: dd-compliance-rs\n  namespace: default\n"
                    .to_string(),
        }];
        assert_eq!(parse_terraform_nodes(&terraform).len(), 2);
        let nodes = parse_kubernetes_nodes(&gitops, "gitops");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].label, "Deployment/dd-compliance-rs");
    }

    #[test]
    fn mermaid_marks_missing_and_unexpected_live_nodes() {
        let desired = vec![InfraNode {
            id: "desired-deploy-compliance".to_string(),
            label: "Deployment/dd-compliance-rs".to_string(),
            kind: "Deployment".to_string(),
            source: "gitops".to_string(),
            namespace: Some("default".to_string()),
        }];
        let live = vec![InfraNode {
            id: "live-service-other".to_string(),
            label: "Service/other".to_string(),
            kind: "Service".to_string(),
            source: "live".to_string(),
            namespace: Some("default".to_string()),
        }];
        let (matches, missing, unexpected) = compare_nodes(&desired, &live);
        let mermaid = render_mermaid(
            "test",
            &desired,
            &live,
            &match_edges(&matches),
            &missing,
            &unexpected,
        );
        assert!(mermaid.contains("missing in live"));
        assert!(mermaid.contains("unexpected live"));
    }
}
