use std::collections::BTreeSet;

use serde_json::json;

use crate::nats::{publish_json, publish_runtime_event};
use crate::state::{AppState, MAX_GRAPH_POINTS, SERVICE_NAME};
use crate::types::{
    AnalysisResult, CorrelationSummary, DataRecord, GrantMatch, GraphData, GraphPoint, GraphSeries,
    ModelNote, TrendSummary,
};
use crate::util::{clean_text, durable_token, now_ms};

pub(crate) fn metric_universe(records: &[DataRecord], requested: &Option<Vec<String>>) -> Vec<String> {
    if let Some(metrics) = requested {
        return metrics
            .iter()
            .filter_map(|metric| clean_text(Some(metric), 80))
            .collect();
    }
    let mut names = BTreeSet::new();
    for record in records {
        names.extend(record.metrics.keys().cloned());
    }
    names.into_iter().collect()
}

pub(crate) fn trend_summaries(records: &[DataRecord], requested: &Option<Vec<String>>) -> Vec<TrendSummary> {
    let mut trends = Vec::new();
    for metric in metric_universe(records, requested) {
        let values = records
            .iter()
            .filter_map(|record| record.metrics.get(&metric).copied())
            .filter(|value| value.is_finite())
            .collect::<Vec<_>>();
        if values.len() < 2 {
            continue;
        }
        let count = values.len();
        let mean = values.iter().sum::<f64>() / count as f64;
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let slope = simple_slope(&values);
        let direction = if slope.abs() < 1e-9 {
            "flat"
        } else if slope > 0.0 {
            "up"
        } else {
            "down"
        };
        trends.push(TrendSummary {
            metric,
            count,
            mean,
            min,
            max,
            slope_per_record: slope,
            direction: direction.to_string(),
        });
    }
    trends.sort_by(|left, right| {
        right
            .slope_per_record
            .abs()
            .partial_cmp(&left.slope_per_record.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    trends
}

pub(crate) fn simple_slope(values: &[f64]) -> f64 {
    let n = values.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = values.iter().sum::<f64>() / n;
    let mut numerator = 0.0;
    let mut denominator = 0.0;
    for (index, value) in values.iter().enumerate() {
        let x = index as f64;
        numerator += (x - mean_x) * (value - mean_y);
        denominator += (x - mean_x).powi(2);
    }
    if denominator.abs() < f64::EPSILON {
        0.0
    } else {
        numerator / denominator
    }
}

pub(crate) fn correlation_summaries(
    records: &[DataRecord],
    requested: &Option<Vec<String>>,
) -> Vec<CorrelationSummary> {
    let metrics = metric_universe(records, requested);
    let mut out = Vec::new();
    for left_index in 0..metrics.len() {
        for right_index in (left_index + 1)..metrics.len() {
            let left = &metrics[left_index];
            let right = &metrics[right_index];
            let pairs = records
                .iter()
                .filter_map(|record| {
                    Some((
                        record.metrics.get(left).copied()?,
                        record.metrics.get(right).copied()?,
                    ))
                })
                .filter(|(a, b)| a.is_finite() && b.is_finite())
                .collect::<Vec<_>>();
            if pairs.len() < 3 {
                continue;
            }
            let pearson = pearson(&pairs);
            out.push(CorrelationSummary {
                left_metric: left.clone(),
                right_metric: right.clone(),
                count: pairs.len(),
                pearson,
                strength: correlation_strength(pearson).to_string(),
            });
        }
    }
    out.sort_by(|left, right| {
        right
            .pearson
            .abs()
            .partial_cmp(&left.pearson.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

pub(crate) fn pearson(pairs: &[(f64, f64)]) -> f64 {
    let n = pairs.len() as f64;
    let mean_x = pairs.iter().map(|pair| pair.0).sum::<f64>() / n;
    let mean_y = pairs.iter().map(|pair| pair.1).sum::<f64>() / n;
    let mut numerator = 0.0;
    let mut left_sum = 0.0;
    let mut right_sum = 0.0;
    for (left, right) in pairs {
        let dx = left - mean_x;
        let dy = right - mean_y;
        numerator += dx * dy;
        left_sum += dx * dx;
        right_sum += dy * dy;
    }
    let denominator = left_sum.sqrt() * right_sum.sqrt();
    if denominator.abs() < f64::EPSILON {
        0.0
    } else {
        (numerator / denominator).clamp(-1.0, 1.0)
    }
}

pub(crate) fn correlation_strength(value: f64) -> &'static str {
    let abs = value.abs();
    if abs >= 0.85 {
        "very-strong"
    } else if abs >= 0.65 {
        "strong"
    } else if abs >= 0.40 {
        "moderate"
    } else if abs >= 0.20 {
        "weak"
    } else {
        "very-weak"
    }
}

pub(crate) fn graph_from_trends(trends: &[TrendSummary], records: &[DataRecord]) -> GraphData {
    let series = trends
        .iter()
        .take(8)
        .map(|trend| {
            let points = records
                .iter()
                .filter_map(|record| {
                    Some(GraphPoint {
                        x: record.collected_at_ms as f64,
                        y: *record.metrics.get(&trend.metric)?,
                        label: record.title.clone(),
                    })
                })
                .take(MAX_GRAPH_POINTS)
                .collect::<Vec<_>>();
            GraphSeries {
                name: trend.metric.clone(),
                points,
            }
        })
        .collect();
    GraphData {
        graph_type: "line".to_string(),
        title: "Public data metric trends".to_string(),
        x_label: "collectedAtMs".to_string(),
        y_label: "metricValue".to_string(),
        series,
    }
}

pub(crate) fn model_notes() -> Vec<ModelNote> {
    vec![
        ModelNote {
            name: "Ordinary Least Squares Trend".to_string(),
            equation: "y_t = alpha + beta t + epsilon_t".to_string(),
            use_case: "Estimate first-pass direction and slope for normalized public metrics."
                .to_string(),
        },
        ModelNote {
            name: "Pearson Correlation".to_string(),
            equation: "rho_xy = cov(x,y) / (sigma_x sigma_y)".to_string(),
            use_case: "Identify candidate relationships for later causal review, not causal proof."
                .to_string(),
        },
        ModelNote {
            name: "Evidence-Weighted Grant Fit".to_string(),
            equation: "score = topic_overlap + source_prior + amount_fit + eligibility_fit"
                .to_string(),
            use_case:
                "Rank grant opportunities against a declared applicant profile and focus areas."
                    .to_string(),
        },
    ]
}

pub(crate) fn build_analysis_result(
    kind: &str,
    request_id: String,
    records: Vec<DataRecord>,
    requested_metrics: Option<Vec<String>>,
    grants: Vec<GrantMatch>,
    markdown: Option<String>,
) -> AnalysisResult {
    let dataset_ids = records
        .iter()
        .map(|record| record.dataset_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let trends = trend_summaries(&records, &requested_metrics);
    let correlations = correlation_summaries(&records, &requested_metrics);
    let summary = format!(
        "Analyzed {} records across {} datasets; {} trends and {} metric correlations qualified.",
        records.len(),
        dataset_ids.len(),
        trends.len(),
        correlations.len()
    );
    let graph = graph_from_trends(&trends, &records);
    AnalysisResult {
        analysis_id: durable_token(
            "public-data-analysis",
            kind,
            &format!("{}-{request_id}", now_ms()),
        ),
        request_id,
        kind: kind.to_string(),
        generated_at_ms: now_ms(),
        dataset_ids,
        summary,
        graph,
        trends,
        correlations,
        grants,
        model_notes: model_notes(),
        markdown,
    }
}

pub(crate) async fn publish_analysis(state: &AppState, result: &AnalysisResult) {
    publish_json(
        state,
        &state.config.analysis_result_subject,
        &json!({
            "schemaVersion": "public_data.analysis.v1",
            "source": SERVICE_NAME,
            "result": result
        }),
    )
    .await;
    publish_runtime_event(
        state,
        "public_data.analysis",
        json!({ "analysisId": result.analysis_id, "kind": result.kind }),
    )
    .await;
}
