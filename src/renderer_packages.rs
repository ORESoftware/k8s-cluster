use serde::Serialize;
use serde_json::{json, Value};

const PACKAGE_NAME: &str = "@dd-data-viz/renderers";
const PACKAGE_VERSION: &str = "0.1.0";
const SCHEMA_VERSION: &str = "data-viz.renderer-client-package.v1";
const MAX_PACKAGE_FILES: usize = 8;
const MAX_FILE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererClientPackageResponse {
    ok: bool,
    schema_version: &'static str,
    package_name: &'static str,
    version: &'static str,
    module_format: &'static str,
    targets: Vec<RendererClientTarget>,
    files: Vec<RendererClientPackageFile>,
    integrity: RendererClientPackageIntegrity,
    limits: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererClientTarget {
    id: &'static str,
    analog: &'static str,
    entrypoint: &'static str,
    supports: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererClientPackageFile {
    path: &'static str,
    kind: &'static str,
    bytes: usize,
    checksum: String,
    content: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererClientPackageIntegrity {
    file_count: usize,
    total_bytes: usize,
    package_checksum: String,
}

pub(crate) fn client_package() -> RendererClientPackageResponse {
    let files = package_files();
    let total_bytes = files.iter().map(|file| file.bytes).sum::<usize>();
    let mut package_input = String::new();
    for file in &files {
        package_input.push_str(file.path);
        package_input.push('\n');
        package_input.push_str(file.content);
        package_input.push('\n');
    }
    RendererClientPackageResponse {
        ok: true,
        schema_version: SCHEMA_VERSION,
        package_name: PACKAGE_NAME,
        version: PACKAGE_VERSION,
        module_format: "typescript-esm",
        targets: targets(),
        integrity: RendererClientPackageIntegrity {
            file_count: files.len(),
            total_bytes,
            package_checksum: stable_checksum(&package_input),
        },
        files,
        limits: limits_payload(),
    }
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxPackageFiles": MAX_PACKAGE_FILES,
        "maxFileBytes": MAX_FILE_BYTES,
        "posture": "static generated TypeScript package blueprint; no user source is executed or bundled"
    })
}

pub(crate) fn summary_payload() -> Value {
    json!({
        "packageName": PACKAGE_NAME,
        "version": PACKAGE_VERSION,
        "moduleFormat": "typescript-esm",
        "targets": targets(),
        "limits": limits_payload()
    })
}

fn targets() -> Vec<RendererClientTarget> {
    vec![
        RendererClientTarget {
            id: "d3-final-layer",
            analog: "D3.js",
            entrypoint: "src/d3.ts",
            supports: vec![
                "final layer validation",
                "DOM mount contract",
                "encoding extraction",
            ],
        },
        RendererClientTarget {
            id: "plotly-traces",
            analog: "Plotly / Dash",
            entrypoint: "src/plotly.ts",
            supports: vec![
                "figure blueprint",
                "trace type mapping",
                "Dash callback payloads",
            ],
        },
        RendererClientTarget {
            id: "evidence-markdown",
            analog: "Evidence.dev",
            entrypoint: "src/evidence.ts",
            supports: vec![
                "SQL block extraction",
                "chart component discovery",
                "frontmatter hints",
            ],
        },
        RendererClientTarget {
            id: "infra-diagrams",
            analog: "Terraform graph / cloud architecture diagrams",
            entrypoint: "src/infra.ts",
            supports: vec![
                "topology graph normalization",
                "node/edge indexing",
                "renderer lookup",
            ],
        },
    ]
}

fn package_files() -> Vec<RendererClientPackageFile> {
    let files = vec![
        file("package.json", "json", PACKAGE_JSON),
        file("README.md", "markdown", README_MD),
        file("src/index.ts", "typescript", INDEX_TS),
        file("src/types.ts", "typescript", TYPES_TS),
        file("src/d3.ts", "typescript", D3_TS),
        file("src/plotly.ts", "typescript", PLOTLY_TS),
        file("src/evidence.ts", "typescript", EVIDENCE_TS),
        file("src/infra.ts", "typescript", INFRA_TS),
    ];
    debug_assert!(files.len() <= MAX_PACKAGE_FILES);
    debug_assert!(files.iter().all(|file| file.bytes <= MAX_FILE_BYTES));
    files
}

fn file(
    path: &'static str,
    kind: &'static str,
    content: &'static str,
) -> RendererClientPackageFile {
    RendererClientPackageFile {
        path,
        kind,
        bytes: content.len(),
        checksum: stable_checksum(content),
        content,
    }
}

fn stable_checksum(input: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

const PACKAGE_JSON: &str = r#"{
  "name": "@dd-data-viz/renderers",
  "version": "0.1.0",
  "type": "module",
  "sideEffects": false,
  "exports": {
    ".": "./src/index.ts",
    "./d3": "./src/d3.ts",
    "./plotly": "./src/plotly.ts",
    "./evidence": "./src/evidence.ts",
    "./infra": "./src/infra.ts"
  },
  "peerDependencies": {
    "d3": "^7.0.0",
    "plotly.js": "^2.0.0"
  }
}
"#;

const README_MD: &str = r#"# @dd-data-viz/renderers

Generated client helpers for dd-data-viz-rs renderer contracts.

This package is emitted by `GET /renderers/client-package`. It is a source blueprint, not a bundled
artifact. Consumers can place these files in a frontend workspace, install their preferred D3 or
Plotly runtime, and connect the typed helpers to app-specific rendering code.
"#;

const INDEX_TS: &str = r#"export * from "./types";
export * from "./d3";
export * from "./plotly";
export * from "./evidence";
export * from "./infra";
"#;

const TYPES_TS: &str = r#"export type Scalar = string | number | boolean | null;

export interface VisualizationEncoding {
  channel: string;
  field: string;
  scale?: string;
  aggregate?: string;
}

export interface VisualizationSpec {
  specId?: string;
  mark: string;
  layout: string;
  encodings: VisualizationEncoding[];
  fitness?: Record<string, number>;
  data?: Record<string, Scalar>[];
}

export interface FinalLayer {
  schemaVersion?: string;
  spec?: VisualizationSpec;
  specs?: VisualizationSpec[];
  rows?: Record<string, Scalar>[];
  metadata?: Record<string, Scalar>;
}

export interface PlotlyFigure {
  data: Array<Record<string, unknown>>;
  layout: Record<string, unknown>;
  config: Record<string, unknown>;
}

export interface DashCallbackBlueprint {
  id: string;
  inputs: string[];
  outputs: string[];
  figure: PlotlyFigure;
}

export interface EvidenceReportCompileResponse {
  ok: boolean;
  schemaVersion: string;
  markdown: string;
  queryCount: number;
  chartCount: number;
  dependencies: Array<Record<string, unknown>>;
}

export interface InfraGraphNode {
  id: string;
  label?: string;
  kind?: string;
  provider?: string;
}

export interface InfraGraphEdge {
  from: string;
  to: string;
  label?: string;
}

export interface InfraDiagramResponse {
  ok: boolean;
  graph?: {
    nodes?: InfraGraphNode[];
    edges?: InfraGraphEdge[];
  };
  renderers?: Record<string, unknown>;
}
"#;

const D3_TS: &str = r#"import type { FinalLayer, VisualizationEncoding, VisualizationSpec } from "./types";

export interface D3MountPlan {
  specId: string;
  mark: string;
  layout: string;
  encodings: VisualizationEncoding[];
  rowCount: number;
}

export function assertD3FinalLayer(layer: FinalLayer): FinalLayer {
  const specs = collectSpecs(layer);
  if (specs.length === 0) {
    throw new Error("dd-data-viz final layer must include at least one visualization spec");
  }
  for (const spec of specs) {
    if (!spec.mark || !spec.layout) {
      throw new Error("visualization spec must include mark and layout");
    }
  }
  return layer;
}

export function d3MountPlans(layer: FinalLayer): D3MountPlan[] {
  const checked = assertD3FinalLayer(layer);
  const rows = checked.rows ?? checked.spec?.data ?? [];
  return collectSpecs(checked).map((spec, index) => ({
    specId: spec.specId ?? `spec-${index}`,
    mark: spec.mark,
    layout: spec.layout,
    encodings: spec.encodings ?? [],
    rowCount: rows.length
  }));
}

export function mountD3FinalLayer(container: Element, layer: FinalLayer): D3MountPlan[] {
  const plans = d3MountPlans(layer);
  container.dispatchEvent(new CustomEvent("dd-data-viz:d3-plan", { detail: { plans, layer } }));
  return plans;
}

function collectSpecs(layer: FinalLayer): VisualizationSpec[] {
  if (Array.isArray(layer.specs)) {
    return layer.specs;
  }
  return layer.spec ? [layer.spec] : [];
}
"#;

const PLOTLY_TS: &str = r#"import type { DashCallbackBlueprint, FinalLayer, PlotlyFigure, VisualizationSpec } from "./types";

const TRACE_BY_MARK: Record<string, string> = {
  bar: "bar",
  line: "scatter",
  scatter: "scatter",
  stem: "scatter",
  histogram: "histogram",
  box: "box",
  violin: "violin",
  ecdf: "scatter",
  map: "scattergeo",
  choropleth: "choropleth",
  funnel: "funnel",
  waterfall: "waterfall",
  treemap: "treemap",
  sunburst: "sunburst",
  sankey: "sankey",
  candlestick: "candlestick",
  bubble: "scatter",
  gauge: "indicator",
  surface: "surface",
  "volume-cloud": "volume",
  "parallel-coordinates": "parcoords",
  "radial-density": "scatterpolar"
};

export function toPlotlyFigure(layer: FinalLayer): PlotlyFigure {
  const spec = firstSpec(layer);
  const rows = layer.rows ?? spec.data ?? [];
  const x = fieldValues(rows, fieldFor(spec, "x"));
  const y = fieldValues(rows, fieldFor(spec, "y"));
  const z = fieldValues(rows, fieldFor(spec, "z"));
  const layout: Record<string, unknown> = {
    title: spec.specId ?? "dd-data-viz",
    scene: spec.layout.includes("3d") ? {} : undefined
  };
  const shapes = referenceLineShapes(spec);
  if (shapes.length > 0) {
    layout.shapes = shapes;
  }

  let data: Array<Record<string, unknown>>;
  switch (spec.mark) {
    case "stem":
      layout.showlegend = false;
      data = stemTraces(x, y);
      break;
    case "histogram":
      // Plotly bins automatically; a single numeric channel is enough.
      data = [{ type: "histogram", x: x.length > 0 ? x : y }];
      break;
    case "box":
      data = [{ type: "box", y, x: x.length > 0 ? x : undefined }];
      break;
    case "violin":
      data = [
        { type: "violin", y, x: x.length > 0 ? x : undefined, box: { visible: true }, meanline: { visible: true } }
      ];
      break;
    case "ecdf":
      data = [ecdfTrace(x.length > 0 ? x : y)];
      break;
    case "map": {
      // Symbol/point map: latitude + longitude markers, optionally sized by a measure.
      const lat = fieldValues(rows, fieldFor(spec, "lat"));
      const lon = fieldValues(rows, fieldFor(spec, "lon"));
      const size = fieldValues(rows, fieldFor(spec, "size"));
      data = [{ type: "scattergeo", lat, lon, mode: "markers", marker: size.length > 0 ? { size } : { size: 6 } }];
      layout.geo = { fitbounds: "locations" };
      break;
    }
    case "choropleth": {
      // Filled map: region codes (location channel) shaded by a measure (value/z channel).
      const locations = fieldValues(rows, fieldFor(spec, "location"));
      const values = fieldValues(rows, fieldFor(spec, "value") ?? fieldFor(spec, "z"));
      data = [{ type: "choropleth", locations, z: values, locationmode: geoLocationMode(spec) }];
      layout.geo = { fitbounds: "locations" };
      break;
    }
    case "treemap":
    case "sunburst": {
      // Hierarchy: each row is a node bound to its parent, optionally weighted.
      const labels = fieldValues(rows, fieldFor(spec, "label"));
      const parents = fieldValues(rows, fieldFor(spec, "parent"));
      const values = fieldValues(rows, fieldFor(spec, "value") ?? fieldFor(spec, "y"));
      const trace: Record<string, unknown> = { type: spec.mark, labels, parents };
      if (values.length > 0) {
        trace.values = values;
        trace.branchvalues = "total";
      }
      data = [trace];
      break;
    }
    case "sankey":
      layout.showlegend = false;
      data = [sankeyTrace(rows, spec)];
      break;
    case "candlestick": {
      // Financial OHLC: each row carries open/high/low/close for an x period.
      const open = fieldValues(rows, fieldFor(spec, "open"));
      const high = fieldValues(rows, fieldFor(spec, "high"));
      const low = fieldValues(rows, fieldFor(spec, "low"));
      const close = fieldValues(rows, fieldFor(spec, "close"));
      data = [{ type: "candlestick", x, open, high, low, close }];
      break;
    }
    case "bubble": {
      // Scatter sized by a third measure (area-scaled markers).
      const size = fieldValues(rows, fieldFor(spec, "size"));
      data = [
        { type: "scatter", mode: "markers", x, y, marker: size.length > 0 ? { size, sizemode: "area" } : { size: 8 } }
      ];
      break;
    }
    case "gauge": {
      // KPI indicator: a single value on a gauge scaled to the observed range.
      const measure = fieldValues(rows, fieldFor(spec, "value") ?? fieldFor(spec, "y"))
        .map((entry) => Number(entry))
        .filter((entry) => Number.isFinite(entry));
      const value = measure.length > 0 ? measure[measure.length - 1] : 0;
      const upper = measure.length > 0 ? Math.max(...measure, value) : 1;
      data = [{ type: "indicator", mode: "gauge+number", value, gauge: { axis: { range: [0, upper] } } }];
      break;
    }
    default: {
      const traceType = TRACE_BY_MARK[spec.mark] ?? "scatter";
      const trace: Record<string, unknown> = { type: traceType, x, y };
      if (z.length > 0) {
        trace.z = z;
      }
      if (spec.mark === "line") {
        trace.mode = "lines+markers";
      }
      data = [trace];
    }
  }
  return { data, layout, config: { responsive: true, displaylogo: false } };
}

// A Sankey flow is built from (source, target, value) rows: node labels are
// interned to indices the first time they are seen, and each row becomes one
// weighted link between those node indices.
function sankeyTrace(rows: Array<Record<string, unknown>>, spec: VisualizationSpec): Record<string, unknown> {
  const sources = fieldValues(rows, fieldFor(spec, "source")).map((value) => String(value));
  const targets = fieldValues(rows, fieldFor(spec, "target")).map((value) => String(value));
  const values = fieldValues(rows, fieldFor(spec, "value"));
  const labels: string[] = [];
  const indexOf = new Map<string, number>();
  const intern = (name: string): number => {
    const existing = indexOf.get(name);
    if (existing !== undefined) {
      return existing;
    }
    const next = labels.length;
    indexOf.set(name, next);
    labels.push(name);
    return next;
  };
  const linkSource = sources.map(intern);
  const linkTarget = targets.map(intern);
  return {
    type: "sankey",
    node: { label: labels, pad: 12, thickness: 14 },
    link: { source: linkSource, target: linkTarget, value: values }
  };
}

// Choropleth region codes can be ISO-3 (default), country names, or USA states;
// the spec carries the mode so the same data binds to different geographies.
function geoLocationMode(spec: VisualizationSpec): string {
  return (spec as { geo?: { locationMode?: string } }).geo?.locationMode ?? "ISO-3";
}

interface ReferenceLine {
  axis?: string;
  value?: number;
}

// Reference lines/bands are carried as an optional spec field and rendered as
// dashed Plotly layout shapes spanning the opposite axis (x => vertical,
// otherwise horizontal).
function referenceLineShapes(spec: VisualizationSpec): Array<Record<string, unknown>> {
  const lines = (spec as { referenceLines?: ReferenceLine[] }).referenceLines ?? [];
  return lines
    .filter((line) => typeof line.value === "number")
    .map((line) =>
      line.axis === "x"
        ? { type: "line", yref: "paper", x0: line.value, x1: line.value, y0: 0, y1: 1, line: { dash: "dash", width: 1 } }
        : { type: "line", xref: "paper", y0: line.value, y1: line.value, x0: 0, x1: 1, line: { dash: "dash", width: 1 } }
    );
}

// Empirical cumulative distribution: sort the numeric values ascending and
// step the cumulative fraction from 0 to 1.
function ecdfTrace(values: unknown[]): Record<string, unknown> {
  const nums = values
    .map((value) => Number(value))
    .filter((value) => Number.isFinite(value))
    .sort((left, right) => left - right);
  const cumulative = nums.map((_, index) => (index + 1) / nums.length);
  return { type: "scatter", mode: "lines", line: { shape: "hv" }, x: nums, y: cumulative };
}

// A stem plot draws a vertical line from a baseline (y = 0) to each (x, y)
// point with a marker at the tip. Plotly has no native "stem" trace, so it is
// emulated with one lines trace whose segments are separated by nulls plus a
// markers trace at the stem heads.
function stemTraces(x: unknown[], y: unknown[]): Array<Record<string, unknown>> {
  const stemX: Array<unknown> = [];
  const stemY: Array<unknown> = [];
  for (let i = 0; i < x.length; i += 1) {
    stemX.push(x[i], x[i], null);
    stemY.push(0, y[i], null);
  }
  return [
    {
      type: "scatter",
      mode: "lines",
      x: stemX,
      y: stemY,
      line: { width: 1.5 },
      hoverinfo: "skip"
    },
    {
      type: "scatter",
      mode: "markers",
      x,
      y,
      marker: { size: 7 }
    }
  ];
}

export function dashCallbackBlueprint(layer: FinalLayer, id = "dd-data-viz-figure"): DashCallbackBlueprint {
  return {
    id,
    inputs: ["dd-data-viz-store.data"],
    outputs: [`${id}.figure`],
    figure: toPlotlyFigure(layer)
  };
}

function firstSpec(layer: FinalLayer): VisualizationSpec {
  const spec = layer.spec ?? layer.specs?.[0];
  if (!spec) {
    throw new Error("Plotly conversion requires at least one visualization spec");
  }
  return spec;
}

function fieldFor(spec: VisualizationSpec, channel: string): string | undefined {
  return spec.encodings?.find((encoding) => encoding.channel === channel)?.field;
}

function fieldValues(rows: Array<Record<string, unknown>>, field?: string): unknown[] {
  return field ? rows.map((row) => row[field]) : [];
}
"#;

const EVIDENCE_TS: &str = r#"import type { EvidenceReportCompileResponse } from "./types";

export interface EvidenceSqlBlock {
  queryName: string;
  sql: string;
}

export interface EvidenceChartComponent {
  component: string;
  queryName?: string;
}

export function evidenceSqlBlocks(report: EvidenceReportCompileResponse | string): EvidenceSqlBlock[] {
  const markdown = typeof report === "string" ? report : report.markdown;
  const blocks: EvidenceSqlBlock[] = [];
  const pattern = /```sql\s+([A-Za-z_][A-Za-z0-9_]*)\n([\s\S]*?)```/g;
  for (const match of markdown.matchAll(pattern)) {
    blocks.push({ queryName: match[1], sql: match[2].trim() });
  }
  return blocks;
}

export function evidenceChartComponents(report: EvidenceReportCompileResponse | string): EvidenceChartComponent[] {
  const markdown = typeof report === "string" ? report : report.markdown;
  const components: EvidenceChartComponent[] = [];
  const pattern = /<(BarChart|LineChart|AreaChart|ScatterPlot|Table|DataTable|BigValue)([^>]*)\/>/g;
  for (const match of markdown.matchAll(pattern)) {
    const data = /data=\{([A-Za-z_][A-Za-z0-9_]*)\}/.exec(match[2]);
    components.push({ component: match[1], queryName: data?.[1] });
  }
  return components;
}
"#;

const INFRA_TS: &str = r#"import type { InfraDiagramResponse, InfraGraphEdge, InfraGraphNode } from "./types";

export interface NormalizedInfraGraph {
  nodes: InfraGraphNode[];
  edges: InfraGraphEdge[];
  nodeById: Map<string, InfraGraphNode>;
  outgoing: Map<string, InfraGraphEdge[]>;
}

export function normalizeInfraGraph(diagram: InfraDiagramResponse): NormalizedInfraGraph {
  const nodes = diagram.graph?.nodes ?? [];
  const edges = diagram.graph?.edges ?? [];
  return {
    nodes,
    edges,
    nodeById: new Map(nodes.map((node) => [node.id, node])),
    outgoing: edges.reduce((index, edge) => {
      const bucket = index.get(edge.from) ?? [];
      bucket.push(edge);
      index.set(edge.from, bucket);
      return index;
    }, new Map<string, InfraGraphEdge[]>())
  };
}

export function rendererPayload(diagram: InfraDiagramResponse, rendererId: string): unknown {
  return diagram.renderers?.[rendererId];
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_package_contains_renderer_targets_and_files() {
        let package = client_package();
        let paths = package
            .files
            .iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();

        assert_eq!(package.schema_version, SCHEMA_VERSION);
        assert_eq!(package.targets.len(), 4);
        assert!(paths.contains(&"src/d3.ts"));
        assert!(paths.contains(&"src/plotly.ts"));
        assert!(paths.contains(&"src/evidence.ts"));
        assert!(paths.contains(&"src/infra.ts"));
    }

    #[test]
    fn client_package_includes_expected_helper_functions() {
        let package = client_package();
        let all_content = package
            .files
            .iter()
            .map(|file| file.content)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(all_content.contains("mountD3FinalLayer"));
        assert!(all_content.contains("toPlotlyFigure"));
        assert!(all_content.contains("evidenceSqlBlocks"));
        assert!(all_content.contains("normalizeInfraGraph"));
    }

    #[test]
    fn plotly_renderer_supports_stem_marks() {
        assert!(PLOTLY_TS.contains("stem: \"scatter\""));
        assert!(PLOTLY_TS.contains("function stemTraces"));
        assert!(PLOTLY_TS.contains("case \"stem\""));
    }

    #[test]
    fn plotly_renderer_supports_statistical_marks() {
        for entry in ["histogram: \"histogram\"", "box: \"box\"", "violin: \"violin\"", "ecdf: \"scatter\""] {
            assert!(PLOTLY_TS.contains(entry), "missing trace mapping: {entry}");
        }
        assert!(PLOTLY_TS.contains("function ecdfTrace"));
        assert!(PLOTLY_TS.contains("function referenceLineShapes"));
    }

    #[test]
    fn plotly_renderer_supports_geospatial_marks() {
        assert!(PLOTLY_TS.contains("map: \"scattergeo\""));
        assert!(PLOTLY_TS.contains("choropleth: \"choropleth\""));
        assert!(PLOTLY_TS.contains("function geoLocationMode"));
        assert!(PLOTLY_TS.contains("case \"map\""));
        assert!(PLOTLY_TS.contains("case \"choropleth\""));
    }

    #[test]
    fn plotly_renderer_supports_flow_and_hierarchy_marks() {
        for entry in [
            "funnel: \"funnel\"",
            "waterfall: \"waterfall\"",
            "treemap: \"treemap\"",
            "sunburst: \"sunburst\"",
            "sankey: \"sankey\"",
        ] {
            assert!(PLOTLY_TS.contains(entry), "missing trace mapping: {entry}");
        }
        assert!(PLOTLY_TS.contains("function sankeyTrace"));
        assert!(PLOTLY_TS.contains("case \"treemap\""));
        assert!(PLOTLY_TS.contains("case \"sunburst\""));
        assert!(PLOTLY_TS.contains("case \"sankey\""));
    }

    #[test]
    fn plotly_renderer_supports_financial_and_kpi_marks() {
        for entry in ["candlestick: \"candlestick\"", "bubble: \"scatter\"", "gauge: \"indicator\""] {
            assert!(PLOTLY_TS.contains(entry), "missing trace mapping: {entry}");
        }
        assert!(PLOTLY_TS.contains("case \"candlestick\""));
        assert!(PLOTLY_TS.contains("case \"bubble\""));
        assert!(PLOTLY_TS.contains("case \"gauge\""));
    }

    #[test]
    fn client_package_integrity_matches_file_set() {
        let package = client_package();
        let total_bytes = package.files.iter().map(|file| file.bytes).sum::<usize>();

        assert_eq!(package.integrity.file_count, package.files.len());
        assert_eq!(package.integrity.total_bytes, total_bytes);
        assert!(package.integrity.package_checksum.starts_with("fnv1a64:"));
        assert!(package
            .files
            .iter()
            .all(|file| file.bytes <= MAX_FILE_BYTES));
    }
}
