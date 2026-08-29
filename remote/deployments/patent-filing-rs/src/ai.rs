use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    claims::{audit_claims, ClaimAudit},
    fees::{estimate_fees, Entity, FeeEstimate},
    package::validate_intake,
    state::AppState,
    types::{DraftSection, PatentIntakeRequest},
    util::normalize_track,
    AI_BRIEF_MAX_CHARS, AI_ERROR_SNIPPET_CHARS, AI_MAX_TOKENS, ANTHROPIC_VERSION,
};

// ---------------------------------------------------------------------------
// AI-assisted drafting (Claude) with a deterministic self-audit + repair loop
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiDraft {
    #[serde(rename = "abstract", alias = "abstractText", default)]
    pub(crate) abstract_text: String,
    #[serde(default)]
    pub(crate) claims: Vec<String>,
    #[serde(default)]
    pub(crate) sections: Vec<DraftSection>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiDraftResponse {
    ok: bool,
    model: String,
    repair_applied: bool,
    draft: AiDraft,
    claim_audit: ClaimAudit,
    fee_estimate: FeeEstimate,
    disclaimer: String,
}

#[derive(Deserialize)]
struct AnthropicTextBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicTextBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

const AI_SYSTEM_PROMPT: &str = "You are a patent drafting assistant that prepares provisional-application \
drafting support for review by a registered patent practitioner. You do not give legal advice and you do not \
file anything. Draft in clear, enabling, US-practice style.\n\n\
Return ONLY a JSON object (no prose, no markdown fences) with exactly these keys:\n\
- \"abstract\": a single paragraph of at most 150 words.\n\
- \"claims\": an array of claim strings. Claim 1 must be independent. Every dependent claim must reference an \
earlier, lower-numbered claim by number (e.g. \"The system of claim 1, wherein ...\") and must not forward- or \
self-reference. Maintain proper antecedent basis: introduce each element with \"a\"/\"an\" before later \
referring to it with \"the\"/\"said\". Include at least one independent apparatus/system claim and one \
independent method claim when the invention supports both.\n\
- \"sections\": an array of {\"heading\", \"body\"} objects covering at least Field, Background, Summary, \
Detailed Description, and Alternative Embodiments.";

/// JSON schema constraining the model output (structured outputs).
pub(crate) fn ai_output_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "abstract": { "type": "string" },
            "claims": { "type": "array", "items": { "type": "string" } },
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "heading": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["heading", "body"]
                }
            }
        },
        "required": ["abstract", "claims", "sections"]
    })
}

pub(crate) fn intake_brief(request: &PatentIntakeRequest) -> String {
    let list = |label: &str, items: &[String]| {
        if items.is_empty() {
            String::new()
        } else {
            format!("\n{label}:\n- {}", items.join("\n- "))
        }
    };
    let brief = format!(
        "Title: {title}\nTechnical field: {field}\nInventors: {inventors}\n\nProblem:\n{problem}\n\nSolution:\n{solution}\n\nInvention summary:\n{summary}{novelty}{embodiments}{alternatives}{advantages}\n\nDesired claim count (approximate): {claims}",
        title = request.title,
        field = if request.technical_field.trim().is_empty() { "(unspecified)" } else { request.technical_field.trim() },
        inventors = if request.inventor_names.is_empty() { "(unspecified)".to_string() } else { request.inventor_names.join(", ") },
        problem = request.problem,
        solution = request.solution,
        summary = request.invention_summary,
        novelty = list("Novelty points", &request.novelty_claims),
        embodiments = list("Embodiments", &request.embodiments),
        alternatives = list("Alternatives", &request.alternatives),
        advantages = list("Advantages", &request.advantages),
        claims = request.desired_claim_count.unwrap_or(10),
    );
    // List fields are not individually length-capped by validate_intake, so bound
    // the whole brief to keep model cost predictable regardless of input size.
    if brief.chars().count() > AI_BRIEF_MAX_CHARS {
        brief.chars().take(AI_BRIEF_MAX_CHARS).collect()
    } else {
        brief
    }
}

/// Strip an optional ```json ... ``` fence and parse the model's JSON output.
pub(crate) fn parse_ai_draft(text: &str) -> Result<AiDraft, String> {
    let trimmed = text.trim();
    let body = if let Some(rest) = trimmed.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        rest.trim_start_matches('\n')
            .strip_suffix("```")
            .unwrap_or(rest)
            .trim()
            .trim_end_matches("```")
            .trim()
    } else {
        trimmed
    };
    serde_json::from_str::<AiDraft>(body)
        .map_err(|error| format!("model did not return the expected JSON: {error}"))
}

async fn anthropic_messages(
    state: &AppState,
    api_key: &str,
    system: &str,
    user_messages: &[serde_json::Value],
) -> Result<AiDraft, String> {
    let body = json!({
        "model": state.config.ai_model,
        "max_tokens": AI_MAX_TOKENS,
        "thinking": { "type": "adaptive" },
        "output_config": {
            "effort": "high",
            "format": { "type": "json_schema", "schema": ai_output_schema() }
        },
        "system": system,
        "messages": user_messages,
    });
    let response = state
        .http
        .post(format!("{}/v1/messages", state.config.anthropic_base_url))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("request to model failed: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("failed to read model response: {error}"))?;
    if !status.is_success() {
        let snippet: String = text.chars().take(AI_ERROR_SNIPPET_CHARS).collect();
        return Err(format!("model returned HTTP {}: {}", status.as_u16(), snippet));
    }
    let parsed: AnthropicResponse = serde_json::from_str(&text)
        .map_err(|error| format!("could not parse model envelope: {error}"))?;
    if parsed.stop_reason.as_deref() == Some("refusal") {
        return Err("model declined to produce a draft for this input".to_string());
    }
    let json_text = parsed
        .content
        .iter()
        .filter(|block| block.kind == "text")
        .filter_map(|block| block.text.clone())
        .collect::<Vec<_>>()
        .join("");
    if json_text.trim().is_empty() {
        return Err("model returned no text content".to_string());
    }
    parse_ai_draft(&json_text)
}

fn user_message(text: String) -> serde_json::Value {
    json!({ "role": "user", "content": text })
}

pub(crate) async fn generate_ai_draft(
    state: &AppState,
    request: PatentIntakeRequest,
) -> Result<AiDraftResponse, String> {
    let api_key = state
        .config
        .anthropic_api_key
        .clone()
        .ok_or("AI drafting is not configured")?;
    validate_intake(&request)?;
    let brief = intake_brief(&request);

    let draft = anthropic_messages(
        state,
        &api_key,
        AI_SYSTEM_PROMPT,
        &[user_message(format!(
            "Draft a provisional patent application from this invention disclosure.\n\n{brief}"
        ))],
    )
    .await?;

    let audit = audit_claims(&draft.claims, Some(&draft.abstract_text));
    let blockers: Vec<String> = audit
        .findings
        .iter()
        .filter(|finding| finding.severity == "blocker")
        .map(|finding| finding.message.clone())
        .collect();

    // Self-audit repair pass: feed the deterministic checker's blockers back to
    // the model exactly once and re-audit the result.
    let (draft, audit, repair_applied) = if blockers.is_empty() {
        (draft, audit, false)
    } else {
        let prior = serde_json::to_string(&draft).unwrap_or_default();
        let repair = anthropic_messages(
            state,
            &api_key,
            AI_SYSTEM_PROMPT,
            &[
                user_message(format!(
                    "Draft a provisional patent application from this invention disclosure.\n\n{brief}"
                )),
                user_message(format!(
                    "Your previous draft was:\n{prior}\n\nAn automated formality checker found these blocking issues:\n- {}\n\nReturn a corrected JSON draft that resolves every issue while keeping the same invention scope.",
                    blockers.join("\n- ")
                )),
            ],
        )
        .await;
        match repair {
            Ok(repaired) => {
                let repaired_audit = audit_claims(&repaired.claims, Some(&repaired.abstract_text));
                (repaired, repaired_audit, true)
            }
            // If the repair call fails, keep the first draft and its findings.
            Err(_) => (draft, audit, false),
        }
    };

    let entity = Entity::parse(request.entity_status.as_deref());
    let track = normalize_track(request.target_filing.as_ref());
    let fee_estimate = estimate_fees(
        entity,
        &track,
        audit.total_claims,
        audit.independent_claims,
        audit.has_multiple_dependent_claim,
    );

    Ok(AiDraftResponse {
        ok: true,
        model: state.config.ai_model.clone(),
        repair_applied,
        draft,
        claim_audit: audit,
        fee_estimate,
        disclaimer:
            "AI-generated drafting support only. Not legal advice and not a filing. A registered patent \
             practitioner must review inventorship, enablement, claim scope, and prior art before any filing."
                .to_string(),
    })
}
