use std::collections::BTreeMap;

use crate::forecast::*;
use crate::shared::*;
use crate::state::*;
use crate::types::*;

pub(crate) fn analyze_sentiment(
    config: &Config,
    request: SentimentAnalyzeRequest,
) -> Result<SentimentAnalyzeResponse, String> {
    if let Some(schema) = request.schema_version.as_deref() {
        if schema != SCHEMA_VERSION {
            return Err(format!("schemaVersion must be {SCHEMA_VERSION}"));
        }
    }
    if request.documents.is_empty() {
        return Err("documents must contain at least one item".to_string());
    }
    if request.documents.len() > MAX_SENTIMENT_DOCUMENTS {
        return Err(format!(
            "documents must contain at most {MAX_SENTIMENT_DOCUMENTS} items"
        ));
    }
    if let Some(instrument_ids) = request.instrument_ids.as_ref() {
        if instrument_ids.len() > MAX_SENTIMENT_CONTEXT_SCORES {
            return Err(format!(
                "instrumentIds must contain at most {MAX_SENTIMENT_CONTEXT_SCORES} items"
            ));
        }
        for instrument_id in instrument_ids {
            clean_token(instrument_id, "instrumentIds[]")?;
        }
    }

    let request_id = request_id(request.request_id.as_ref(), "sentiment-analyze");
    let mut weighted_sum = 0.0;
    let mut weight_total = 0.0;
    let mut source_totals: BTreeMap<String, (usize, f64, f64)> = BTreeMap::new();
    let mut term_counts: BTreeMap<String, usize> = BTreeMap::new();

    for document in &request.documents {
        let source = clean_token(&document.source, "documents[].source")?;
        let text = document.text.trim();
        if text.is_empty() {
            return Err("documents[].text must not be empty".to_string());
        }
        if text.len() > MAX_SENTIMENT_TEXT_BYTES {
            return Err(format!(
                "documents[].text must be at most {MAX_SENTIMENT_TEXT_BYTES} bytes"
            ));
        }
        clean_optional_token(&document.author, "documents[].author")?;
        clean_optional_token(&document.published_at, "documents[].publishedAt")?;
        if let Some(url) = document.url.as_deref() {
            if url.len() > MAX_URL_LEN || url.chars().any(char::is_control) {
                return Err(format!(
                    "documents[].url must be at most {MAX_URL_LEN} bytes and contain no control characters"
                ));
            }
        }
        let weight = document
            .weight
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(1.0)
            .min(25.0);
        let score = score_sentiment_text(text);
        weighted_sum += score * weight;
        weight_total += weight;
        let entry = source_totals.entry(source).or_insert((0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += score * weight;
        entry.2 += weight;
        collect_sentiment_terms(text, &mut term_counts);
    }

    let average_sentiment = weighted_sum / weight_total.max(f64::EPSILON);
    let source_scores = source_totals
        .into_iter()
        .map(|(source, (document_count, score_sum, source_weight))| {
            let average = score_sum / source_weight.max(f64::EPSILON);
            SentimentSourceScore {
                source,
                document_count,
                average_sentiment: round6(average),
                confidence: round6(sentiment_confidence(document_count, average)),
            }
        })
        .collect::<Vec<_>>();
    let mut terms = term_counts.into_iter().collect::<Vec<_>>();
    terms.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let top_terms = terms
        .into_iter()
        .take(16)
        .map(|(term, _)| term)
        .collect::<Vec<_>>();

    Ok(SentimentAnalyzeResponse {
        ok: true,
        request_id,
        schema_version: SCHEMA_VERSION,
        query: request.query,
        document_count: request.documents.len(),
        average_sentiment: round6(average_sentiment),
        confidence: round6(sentiment_confidence(
            request.documents.len(),
            average_sentiment,
        )),
        source_scores,
        top_terms,
        credential_status: config.sentiment_credentials.clone(),
        generated_at_ms: now_ms(),
    })
}

pub(crate) fn score_sentiment_text(text: &str) -> f64 {
    let lower = text.to_ascii_lowercase();
    let positive = [
        "beat",
        "bull",
        "bullish",
        "breakout",
        "growth",
        "upgrade",
        "surge",
        "rally",
        "accumulate",
        "strong",
        "resilient",
        "expansion",
        "demand",
        "profit",
        "record",
        "approval",
        "adoption",
        "inflow",
        "soft landing",
    ];
    let negative = [
        "miss",
        "bear",
        "bearish",
        "crash",
        "recession",
        "downgrade",
        "default",
        "fraud",
        "lawsuit",
        "weak",
        "shortage",
        "glut",
        "outflow",
        "layoff",
        "bankruptcy",
        "tariff",
        "war",
        "inflation shock",
        "liquidity crunch",
    ];
    let pos = positive
        .iter()
        .filter(|term| lower.contains(**term))
        .count() as f64;
    let neg = negative
        .iter()
        .filter(|term| lower.contains(**term))
        .count() as f64;
    if pos == 0.0 && neg == 0.0 {
        0.0
    } else {
        clamp((pos - neg) / (pos + neg + 1.0), -1.0, 1.0)
    }
}

pub(crate) fn collect_sentiment_terms(text: &str, counts: &mut BTreeMap<String, usize>) {
    for raw in text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '$' && ch != '#') {
        let token = raw.trim().to_ascii_lowercase();
        if token.len() < 3 || token.len() > 32 {
            continue;
        }
        if matches!(
            token.as_str(),
            "the" | "and" | "for" | "this" | "that" | "with" | "from" | "market" | "price"
        ) {
            continue;
        }
        *counts.entry(token).or_insert(0) += 1;
    }
}

pub(crate) fn sentiment_confidence(document_count: usize, average_sentiment: f64) -> f64 {
    clamp(
        0.25 + (document_count as f64).ln_1p() / 8.0 + average_sentiment.abs() * 0.35,
        0.0,
        0.95,
    )
}
