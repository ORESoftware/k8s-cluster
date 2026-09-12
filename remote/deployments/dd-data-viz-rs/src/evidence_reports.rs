use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    util::{clean_identifier, now_ms},
    QueryDialect, QueryRequest,
};

const MAX_REPORT_SECTIONS: usize = 32;
const MAX_REPORT_TITLE_BYTES: usize = 160;
const MAX_REPORT_SLUG_BYTES: usize = 96;
const MAX_SECTION_HEADING_BYTES: usize = 160;
const MAX_MARKDOWN_BYTES: usize = 8 * 1024;
const MAX_SQL_BYTES: usize = 8 * 1024;
const MAX_CHART_FIELDS: usize = 8;
const MAX_VARIABLES: usize = 32;
const MAX_VARIABLE_VALUE_BYTES: usize = 256;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompileEvidenceReportRequest {
    pub title: String,
    pub slug: Option<String>,
    pub owner: Option<String>,
    pub default_dataset_id: Option<String>,
    pub variables: Option<BTreeMap<String, String>>,
    pub sections: Vec<EvidenceSectionRequest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceSectionRequest {
    pub section_id: Option<String>,
    pub heading: String,
    #[serde(alias = "body")]
    pub markdown: Option<String>,
    pub sql: Option<String>,
    pub dataset_id: Option<String>,
    pub query_name: Option<String>,
    pub limit: Option<usize>,
    pub chart: Option<EvidenceChartRequest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceChartRequest {
    pub chart_type: EvidenceChartType,
    pub title: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
    pub color: Option<String>,
    pub fields: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EvidenceChartType {
    Bar,
    Line,
    Area,
    Scatter,
    Table,
    DataTable,
    BigValue,
}

#[derive(Debug, Clone)]
pub(crate) struct EvidenceQueryValidation {
    pub section_id: String,
    pub query_name: String,
    pub query: QueryRequest,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceQueryPlan {
    pub section_id: String,
    pub query_name: String,
    pub dataset_id: String,
    pub fields: BTreeMap<String, String>,
    pub logical_plan: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompileEvidenceReportResponse {
    ok: bool,
    schema_version: &'static str,
    report_id: String,
    title: String,
    slug: String,
    owner: Option<String>,
    compiled_at_ms: u128,
    section_count: usize,
    query_count: usize,
    chart_count: usize,
    markdown: String,
    sections: Vec<CompiledEvidenceSection>,
    queries: Vec<EvidenceQueryPlan>,
    dependencies: Vec<EvidenceReportDependency>,
    limits: Value,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompiledEvidenceSection {
    section_id: String,
    heading: String,
    has_query: bool,
    has_chart: bool,
    query_name: Option<String>,
    chart_type: Option<EvidenceChartType>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceReportDependency {
    section_id: String,
    query_name: String,
    dataset_id: String,
    fields: BTreeMap<String, String>,
}

pub(crate) fn query_validations(
    request: &CompileEvidenceReportRequest,
) -> Result<Vec<EvidenceQueryValidation>, String> {
    let report_defaults = normalize_report_header(request)?;
    let sections = normalize_sections(request, &report_defaults)?;
    let mut queries = Vec::new();
    for section in sections {
        let Some(sql) = section.sql else {
            continue;
        };
        let dataset_id = section
            .dataset_id
            .or_else(|| report_defaults.default_dataset_id.clone())
            .ok_or_else(|| {
                format!(
                    "section `{}` requires datasetId or report defaultDatasetId",
                    section.section_id
                )
            })?;
        queries.push(EvidenceQueryValidation {
            section_id: section.section_id.clone(),
            query_name: section
                .query_name
                .clone()
                .unwrap_or_else(|| evidence_binding_name(&section.section_id)),
            query: QueryRequest {
                dialect: QueryDialect::Sql,
                query: sql,
                dataset_id: Some(dataset_id),
                limit: Some(section.limit.unwrap_or(500)),
            },
        });
    }
    Ok(queries)
}

pub(crate) fn compile(
    request: CompileEvidenceReportRequest,
    query_plans: BTreeMap<String, EvidenceQueryPlan>,
) -> Result<CompileEvidenceReportResponse, String> {
    let report = normalize_report_header(&request)?;
    let sections = normalize_sections(&request, &report)?;
    let mut markdown = String::new();
    let mut warnings = Vec::new();
    let mut compiled_sections = Vec::new();
    let mut query_count = 0usize;
    let mut chart_count = 0usize;
    let compiled_at_ms = now_ms();

    markdown.push_str("---\n");
    markdown.push_str("title: \"");
    markdown.push_str(&yaml_escape(&report.title));
    markdown.push_str("\"\n");
    markdown.push_str("source: \"dd-data-viz-rs\"\n");
    markdown.push_str("schemaVersion: \"data-viz.evidence-report-compiled.v1\"\n");
    markdown.push_str("compiledAtMs: ");
    markdown.push_str(&compiled_at_ms.to_string());
    markdown.push('\n');
    if let Some(owner) = &report.owner {
        markdown.push_str("owner: \"");
        markdown.push_str(&yaml_escape(owner));
        markdown.push_str("\"\n");
    }
    if !report.variables.is_empty() {
        markdown.push_str("variables:\n");
        for (key, value) in &report.variables {
            markdown.push_str("  ");
            markdown.push_str(key);
            markdown.push_str(": \"");
            markdown.push_str(&yaml_escape(value));
            markdown.push_str("\"\n");
        }
    }
    markdown.push_str("---\n\n# ");
    markdown.push_str(&report.title);
    markdown.push_str("\n\n");

    for section in sections {
        markdown.push_str("## ");
        markdown.push_str(&section.heading);
        markdown.push_str("\n\n");
        if let Some(body) = &section.markdown {
            markdown.push_str(body);
            markdown.push_str("\n\n");
        }

        let mut query_name = None;
        if let Some(sql) = &section.sql {
            let plan = query_plans.get(&section.section_id).ok_or_else(|| {
                format!(
                    "compiled query plan missing for section `{}`",
                    section.section_id
                )
            })?;
            query_count += 1;
            query_name = Some(plan.query_name.clone());
            markdown.push_str("```sql ");
            markdown.push_str(&plan.query_name);
            markdown.push('\n');
            markdown.push_str(sql);
            markdown.push_str("\n```\n\n");
            markdown.push_str("<!-- logical-plan-source: ");
            markdown.push_str(&plan.dataset_id);
            markdown.push_str(" -->\n\n");
        }

        if let Some(chart) = &section.chart {
            chart_count += 1;
            if query_name.is_none() {
                warnings.push(format!(
                    "section `{}` has chart metadata without a SQL data block",
                    section.section_id
                ));
            }
            markdown.push_str(&chart_component(chart, query_name.as_deref()));
            markdown.push_str("\n\n");
        }

        compiled_sections.push(CompiledEvidenceSection {
            section_id: section.section_id,
            heading: section.heading,
            has_query: query_name.is_some(),
            has_chart: section.chart.is_some(),
            query_name,
            chart_type: section.chart.as_ref().map(|chart| chart.chart_type),
        });
    }
    let dependencies = query_plans
        .values()
        .map(|plan| EvidenceReportDependency {
            section_id: plan.section_id.clone(),
            query_name: plan.query_name.clone(),
            dataset_id: plan.dataset_id.clone(),
            fields: plan.fields.clone(),
        })
        .collect::<Vec<_>>();

    Ok(CompileEvidenceReportResponse {
        ok: true,
        schema_version: "data-viz.evidence-report-compiled.v1",
        report_id: report.slug.clone(),
        title: report.title,
        slug: report.slug,
        owner: report.owner,
        compiled_at_ms,
        section_count: compiled_sections.len(),
        query_count,
        chart_count,
        markdown,
        sections: compiled_sections,
        queries: query_plans.into_values().collect(),
        dependencies,
        limits: limits_payload(),
        warnings,
    })
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxReportSections": MAX_REPORT_SECTIONS,
        "maxReportTitleBytes": MAX_REPORT_TITLE_BYTES,
        "maxReportSlugBytes": MAX_REPORT_SLUG_BYTES,
        "maxSectionHeadingBytes": MAX_SECTION_HEADING_BYTES,
        "maxMarkdownBytes": MAX_MARKDOWN_BYTES,
        "maxSqlBytes": MAX_SQL_BYTES,
        "maxChartFields": MAX_CHART_FIELDS,
        "maxVariables": MAX_VARIABLES,
        "maxVariableValueBytes": MAX_VARIABLE_VALUE_BYTES
    })
}

#[derive(Debug, Clone)]
struct NormalizedReport {
    title: String,
    slug: String,
    owner: Option<String>,
    default_dataset_id: Option<String>,
    variables: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct NormalizedSection {
    section_id: String,
    heading: String,
    markdown: Option<String>,
    sql: Option<String>,
    dataset_id: Option<String>,
    query_name: Option<String>,
    limit: Option<usize>,
    chart: Option<EvidenceChartRequest>,
}

fn normalize_report_header(
    request: &CompileEvidenceReportRequest,
) -> Result<NormalizedReport, String> {
    let title = bounded_line("title", &request.title, MAX_REPORT_TITLE_BYTES)?;
    reject_secret_text("title", &title)?;
    let slug = match &request.slug {
        Some(slug) => clean_identifier(slug)
            .filter(|slug| slug.len() <= MAX_REPORT_SLUG_BYTES)
            .ok_or_else(|| "slug must be a safe identifier".to_string())?,
        None => slugify(&title)?,
    };
    let owner = request
        .owner
        .as_deref()
        .map(|owner| bounded_line("owner", owner, 120))
        .transpose()?;
    if let Some(owner) = &owner {
        reject_secret_text("owner", owner)?;
    }
    let default_dataset_id = request
        .default_dataset_id
        .as_deref()
        .map(|dataset_id| {
            clean_identifier(dataset_id)
                .ok_or_else(|| "defaultDatasetId must be a safe identifier".to_string())
        })
        .transpose()?;
    let variables = normalize_variables(request.variables.clone().unwrap_or_default())?;
    Ok(NormalizedReport {
        title,
        slug,
        owner,
        default_dataset_id,
        variables,
    })
}

fn normalize_sections(
    request: &CompileEvidenceReportRequest,
    report: &NormalizedReport,
) -> Result<Vec<NormalizedSection>, String> {
    if request.sections.is_empty() {
        return Err("Evidence report requires at least one section".to_string());
    }
    if request.sections.len() > MAX_REPORT_SECTIONS {
        return Err(format!("sections exceeds max {MAX_REPORT_SECTIONS}"));
    }

    request
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| normalize_section(section, report, index))
        .collect()
}

fn normalize_section(
    section: &EvidenceSectionRequest,
    report: &NormalizedReport,
    index: usize,
) -> Result<NormalizedSection, String> {
    let heading = bounded_line(
        "section heading",
        &section.heading,
        MAX_SECTION_HEADING_BYTES,
    )?;
    reject_secret_text("section heading", &heading)?;
    let section_id = match &section.section_id {
        Some(section_id) => clean_identifier(section_id)
            .ok_or_else(|| "sectionId must be a safe identifier".to_string())?,
        None => clean_identifier(&format!("section-{}", index + 1)).unwrap(),
    };
    let markdown = section
        .markdown
        .as_deref()
        .map(|markdown| bounded_text("section markdown", markdown, MAX_MARKDOWN_BYTES))
        .transpose()?;
    if let Some(markdown) = &markdown {
        reject_secret_text("section markdown", markdown)?;
        reject_markdown_fence("section markdown", markdown)?;
    }
    let sql = section
        .sql
        .as_deref()
        .map(|sql| bounded_text("section SQL", sql, MAX_SQL_BYTES))
        .transpose()?;
    if let Some(sql) = &sql {
        reject_secret_text("section SQL", sql)?;
        if sql.contains(';') || sql.contains("--") || sql.contains("/*") || sql.contains("```") {
            return Err(format!(
                "section `{section_id}` SQL cannot contain comments or statement separators"
            ));
        }
    }
    let dataset_id = section
        .dataset_id
        .as_deref()
        .map(|dataset_id| {
            clean_identifier(dataset_id).ok_or_else(|| "datasetId is invalid".to_string())
        })
        .transpose()?
        .or_else(|| report.default_dataset_id.clone());
    let query_name = section
        .query_name
        .as_deref()
        .map(|query_name| {
            clean_identifier(query_name)
                .map(|query_name| evidence_binding_name(&query_name))
                .ok_or_else(|| "queryName must be a safe identifier".to_string())
        })
        .transpose()?;
    let chart = section.chart.clone().map(normalize_chart).transpose()?;
    Ok(NormalizedSection {
        section_id,
        heading,
        markdown,
        sql,
        dataset_id,
        query_name,
        limit: section.limit,
        chart,
    })
}

fn normalize_chart(chart: EvidenceChartRequest) -> Result<EvidenceChartRequest, String> {
    let title = chart
        .title
        .as_deref()
        .map(|title| bounded_line("chart title", title, 160))
        .transpose()?;
    if let Some(title) = &title {
        reject_secret_text("chart title", title)?;
    }
    let x = normalize_optional_field(chart.x, "x")?;
    let y = normalize_optional_field(chart.y, "y")?;
    let color = normalize_optional_field(chart.color, "color")?;
    let fields = chart
        .fields
        .unwrap_or_default()
        .into_iter()
        .map(|field| {
            clean_identifier(&field).ok_or_else(|| format!("chart field `{field}` is invalid"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if fields.len() > MAX_CHART_FIELDS {
        return Err(format!("chart fields exceeds max {MAX_CHART_FIELDS}"));
    }
    Ok(EvidenceChartRequest {
        chart_type: chart.chart_type,
        title,
        x,
        y,
        color,
        fields: Some(fields),
    })
}

fn normalize_optional_field(
    value: Option<String>,
    label: &'static str,
) -> Result<Option<String>, String> {
    value
        .map(|value| {
            clean_identifier(&value).ok_or_else(|| format!("chart {label} field is invalid"))
        })
        .transpose()
}

fn normalize_variables(
    variables: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    if variables.len() > MAX_VARIABLES {
        return Err(format!("variables exceeds max {MAX_VARIABLES}"));
    }
    let mut normalized = BTreeMap::new();
    for (key, value) in variables {
        let key =
            clean_identifier(&key).ok_or_else(|| format!("variable key `{key}` is invalid"))?;
        let value = bounded_line("variable value", &value, MAX_VARIABLE_VALUE_BYTES)?;
        reject_secret_text("variable value", &value)?;
        normalized.insert(key, value);
    }
    Ok(normalized)
}

fn evidence_binding_name(input: &str) -> String {
    let mut output = String::new();
    let mut previous_underscore = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            output.push(ch);
            previous_underscore = false;
        } else if !previous_underscore {
            output.push('_');
            previous_underscore = true;
        }
    }
    let output = output.trim_matches('_');
    if output.is_empty() {
        "query".to_string()
    } else if output
        .chars()
        .next()
        .map(|ch| ch.is_ascii_digit())
        .unwrap_or(false)
    {
        format!("q_{output}")
    } else {
        output.to_string()
    }
}

fn chart_component(chart: &EvidenceChartRequest, query_name: Option<&str>) -> String {
    let component = match chart.chart_type {
        EvidenceChartType::Bar => "BarChart",
        EvidenceChartType::Line => "LineChart",
        EvidenceChartType::Area => "AreaChart",
        EvidenceChartType::Scatter => "ScatterPlot",
        EvidenceChartType::Table => "Table",
        EvidenceChartType::DataTable => "DataTable",
        EvidenceChartType::BigValue => "BigValue",
    };
    let mut output = String::new();
    output.push('<');
    output.push_str(component);
    if let Some(query_name) = query_name {
        output.push_str(" data={");
        output.push_str(query_name);
        output.push('}');
    }
    if let Some(title) = &chart.title {
        output.push_str(" title=\"");
        output.push_str(&xml_escape(title));
        output.push('"');
    }
    if let Some(x) = &chart.x {
        output.push_str(" x=\"");
        output.push_str(x);
        output.push('"');
    }
    if let Some(y) = &chart.y {
        output.push_str(" y=\"");
        output.push_str(y);
        output.push('"');
    }
    if let Some(color) = &chart.color {
        output.push_str(" color=\"");
        output.push_str(color);
        output.push('"');
    }
    output.push_str(" />");
    output
}

fn bounded_text(label: &str, value: &str, max_bytes: usize) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.len() > max_bytes {
        Err(format!("{label} must be 1-{max_bytes} bytes"))
    } else {
        Ok(value)
    }
}

fn bounded_line(label: &str, value: &str, max_bytes: usize) -> Result<String, String> {
    let value = bounded_text(label, value, max_bytes)?;
    if value.contains('\n') || value.contains('\r') {
        Err(format!("{label} cannot contain line breaks"))
    } else {
        Ok(value)
    }
}

fn slugify(title: &str) -> Result<String, String> {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= MAX_REPORT_SLUG_BYTES {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        Err("slug could not be derived from title".to_string())
    } else {
        Ok(slug)
    }
}

fn reject_secret_text(label: &str, value: &str) -> Result<(), String> {
    if looks_secret_bearing(value) {
        Err(format!("{label} contains secret-looking text"))
    } else {
        Ok(())
    }
}

fn reject_markdown_fence(label: &str, value: &str) -> Result<(), String> {
    if value.contains("```") {
        Err(format!("{label} cannot contain fenced code blocks"))
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

fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CompileEvidenceReportRequest {
        CompileEvidenceReportRequest {
            title: "Revenue Review".to_string(),
            slug: None,
            owner: Some("analytics".to_string()),
            default_dataset_id: Some("sales-lab".to_string()),
            variables: Some(BTreeMap::from([(
                "region".to_string(),
                "north".to_string(),
            )])),
            sections: vec![EvidenceSectionRequest {
                section_id: Some("regional-revenue".to_string()),
                heading: "Regional revenue".to_string(),
                markdown: Some("Revenue by region.".to_string()),
                sql: Some(
                    "SELECT region, SUM(revenue) AS totalRevenue FROM sales-lab GROUP BY region"
                        .to_string(),
                ),
                dataset_id: None,
                query_name: Some("regional_revenue".to_string()),
                limit: Some(100),
                chart: Some(EvidenceChartRequest {
                    chart_type: EvidenceChartType::Bar,
                    title: Some("Revenue by region".to_string()),
                    x: Some("region".to_string()),
                    y: Some("totalRevenue".to_string()),
                    color: None,
                    fields: None,
                }),
            }],
        }
    }

    #[test]
    fn evidence_report_compiles_markdown_with_query_and_chart() {
        let validations = query_validations(&request()).expect("queries");
        assert_eq!(validations.len(), 1);
        assert_eq!(validations[0].query.dialect, QueryDialect::Sql);
        assert_eq!(validations[0].query_name, "regional_revenue");

        let response = compile(
            request(),
            BTreeMap::from([(
                "regional-revenue".to_string(),
                EvidenceQueryPlan {
                    section_id: "regional-revenue".to_string(),
                    query_name: "regional_revenue".to_string(),
                    dataset_id: "sales-lab".to_string(),
                    fields: BTreeMap::from([
                        ("region".to_string(), "string".to_string()),
                        ("revenue".to_string(), "number".to_string()),
                    ]),
                    logical_plan: json!({ "source": "sales-lab" }),
                },
            )]),
        )
        .expect("compiled");

        assert_eq!(response.report_id, "revenue-review");
        assert_eq!(response.query_count, 1);
        assert_eq!(response.chart_count, 1);
        assert_eq!(response.dependencies[0].dataset_id, "sales-lab");
        assert!(response.markdown.contains("```sql regional_revenue"));
        assert!(response
            .markdown
            .contains("<BarChart data={regional_revenue}"));
    }

    #[test]
    fn evidence_report_rejects_secret_like_markdown() {
        let mut request = request();
        request.sections[0].markdown = Some("token=abc".to_string());

        let error = query_validations(&request).expect_err("secret markdown rejected");
        assert!(error.contains("secret-looking"));
    }

    #[test]
    fn evidence_report_rejects_secret_like_variables() {
        let mut request = request();
        request.variables = Some(BTreeMap::from([(
            "region".to_string(),
            "bearer abc".to_string(),
        )]));

        let error = query_validations(&request).expect_err("secret variable rejected");
        assert!(error.contains("secret-looking"));
    }

    #[test]
    fn evidence_report_requires_dataset_for_sql() {
        let mut request = request();
        request.default_dataset_id = None;

        let error = query_validations(&request).expect_err("dataset required");
        assert!(error.contains("requires datasetId"));
    }

    #[test]
    fn evidence_report_fallback_query_name_is_component_safe() {
        let mut request = request();
        request.sections[0].query_name = None;

        let validations = query_validations(&request).expect("validations");

        assert_eq!(validations[0].query_name, "regional_revenue");
    }
}
