use crate::types::{AnalysisResult, WhitePaperRequest};

pub(crate) fn markdown_brief(
    request: &WhitePaperRequest,
    result: &AnalysisResult,
    record_count: usize,
) -> String {
    let title = request
        .title
        .clone()
        .unwrap_or_else(|| "Public Data Evidence Brief".to_string());
    let mut lines = vec![
        format!("# {title}"),
        String::new(),
        format!("Research question: {}", request.research_question.trim()),
        String::new(),
        format!(
            "Evidence base: {record_count} normalized records across {} datasets.",
            result.dataset_ids.len()
        ),
        String::new(),
        "## Candidate Trends".to_string(),
    ];
    if result.trends.is_empty() {
        lines.push("- No numeric trend had enough points yet.".to_string());
    } else {
        for trend in result.trends.iter().take(12) {
            lines.push(format!(
                "- `{}` is `{}` with slope {:.4}, mean {:.4}, range {:.4}..{:.4} across {} points.",
                trend.metric,
                trend.direction,
                trend.slope_per_record,
                trend.mean,
                trend.min,
                trend.max,
                trend.count
            ));
        }
    }
    lines.push(String::new());
    lines.push("## Candidate Correlations".to_string());
    if result.correlations.is_empty() {
        lines.push("- No metric pair had enough paired observations yet.".to_string());
    } else {
        for correlation in result.correlations.iter().take(12) {
            lines.push(format!(
                "- `{}` vs `{}`: Pearson {:.4} ({}, n={}).",
                correlation.left_metric,
                correlation.right_metric,
                correlation.pearson,
                correlation.strength,
                correlation.count
            ));
        }
    }
    if !result.grants.is_empty() {
        lines.push(String::new());
        lines.push("## Grant Opportunities".to_string());
        for grant in result.grants.iter().take(12) {
            lines.push(format!(
                "- `{}` score {:.2}; agency={}; program={}; amount={}.",
                grant.title,
                grant.score,
                grant
                    .agency
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                grant
                    .program
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                grant
                    .amount
                    .map(|amount| format!("{amount:.0}"))
                    .unwrap_or_else(|| "unknown".to_string())
            ));
        }
    }
    lines.push(String::new());
    lines.push("## Model Notes".to_string());
    for note in &result.model_notes {
        lines.push(format!(
            "- {}: `{}`. {}",
            note.name, note.equation, note.use_case
        ));
    }
    lines.push(String::new());
    lines.push("This brief is generated evidence for internal research review. Correlations are not causal claims until validated against domain assumptions, confounders, and source quality.".to_string());
    lines.join("\n")
}
