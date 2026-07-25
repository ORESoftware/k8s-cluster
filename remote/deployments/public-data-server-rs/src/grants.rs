use crate::types::{DataRecord, GrantMatch, GrantMatchRequest};
use crate::util::clean_tags;

pub(crate) fn grant_matches_from_records(
    records: &[DataRecord],
    request: &GrantMatchRequest,
) -> Vec<GrantMatch> {
    let focus = clean_tags(request.focus_areas.clone());
    let profile_terms = clean_tags(
        request
            .applicant_profile
            .split_whitespace()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
    );
    let min_amount = request.min_amount.unwrap_or(0.0);
    let mut matches = Vec::new();
    for record in records {
        let Some(grant) = record.grant.as_ref() else {
            continue;
        };
        if grant.amount.unwrap_or(0.0) < min_amount {
            continue;
        }
        let mut reasons = Vec::new();
        let mut score = 0.0;
        let mut haystack = record.tags.clone();
        haystack.extend(grant.topics.iter().cloned());
        if let Some(eligibility) = grant.eligibility.as_ref() {
            haystack.extend(
                eligibility
                    .split_whitespace()
                    .map(|value| value.to_string()),
            );
        }
        if let Some(summary) = record.summary.as_ref() {
            haystack.extend(summary.split_whitespace().map(|value| value.to_string()));
        }
        haystack.extend(
            grant
                .title
                .split_whitespace()
                .map(|value| value.to_string()),
        );
        let haystack = clean_tags(haystack);
        let focus_hits = focus
            .iter()
            .filter(|term| {
                haystack
                    .iter()
                    .any(|item| item.contains(*term) || term.contains(item))
            })
            .count();
        if focus_hits > 0 {
            score += focus_hits as f64 * 2.5;
            reasons.push(format!("{focus_hits} focus-area terms matched"));
        }
        let profile_hits = profile_terms
            .iter()
            .filter(|term| {
                haystack
                    .iter()
                    .any(|item| item.contains(*term) || term.contains(item))
            })
            .count();
        if profile_hits > 0 {
            score += profile_hits as f64 * 0.5;
            reasons.push(format!("{profile_hits} applicant-profile terms matched"));
        }
        if record.source.to_ascii_lowercase().contains("sbir")
            || grant
                .program
                .as_ref()
                .map(|program| program.to_ascii_lowercase().contains("sbir"))
                .unwrap_or(false)
        {
            score += 1.5;
            reasons.push("SBIR source/program prior".to_string());
        }
        if grant.amount.unwrap_or(0.0) > 0.0 {
            score += (grant.amount.unwrap_or(0.0).log10() / 10.0).clamp(0.0, 1.0);
            reasons.push("funding amount is specified".to_string());
        }
        if grant.due_date.is_some() {
            score += 0.4;
            reasons.push("deadline is available".to_string());
        }
        if score <= 0.0 {
            continue;
        }
        matches.push(GrantMatch {
            record_id: record.record_id.clone(),
            dataset_id: record.dataset_id.clone(),
            source: record.source.clone(),
            title: grant.title.clone(),
            url: grant.url.clone().or_else(|| record.source_url.clone()),
            agency: grant.agency.clone(),
            program: grant.program.clone(),
            amount: grant.amount,
            due_date: grant.due_date.clone(),
            score,
            reasons,
        });
    }
    matches.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches.truncate(request.limit.unwrap_or(20).min(100));
    matches
}
