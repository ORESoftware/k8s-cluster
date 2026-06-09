use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::util::clean_identifier;

const SCHEMA_VERSION: &str = "data-viz.renderer-artifact-verification.v1";
const MAX_ARTIFACT_BYTES: usize = 96 * 1024;
const MAX_CONSOLE_ERRORS: usize = 32;
const MAX_TEXT_BOXES: usize = 96;
const MAX_REQUIRED_TEXT: usize = 16;
const DEFAULT_MIN_WIDTH: u32 = 32;
const DEFAULT_MIN_HEIGHT: u32 = 32;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifyRendererArtifactRequest {
    pub artifact_id: Option<String>,
    pub artifact_kind: RendererArtifactKind,
    pub content: Option<String>,
    pub json: Option<Value>,
    pub metadata: Option<RendererArtifactMetadata>,
    pub expectations: Option<RendererArtifactExpectations>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RendererArtifactKind {
    D3Svg,
    D3Html,
    PlotlyJson,
    EvidenceMarkdown,
    FinalLayerJson,
    ScreenshotMetadata,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererArtifactMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub non_empty_pixels: Option<usize>,
    pub transparent_pixels: Option<usize>,
    pub console_errors: Option<Vec<String>>,
    pub text_boxes: Option<Vec<RendererTextBox>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererTextBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererArtifactExpectations {
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub min_marks: Option<usize>,
    pub required_text: Option<Vec<String>>,
    pub max_console_errors: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerifyRendererArtifactResponse {
    ok: bool,
    schema_version: &'static str,
    artifact_id: String,
    artifact_kind: RendererArtifactKind,
    verdict: RendererVerificationVerdict,
    score: f64,
    checks: Vec<RendererVerificationCheck>,
    limits: Value,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RendererVerificationVerdict {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RendererVerificationStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RendererVerificationCheck {
    id: &'static str,
    status: RendererVerificationStatus,
    score: f64,
    message: String,
    evidence: Value,
}

pub(crate) fn verify(
    request: VerifyRendererArtifactRequest,
) -> Result<VerifyRendererArtifactResponse, String> {
    let artifact_id = request
        .artifact_id
        .as_deref()
        .and_then(clean_identifier)
        .unwrap_or_else(|| "renderer-artifact".to_string());
    let expectations = normalize_expectations(request.expectations.unwrap_or_default())?;
    let metadata = normalize_metadata(request.metadata)?;
    let mut checks = Vec::new();
    let mut warnings = vec![
        "static verifier only; browser pixel-diff and interaction replay still belong in a renderer worker"
            .to_string(),
    ];

    match request.artifact_kind {
        RendererArtifactKind::D3Svg => {
            let content = bounded_content("SVG artifact content", request.content.as_deref())?;
            verify_svg(content, metadata.as_ref(), &expectations, &mut checks)?;
        }
        RendererArtifactKind::D3Html => {
            let content = bounded_content("HTML artifact content", request.content.as_deref())?;
            verify_html(content, metadata.as_ref(), &expectations, &mut checks)?;
        }
        RendererArtifactKind::PlotlyJson => {
            let value = bounded_json("Plotly artifact JSON", request.json.as_ref())?;
            verify_plotly(value, &expectations, &mut checks);
        }
        RendererArtifactKind::EvidenceMarkdown => {
            let content = bounded_content(
                "Evidence markdown artifact content",
                request.content.as_deref(),
            )?;
            verify_evidence_markdown(content, &expectations, &mut checks);
        }
        RendererArtifactKind::FinalLayerJson => {
            let value = bounded_json("final-layer artifact JSON", request.json.as_ref())?;
            verify_final_layer(value, &mut checks);
        }
        RendererArtifactKind::ScreenshotMetadata => {
            if request.content.is_some() || request.json.is_some() {
                warnings.push(
                    "screenshot-metadata verification ignores raw image bytes and JSON artifacts"
                        .to_string(),
                );
            }
        }
    }

    if let Some(metadata) = &metadata {
        verify_metadata(metadata, &expectations, &mut checks);
    } else if request.artifact_kind == RendererArtifactKind::ScreenshotMetadata {
        checks.push(check(
            "screenshot-metadata-present",
            RendererVerificationStatus::Fail,
            "screenshot metadata is required for screenshot-metadata artifacts",
            json!({ "metadataPresent": false }),
        ));
    }

    let verdict = verdict_for(&checks);
    let score = score_for(&checks);
    Ok(VerifyRendererArtifactResponse {
        ok: true,
        schema_version: SCHEMA_VERSION,
        artifact_id,
        artifact_kind: request.artifact_kind,
        verdict,
        score,
        checks,
        limits: limits_payload(),
        warnings,
    })
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxArtifactBytes": MAX_ARTIFACT_BYTES,
        "maxConsoleErrors": MAX_CONSOLE_ERRORS,
        "maxTextBoxes": MAX_TEXT_BOXES,
        "maxRequiredText": MAX_REQUIRED_TEXT,
        "defaultMinWidth": DEFAULT_MIN_WIDTH,
        "defaultMinHeight": DEFAULT_MIN_HEIGHT,
        "posture": "static rendered-artifact verification; no browser, package manager, network, or user code execution"
    })
}

fn normalize_expectations(
    mut expectations: RendererArtifactExpectations,
) -> Result<RendererArtifactExpectations, String> {
    let required_text = expectations.required_text.take().unwrap_or_default();
    if required_text.len() > MAX_REQUIRED_TEXT {
        return Err(format!("requiredText exceeds max {MAX_REQUIRED_TEXT}"));
    }
    let mut normalized_text = Vec::new();
    for value in required_text {
        let text = bounded_text("required text", &value, 256)?;
        reject_secret_text("required text", &text)?;
        normalized_text.push(text);
    }
    expectations.required_text = Some(normalized_text);
    Ok(expectations)
}

fn normalize_metadata(
    metadata: Option<RendererArtifactMetadata>,
) -> Result<Option<RendererArtifactMetadata>, String> {
    let Some(mut metadata) = metadata else {
        return Ok(None);
    };
    if let Some(errors) = &metadata.console_errors {
        if errors.len() > MAX_CONSOLE_ERRORS {
            return Err(format!("consoleErrors exceeds max {MAX_CONSOLE_ERRORS}"));
        }
        for error in errors {
            reject_secret_text("console error", error)?;
        }
    }
    if let Some(boxes) = &metadata.text_boxes {
        if boxes.len() > MAX_TEXT_BOXES {
            return Err(format!("textBoxes exceeds max {MAX_TEXT_BOXES}"));
        }
        for text_box in boxes {
            if !text_box.x.is_finite()
                || !text_box.y.is_finite()
                || !text_box.width.is_finite()
                || !text_box.height.is_finite()
                || text_box.width < 0.0
                || text_box.height < 0.0
            {
                return Err("textBoxes must contain finite non-negative dimensions".to_string());
            }
            if let Some(label) = &text_box.label {
                reject_secret_text("text box label", label)?;
            }
        }
    }
    metadata.console_errors = Some(metadata.console_errors.unwrap_or_default());
    metadata.text_boxes = Some(metadata.text_boxes.unwrap_or_default());
    Ok(Some(metadata))
}

fn verify_svg(
    content: &str,
    metadata: Option<&RendererArtifactMetadata>,
    expectations: &RendererArtifactExpectations,
    checks: &mut Vec<RendererVerificationCheck>,
) -> Result<(), String> {
    let lower = content.to_ascii_lowercase();
    checks.push(check_status(
        "svg-root",
        lower.contains("<svg"),
        "SVG root element is present",
        "SVG root element is missing",
        json!({ "containsSvg": lower.contains("<svg") }),
    ));
    checks.push(check_status(
        "no-active-script",
        !contains_active_script(&lower),
        "artifact does not include active script hooks",
        "artifact includes script or inline event hooks",
        json!({ "activeScript": contains_active_script(&lower) }),
    ));

    let marks = count_svg_marks(&lower);
    let min_marks = expectations.min_marks.unwrap_or(1);
    checks.push(check_status(
        "visible-marks",
        marks >= min_marks,
        "artifact includes visible SVG marks",
        "artifact does not include enough visible SVG marks",
        json!({ "markCount": marks, "minMarks": min_marks }),
    ));

    let (width, height, has_view_box) = dimensions_from_svg(content, metadata);
    add_dimension_check(width, height, has_view_box, expectations, checks);
    add_required_text_checks(content, expectations, checks);
    Ok(())
}

fn verify_html(
    content: &str,
    metadata: Option<&RendererArtifactMetadata>,
    expectations: &RendererArtifactExpectations,
    checks: &mut Vec<RendererVerificationCheck>,
) -> Result<(), String> {
    let lower = content.to_ascii_lowercase();
    let has_renderer_surface = lower.contains("<svg") || lower.contains("<canvas");
    checks.push(check_status(
        "renderer-surface",
        has_renderer_surface,
        "HTML artifact includes an SVG or canvas surface",
        "HTML artifact is missing an SVG or canvas surface",
        json!({ "hasRendererSurface": has_renderer_surface }),
    ));
    checks.push(check_status(
        "no-active-script",
        !contains_active_script(&lower),
        "artifact does not include active script hooks",
        "artifact includes script or inline event hooks",
        json!({ "activeScript": contains_active_script(&lower) }),
    ));
    let marks = count_svg_marks(&lower);
    let has_canvas = lower.contains("<canvas");
    let min_marks = expectations.min_marks.unwrap_or(1);
    checks.push(check_status(
        "visible-marks",
        marks >= min_marks || has_canvas,
        "HTML artifact includes drawable marks or canvas",
        "HTML artifact does not include enough drawable marks",
        json!({ "markCount": marks, "hasCanvas": has_canvas, "minMarks": min_marks }),
    ));
    let (width, height, has_view_box) = dimensions_from_svg(content, metadata);
    add_dimension_check(
        width,
        height,
        has_view_box || has_canvas,
        expectations,
        checks,
    );
    add_required_text_checks(content, expectations, checks);
    Ok(())
}

fn verify_plotly(
    value: &Value,
    expectations: &RendererArtifactExpectations,
    checks: &mut Vec<RendererVerificationCheck>,
) {
    let Some(object) = value.as_object() else {
        checks.push(check(
            "plotly-object",
            RendererVerificationStatus::Fail,
            "Plotly artifact must be a JSON object",
            json!({ "jsonType": value_type(value) }),
        ));
        return;
    };
    let data = object.get("data").and_then(Value::as_array);
    let trace_count = data.map(Vec::len).unwrap_or(0);
    checks.push(check_status(
        "plotly-traces",
        trace_count > 0,
        "Plotly figure includes traces",
        "Plotly figure has no traces",
        json!({ "traceCount": trace_count }),
    ));

    let allowed_types = [
        "bar",
        "scatter",
        "surface",
        "volume",
        "parcoords",
        "scatterpolar",
        "heatmap",
        "histogram",
    ];
    let mut traces_with_points = 0usize;
    let mut unknown_trace_types = Vec::new();
    if let Some(data) = data {
        for trace in data {
            let trace_type = trace
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("scatter");
            if !allowed_types.contains(&trace_type) {
                unknown_trace_types.push(trace_type.to_string());
            }
            if trace_array_len(trace, "x") > 0
                || trace_array_len(trace, "y") > 0
                || trace_array_len(trace, "z") > 0
            {
                traces_with_points += 1;
            }
        }
    }
    checks.push(check_status(
        "plotly-points",
        traces_with_points > 0,
        "Plotly traces include x, y, or z values",
        "Plotly traces do not include x, y, or z values",
        json!({ "tracesWithPoints": traces_with_points }),
    ));
    checks.push(check_status(
        "plotly-trace-types",
        unknown_trace_types.is_empty(),
        "Plotly trace types are known to the client helper",
        "Plotly figure includes unknown trace types",
        json!({ "unknownTraceTypes": unknown_trace_types }),
    ));
    let has_layout = object.get("layout").and_then(Value::as_object).is_some();
    checks.push(check(
        "plotly-layout",
        if has_layout {
            RendererVerificationStatus::Pass
        } else {
            RendererVerificationStatus::Warn
        },
        if has_layout {
            "Plotly layout object is present"
        } else {
            "Plotly layout object is missing"
        },
        json!({ "layoutPresent": has_layout }),
    ));
    if let Some(required) = expectations.required_text.as_ref() {
        let haystack = value.to_string();
        for text in required {
            checks.push(check_status(
                "required-text",
                haystack.contains(text),
                "required text appears in Plotly artifact",
                "required text is missing from Plotly artifact",
                json!({ "text": text }),
            ));
        }
    }
}

fn verify_evidence_markdown(
    content: &str,
    expectations: &RendererArtifactExpectations,
    checks: &mut Vec<RendererVerificationCheck>,
) {
    let sql_blocks = content.matches("```sql ").count();
    let chart_components = [
        "<BarChart",
        "<LineChart",
        "<AreaChart",
        "<ScatterPlot",
        "<Table",
        "<DataTable",
        "<BigValue",
    ]
    .iter()
    .map(|needle| content.matches(needle).count())
    .sum::<usize>();
    checks.push(check_status(
        "evidence-sql-blocks",
        sql_blocks > 0,
        "Evidence markdown includes SQL query blocks",
        "Evidence markdown is missing SQL query blocks",
        json!({ "sqlBlockCount": sql_blocks }),
    ));
    checks.push(check_status(
        "evidence-chart-components",
        chart_components > 0,
        "Evidence markdown includes chart components",
        "Evidence markdown is missing chart components",
        json!({ "chartComponentCount": chart_components }),
    ));
    let has_frontmatter = content.trim_start().starts_with("---");
    checks.push(check(
        "evidence-frontmatter",
        if has_frontmatter {
            RendererVerificationStatus::Pass
        } else {
            RendererVerificationStatus::Warn
        },
        if has_frontmatter {
            "Evidence markdown includes frontmatter"
        } else {
            "Evidence markdown is missing frontmatter"
        },
        json!({ "frontmatterPresent": has_frontmatter }),
    ));
    add_required_text_checks(content, expectations, checks);
}

fn verify_final_layer(value: &Value, checks: &mut Vec<RendererVerificationCheck>) {
    let Some(object) = value.as_object() else {
        checks.push(check(
            "final-layer-object",
            RendererVerificationStatus::Fail,
            "final layer artifact must be a JSON object",
            json!({ "jsonType": value_type(value) }),
        ));
        return;
    };
    let specs = collect_specs(value);
    checks.push(check_status(
        "final-layer-specs",
        !specs.is_empty(),
        "final layer includes visualization specs",
        "final layer has no visualization specs",
        json!({ "specCount": specs.len() }),
    ));
    let complete_specs = specs
        .iter()
        .filter(|spec| {
            spec.get("mark").and_then(Value::as_str).is_some()
                && spec.get("layout").and_then(Value::as_str).is_some()
                && spec.get("encodings").and_then(Value::as_array).is_some()
        })
        .count();
    checks.push(check_status(
        "final-layer-spec-shape",
        !specs.is_empty() && complete_specs == specs.len(),
        "all final-layer specs include mark, layout, and encodings",
        "some final-layer specs are missing mark, layout, or encodings",
        json!({ "completeSpecs": complete_specs, "specCount": specs.len() }),
    ));
    let row_count = object
        .get("rows")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    checks.push(check(
        "final-layer-data",
        if row_count > 0 {
            RendererVerificationStatus::Pass
        } else {
            RendererVerificationStatus::Warn
        },
        if row_count > 0 {
            "final layer includes rendered row data"
        } else {
            "final layer has no top-level row data"
        },
        json!({ "rowCount": row_count }),
    ));
}

fn verify_metadata(
    metadata: &RendererArtifactMetadata,
    expectations: &RendererArtifactExpectations,
    checks: &mut Vec<RendererVerificationCheck>,
) {
    add_dimension_check(metadata.width, metadata.height, false, expectations, checks);
    let non_empty_pixels = metadata.non_empty_pixels.unwrap_or(0);
    checks.push(check_status(
        "non-empty-pixels",
        non_empty_pixels > 0,
        "screenshot metadata reports non-empty pixels",
        "screenshot metadata reports no non-empty pixels",
        json!({ "nonEmptyPixels": non_empty_pixels }),
    ));
    let console_errors = metadata.console_errors.as_deref().unwrap_or(&[]);
    let max_errors = expectations.max_console_errors.unwrap_or(0);
    checks.push(check_status(
        "console-errors",
        console_errors.len() <= max_errors,
        "console error count is within expectation",
        "console error count exceeds expectation",
        json!({ "consoleErrorCount": console_errors.len(), "maxConsoleErrors": max_errors }),
    ));
    let overlaps = count_overlaps(metadata.text_boxes.as_deref().unwrap_or(&[]));
    checks.push(check_status(
        "text-overlap",
        overlaps == 0,
        "text boxes do not overlap",
        "text boxes overlap",
        json!({ "overlapCount": overlaps }),
    ));
}

fn add_dimension_check(
    width: Option<u32>,
    height: Option<u32>,
    has_view_box: bool,
    expectations: &RendererArtifactExpectations,
    checks: &mut Vec<RendererVerificationCheck>,
) {
    let min_width = expectations.min_width.unwrap_or(DEFAULT_MIN_WIDTH);
    let min_height = expectations.min_height.unwrap_or(DEFAULT_MIN_HEIGHT);
    let dimensions_ok = width
        .zip(height)
        .map(|(width, height)| width >= min_width && height >= min_height)
        .unwrap_or(has_view_box);
    checks.push(check_status(
        "artifact-dimensions",
        dimensions_ok,
        "artifact dimensions meet minimum expectations",
        "artifact dimensions are missing or below minimum expectations",
        json!({
            "width": width,
            "height": height,
            "hasViewBox": has_view_box,
            "minWidth": min_width,
            "minHeight": min_height
        }),
    ));
}

fn add_required_text_checks(
    content: &str,
    expectations: &RendererArtifactExpectations,
    checks: &mut Vec<RendererVerificationCheck>,
) {
    if let Some(required) = expectations.required_text.as_ref() {
        for text in required {
            checks.push(check_status(
                "required-text",
                content.contains(text),
                "required text appears in artifact",
                "required text is missing from artifact",
                json!({ "text": text }),
            ));
        }
    }
}

fn bounded_content<'a>(label: &str, content: Option<&'a str>) -> Result<&'a str, String> {
    let Some(content) = content else {
        return Err(format!("{label} is required"));
    };
    if content.trim().is_empty() || content.len() > MAX_ARTIFACT_BYTES {
        return Err(format!("{label} must be 1-{MAX_ARTIFACT_BYTES} bytes"));
    }
    reject_secret_text(label, content)?;
    Ok(content)
}

fn bounded_json<'a>(label: &str, value: Option<&'a Value>) -> Result<&'a Value, String> {
    let Some(value) = value else {
        return Err(format!("{label} is required"));
    };
    let serialized = value.to_string();
    if serialized.len() > MAX_ARTIFACT_BYTES {
        return Err(format!("{label} must be <= {MAX_ARTIFACT_BYTES} bytes"));
    }
    reject_secret_text(label, &serialized)?;
    Ok(value)
}

fn bounded_text(label: &str, value: &str, max_bytes: usize) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > max_bytes {
        Err(format!("{label} must be 1-{max_bytes} bytes"))
    } else {
        Ok(value)
    }
}

fn check(
    id: &'static str,
    status: RendererVerificationStatus,
    message: impl Into<String>,
    evidence: Value,
) -> RendererVerificationCheck {
    let score = match status {
        RendererVerificationStatus::Pass => 1.0,
        RendererVerificationStatus::Warn => 0.5,
        RendererVerificationStatus::Fail => 0.0,
    };
    RendererVerificationCheck {
        id,
        status,
        score,
        message: message.into(),
        evidence,
    }
}

fn check_status(
    id: &'static str,
    passed: bool,
    pass_message: &'static str,
    fail_message: &'static str,
    evidence: Value,
) -> RendererVerificationCheck {
    if passed {
        check(id, RendererVerificationStatus::Pass, pass_message, evidence)
    } else {
        check(id, RendererVerificationStatus::Fail, fail_message, evidence)
    }
}

fn verdict_for(checks: &[RendererVerificationCheck]) -> RendererVerificationVerdict {
    if checks
        .iter()
        .any(|check| check.status == RendererVerificationStatus::Fail)
    {
        RendererVerificationVerdict::Fail
    } else if checks
        .iter()
        .any(|check| check.status == RendererVerificationStatus::Warn)
    {
        RendererVerificationVerdict::Warn
    } else {
        RendererVerificationVerdict::Pass
    }
}

fn score_for(checks: &[RendererVerificationCheck]) -> f64 {
    if checks.is_empty() {
        return 0.0;
    }
    let score = checks.iter().map(|check| check.score).sum::<f64>() / checks.len() as f64;
    (score * 10_000.0).round() / 10_000.0
}

fn contains_active_script(lower: &str) -> bool {
    lower.contains("<script") || lower.contains(" onload=") || lower.contains(" onclick=")
}

fn count_svg_marks(lower: &str) -> usize {
    [
        "<rect",
        "<circle",
        "<path",
        "<line",
        "<polyline",
        "<polygon",
        "<text",
    ]
    .into_iter()
    .map(|needle| lower.matches(needle).count())
    .sum()
}

fn dimensions_from_svg(
    content: &str,
    metadata: Option<&RendererArtifactMetadata>,
) -> (Option<u32>, Option<u32>, bool) {
    let lower = content.to_ascii_lowercase();
    let width = metadata
        .and_then(|metadata| metadata.width)
        .or_else(|| extract_numeric_attr(content, "width"));
    let height = metadata
        .and_then(|metadata| metadata.height)
        .or_else(|| extract_numeric_attr(content, "height"));
    (width, height, lower.contains("viewbox="))
}

fn extract_numeric_attr(content: &str, attr: &str) -> Option<u32> {
    for quote in ['"', '\''] {
        let needle = format!("{attr}={quote}");
        if let Some(start) = content.find(&needle) {
            let start = start + needle.len();
            let value = content[start..]
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect::<String>();
            if let Ok(parsed) = value.parse::<f64>() {
                if parsed.is_finite() && parsed >= 0.0 {
                    return Some(parsed.round() as u32);
                }
            }
        }
    }
    None
}

fn trace_array_len(trace: &Value, field: &str) -> usize {
    trace
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

fn collect_specs(value: &Value) -> Vec<&Value> {
    if let Some(specs) = value.get("specs").and_then(Value::as_array) {
        specs.iter().collect()
    } else if let Some(spec) = value.get("spec") {
        vec![spec]
    } else {
        Vec::new()
    }
}

fn count_overlaps(boxes: &[RendererTextBox]) -> usize {
    let mut overlaps = 0usize;
    for left_index in 0..boxes.len() {
        for right_index in left_index + 1..boxes.len() {
            if rectangles_overlap(&boxes[left_index], &boxes[right_index]) {
                overlaps += 1;
            }
        }
    }
    overlaps
}

fn rectangles_overlap(left: &RendererTextBox, right: &RendererTextBox) -> bool {
    left.x < right.x + right.width
        && left.x + left.width > right.x
        && left.y < right.y + right.height
        && left.y + left.height > right.y
}

fn reject_secret_text(label: &str, value: &str) -> Result<(), String> {
    if looks_secret_bearing(value) {
        Err(format!("{label} contains secret-looking text"))
    } else {
        Ok(())
    }
}

fn looks_secret_bearing(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "secret",
        "token",
        "password",
        "authorization",
        "bearer",
        "api_key",
        "private_key",
        "access_key",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_artifact_passes_static_checks() {
        let response = verify(VerifyRendererArtifactRequest {
            artifact_id: Some("chart-svg".to_string()),
            artifact_kind: RendererArtifactKind::D3Svg,
            content: Some(
                r#"<svg width="640" height="360"><rect x="10" y="10" width="40" height="20"></rect><text>Revenue</text></svg>"#
                    .to_string(),
            ),
            json: None,
            metadata: None,
            expectations: Some(RendererArtifactExpectations {
                required_text: Some(vec!["Revenue".to_string()]),
                ..RendererArtifactExpectations::default()
            }),
        })
        .expect("verified");

        assert_eq!(response.verdict, RendererVerificationVerdict::Pass);
        assert!(response.score > 0.9);
    }

    #[test]
    fn svg_artifact_fails_blank_output() {
        let response = verify(VerifyRendererArtifactRequest {
            artifact_id: None,
            artifact_kind: RendererArtifactKind::D3Svg,
            content: Some(r#"<svg width="640" height="360"></svg>"#.to_string()),
            json: None,
            metadata: None,
            expectations: None,
        })
        .expect("verified");

        assert_eq!(response.verdict, RendererVerificationVerdict::Fail);
        assert!(response
            .checks
            .iter()
            .any(|check| check.id == "visible-marks"
                && check.status == RendererVerificationStatus::Fail));
    }

    #[test]
    fn plotly_artifact_verifies_traces() {
        let response = verify(VerifyRendererArtifactRequest {
            artifact_id: Some("plotly".to_string()),
            artifact_kind: RendererArtifactKind::PlotlyJson,
            content: None,
            json: Some(json!({
                "data": [{ "type": "scatter", "x": [1, 2], "y": [3, 4] }],
                "layout": { "title": "Revenue" }
            })),
            metadata: None,
            expectations: None,
        })
        .expect("verified");

        assert_eq!(response.verdict, RendererVerificationVerdict::Pass);
    }

    #[test]
    fn evidence_markdown_rejects_secret_like_text() {
        let error = verify(VerifyRendererArtifactRequest {
            artifact_id: None,
            artifact_kind: RendererArtifactKind::EvidenceMarkdown,
            content: Some("```sql q\nSELECT 'token'\n```".to_string()),
            json: None,
            metadata: None,
            expectations: None,
        })
        .expect_err("secret rejected");

        assert!(error.contains("secret-looking"));
    }

    #[test]
    fn screenshot_metadata_detects_text_overlap() {
        let response = verify(VerifyRendererArtifactRequest {
            artifact_id: Some("shot".to_string()),
            artifact_kind: RendererArtifactKind::ScreenshotMetadata,
            content: None,
            json: None,
            metadata: Some(RendererArtifactMetadata {
                width: Some(800),
                height: Some(600),
                non_empty_pixels: Some(1200),
                transparent_pixels: None,
                console_errors: Some(Vec::new()),
                text_boxes: Some(vec![
                    RendererTextBox {
                        x: 10.0,
                        y: 10.0,
                        width: 100.0,
                        height: 20.0,
                        label: Some("A".to_string()),
                    },
                    RendererTextBox {
                        x: 50.0,
                        y: 15.0,
                        width: 100.0,
                        height: 20.0,
                        label: Some("B".to_string()),
                    },
                ]),
            }),
            expectations: None,
        })
        .expect("verified");

        assert_eq!(response.verdict, RendererVerificationVerdict::Fail);
        assert!(response
            .checks
            .iter()
            .any(|check| check.id == "text-overlap"
                && check.status == RendererVerificationStatus::Fail));
    }
}
