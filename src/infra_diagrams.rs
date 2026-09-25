use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::infra_imports;
use crate::util::{clean_identifier, now_ms, xml_escape};

const MAX_TERRAFORM_FILES: usize = 64;
const MAX_TERRAFORM_BYTES: usize = 512 * 1024;
const MAX_IMPORT_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_INVENTORY_RESOURCES: usize = 2_000;
const MAX_DIAGRAM_NODES: usize = 2_000;
const MAX_DIAGRAM_EDGES: usize = 6_000;

const SOURCE_CATALOG: &[&str] = &[
    "terraform",
    "terraform-plan",
    "terraform-state",
    "aws-inventory",
    "aws-resource-explorer",
    "aws-config",
    "gcp-inventory",
    "gcp-cloud-asset",
    "mixed",
];

const RENDERER_CATALOG: &[&str] = &[
    "mermaid",
    "mermaid-c4",
    "mermaid-mindmap",
    "graphviz-dot",
    "plantuml",
    "d2",
    "structurizr-dsl",
    "nomnoml",
    "blockdiag",
    "cytoscape-json",
    "drawio-mxfile",
    "excalidraw-json",
    "vega-force-json",
    "networkx-json",
    "json-graph",
    "vis-network-json",
    "sigma-graph-json",
    "echarts-graph-json",
    "elk-graph-json",
    "deckgl-layers-json",
    "gexf",
    "markmap-markdown",
    "markdown-inventory",
    "topology-json",
    "kroki-manifest",
];

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
    TerraformPlan {
        plan: Value,
    },
    TerraformState {
        state: Value,
    },
    AwsInventory {
        resources: Vec<InventoryResource>,
    },
    AwsResourceExplorer {
        resources: Vec<Value>,
    },
    AwsConfig {
        configuration_items: Vec<Value>,
    },
    GcpInventory {
        resources: Vec<InventoryResource>,
    },
    GcpCloudAsset {
        assets: Vec<Value>,
    },
    Mixed {
        terraform: Option<BTreeMap<String, String>>,
        terraform_plan: Option<Value>,
        terraform_state: Option<Value>,
        aws_config: Option<Vec<Value>>,
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
    mermaid_c4: String,
    mermaid_mindmap: String,
    graphviz_dot: String,
    plantuml: String,
    d2: String,
    structurizr_dsl: String,
    nomnoml: String,
    blockdiag: String,
    cytoscape: Value,
    drawio_mxfile: String,
    excalidraw: Value,
    vega_force: Value,
    networkx_json: Value,
    json_graph: Value,
    vis_network: Value,
    sigma_graph: Value,
    echarts_graph: Value,
    elk_graph: Value,
    deckgl_layers: Value,
    gexf: String,
    markmap_markdown: String,
    markdown_inventory: String,
    topology_json: Value,
    kroki_manifest: Value,
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
        InfraSource::TerraformPlan { plan } => {
            parse_terraform_plan(plan, &mut builder, &mut warnings)?
        }
        InfraSource::TerraformState { state } => {
            parse_terraform_state(state, &mut builder, &mut warnings)?
        }
        InfraSource::AwsInventory { resources } => {
            parse_inventory("aws", resources, &mut builder, &mut warnings)?
        }
        InfraSource::AwsResourceExplorer { resources } => {
            parse_aws_resource_explorer(resources, &mut builder, &mut warnings)?
        }
        InfraSource::AwsConfig {
            configuration_items,
        } => parse_aws_config(configuration_items, &mut builder, &mut warnings)?,
        InfraSource::GcpInventory { resources } => {
            parse_inventory("gcp", resources, &mut builder, &mut warnings)?
        }
        InfraSource::GcpCloudAsset { assets } => {
            parse_gcp_cloud_assets(assets, &mut builder, &mut warnings)?
        }
        InfraSource::Mixed {
            terraform,
            terraform_plan,
            terraform_state,
            aws_config,
            resources,
        } => {
            if let Some(files) = terraform {
                parse_terraform_files(files, &mut builder, &mut warnings)?;
            }
            if let Some(plan) = terraform_plan {
                parse_terraform_plan(plan, &mut builder, &mut warnings)?;
            }
            if let Some(state) = terraform_state {
                parse_terraform_state(state, &mut builder, &mut warnings)?;
            }
            if let Some(configuration_items) = aws_config {
                parse_aws_config(configuration_items, &mut builder, &mut warnings)?;
            }
            if !resources.is_empty() {
                parse_inventory("mixed", resources, &mut builder, &mut warnings)?;
            }
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

pub(crate) fn source_names() -> &'static [&'static str] {
    SOURCE_CATALOG
}

pub(crate) fn renderer_names() -> &'static [&'static str] {
    RENDERER_CATALOG
}

pub(crate) fn tool_catalog_payload() -> Value {
    json!({
        "schemaVersion": "data-viz.infra-diagram-tools.v1",
        "sources": source_names(),
        "renderers": renderer_names(),
        "toolFamilies": [
            {
                "family": "diagram-as-code",
                "tools": ["Mermaid", "Graphviz DOT", "PlantUML", "D2", "Structurizr DSL", "Nomnoml", "BlockDiag"],
                "bestFor": "Architecture review, pull request diffs, text-first diagrams, and repeatable docs pipelines."
            },
            {
                "family": "whiteboard-and-docs",
                "tools": ["draw.io", "Excalidraw", "Markmap", "Markdown inventory"],
                "bestFor": "Human handoff, design docs, presentation drafts, and quick inventory walkthroughs."
            },
            {
                "family": "interactive-web-graphs",
                "tools": ["Cytoscape.js", "Vega force", "vis-network", "Sigma.js", "Apache ECharts", "ELK"],
                "bestFor": "Large topology browsing, dynamic filtering, graph search, and layout experimentation."
            },
            {
                "family": "graph-analytics",
                "tools": ["NetworkX", "JSON Graph", "GEXF / Gephi", "topology JSON"],
                "bestFor": "Centrality, blast-radius analysis, ownership maps, dependency clustering, and offline graph science."
            },
            {
                "family": "spatial-and-presentation",
                "tools": ["deck.gl layers", "Kroki manifest", "PowerPoint / Google Slides downstream layers"],
                "bestFor": "Region-aware cloud maps, service overlays, exported decks, and multi-renderer batch jobs."
            }
        ],
        "layoutStrategies": [
            "layered-dag",
            "force-directed",
            "circular-service-clusters",
            "provider-service-swimlanes",
            "region-zone-facets",
            "blast-radius-neighborhood",
            "c4-context",
            "ownership-tree"
        ],
        "pipelines": [
            {
                "id": "terraform-to-review-pack",
                "inputs": ["terraform", "terraform-plan", "terraform-state"],
                "outputs": ["mermaid", "graphviz-dot", "d2", "structurizr-dsl", "kroki-manifest"],
                "purpose": "Generate pull-request friendly architecture diagrams from IaC."
            },
            {
                "id": "cloud-inventory-to-live-map",
                "inputs": ["aws-resource-explorer", "aws-config", "gcp-cloud-asset", "mixed"],
                "outputs": ["cytoscape-json", "sigma-graph-json", "echarts-graph-json", "deckgl-layers-json"],
                "purpose": "Feed interactive cloud topology browsers without exposing raw inventory attributes."
            },
            {
                "id": "topology-to-analytics",
                "inputs": ["terraform-plan", "terraform-state", "aws-inventory", "aws-config", "gcp-inventory", "mixed"],
                "outputs": ["networkx-json", "json-graph", "gexf", "topology-json"],
                "purpose": "Support graph algorithms, dependency scoring, and ownership or blast-radius analysis."
            }
        ],
        "posture": "Derived topology only; raw cloud/IaC attributes are not echoed, renderer outputs are bounded by graph node and edge caps."
    })
}

impl InfraSource {
    fn kind_label(&self) -> &'static str {
        match self {
            Self::Terraform { .. } => "terraform",
            Self::TerraformPlan { .. } => "terraform-plan",
            Self::TerraformState { .. } => "terraform-state",
            Self::AwsInventory { .. } => "aws-inventory",
            Self::AwsResourceExplorer { .. } => "aws-resource-explorer",
            Self::AwsConfig { .. } => "aws-config",
            Self::GcpInventory { .. } => "gcp-inventory",
            Self::GcpCloudAsset { .. } => "gcp-cloud-asset",
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

fn parse_terraform_plan(
    plan: Value,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    parse_terraform_json(plan, builder, warnings, "terraform-plan", true)
}

fn parse_terraform_state(
    state: Value,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    parse_terraform_json(state, builder, warnings, "terraform-state", false)
}

fn parse_terraform_json(
    value: Value,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
    source_label: &'static str,
    allow_planned_values: bool,
) -> Result<(), String> {
    let estimated_bytes = value.to_string().len();
    if estimated_bytes > MAX_IMPORT_JSON_BYTES {
        return Err(format!(
            "{source_label} JSON exceeds max {MAX_IMPORT_JSON_BYTES} bytes"
        ));
    }
    let root_module = if allow_planned_values {
        value
            .pointer("/planned_values/root_module")
            .or_else(|| value.pointer("/values/root_module"))
    } else {
        value
            .pointer("/values/root_module")
            .or_else(|| value.pointer("/planned_values/root_module"))
    };
    let Some(root_module) = root_module else {
        return Err(format!(
            "{source_label} source requires values.root_module or planned_values.root_module"
        ));
    };
    parse_terraform_json_module(root_module, builder, warnings, "root", source_label);
    Ok(())
}

fn parse_terraform_json_module(
    module: &Value,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
    module_path: &str,
    source_label: &'static str,
) {
    if let Some(resources) = module.get("resources").and_then(Value::as_array) {
        for resource in resources {
            let Some(address) = value_string(resource, &["address"]) else {
                warnings.push(format!(
                    "ignored {source_label} resource without address in {module_path}"
                ));
                continue;
            };
            let id = clean_identifier(&address).unwrap_or_else(|| {
                safe_id(&address)
                    .unwrap_or_else(|| format!("terraform-resource-{}", builder.nodes.len()))
            });
            let resource_type = value_string(resource, &["type"])
                .and_then(|value| clean_identifier(&value))
                .unwrap_or_else(|| "terraform_resource".to_string());
            let label = value_string(resource, &["name"]).unwrap_or_else(|| id.clone());
            let tag_count = resource
                .get("values")
                .and_then(|values| values.get("tags"))
                .and_then(Value::as_object)
                .map(serde_json::Map::len)
                .unwrap_or(0);
            builder.add_node(InfraNode {
                id: id.clone(),
                label,
                provider: provider_for_terraform_type(&resource_type).to_string(),
                service: service_for_type(&resource_type).to_string(),
                resource_type,
                region: value_string(resource, &["values", "region"]),
                zone: value_string(resource, &["values", "zone"]),
                tag_count,
                source: format!("{source_label}:{module_path}"),
            });
            for dependency in string_array(resource.get("depends_on")) {
                if let Some(dependency) =
                    clean_identifier(&dependency).or_else(|| safe_id(&dependency))
                {
                    builder.add_edge(dependency, id.clone(), "terraform-depends-on");
                }
            }
        }
    }

    if let Some(child_modules) = module.get("child_modules").and_then(Value::as_array) {
        for child in child_modules {
            let child_path =
                value_string(child, &["address"]).unwrap_or_else(|| module_path.to_string());
            parse_terraform_json_module(child, builder, warnings, &child_path, source_label);
        }
    }
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

fn parse_aws_resource_explorer(
    resources: Vec<Value>,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if resources.is_empty() {
        return Err("AWS Resource Explorer source requires at least one resource".to_string());
    }
    if resources.len() > MAX_INVENTORY_RESOURCES {
        return Err(format!(
            "AWS Resource Explorer resources exceeds max {MAX_INVENTORY_RESOURCES}"
        ));
    }
    let estimated_bytes = Value::Array(resources.clone()).to_string().len();
    if estimated_bytes > MAX_IMPORT_JSON_BYTES {
        return Err(format!(
            "AWS Resource Explorer JSON exceeds max {MAX_IMPORT_JSON_BYTES} bytes"
        ));
    }

    for resource in resources {
        let raw_id = value_string_any(&resource, &["Arn", "arn", "ResourceArn", "id", "Id"])
            .unwrap_or_else(|| format!("aws-resource-{}", builder.nodes.len()));
        let id = clean_identifier(&raw_id).unwrap_or_else(|| {
            safe_id(&raw_id).unwrap_or_else(|| format!("aws-resource-{}", builder.nodes.len()))
        });
        let resource_type = value_string_any(
            &resource,
            &["ResourceType", "resourceType", "Type", "type", "Service"],
        )
        .and_then(|value| safe_id(&value))
        .unwrap_or_else(|| aws_type_from_arn(&raw_id));
        let label = value_string_any(&resource, &["Name", "name", "DisplayName", "displayName"])
            .or_else(|| name_tag(&resource))
            .unwrap_or_else(|| arn_tail(&raw_id));
        let service = value_string_any(&resource, &["Service", "service"])
            .and_then(|value| safe_id(&value))
            .unwrap_or_else(|| aws_service_from_type(&resource_type));
        let tag_count = tag_count(&resource);
        builder.add_node(InfraNode {
            id: id.clone(),
            label,
            provider: "aws".to_string(),
            service,
            resource_type,
            region: value_string_any(&resource, &["Region", "region", "AwsRegion", "awsRegion"]),
            zone: value_string_any(
                &resource,
                &["AvailabilityZone", "availabilityZone", "Zone", "zone"],
            ),
            tag_count,
            source: "aws-resource-explorer".to_string(),
        });
        for reference in relationship_targets(&resource) {
            if let Some(reference) = clean_identifier(&reference).or_else(|| safe_id(&reference)) {
                builder.add_edge(reference, id.clone(), "aws-relationship");
            } else {
                warnings.push(format!(
                    "ignored invalid AWS relationship target `{reference}`"
                ));
            }
        }
    }
    Ok(())
}

fn parse_aws_config(
    configuration_items: Vec<Value>,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if configuration_items.is_empty() {
        return Err("AWS Config source requires at least one configuration item".to_string());
    }
    if configuration_items.len() > MAX_INVENTORY_RESOURCES {
        return Err(format!(
            "AWS Config configuration items exceeds max {MAX_INVENTORY_RESOURCES}"
        ));
    }
    let estimated_bytes = Value::Array(configuration_items.clone()).to_string().len();
    if estimated_bytes > MAX_IMPORT_JSON_BYTES {
        return Err(format!(
            "AWS Config JSON exceeds max {MAX_IMPORT_JSON_BYTES} bytes"
        ));
    }

    for resource in infra_imports::aws_config_resources(&configuration_items) {
        let id = clean_identifier(&resource.id).unwrap_or_else(|| {
            safe_id(&resource.id).unwrap_or_else(|| format!("aws-config-{}", builder.nodes.len()))
        });
        let resource_type = clean_identifier(&resource.resource_type).unwrap_or_else(|| {
            safe_id(&resource.resource_type).unwrap_or_else(|| "AWS::Resource".to_string())
        });
        let service = clean_identifier(&resource.service)
            .or_else(|| safe_id(&resource.service))
            .unwrap_or_else(|| aws_service_from_type(&resource_type));
        builder.add_node(InfraNode {
            id: id.clone(),
            label: resource.label,
            provider: "aws".to_string(),
            service,
            resource_type,
            region: resource.region,
            zone: resource.zone,
            tag_count: resource.tag_count,
            source: "aws-config".to_string(),
        });
        for relationship in resource.relationships {
            let target =
                clean_identifier(&relationship.target).or_else(|| safe_id(&relationship.target));
            let relation = clean_identifier(&relationship.relation)
                .or_else(|| safe_id(&relationship.relation));
            match (target, relation) {
                (Some(target), Some(relation)) => builder.add_edge(target, id.clone(), relation),
                (None, _) => warnings.push(format!(
                    "ignored invalid AWS Config relationship target `{}`",
                    relationship.target
                )),
                (_, None) => warnings.push(format!(
                    "ignored invalid AWS Config relationship name `{}`",
                    relationship.relation
                )),
            }
        }
    }
    Ok(())
}

fn parse_gcp_cloud_assets(
    assets: Vec<Value>,
    builder: &mut GraphBuilder,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    if assets.is_empty() {
        return Err("GCP Cloud Asset source requires at least one asset".to_string());
    }
    if assets.len() > MAX_INVENTORY_RESOURCES {
        return Err(format!(
            "GCP Cloud Asset resources exceeds max {MAX_INVENTORY_RESOURCES}"
        ));
    }
    let estimated_bytes = Value::Array(assets.clone()).to_string().len();
    if estimated_bytes > MAX_IMPORT_JSON_BYTES {
        return Err(format!(
            "GCP Cloud Asset JSON exceeds max {MAX_IMPORT_JSON_BYTES} bytes"
        ));
    }

    for asset in assets {
        let raw_name = value_string_any(&asset, &["name", "Name"])
            .unwrap_or_else(|| format!("gcp-asset-{}", builder.nodes.len()));
        let id = clean_identifier(&raw_name).unwrap_or_else(|| {
            safe_id(&raw_name).unwrap_or_else(|| format!("gcp-asset-{}", builder.nodes.len()))
        });
        let asset_type = value_string_any(&asset, &["assetType", "AssetType", "type"])
            .unwrap_or_else(|| "gcp.resource".to_string());
        let resource_type = safe_id(&asset_type).unwrap_or_else(|| "gcp-resource".to_string());
        let service = gcp_service_from_asset_type(&asset_type);
        let label = value_string_any(&asset, &["displayName", "name"])
            .map(|value| gcp_label(&value))
            .unwrap_or_else(|| gcp_label(&raw_name));
        let tag_count = asset
            .pointer("/resource/data/labels")
            .and_then(Value::as_object)
            .map(serde_json::Map::len)
            .unwrap_or(0);
        builder.add_node(InfraNode {
            id: id.clone(),
            label,
            provider: "gcp".to_string(),
            service,
            resource_type,
            region: value_string_path(&asset, "/resource/location")
                .or_else(|| value_string_path(&asset, "/resource/data/region")),
            zone: value_string_path(&asset, "/resource/data/zone"),
            tag_count,
            source: "gcp-cloud-asset".to_string(),
        });
        for ancestor in string_array(asset.get("ancestors")).into_iter().take(1) {
            let ancestor_id = safe_id(&ancestor).unwrap_or_else(|| gcp_label(&ancestor));
            builder.add_node(InfraNode {
                id: ancestor_id.clone(),
                label: gcp_label(&ancestor),
                provider: "gcp".to_string(),
                service: "resource-manager".to_string(),
                resource_type: "ancestor".to_string(),
                region: None,
                zone: None,
                tag_count: 0,
                source: "gcp-cloud-asset-ancestor".to_string(),
            });
            builder.add_edge(ancestor_id, id.clone(), "gcp-ancestor");
        }
        for reference in gcp_references(&asset) {
            if let Some(reference) = clean_identifier(&reference).or_else(|| safe_id(&reference)) {
                builder.add_edge(reference, id.clone(), "gcp-reference");
            } else {
                warnings.push(format!("ignored invalid GCP reference `{reference}`"));
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
        mermaid_c4: mermaid_c4(title, graph),
        mermaid_mindmap: mermaid_mindmap(title, graph),
        graphviz_dot: graphviz_dot(title, graph),
        plantuml: plantuml(title, graph),
        d2: d2(title, graph),
        structurizr_dsl: structurizr_dsl(title, graph),
        nomnoml: nomnoml(title, graph),
        blockdiag: blockdiag(title, graph),
        cytoscape: cytoscape(graph),
        drawio_mxfile: drawio_mxfile(title, graph),
        excalidraw: excalidraw(graph),
        vega_force: vega_force(graph),
        networkx_json: networkx_json(title, graph),
        json_graph: json_graph(title, graph),
        vis_network: vis_network(graph),
        sigma_graph: sigma_graph(graph),
        echarts_graph: echarts_graph(title, graph),
        elk_graph: elk_graph(graph),
        deckgl_layers: deckgl_layers(graph),
        gexf: gexf(title, graph),
        markmap_markdown: markmap_markdown(title, graph),
        markdown_inventory: markdown_inventory(title, graph),
        topology_json: topology_json(graph),
        kroki_manifest: kroki_manifest(title, graph),
        renderer_catalog: renderer_names().to_vec(),
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

fn mermaid_c4(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!(
        "C4Context\n  title {}\n",
        title.replace('"', "'").replace('\n', " ")
    );
    for node in &graph.nodes {
        output.push_str("  System(");
        output.push_str(&mermaid_id(&node.id));
        output.push_str(", \"");
        output.push_str(&node.label.replace('"', "'"));
        output.push_str("\", \"");
        output.push_str(&format!("{} / {}", node.provider, node.resource_type).replace('"', "'"));
        output.push_str("\")\n");
    }
    for edge in &graph.edges {
        output.push_str("  Rel(");
        output.push_str(&mermaid_id(&edge.from));
        output.push_str(", ");
        output.push_str(&mermaid_id(&edge.to));
        output.push_str(", \"");
        output.push_str(&edge.relation.replace('"', "'"));
        output.push_str("\")\n");
    }
    output
}

fn mermaid_mindmap(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("mindmap\n  root(({}))\n", title.replace('\n', " "));
    let mut by_group = BTreeMap::<(&str, &str), Vec<&InfraNode>>::new();
    for node in &graph.nodes {
        by_group
            .entry((&node.provider, &node.service))
            .or_default()
            .push(node);
    }
    for ((provider, service), nodes) in by_group {
        output.push_str(&format!("    {} {}\n", provider, service));
        for node in nodes {
            output.push_str(&format!(
                "      {} [{}]\n",
                node.label.replace('\n', " "),
                node.resource_type
            ));
        }
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

fn nomnoml(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("#title: {}\n#direction: right\n", title.replace('\n', " "));
    for node in &graph.nodes {
        output.push_str("[");
        output.push_str(&mermaid_id(&node.id));
        output.push_str("|");
        output.push_str(
            &node
                .label
                .replace('[', " ")
                .replace(']', " ")
                .replace('|', " "),
        );
        output.push_str("|");
        output.push_str(
            &node
                .resource_type
                .replace('[', " ")
                .replace(']', " ")
                .replace('|', " "),
        );
        output.push_str("]\n");
    }
    for edge in &graph.edges {
        output.push_str("[");
        output.push_str(&mermaid_id(&edge.from));
        output.push_str("] -> [");
        output.push_str(&mermaid_id(&edge.to));
        output.push_str("]\n");
    }
    output
}

fn blockdiag(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("blockdiag {{\n  orientation = landscape;\n  // {}\n", title);
    for node in &graph.nodes {
        output.push_str("  ");
        output.push_str(&mermaid_id(&node.id));
        output.push_str(" [label = \"");
        output.push_str(&node.label.replace('"', "'"));
        output.push_str("\\n");
        output.push_str(&node.resource_type.replace('"', "'"));
        output.push_str("\"];\n");
    }
    for edge in &graph.edges {
        output.push_str("  ");
        output.push_str(&mermaid_id(&edge.from));
        output.push_str(" -> ");
        output.push_str(&mermaid_id(&edge.to));
        output.push_str(" [label = \"");
        output.push_str(&edge.relation.replace('"', "'"));
        output.push_str("\"];\n");
    }
    output.push_str("}\n");
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

fn vega_force(graph: &InfraGraph) -> Value {
    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "provider": node.provider,
                "service": node.service,
                "resourceType": node.resource_type,
                "region": node.region,
                "zone": node.zone,
                "tagCount": node.tag_count
            })
        })
        .collect::<Vec<_>>();
    let links = graph
        .edges
        .iter()
        .map(|edge| {
            json!({
                "source": edge.from,
                "target": edge.to,
                "relation": edge.relation
            })
        })
        .collect::<Vec<_>>();
    json!({
        "$schema": "https://vega.github.io/schema/vega/v5.json",
        "description": "Force-directed infrastructure dependency graph.",
        "width": 960,
        "height": 640,
        "padding": 8,
        "autosize": "fit",
        "data": [
            { "name": "node-data", "values": nodes },
            { "name": "link-data", "values": links }
        ],
        "signals": [
            { "name": "cx", "update": "width / 2" },
            { "name": "cy", "update": "height / 2" },
            { "name": "nodeRadius", "value": 8 },
            { "name": "nodeCharge", "value": -45 },
            { "name": "linkDistance", "value": 45 }
        ],
        "scales": [
            {
                "name": "providerColor",
                "type": "ordinal",
                "domain": { "data": "node-data", "field": "provider" },
                "range": "category"
            }
        ],
        "marks": [
            {
                "name": "nodes",
                "type": "symbol",
                "from": { "data": "node-data" },
                "encode": {
                    "enter": {
                        "fill": { "scale": "providerColor", "field": "provider" },
                        "size": { "value": 160 },
                        "tooltip": {
                            "signal": "datum.label + ' (' + datum.resourceType + ')'"
                        }
                    }
                },
                "transform": [
                    {
                        "type": "force",
                        "iterations": 180,
                        "static": false,
                        "forces": [
                            { "force": "center", "x": { "signal": "cx" }, "y": { "signal": "cy" } },
                            { "force": "collide", "radius": { "signal": "nodeRadius" } },
                            { "force": "nbody", "strength": { "signal": "nodeCharge" } },
                            { "force": "link", "links": "link-data", "distance": { "signal": "linkDistance" }, "id": "id" }
                        ]
                    }
                ]
            },
            {
                "type": "path",
                "from": { "data": "link-data" },
                "interactive": false,
                "encode": {
                    "update": {
                        "stroke": { "value": "#777" },
                        "strokeOpacity": { "value": 0.55 }
                    }
                },
                "transform": [
                    {
                        "type": "linkpath",
                        "shape": "line",
                        "sourceX": "datum.source.x",
                        "sourceY": "datum.source.y",
                        "targetX": "datum.target.x",
                        "targetY": "datum.target.y"
                    }
                ]
            }
        ]
    })
}

fn networkx_json(title: &str, graph: &InfraGraph) -> Value {
    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "provider": node.provider,
                "service": node.service,
                "resourceType": node.resource_type,
                "region": node.region,
                "zone": node.zone,
                "tagCount": node.tag_count,
                "source": node.source
            })
        })
        .collect::<Vec<_>>();
    let links = graph
        .edges
        .iter()
        .map(|edge| {
            json!({
                "source": edge.from,
                "target": edge.to,
                "relation": edge.relation
            })
        })
        .collect::<Vec<_>>();
    json!({
        "directed": true,
        "multigraph": false,
        "graph": { "name": title },
        "nodes": nodes,
        "links": links
    })
}

fn json_graph(title: &str, graph: &InfraGraph) -> Value {
    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "metadata": {
                    "provider": node.provider,
                    "service": node.service,
                    "resourceType": node.resource_type,
                    "region": node.region,
                    "zone": node.zone,
                    "tagCount": node.tag_count,
                    "source": node.source
                }
            })
        })
        .collect::<Vec<_>>();
    let edges = graph
        .edges
        .iter()
        .map(|edge| {
            json!({
                "source": edge.from,
                "target": edge.to,
                "relation": edge.relation
            })
        })
        .collect::<Vec<_>>();
    json!({
        "graph": {
            "id": safe_id(title).unwrap_or_else(|| "infrastructure-diagram".to_string()),
            "label": title,
            "directed": true,
            "type": "infrastructure-topology",
            "nodes": nodes,
            "edges": edges
        }
    })
}

fn vis_network(graph: &InfraGraph) -> Value {
    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "group": format!("{}:{}", node.provider, node.service),
                "title": format!("{} / {}", node.provider, node.resource_type),
                "value": node.tag_count.max(1)
            })
        })
        .collect::<Vec<_>>();
    let edges = graph
        .edges
        .iter()
        .map(|edge| {
            json!({
                "from": edge.from,
                "to": edge.to,
                "label": edge.relation,
                "arrows": "to"
            })
        })
        .collect::<Vec<_>>();
    json!({
        "nodes": nodes,
        "edges": edges,
        "options": {
            "layout": { "improvedLayout": true },
            "physics": {
                "solver": "forceAtlas2Based",
                "stabilization": { "iterations": 160 }
            },
            "interaction": { "hover": true, "navigationButtons": true }
        }
    })
}

fn sigma_graph(graph: &InfraGraph) -> Value {
    let positions = node_positions(graph);
    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            let position = positions.get(&node.id).copied().unwrap_or([0.0, 0.0, 0.0]);
            json!({
                "key": node.id,
                "label": node.label,
                "x": position[0],
                "y": position[1],
                "size": 4 + node.tag_count.min(12),
                "color": provider_color(&node.provider),
                "attributes": {
                    "provider": node.provider,
                    "service": node.service,
                    "resourceType": node.resource_type,
                    "region": node.region,
                    "zone": node.zone
                }
            })
        })
        .collect::<Vec<_>>();
    let edges = graph
        .edges
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            json!({
                "key": format!("edge-{index}"),
                "source": edge.from,
                "target": edge.to,
                "label": edge.relation,
                "type": "arrow"
            })
        })
        .collect::<Vec<_>>();
    json!({ "nodes": nodes, "edges": edges })
}

fn echarts_graph(title: &str, graph: &InfraGraph) -> Value {
    let categories = graph
        .groups
        .iter()
        .map(|group| {
            json!({
                "name": group.id,
                "provider": group.provider,
                "service": group.service
            })
        })
        .collect::<Vec<_>>();
    let nodes = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "name": node.label,
                "value": node.tag_count,
                "category": format!("{}:{}", node.provider, node.service),
                "symbolSize": 14 + node.tag_count.min(18),
                "resourceType": node.resource_type,
                "region": node.region,
                "zone": node.zone
            })
        })
        .collect::<Vec<_>>();
    let links = graph
        .edges
        .iter()
        .map(|edge| {
            json!({
                "source": edge.from,
                "target": edge.to,
                "name": edge.relation
            })
        })
        .collect::<Vec<_>>();
    json!({
        "title": { "text": title },
        "tooltip": {},
        "legend": [{ "data": graph.groups.iter().map(|group| group.id.clone()).collect::<Vec<_>>() }],
        "series": [{
            "type": "graph",
            "layout": "force",
            "roam": true,
            "draggable": true,
            "categories": categories,
            "data": nodes,
            "links": links,
            "label": { "show": true, "position": "right" },
            "force": { "repulsion": 180, "edgeLength": 90 }
        }]
    })
}

fn elk_graph(graph: &InfraGraph) -> Value {
    let children = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "width": 180,
                "height": 72,
                "labels": [{ "text": format!("{}\\n{}", node.label, node.resource_type) }],
                "properties": {
                    "provider": node.provider,
                    "service": node.service,
                    "resourceType": node.resource_type
                }
            })
        })
        .collect::<Vec<_>>();
    let edges = graph
        .edges
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            json!({
                "id": format!("edge-{index}"),
                "sources": [edge.from],
                "targets": [edge.to],
                "labels": [{ "text": edge.relation }]
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": "root",
        "layoutOptions": {
            "elk.algorithm": "layered",
            "elk.direction": "RIGHT",
            "elk.spacing.nodeNode": "48"
        },
        "children": children,
        "edges": edges
    })
}

fn deckgl_layers(graph: &InfraGraph) -> Value {
    let positions = node_positions(graph);
    let points = graph
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "provider": node.provider,
                "service": node.service,
                "resourceType": node.resource_type,
                "position": positions.get(&node.id).copied().unwrap_or([0.0, 0.0, 0.0]),
                "color": provider_rgb(&node.provider)
            })
        })
        .collect::<Vec<_>>();
    let arcs = graph
        .edges
        .iter()
        .filter_map(|edge| {
            let source = positions.get(&edge.from)?;
            let target = positions.get(&edge.to)?;
            Some(json!({
                "sourceId": edge.from,
                "targetId": edge.to,
                "relation": edge.relation,
                "sourcePosition": source,
                "targetPosition": target
            }))
        })
        .collect::<Vec<_>>();
    json!({
        "coordinateSystem": "synthetic-grid",
        "layers": [
            {
                "id": "infra-resource-points",
                "type": "ScatterplotLayer",
                "data": points,
                "getPosition": "position",
                "getFillColor": "color",
                "getRadius": 30000,
                "pickable": true
            },
            {
                "id": "infra-resource-arcs",
                "type": "ArcLayer",
                "data": arcs,
                "getSourcePosition": "sourcePosition",
                "getTargetPosition": "targetPosition",
                "getSourceColor": [80, 80, 80],
                "getTargetColor": [20, 20, 20],
                "pickable": true
            }
        ]
    })
}

fn topology_json(graph: &InfraGraph) -> Value {
    let adjacency = graph
        .nodes
        .iter()
        .map(|node| {
            let outgoing = graph
                .edges
                .iter()
                .filter(|edge| edge.from == node.id)
                .map(|edge| json!({ "to": edge.to, "relation": edge.relation }))
                .collect::<Vec<_>>();
            let incoming = graph
                .edges
                .iter()
                .filter(|edge| edge.to == node.id)
                .map(|edge| json!({ "from": edge.from, "relation": edge.relation }))
                .collect::<Vec<_>>();
            json!({
                "id": node.id,
                "incoming": incoming,
                "outgoing": outgoing
            })
        })
        .collect::<Vec<_>>();
    json!({
        "summary": {
            "nodes": graph.nodes.len(),
            "edges": graph.edges.len(),
            "groups": graph.groups.len()
        },
        "groups": graph.groups,
        "nodes": graph.nodes,
        "edges": graph.edges,
        "adjacency": adjacency
    })
}

fn kroki_manifest(title: &str, graph: &InfraGraph) -> Value {
    json!({
        "title": title,
        "service": "kroki",
        "diagrams": [
            { "type": "mermaid", "outputField": "renderers.mermaid" },
            { "type": "graphviz", "outputField": "renderers.graphvizDot" },
            { "type": "plantuml", "outputField": "renderers.plantuml" },
            { "type": "d2", "outputField": "renderers.d2" },
            { "type": "nomnoml", "outputField": "renderers.nomnoml" },
            { "type": "blockdiag", "outputField": "renderers.blockdiag" }
        ],
        "graphSize": {
            "nodes": graph.nodes.len(),
            "edges": graph.edges.len()
        }
    })
}

fn gexf(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><gexf xmlns="http://www.gexf.net/1.3" version="1.3"><meta><creator>dd-data-viz-rs</creator><description>{}</description></meta><graph mode="static" defaultedgetype="directed"><attributes class="node"><attribute id="provider" title="provider" type="string"/><attribute id="service" title="service" type="string"/><attribute id="resourceType" title="resourceType" type="string"/><attribute id="tagCount" title="tagCount" type="integer"/></attributes><nodes>"#,
        xml_escape(title)
    );
    for node in &graph.nodes {
        output.push_str(&format!(
            r#"<node id="{}" label="{}"><attvalues><attvalue for="provider" value="{}"/><attvalue for="service" value="{}"/><attvalue for="resourceType" value="{}"/><attvalue for="tagCount" value="{}"/></attvalues></node>"#,
            xml_escape(&node.id),
            xml_escape(&node.label),
            xml_escape(&node.provider),
            xml_escape(&node.service),
            xml_escape(&node.resource_type),
            node.tag_count
        ));
    }
    output.push_str("</nodes><edges>");
    for (index, edge) in graph.edges.iter().enumerate() {
        output.push_str(&format!(
            r#"<edge id="edge-{}" source="{}" target="{}" label="{}"/>"#,
            index,
            xml_escape(&edge.from),
            xml_escape(&edge.to),
            xml_escape(&edge.relation)
        ));
    }
    output.push_str("</edges></graph></gexf>");
    output
}

fn markmap_markdown(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("# {}\n", markdown_cell(title));
    let mut by_group = BTreeMap::<(&str, &str), Vec<&InfraNode>>::new();
    for node in &graph.nodes {
        by_group
            .entry((&node.provider, &node.service))
            .or_default()
            .push(node);
    }
    for ((provider, service), nodes) in by_group {
        output.push_str(&format!(
            "## {} / {}\n",
            markdown_cell(provider),
            markdown_cell(service)
        ));
        for node in nodes {
            output.push_str(&format!(
                "- {} `{}`\n  - type: `{}`\n  - source: `{}`\n",
                markdown_cell(&node.label),
                node.id,
                node.resource_type,
                node.source
            ));
        }
    }
    if !graph.edges.is_empty() {
        output.push_str("## Relationships\n");
        for edge in &graph.edges {
            output.push_str(&format!(
                "- `{}` -> `{}`: {}\n",
                edge.from,
                edge.to,
                markdown_cell(&edge.relation)
            ));
        }
    }
    output
}

fn markdown_inventory(title: &str, graph: &InfraGraph) -> String {
    let mut output = format!("# {}\n\n", markdown_cell(title));
    output.push_str("| ID | Label | Provider | Service | Type | Region | Zone | Tags | Source |\n");
    output.push_str("| --- | --- | --- | --- | --- | --- | --- | ---: | --- |\n");
    for node in &graph.nodes {
        output.push_str(&format!(
            "| `{}` | {} | {} | {} | `{}` | {} | {} | {} | `{}` |\n",
            markdown_cell(&node.id),
            markdown_cell(&node.label),
            markdown_cell(&node.provider),
            markdown_cell(&node.service),
            markdown_cell(&node.resource_type),
            markdown_cell(node.region.as_deref().unwrap_or("")),
            markdown_cell(node.zone.as_deref().unwrap_or("")),
            node.tag_count,
            markdown_cell(&node.source)
        ));
    }
    if !graph.edges.is_empty() {
        output.push_str("\n| From | To | Relation |\n");
        output.push_str("| --- | --- | --- |\n");
        for edge in &graph.edges {
            output.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                markdown_cell(&edge.from),
                markdown_cell(&edge.to),
                markdown_cell(&edge.relation)
            ));
        }
    }
    output
}

fn node_positions(graph: &InfraGraph) -> BTreeMap<String, [f64; 3]> {
    graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let column = (index % 12) as f64;
            let row = (index / 12) as f64;
            let altitude = (node.tag_count.min(10) as f64) * 0.1;
            (
                node.id.clone(),
                [-122.0 + column * 0.6, 37.0 - row * 0.45, altitude],
            )
        })
        .collect()
}

fn provider_color(provider: &str) -> &'static str {
    match provider {
        "aws" => "#ff9900",
        "gcp" => "#4285f4",
        "azure" => "#0078d4",
        "kubernetes" => "#326ce5",
        "terraform" => "#7b42bc",
        _ => "#546e7a",
    }
}

fn provider_rgb(provider: &str) -> [u8; 3] {
    match provider {
        "aws" => [255, 153, 0],
        "gcp" => [66, 133, 244],
        "azure" => [0, 120, 212],
        "kubernetes" => [50, 108, 229],
        "terraform" => [123, 66, 188],
        _ => [84, 110, 122],
    }
}

fn value_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    scalar_string(current)
}

fn value_string_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(scalar_string))
}

fn value_string_path(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(scalar_string)
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values.iter().filter_map(scalar_string).collect(),
        Some(value) => scalar_string(value).into_iter().collect(),
        None => Vec::new(),
    }
}

fn aws_type_from_arn(arn: &str) -> String {
    let service = arn.split(':').nth(2).unwrap_or("resource");
    let resource = arn.split(':').skip(5).collect::<Vec<_>>().join(":");
    let resource_type = resource
        .split(['/', ':'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("resource");
    safe_id(&format!("{service}:{resource_type}")).unwrap_or_else(|| "aws_resource".to_string())
}

fn aws_service_from_type(resource_type: &str) -> String {
    if resource_type.contains(':') {
        if let Some(service) = resource_type
            .split(':')
            .next()
            .filter(|value| !value.is_empty())
        {
            return safe_id(service).unwrap_or_else(|| "aws".to_string());
        }
    }
    service_for_type(resource_type).to_string()
}

fn arn_tail(arn: &str) -> String {
    arn.rsplit(['/', ':'])
        .find(|part| !part.trim().is_empty())
        .map(|part| part.trim().to_string())
        .unwrap_or_else(|| arn.to_string())
}

fn name_tag(value: &Value) -> Option<String> {
    tag_value_by_key(value.get("Tags").or_else(|| value.get("tags")), "Name").or_else(|| {
        tags_from_properties(value).and_then(|tags| tag_value_by_key(Some(tags), "Name"))
    })
}

fn tag_count(value: &Value) -> usize {
    value
        .get("Tags")
        .or_else(|| value.get("tags"))
        .map(tag_count_value)
        .filter(|count| *count > 0)
        .or_else(|| tags_from_properties(value).map(tag_count_value))
        .unwrap_or(0)
}

fn tags_from_properties(value: &Value) -> Option<&Value> {
    value
        .get("Properties")
        .or_else(|| value.get("properties"))
        .and_then(Value::as_array)?
        .iter()
        .find(|property| {
            value_string_any(property, &["Name", "name"])
                .map(|name| name.eq_ignore_ascii_case("tags"))
                .unwrap_or(false)
        })
        .and_then(|property| property.get("Data").or_else(|| property.get("data")))
}

fn tag_count_value(value: &Value) -> usize {
    match value {
        Value::Object(tags) => tags.len(),
        Value::Array(tags) => tags.len(),
        _ => 0,
    }
}

fn tag_value_by_key(value: Option<&Value>, key: &str) -> Option<String> {
    match value? {
        Value::Object(tags) => tags.get(key).and_then(scalar_string),
        Value::Array(tags) => tags.iter().find_map(|tag| {
            let tag_key = value_string_any(tag, &["Key", "key"]);
            if tag_key
                .as_deref()
                .map(|candidate| candidate.eq_ignore_ascii_case(key))
                .unwrap_or(false)
            {
                value_string_any(tag, &["Value", "value"])
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn relationship_targets(value: &Value) -> Vec<String> {
    let mut targets = BTreeSet::new();
    for key in [
        "References",
        "references",
        "Relationships",
        "relationships",
        "RelatedResources",
        "relatedResources",
        "ParentArn",
        "parentArn",
    ] {
        collect_reference_values(value.get(key), &mut targets);
    }
    targets.into_iter().collect()
}

fn collect_reference_values(value: Option<&Value>, targets: &mut BTreeSet<String>) {
    match value {
        Some(Value::String(value)) => {
            targets.insert(value.clone());
        }
        Some(Value::Array(values)) => {
            for value in values {
                collect_reference_values(Some(value), targets);
            }
        }
        Some(Value::Object(_)) => {
            for key in [
                "Arn",
                "arn",
                "ResourceArn",
                "resourceArn",
                "TargetArn",
                "targetArn",
                "Id",
                "id",
                "ResourceId",
                "resourceId",
            ] {
                if let Some(value) = value
                    .and_then(|value| value.get(key))
                    .and_then(scalar_string)
                {
                    targets.insert(value);
                }
            }
        }
        _ => {}
    }
}

fn gcp_service_from_asset_type(asset_type: &str) -> String {
    asset_type
        .split('/')
        .next()
        .unwrap_or(asset_type)
        .trim_end_matches(".googleapis.com")
        .split('.')
        .next()
        .and_then(safe_id)
        .unwrap_or_else(|| "gcp".to_string())
}

fn gcp_label(value: &str) -> String {
    value
        .rsplit(['/', ':'])
        .find(|part| !part.trim().is_empty())
        .map(|part| part.trim().to_string())
        .unwrap_or_else(|| value.to_string())
}

fn gcp_references(value: &Value) -> Vec<String> {
    let mut references = BTreeSet::new();
    collect_named_strings(
        value
            .get("resource")
            .and_then(|resource| resource.get("data")),
        &[
            "network",
            "subnetwork",
            "subnet",
            "kmsKeyName",
            "serviceAccount",
            "target",
            "backendService",
            "instance",
        ],
        &mut references,
    );
    references.into_iter().collect()
}

fn collect_named_strings(value: Option<&Value>, keys: &[&str], output: &mut BTreeSet<String>) {
    match value {
        Some(Value::Object(map)) => {
            for (key, value) in map {
                if keys.iter().any(|candidate| key == candidate) {
                    if let Some(value) = scalar_string(value) {
                        output.insert(value);
                    }
                }
                collect_named_strings(Some(value), keys, output);
            }
        }
        Some(Value::Array(values)) => {
            for value in values {
                collect_named_strings(Some(value), keys, output);
            }
        }
        _ => {}
    }
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace('\n', " ")
        .trim()
        .to_string()
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
    let mut cleaned = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.len() > 128 {
        let checksum = input.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
            hash.wrapping_mul(0x100000001b3) ^ u64::from(byte)
        });
        cleaned = format!("{}-{checksum:016x}", &cleaned[..111]);
    }
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
        assert!(response.renderers.mermaid_c4.contains("C4Context"));
        assert!(response.renderers.graphviz_dot.contains("digraph"));
        assert!(response.renderers.renderer_catalog.contains(&"plantuml"));
        assert!(response
            .renderers
            .renderer_catalog
            .contains(&"sigma-graph-json"));
        assert!(response.renderers.nomnoml.contains("#direction: right"));
        assert_eq!(response.renderers.json_graph["graph"]["directed"], true);
    }

    #[test]
    fn diagram_tool_catalog_lists_many_interop_targets() {
        let catalog = tool_catalog_payload();
        assert!(renderer_names().len() >= 20);
        assert!(source_names().contains(&"terraform-state"));
        assert!(source_names().contains(&"aws-resource-explorer"));
        assert!(source_names().contains(&"aws-config"));
        assert!(catalog["renderers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|renderer| renderer == "deckgl-layers-json"));
        assert!(catalog["toolFamilies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|family| family["family"] == "interactive-web-graphs"));
    }

    #[test]
    fn terraform_plan_diagram_uses_depends_on() {
        let response = generate(InfraDiagramRequest {
            title: Some("Plan Network".to_string()),
            source: InfraSource::TerraformPlan {
                plan: serde_json::json!({
                    "planned_values": {
                        "root_module": {
                            "resources": [
                                {
                                    "address": "aws_vpc.main",
                                    "type": "aws_vpc",
                                    "name": "main",
                                    "values": {
                                        "tags": { "Name": "main" }
                                    }
                                },
                                {
                                    "address": "aws_subnet.public",
                                    "type": "aws_subnet",
                                    "name": "public",
                                    "depends_on": ["aws_vpc.main"],
                                    "values": {
                                        "region": "us-east-1"
                                    }
                                }
                            ]
                        }
                    }
                }),
            },
            max_nodes: None,
        })
        .expect("terraform plan diagram");

        assert_eq!(response.node_count, 2);
        assert!(response.graph.edges.iter().any(|edge| {
            edge.from == "aws_vpc.main"
                && edge.to == "aws_subnet.public"
                && edge.relation == "terraform-depends-on"
        }));
        assert!(response.renderers.renderer_catalog.contains(&"gexf"));
        assert_eq!(
            response.renderers.vega_force["data"][0]["name"],
            "node-data"
        );
    }

    #[test]
    fn terraform_state_diagram_uses_values_root_module() {
        let response = generate(InfraDiagramRequest {
            title: Some("State Network".to_string()),
            source: InfraSource::TerraformState {
                state: serde_json::json!({
                    "values": {
                        "root_module": {
                            "resources": [
                                {
                                    "address": "google_compute_network.shared",
                                    "type": "google_compute_network",
                                    "name": "shared",
                                    "values": {
                                        "name": "shared-vpc"
                                    }
                                },
                                {
                                    "address": "google_compute_subnetwork.app",
                                    "type": "google_compute_subnetwork",
                                    "name": "app",
                                    "depends_on": ["google_compute_network.shared"],
                                    "values": {
                                        "region": "us-central1"
                                    }
                                }
                            ]
                        }
                    }
                }),
            },
            max_nodes: None,
        })
        .expect("terraform state diagram");

        assert_eq!(response.source_kind, "terraform-state");
        assert_eq!(response.node_count, 2);
        assert!(response
            .graph
            .nodes
            .iter()
            .all(|node| node.source.starts_with("terraform-state:")));
        assert!(response.graph.edges.iter().any(|edge| {
            edge.from == "google_compute_network.shared"
                && edge.to == "google_compute_subnetwork.app"
                && edge.relation == "terraform-depends-on"
        }));
    }

    #[test]
    fn aws_resource_explorer_diagram_extracts_arn_nodes() {
        let response = generate(InfraDiagramRequest {
            title: Some("AWS Inventory".to_string()),
            source: InfraSource::AwsResourceExplorer {
                resources: vec![serde_json::json!({
                    "Arn": "arn:aws:ec2:us-east-1:123456789012:vpc/vpc-123",
                    "ResourceType": "ec2:vpc",
                    "Service": "ec2",
                    "Region": "us-east-1",
                    "Tags": [
                        { "Key": "Name", "Value": "Core VPC" },
                        { "Key": "env", "Value": "dev" }
                    ]
                })],
            },
            max_nodes: None,
        })
        .expect("aws resource explorer diagram");

        assert_eq!(response.node_count, 1);
        assert_eq!(response.graph.nodes[0].provider, "aws");
        assert_eq!(response.graph.nodes[0].service, "ec2");
        assert_eq!(response.graph.nodes[0].tag_count, 2);
        assert!(response.renderers.markdown_inventory.contains("Core VPC"));
    }

    #[test]
    fn aws_config_diagram_extracts_relationship_edges() {
        let response = generate(InfraDiagramRequest {
            title: Some("AWS Config".to_string()),
            source: InfraSource::AwsConfig {
                configuration_items: vec![
                    serde_json::json!({
                        "resourceType": "AWS::EC2::VPC",
                        "resourceId": "vpc-123",
                        "resourceName": "core",
                        "awsRegion": "us-east-1"
                    }),
                    serde_json::json!({
                        "resourceType": "AWS::EC2::Subnet",
                        "resourceId": "subnet-123",
                        "resourceName": "public",
                        "awsRegion": "us-east-1",
                        "relationships": [
                            {
                                "resourceType": "AWS::EC2::VPC",
                                "resourceId": "vpc-123",
                                "relationshipName": "Is contained in Vpc"
                            }
                        ],
                        "configuration": "{\"vpcId\":\"vpc-123\"}"
                    }),
                ],
            },
            max_nodes: None,
        })
        .expect("aws config diagram");

        assert_eq!(response.source_kind, "aws-config");
        assert_eq!(response.node_count, 2);
        assert!(response
            .graph
            .nodes
            .iter()
            .any(|node| node.id == "subnet-123" && node.service == "ec2"));
        assert!(response.graph.edges.iter().any(|edge| {
            edge.from == "vpc-123"
                && edge.to == "subnet-123"
                && (edge.relation == "Is-contained-in-Vpc" || edge.relation == "aws-config:vpcId")
        }));
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

    #[test]
    fn gcp_cloud_asset_diagram_extracts_assets() {
        let response = generate(InfraDiagramRequest {
            title: Some("GCP Assets".to_string()),
            source: InfraSource::GcpCloudAsset {
                assets: vec![serde_json::json!({
                    "name": "//compute.googleapis.com/projects/demo/global/networks/default",
                    "assetType": "compute.googleapis.com/Network",
                    "resource": {
                        "location": "global",
                        "data": {
                            "labels": {
                                "env": "dev"
                            }
                        }
                    },
                    "ancestors": ["projects/123456789"]
                })],
            },
            max_nodes: None,
        })
        .expect("gcp cloud asset diagram");

        assert_eq!(response.node_count, 2);
        assert!(response.graph.nodes.iter().any(|node| {
            node.provider == "gcp" && node.service == "compute" && node.label == "default"
        }));
        assert!(response
            .graph
            .edges
            .iter()
            .any(|edge| edge.relation == "gcp-ancestor"));
        assert!(response.renderers.networkx_json["directed"]
            .as_bool()
            .unwrap());
    }
}
