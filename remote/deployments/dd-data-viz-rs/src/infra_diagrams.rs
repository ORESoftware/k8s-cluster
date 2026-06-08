use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::{clean_identifier, now_ms, xml_escape};

const MAX_TERRAFORM_FILES: usize = 64;
const MAX_TERRAFORM_BYTES: usize = 512 * 1024;
const MAX_INVENTORY_RESOURCES: usize = 2_000;
const MAX_DIAGRAM_NODES: usize = 2_000;
const MAX_DIAGRAM_EDGES: usize = 6_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InfraDiagramRequest {
    pub title: Option<String>,
    pub source: InfraSource,
    pub max_nodes: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum InfraSource {
    Terraform {
        files: BTreeMap<String, String>,
    },
    AwsInventory {
        resources: Vec<InventoryResource>,
    },
    GcpInventory {
        resources: Vec<InventoryResource>,
    },
    Mixed {
        terraform: Option<BTreeMap<String, String>>,
        resources: Vec<InventoryResource>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InventoryResource {
    pub id: String,
    pub label: Option<String>,
    pub provider: Option<String>,
    pub resource_type: String,
    pub service: Option<String>,
    pub region: Option<String>,
    pub zone: Option<String>,
    pub parent_id: Option<String>,
    pub references: Option<Vec<String>>,
    pub tags: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InfraDiagramResponse {
    ok: bool,
    schema_version: &'static str,
    title: String,
    source_kind: String,
    node_count: usize,
    edge_count: usize,
    group_count: usize,
    generated_at_ms: u128,
    graph: InfraGraph,
    renderers: DiagramRenderers,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InfraGraph {
    nodes: Vec<InfraNode>,
    edges: Vec<InfraEdge>,
    groups: Vec<InfraGroup>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InfraNode {
    id: String,
    label: String,
    provider: String,
    service: String,
    resource_type: String,
    region: Option<String>,
    zone: Option<String>,
    tag_count: usize,
    source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InfraEdge {
    from: String,
    to: String,
    relation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InfraGroup {
    id: String,
    label: String,
    provider: String,
    service: String,
    node_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagramRenderers {
    mermaid: String,
    graphviz_dot: String,
    plantuml: String,
    d2: String,
    structurizr_dsl: String,
    cytoscape: Value,
    drawio_mxfile: String,
    excalidraw: Value,
    renderer_catalog: Vec<&'static str>,
}

pub(crate) fn generate(request: InfraDiagramRequest) -> Result<InfraDiagramResponse, String> {
    let max_nodes = request
        .max_nodes
        .unwrap_or(MAX_DIAGRAM_NODES)
        .clamp(1, MAX_DIAGRAM_NODES);
    let title = request
        .title
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Infrastructure Diagram".to_string());
    let source_kind = request.source.kind_label().to_string();
    let mut warnings = Vec::new();
    let mut builder = GraphBuilder::default();
    match request.source {
        InfraSource::Terraform { files } => {
            parse_terraform_files(files, &mut builder, &mut warnings)?
        }
        InfraSource::AwsInventory { resources } => {
            parse_inventory("aws", resources, &mut builder, &mut warnings)?
        }
        InfraSource::GcpInventory { resources } => {
            parse_inventory("gcp", resources, &mut builder, &mut warnings)?
        }
        InfraSource::Mixed {
            terraform,
            resources,
        } => {
            if let Some(files) = terraform {
                parse_terraform_files(files, &mut builder, &mut warnings)?;
            }
            parse_inventory("mixed", resources, &mut builder, &mut warnings)?;
        }
    }
    let graph = builder.finish(max_nodes, &mut warnings)?;
    let renderers = renderers(&title, &graph);

    Ok(InfraDiagramResponse {
        ok: true,
        schema_version: "data-viz.infra-diagram.v1",
        title,
        source_kind,
        node_count: graph.nodes.len(),
        edge_count: graph.edges.len(),
        group_count: graph.groups.len(),
        generated_at_ms: now_ms(),
        graph,
        renderers,
        warnings,
    })
}

impl InfraSource {
    fn kind_label(&self) -> &'static str {
        match self {
            Self::Terraform { .. } => "terraform",
            Self::AwsInventory { .. } => "aws-inventory",
            Self::GcpInventory { .. } => "gcp-inventory",
            Self::Mixed { .. } => "mixed",
        }
    }
}

#[derive(Default)]
struct GraphBuilder {
    nodes: BTreeMap<String, InfraNode>,
    edges: BTreeSet<(String, String, String)>,
}

impl GraphBuilder {
    fn add_node(&mut self, node: InfraNode) {
        self.nodes.entry(node.id.clone()).or_insert(node);
    }

    fn add_edge(&mut self, from: String, to: String, relation: impl Into<String>) {
        if from == to || from.is_empty() || to.is_empty() {
            return;
        }
        self.edges.insert((from, to, relation.into()));
    }

    fn finish(self, max_nodes: usize, warnings: &mut Vec<String>) -> Result<InfraGraph, String> {
        if self.nodes.is_empty() {
            return Err("infrastructure diagram needs at least one resource".to_string());
        }
        let mut nodes = self.nodes.into_values().collect::<Vec<_>>();
        nodes.sort_by(|left, right| left.id.cmp(&right.id));
        if nodes.len() > max_nodes {
            warnings.push(format!(
                "diagram node count {} exceeded max {}; graph was truncated",
                nodes.len(),
                max_nodes
            ));
            let keep = nodes
                .iter()
                .take(max_nodes)
                .map(|node| node.id.clone())
                .collect::<BTreeSet<_>>();
            nodes.truncate(max_nodes);
            let edges = self
                .edges
                .into_iter()
                .filter(|(from, to, _)| keep.contains(from) && keep.contains(to))
                .take(MAX_DIAGRAM_EDGES)
                .map(|(from, to, relation)| InfraEdge { from, to, relation })
                .collect::<Vec<_>>();
            let groups = groups_for_nodes(&nodes);
            return Ok(InfraGraph {
                nodes,
                edges,
                groups,
            });
        }
        let node_ids = nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>();
        let mut edges = self
            .edges
            .into_iter()
            .filter(|(from, to, _)| node_ids.contains(from) && node_ids.contains(to))
            .take(MAX_DIAGRAM_EDGES)
            .map(|(from, to, relation)| InfraEdge { from, to, relation })
            .collect::<Vec<_>>();
        edges.sort_by(|left, right| {
            left.from
                .cmp(&right.from)
                .then_with(|| left.to.cmp(&right.to))
                .then_with(|| left.relation.cmp(&right.relation))
        });
        let groups = groups_for_nodes(&nodes);
        Ok(InfraGraph {
            nodes,
            edges,
            groups,
        })
    }
}

fn parse_terraform_files(
    files: BTreeMap<String, String>,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if files.is_empty() {
        return Err("terraform source requires at least one file".to_string());
    }
    if files.len() > MAX_TERRAFORM_FILES {
        return Err(format!("terraform files exceeds max {MAX_TERRAFORM_FILES}"));
    }
    let total_bytes = files.values().map(String::len).sum::<usize>();
    if total_bytes > MAX_TERRAFORM_BYTES {
        return Err(format!(
            "terraform files exceed max {MAX_TERRAFORM_BYTES} bytes"
        ));
    }

    let mut references = Vec::<(String, String, String)>::new();
    for (path, content) in files {
        let resources = terraform_resources(&path, &content, warnings);
        for resource in resources {
            builder.add_node(InfraNode {
                id: resource.id.clone(),
                label: resource.name.clone(),
                provider: provider_for_terraform_type(&resource.resource_type).to_string(),
                service: service_for_type(&resource.resource_type).to_string(),
                resource_type: resource.resource_type.clone(),
                region: None,
                zone: None,
                tag_count: 0,
                source: format!("terraform:{path}"),
            });
            for reference in resource.references {
                references.push((
                    reference,
                    resource.id.clone(),
                    "terraform-reference".to_string(),
                ));
            }
        }
    }
    for (from, to, relation) in references {
        builder.add_edge(from, to, relation);
    }
    Ok(())
}

fn terraform_resources(path: &str, content: &str, warnings: &mut Vec<String>) -> Vec<TfResource> {
    let mut resources = Vec::new();
    let lines = content.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if !line.starts_with("resource ") {
            index += 1;
            continue;
        }
        let Some((resource_type, name)) = parse_resource_header(line) else {
            warnings.push(format!(
                "could not parse Terraform resource header in {path}: {line}"
            ));
            index += 1;
            continue;
        };
        let id = format!("{resource_type}.{name}");
        let mut references = BTreeSet::new();
        let mut brace_depth = count_char(line, '{') as isize - count_char(line, '}') as isize;
        index += 1;
        while index < lines.len() && brace_depth > 0 {
            let body_line = lines[index].trim();
            for reference in terraform_references(body_line) {
                if reference != id {
                    references.insert(reference);
                }
            }
            brace_depth += count_char(body_line, '{') as isize;
            brace_depth -= count_char(body_line, '}') as isize;
            index += 1;
        }
        resources.push(TfResource {
            id,
            resource_type,
            name,
            references: references.into_iter().collect(),
        });
    }
    resources
}

fn parse_inventory(
    default_provider: &str,
    resources: Vec<InventoryResource>,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if resources.is_empty() {
        return Err("inventory source requires at least one resource".to_string());
    }
    if resources.len() > MAX_INVENTORY_RESOURCES {
        return Err(format!(
            "inventory resources exceeds max {MAX_INVENTORY_RESOURCES}"
        ));
    }
    for resource in resources {
        let id = clean_identifier(&resource.id)
            .unwrap_or_else(|| safe_id(&resource.id).unwrap_or_else(|| "resource".to_string()));
        let provider = resource
            .provider
            .as_deref()
            .and_then(clean_identifier)
            .unwrap_or_else(|| default_provider.to_string());
        let resource_type = clean_identifier(&resource.resource_type).unwrap_or_else(|| {
            safe_id(&resource.resource_type).unwrap_or_else(|| "resource".to_string())
        });
        let service = resource
            .service
            .as_deref()
            .and_then(clean_identifier)
            .unwrap_or_else(|| service_for_type(&resource_type).to_string());
        let label = resource
            .label
            .map(|label| label.trim().to_string())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| id.clone());
        let tag_count = resource.tags.as_ref().map(BTreeMap::len).unwrap_or(0);
        builder.add_node(InfraNode {
            id: id.clone(),
            label,
            provider,
            service,
            resource_type,
            region: resource.region,
            zone: resource.zone,
            tag_count,
            source: "inventory".to_string(),
        });
        if let Some(parent_id) = resource
            .parent_id
            .and_then(|value| clean_identifier(&value))
        {
            builder.add_edge(parent_id, id.clone(), "contains");
        }
        for reference in resource.references.unwrap_or_default() {
            if let Some(reference) = clean_identifier(&reference) {
                builder.add_edge(reference, id.clone(), "references");
            } else {
                warnings.push(format!("ignored invalid inventory reference `{reference}`"));
            }
        }
    }
    Ok(())
}

fn parse_resource_header(line: &str) -> Option<(String, String)> {
    let mut quoted = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    for ch in line.chars() {
        if ch == '"' {
            if in_quote {
                quoted.push(current.clone());
                current.clear();
            }
            in_quote = !in_quote;
        } else if in_quote {
            current.push(ch);
        }
    }
    if quoted.len() < 2 {
        return None;
    }
    let resource_type = clean_identifier(&quoted[0])?;
    let name = clean_identifier(&quoted[1])?;
    Some((resource_type, name))
}

fn terraform_references(line: &str) -> Vec<String> {
    let mut references = BTreeSet::new();
    for raw_token in
        line.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')))
    {
        let parts = raw_token.split('.').collect::<Vec<_>>();
        if parts.len() < 2 {
            continue;
        }
        let resource_type = parts[0];
        let name = parts[1];
        if !(resource_type.starts_with("aws_")
            || resource_type.starts_with("google_")
            || resource_type.starts_with("azurerm_")
            || resource_type.starts_with("kubernetes_"))
        {
            continue;
        }
        if let (Some(resource_type), Some(name)) =
            (clean_identifier(resource_type), clean_identifier(name))
        {
            references.insert(format!("{resource_type}.{name}"));
        }
    }
    references.into_iter().collect()
}

fn groups_for_nodes(nodes: &[InfraNode]) -> Vec<InfraGroup> {
    let mut groups = BTreeMap::<(String, String), usize>::new();
    for node in nodes {
        *groups
            .entry((node.provider.clone(), node.service.clone()))
            .or_default() += 1;
    }
    groups
        .into_iter()
        .map(|((provider, service), node_count)| {
            let id = format!("{provider}:{service}");
            InfraGroup {
                id,
                label: format!("{provider} {service}"),
                provider,
                service,
                node_count,
            }
        })
        .collect()
}

fn renderers(title: &str, graph: &InfraGraph) -> DiagramRenderers {
    DiagramRenderers {
        mermaid: mermaid(title, graph),
        graphviz_dot: graphviz_dot(title, graph),
        plantuml: plantuml(title, graph),
        d2: d2(title, graph),
        structurizr_dsl: structurizr_dsl(title, graph),
        cytoscape: cytoscape(graph),
        drawio_mxfile: drawio_mxfile(title, graph),
        excalidraw: excalidraw(graph),
        renderer_catalog: vec![
            "mermaid",
            "graphviz-dot",
            "plantuml",
            "d2",
            "structurizr-dsl",
            "cytoscape-json",
            "drawio-mxfile",
            "excalidraw-json",
        ],
    }
}

fn mermaid(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("---\ntitle: {}\n---\nflowchart LR\n", title);
    for node in &graph.nodes {
        output.push_str("  ");
        output.push_str(&mermaid_id(&node.id));
        output.push_str("[\"");
        output.push_str(&node.label.replace('"', "'"));
        output.push_str("<br/>");
        output.push_str(&node.resource_type);
        output.push_str("\"]\n");
    }
    for edge in &graph.edges {
        output.push_str("  ");
        output.push_str(&mermaid_id(&edge.from));
        output.push_str(" -->|");
        output.push_str(&edge.relation);
        output.push_str("| ");
        output.push_str(&mermaid_id(&edge.to));
        output.push('\n');
    }
    output
}

fn graphviz_dot(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!(
        "digraph \"{}\" {{\n  rankdir=LR;\n",
        title.replace('"', "'")
    );
    for node in &graph.nodes {
        output.push_str("  \"");
        output.push_str(&node.id);
        output.push_str("\" [label=\"");
        output.push_str(&node.label.replace('"', "'"));
        output.push_str("\\n");
        output.push_str(&node.resource_type);
        output.push_str("\"];\n");
    }
    for edge in &graph.edges {
        output.push_str("  \"");
        output.push_str(&edge.from);
        output.push_str("\" -> \"");
        output.push_str(&edge.to);
        output.push_str("\" [label=\"");
        output.push_str(&edge.relation);
        output.push_str("\"];\n");
    }
    output.push_str("}\n");
    output
}

fn plantuml(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("@startuml\n title {}\n", title.replace('\n', " "));
    for node in &graph.nodes {
        output.push_str(" rectangle \"");
        output.push_str(&node.label.replace('"', "'"));
        output.push_str("\\n");
        output.push_str(&node.resource_type);
        output.push_str("\" as ");
        output.push_str(&mermaid_id(&node.id));
        output.push('\n');
    }
    for edge in &graph.edges {
        output.push(' ');
        output.push_str(&mermaid_id(&edge.from));
        output.push_str(" --> ");
        output.push_str(&mermaid_id(&edge.to));
        output.push_str(" : ");
        output.push_str(&edge.relation);
        output.push('\n');
    }
    output.push_str("@enduml\n");
    output
}

fn d2(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("title: {}\n", title.replace('\n', " "));
    for node in &graph.nodes {
        output.push_str(&mermaid_id(&node.id));
        output.push_str(": \"");
        output.push_str(&node.label.replace('"', "'"));
        output.push_str("\\n");
        output.push_str(&node.resource_type);
        output.push_str("\"\n");
    }
    for edge in &graph.edges {
        output.push_str(&mermaid_id(&edge.from));
        output.push_str(" -> ");
        output.push_str(&mermaid_id(&edge.to));
        output.push_str(": ");
        output.push_str(&edge.relation);
        output.push('\n');
    }
    output
}

fn structurizr_dsl(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!(
        "workspace \"{}\" \"Generated infrastructure model\" {{\n  model {{\n",
        title.replace('"', "'")
    );
    for node in &graph.nodes {
        output.push_str("    ");
        output.push_str(&mermaid_id(&node.id));
        output.push_str(" = softwareSystem \"");
        output.push_str(&node.label.replace('"', "'"));
        output.push_str("\" \"");
        output.push_str(&node.resource_type);
        output.push_str("\"\n");
    }
    for edge in &graph.edges {
        output.push_str("    ");
        output.push_str(&mermaid_id(&edge.from));
        output.push_str(" -> ");
        output.push_str(&mermaid_id(&edge.to));
        output.push_str(" \"");
        output.push_str(&edge.relation);
        output.push_str("\"\n");
    }
    output.push_str("  }\n  views { systemLandscape { include * autolayout lr } }\n}\n");
    output
}

fn cytoscape(graph: &InfraGraph) -> Value {
    let mut elements = Vec::new();
    for node in &graph.nodes {
        elements.push(json!({
            "data": {
                "id": node.id,
                "label": node.label,
                "provider": node.provider,
                "service": node.service,
                "resourceType": node.resource_type,
                "tagCount": node.tag_count
            }
        }));
    }
    for (index, edge) in graph.edges.iter().enumerate() {
        elements.push(json!({
            "data": {
                "id": format!("edge-{index}"),
                "source": edge.from,
                "target": edge.to,
                "label": edge.relation
            }
        }));
    }
    json!({ "elements": elements })
}

fn drawio_mxfile(title: &str, graph: &InfraGraph) -> String {
    let mut cells = String::from(r#"<mxCell id="0"/><mxCell id="1" parent="0"/>"#);
    for (index, node) in graph.nodes.iter().enumerate() {
        let x = 80 + (index % 5) * 220;
        let y = 80 + (index / 5) * 140;
        cells.push_str(&format!(
            r#"<mxCell id="{}" value="{}&#xa;{}" style="rounded=1;whiteSpace=wrap;html=1;" vertex="1" parent="1"><mxGeometry x="{}" y="{}" width="180" height="80" as="geometry"/></mxCell>"#,
            xml_escape(&node.id),
            xml_escape(&node.label),
            xml_escape(&node.resource_type),
            x,
            y
        ));
    }
    for (index, edge) in graph.edges.iter().enumerate() {
        cells.push_str(&format!(
            r#"<mxCell id="edge-{}" value="{}" edge="1" source="{}" target="{}" parent="1"><mxGeometry relative="1" as="geometry"/></mxCell>"#,
            index,
            xml_escape(&edge.relation),
            xml_escape(&edge.from),
            xml_escape(&edge.to)
        ));
    }
    format!(
        r#"<mxfile><diagram name="{}"><mxGraphModel><root>{}</root></mxGraphModel></diagram></mxfile>"#,
        xml_escape(title),
        cells
    )
}

fn excalidraw(graph: &InfraGraph) -> Value {
    let mut elements = Vec::new();
    for (index, node) in graph.nodes.iter().enumerate() {
        elements.push(json!({
            "id": node.id,
            "type": "rectangle",
            "x": 80 + (index % 5) * 220,
            "y": 80 + (index / 5) * 140,
            "width": 180,
            "height": 80,
            "label": format!("{}\\n{}", node.label, node.resource_type)
        }));
    }
    for (index, edge) in graph.edges.iter().enumerate() {
        elements.push(json!({
            "id": format!("edge-{index}"),
            "type": "arrow",
            "startBinding": edge.from,
            "endBinding": edge.to,
            "label": edge.relation
        }));
    }
    json!({
        "type": "excalidraw",
        "version": 2,
        "source": "dd-data-viz-rs",
        "elements": elements
    })
}

fn provider_for_terraform_type(resource_type: &str) -> &'static str {
    if resource_type.starts_with("aws_") {
        "aws"
    } else if resource_type.starts_with("google_") {
        "gcp"
    } else if resource_type.starts_with("azurerm_") {
        "azure"
    } else if resource_type.starts_with("kubernetes_") {
        "kubernetes"
    } else {
        "terraform"
    }
}

fn service_for_type(resource_type: &str) -> &str {
    resource_type
        .trim_start_matches("aws_")
        .trim_start_matches("google_")
        .trim_start_matches("azurerm_")
        .trim_start_matches("kubernetes_")
        .split('_')
        .next()
        .unwrap_or("resource")
}

fn mermaid_id(id: &str) -> String {
    id.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn safe_id(input: &str) -> Option<String> {
    let cleaned = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    clean_identifier(&cleaned)
}

fn count_char(input: &str, target: char) -> usize {
    input.chars().filter(|ch| *ch == target).count()
}

#[derive(Debug)]
struct TfResource {
    id: String,
    resource_type: String,
    name: String,
    references: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terraform_diagram_links_referenced_resources() {
        let response = generate(InfraDiagramRequest {
            title: Some("AWS Network".to_string()),
            source: InfraSource::Terraform {
                files: BTreeMap::from([(
                    "main.tf".to_string(),
                    r#"
                    resource "aws_vpc" "main" {
                      cidr_block = "10.0.0.0/16"
                    }

                    resource "aws_subnet" "public" {
                      vpc_id = aws_vpc.main.id
                    }
                    "#
                    .to_string(),
                )]),
            },
            max_nodes: None,
        })
        .expect("diagram generates");

        assert_eq!(response.node_count, 2);
        assert!(response
            .graph
            .edges
            .iter()
            .any(|edge| edge.from == "aws_vpc.main" && edge.to == "aws_subnet.public"));
        assert!(response.renderers.mermaid.contains("flowchart LR"));
        assert!(response.renderers.graphviz_dot.contains("digraph"));
        assert!(response.renderers.renderer_catalog.contains(&"plantuml"));
    }

    #[test]
    fn gcp_inventory_diagram_uses_parent_and_references() {
        let response = generate(InfraDiagramRequest {
            title: None,
            source: InfraSource::GcpInventory {
                resources: vec![
                    InventoryResource {
                        id: "vpc".to_string(),
                        label: Some("Shared VPC".to_string()),
                        provider: Some("gcp".to_string()),
                        resource_type: "compute_network".to_string(),
                        service: Some("compute".to_string()),
                        region: None,
                        zone: None,
                        parent_id: None,
                        references: None,
                        tags: None,
                    },
                    InventoryResource {
                        id: "subnet".to_string(),
                        label: None,
                        provider: Some("gcp".to_string()),
                        resource_type: "compute_subnetwork".to_string(),
                        service: Some("compute".to_string()),
                        region: Some("us-central1".to_string()),
                        zone: None,
                        parent_id: Some("vpc".to_string()),
                        references: None,
                        tags: None,
                    },
                ],
            },
            max_nodes: Some(20),
        })
        .expect("inventory diagram");

        assert_eq!(response.node_count, 2);
        assert_eq!(response.edge_count, 1);
        assert_eq!(response.graph.groups[0].provider, "gcp");
        assert!(response.renderers.drawio_mxfile.contains("<mxfile>"));
    }
}
