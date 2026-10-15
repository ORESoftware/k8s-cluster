use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::AggregationOp;
use crate::self_service::{
    QuestionAggregation, QuestionBuilder, QuestionChartEncoding, QuestionChartSpec,
};
use crate::util::clean_identifier;

const MAX_PROMPT_BYTES: usize = 512;
const MAX_SUGGESTIONS: usize = 12;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NaturalLanguageQuestionRequest {
    pub dataset_id: String,
    pub prompt: String,
    pub max_suggestions: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionProposal {
    proposal_id: String,
    title: String,
    interpreted_intent: String,
    confidence: f64,
    query: QuestionBuilder,
    chart: Option<QuestionChartSpec>,
    rationale: String,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuestionProposalResponse {
    ok: bool,
    schema_version: &'static str,
    dataset_id: String,
    proposals: Vec<QuestionProposal>,
    limits: Value,
}

pub(crate) fn suggestions_for_dataset(
    dataset_id: &str,
    field_catalog: &BTreeMap<String, String>,
    max_suggestions: Option<usize>,
) -> Result<QuestionProposalResponse, String> {
    let dataset_id = clean_identifier(dataset_id)
        .ok_or_else(|| "datasetId must be a safe identifier".to_string())?;
    let max_suggestions = max_suggestions.unwrap_or(6).clamp(1, MAX_SUGGESTIONS);
    let fields = FieldChoices::from_catalog(field_catalog);
    let mut proposals = Vec::new();

    if let (Some(measure), Some(dimension)) = (fields.numeric.first(), fields.groupable.first()) {
        proposals.push(aggregate_proposal(
            &dataset_id,
            "total",
            AggregationOp::Sum,
            measure,
            Some(dimension),
            0.92,
            "Aggregate a numeric measure by the most useful categorical field.",
        ));
        proposals.push(aggregate_proposal(
            &dataset_id,
            "average",
            AggregationOp::Avg,
            measure,
            Some(dimension),
            0.86,
            "Compare central tendency across categories.",
        ));
    }

    if let Some(dimension) = fields.groupable.first() {
        proposals.push(aggregate_proposal(
            &dataset_id,
            "count",
            AggregationOp::Count,
            "",
            Some(dimension),
            0.82,
            "Count records by category for a quick distribution check.",
        ));
    }

    if let Some(measure) = fields.numeric.first() {
        proposals.push(aggregate_proposal(
            &dataset_id,
            "maximum",
            AggregationOp::Max,
            measure,
            None,
            0.78,
            "Surface the highest observed value for a numeric measure.",
        ));
    }

    if !fields.all.is_empty() {
        let selected = fields.all.iter().take(6).cloned().collect::<Vec<_>>();
        proposals.push(QuestionProposal {
            proposal_id: format!("{dataset_id}:detail-table"),
            title: "Detail rows".to_string(),
            interpreted_intent: "list rows".to_string(),
            confidence: 0.74,
            query: QuestionBuilder {
                fields: Some(selected.clone()),
                filters: None,
                group_by: None,
                aggregations: None,
                limit: Some(100),
            },
            chart: Some(QuestionChartSpec {
                chart_id: Some(format!("{dataset_id}:detail-table:chart")),
                title: Some("Detail rows".to_string()),
                mark: "table".to_string(),
                encodings: selected
                    .first()
                    .map(|field| {
                        vec![QuestionChartEncoding {
                            channel: "text".to_string(),
                            field: field.clone(),
                        }]
                    })
                    .unwrap_or_default(),
            }),
            rationale: "Show bounded detail rows for inspection before choosing an aggregate."
                .to_string(),
            warnings: Vec::new(),
        });
    }

    proposals.truncate(max_suggestions);
    Ok(response(dataset_id, proposals))
}

pub(crate) fn propose_from_prompt(
    request: NaturalLanguageQuestionRequest,
    field_catalog: &BTreeMap<String, String>,
) -> Result<QuestionProposalResponse, String> {
    let dataset_id = clean_identifier(&request.dataset_id)
        .ok_or_else(|| "datasetId must be a safe identifier".to_string())?;
    let prompt = normalize_prompt(&request.prompt)?;
    let fields = FieldChoices::from_catalog(field_catalog);
    if fields.all.is_empty() {
        return Err("question suggestions require at least one dataset field".to_string());
    }
    let max_suggestions = request
        .max_suggestions
        .unwrap_or(1)
        .clamp(1, MAX_SUGGESTIONS);
    let prompt_lower = prompt.to_ascii_lowercase();
    let aggregation = aggregation_from_prompt(&prompt_lower);
    let dimension = choose_field(&prompt_lower, &fields.groupable);
    let measure =
        choose_field(&prompt_lower, &fields.numeric).or_else(|| fields.numeric.first().cloned());

    let mut warnings = Vec::new();
    let proposal = if wants_detail_rows(&prompt_lower) && aggregation.is_none() {
        detail_proposal(&dataset_id, &fields, &prompt)
    } else {
        let op = aggregation.unwrap_or_else(|| {
            if measure.is_some() {
                AggregationOp::Sum
            } else {
                AggregationOp::Count
            }
        });
        let measure = if op == AggregationOp::Count {
            measure.unwrap_or_default()
        } else {
            measure.ok_or_else(|| {
                "natural-language aggregate questions need a numeric field".to_string()
            })?
        };
        let dimension = dimension.or_else(|| fields.groupable.first().cloned());
        if dimension.is_none() {
            warnings.push(
                "no categorical grouping field was detected; returning a metric proposal"
                    .to_string(),
            );
        }
        let mut proposal = aggregate_proposal(
            &dataset_id,
            aggregation_label(op),
            op,
            &measure,
            dimension.as_deref(),
            0.7,
            "Prompt mapped to a bounded self-service question proposal.",
        );
        proposal.title = title_from_prompt(&prompt, &proposal.title);
        proposal.interpreted_intent = prompt;
        proposal.warnings.extend(warnings);
        proposal
    };

    let mut proposals = vec![proposal];
    if proposals.len() < max_suggestions {
        let mut fallback =
            suggestions_for_dataset(&dataset_id, field_catalog, Some(max_suggestions))?.proposals;
        proposals.append(&mut fallback);
        dedupe_proposals(&mut proposals);
        proposals.truncate(max_suggestions);
    }
    Ok(response(dataset_id, proposals))
}

pub(crate) fn limits_payload() -> Value {
    json!({
        "maxPromptBytes": MAX_PROMPT_BYTES,
        "maxSuggestions": MAX_SUGGESTIONS,
        "posture": "deterministic parser over dataset field names; no model calls and no query execution"
    })
}

fn response(dataset_id: String, proposals: Vec<QuestionProposal>) -> QuestionProposalResponse {
    QuestionProposalResponse {
        ok: true,
        schema_version: "data-viz.question-nl.v1",
        dataset_id,
        proposals,
        limits: limits_payload(),
    }
}

fn aggregate_proposal(
    dataset_id: &str,
    label: &str,
    op: AggregationOp,
    measure: &str,
    dimension: Option<&str>,
    confidence: f64,
    rationale: &str,
) -> QuestionProposal {
    let alias = if op == AggregationOp::Count {
        "record_count".to_string()
    } else {
        format!("{}_{}", label, measure)
    };
    let group_by = dimension.map(|field| vec![field.to_string()]);
    let mut output_fields = group_by.clone().unwrap_or_default();
    output_fields.push(alias.clone());
    let mark = if dimension.is_some() { "bar" } else { "metric" };
    let encodings = if let Some(dimension) = dimension {
        vec![
            QuestionChartEncoding {
                channel: "x".to_string(),
                field: dimension.to_string(),
            },
            QuestionChartEncoding {
                channel: "y".to_string(),
                field: alias.clone(),
            },
        ]
    } else {
        vec![QuestionChartEncoding {
            channel: "value".to_string(),
            field: alias.clone(),
        }]
    };
    let title = if let Some(dimension) = dimension {
        if op == AggregationOp::Count || measure.is_empty() {
            format!("{} records by {}", title_word(label), dimension)
        } else {
            format!("{} {} by {}", title_word(label), measure, dimension)
        }
    } else {
        format!("{} {}", title_word(label), measure_or_records(measure, op))
    };
    QuestionProposal {
        proposal_id: format!(
            "{dataset_id}:{}:{}:{}",
            label,
            if measure.is_empty() {
                "records"
            } else {
                measure
            },
            dimension.unwrap_or("metric")
        ),
        title: title.clone(),
        interpreted_intent: title.to_ascii_lowercase(),
        confidence,
        query: QuestionBuilder {
            fields: None,
            filters: None,
            group_by,
            aggregations: Some(vec![QuestionAggregation {
                alias,
                op,
                field: if op == AggregationOp::Count || measure.is_empty() {
                    None
                } else {
                    Some(measure.to_string())
                },
            }]),
            limit: Some(100),
        },
        chart: Some(QuestionChartSpec {
            chart_id: Some(format!("{dataset_id}:{}:chart", slug_title(&title))),
            title: Some(title),
            mark: mark.to_string(),
            encodings,
        }),
        rationale: rationale.to_string(),
        warnings: if output_fields.is_empty() {
            vec!["proposal has no output fields".to_string()]
        } else {
            Vec::new()
        },
    }
}

fn detail_proposal(dataset_id: &str, fields: &FieldChoices, prompt: &str) -> QuestionProposal {
    let selected = fields.all.iter().take(6).cloned().collect::<Vec<_>>();
    QuestionProposal {
        proposal_id: format!("{dataset_id}:nl-detail"),
        title: title_from_prompt(prompt, "Detail rows"),
        interpreted_intent: prompt.to_string(),
        confidence: 0.72,
        query: QuestionBuilder {
            fields: Some(selected.clone()),
            filters: None,
            group_by: None,
            aggregations: None,
            limit: Some(100),
        },
        chart: Some(QuestionChartSpec {
            chart_id: Some(format!("{dataset_id}:nl-detail:chart")),
            title: Some("Detail rows".to_string()),
            mark: "table".to_string(),
            encodings: selected
                .first()
                .map(|field| {
                    vec![QuestionChartEncoding {
                        channel: "text".to_string(),
                        field: field.clone(),
                    }]
                })
                .unwrap_or_default(),
        }),
        rationale: "Prompt asked for records or rows, so the proposal stays as a bounded table."
            .to_string(),
        warnings: Vec::new(),
    }
}

fn normalize_prompt(prompt: &str) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(format!("prompt must be 1-{MAX_PROMPT_BYTES} bytes"));
    }
    let lower = prompt.to_ascii_lowercase();
    for marker in [
        "password",
        "passwd",
        "secret",
        "token",
        "api_key",
        "private_key",
        "credential",
    ] {
        if lower.contains(marker) {
            return Err("prompt looks secret-bearing; do not send credentials".to_string());
        }
    }
    Ok(prompt.to_string())
}

fn aggregation_from_prompt(prompt: &str) -> Option<AggregationOp> {
    if prompt.contains("average") || prompt.contains("avg") || prompt.contains("mean") {
        Some(AggregationOp::Avg)
    } else if prompt.contains("count")
        || prompt.contains("how many")
        || prompt.contains("number of")
    {
        Some(AggregationOp::Count)
    } else if prompt.contains("minimum") || prompt.contains("lowest") || prompt.contains("min") {
        Some(AggregationOp::Min)
    } else if prompt.contains("maximum") || prompt.contains("highest") || prompt.contains("max") {
        Some(AggregationOp::Max)
    } else if prompt.contains("total") || prompt.contains("sum") {
        Some(AggregationOp::Sum)
    } else {
        None
    }
}

fn wants_detail_rows(prompt: &str) -> bool {
    prompt.contains("show rows")
        || prompt.contains("list rows")
        || prompt.contains("records")
        || prompt.contains("details")
}

fn choose_field(prompt: &str, fields: &[String]) -> Option<String> {
    fields
        .iter()
        .find(|field| prompt.contains(&field.to_ascii_lowercase().replace('_', " ")))
        .cloned()
        .or_else(|| {
            fields
                .iter()
                .find(|field| prompt.contains(&field.to_ascii_lowercase()))
                .cloned()
        })
}

fn title_from_prompt(prompt: &str, fallback: &str) -> String {
    let trimmed = prompt.trim_matches(|ch: char| ch == '?' || ch == '.' || ch.is_whitespace());
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        let mut chars = trimmed.chars();
        match chars.next() {
            Some(first) => format!("{}{}", first.to_uppercase(), chars.collect::<String>()),
            None => fallback.to_string(),
        }
    }
}

fn slug_title(title: &str) -> String {
    clean_identifier(&title.to_ascii_lowercase().replace(' ', "-"))
        .unwrap_or_else(|| "question".to_string())
}

fn aggregation_label(op: AggregationOp) -> &'static str {
    match op {
        AggregationOp::Count => "count",
        AggregationOp::Sum => "total",
        AggregationOp::Avg => "average",
        AggregationOp::Min => "minimum",
        AggregationOp::Max => "maximum",
    }
}

fn title_word(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.collect::<String>()),
        None => label.to_string(),
    }
}

fn measure_or_records(measure: &str, op: AggregationOp) -> &str {
    if op == AggregationOp::Count || measure.is_empty() {
        "records"
    } else {
        measure
    }
}

fn dedupe_proposals(proposals: &mut Vec<QuestionProposal>) {
    let mut seen = std::collections::BTreeSet::new();
    proposals.retain(|proposal| seen.insert(proposal.proposal_id.clone()));
}

struct FieldChoices {
    all: Vec<String>,
    numeric: Vec<String>,
    groupable: Vec<String>,
}

impl FieldChoices {
    fn from_catalog(field_catalog: &BTreeMap<String, String>) -> Self {
        let all = field_catalog.keys().cloned().collect::<Vec<_>>();
        let mut numeric = field_catalog
            .iter()
            .filter(|(_, data_type)| data_type.as_str() == "number")
            .map(|(field, _)| field.clone())
            .collect::<Vec<_>>();
        numeric.sort_by_key(|field| (numeric_priority(field), field.clone()));
        let mut groupable = field_catalog
            .iter()
            .filter(|(_, data_type)| {
                matches!(
                    data_type.as_str(),
                    "category" | "boolean" | "bool" | "string"
                )
            })
            .map(|(field, _)| field.clone())
            .collect::<Vec<_>>();
        groupable.sort_by_key(|field| (groupable_priority(field), field.clone()));
        Self {
            all,
            numeric,
            groupable,
        }
    }
}

fn numeric_priority(field: &str) -> u8 {
    let field = field.to_ascii_lowercase();
    if field.contains("revenue") || field.contains("sales") || field.contains("amount") {
        0
    } else if field.contains("profit") || field.contains("margin") {
        1
    } else if field.contains("cost") || field.contains("price") {
        2
    } else {
        3
    }
}

fn groupable_priority(field: &str) -> u8 {
    let field = field.to_ascii_lowercase();
    if field.contains("region") || field.contains("country") || field.contains("market") {
        0
    } else if field.contains("segment") || field.contains("category") {
        1
    } else if field.contains("status") || field.contains("type") {
        2
    } else {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("region".to_string(), "category".to_string()),
            ("segment".to_string(), "category".to_string()),
            ("revenue".to_string(), "number".to_string()),
            ("margin".to_string(), "number".to_string()),
        ])
    }

    #[test]
    fn suggestions_generate_question_builder_contracts() {
        let response =
            suggestions_for_dataset("sales-lab", &fields(), Some(3)).expect("suggestions build");

        assert_eq!(response.proposals.len(), 3);
        assert_eq!(response.proposals[0].title, "Total revenue by region");
        assert!(response.proposals.iter().any(|proposal| {
            proposal
                .query
                .aggregations
                .as_ref()
                .is_some_and(|aggregations| aggregations[0].op == AggregationOp::Sum)
        }));
    }

    #[test]
    fn natural_language_prompt_maps_to_average_by_dimension() {
        let response = propose_from_prompt(
            NaturalLanguageQuestionRequest {
                dataset_id: "sales-lab".to_string(),
                prompt: "average margin by region".to_string(),
                max_suggestions: Some(1),
            },
            &fields(),
        )
        .expect("prompt parses");
        let proposal = &response.proposals[0];

        assert_eq!(
            proposal.query.group_by.as_ref().expect("group by"),
            &vec!["region".to_string()]
        );
        let aggregation = &proposal.query.aggregations.as_ref().expect("aggregation")[0];
        assert_eq!(aggregation.op, AggregationOp::Avg);
        assert_eq!(aggregation.field.as_deref(), Some("margin"));
    }

    #[test]
    fn natural_language_prompt_rejects_secret_like_text() {
        let error = propose_from_prompt(
            NaturalLanguageQuestionRequest {
                dataset_id: "sales-lab".to_string(),
                prompt: "show api_key by region".to_string(),
                max_suggestions: None,
            },
            &fields(),
        )
        .expect_err("secret-like prompt rejected");

        assert!(error.contains("secret-bearing"));
    }
}
