use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    types::{finding, FilingFinding},
    util::clean_text,
    ABSTRACT_WORD_LIMIT, MAX_CLAIMS, MAX_TEXT_LEN,
};

// ---------------------------------------------------------------------------
// Claim formality / proofreading checks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimAudit {
    pub(crate) total_claims: usize,
    pub(crate) independent_claims: usize,
    pub(crate) dependent_claims: usize,
    pub(crate) multiple_dependent_claims: usize,
    pub(crate) has_multiple_dependent_claim: bool,
    pub(crate) abstract_word_count: Option<usize>,
    pub(crate) findings: Vec<FilingFinding>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimCheckRequest {
    #[serde(default)]
    pub(crate) claims: Vec<String>,
    #[serde(rename = "abstract", alias = "abstractText", default)]
    pub(crate) abstract_text: Option<String>,
}

/// Pull the claim numbers a claim depends on, plus whether the reference spans
/// multiple base claims (i.e. it is a multiple-dependent claim).
pub(crate) fn parse_claim_dependencies(text: &str) -> (Vec<usize>, bool) {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut refs = BTreeSet::new();
    let mut multi_phrase = false;
    // "any of", "any one of", "either of" signal multiple-dependent form.
    for marker in ["any of", "any one of", "either of", "one of claims"] {
        if lower.contains(marker) {
            multi_phrase = true;
        }
    }
    let mut idx = 0;
    while let Some(pos) = lower[idx..].find("claim") {
        let mut cursor = idx + pos + "claim".len();
        if lower[cursor..].starts_with('s') {
            cursor += 1;
        }
        // Parse a run of claim numbers possibly joined by ranges/lists.
        let mut local: Vec<usize> = Vec::new();
        let mut last_num: Option<usize> = None;
        let mut pending_range = false;
        loop {
            while cursor < bytes.len() && (bytes[cursor] as char).is_whitespace() {
                cursor += 1;
            }
            let connector = &lower[cursor..];
            if connector.starts_with('-') || connector.starts_with("to ") || connector.starts_with("through ") {
                pending_range = true;
                cursor += if connector.starts_with('-') { 1 } else if connector.starts_with("to ") { 3 } else { 8 };
                continue;
            }
            if connector.starts_with(',') || connector.starts_with("or ") || connector.starts_with("and ") {
                cursor += if connector.starts_with(',') { 1 } else if connector.starts_with("or ") { 3 } else { 4 };
                continue;
            }
            let num_start = cursor;
            while cursor < bytes.len() && (bytes[cursor] as char).is_ascii_digit() {
                cursor += 1;
            }
            if cursor == num_start {
                break;
            }
            if let Ok(num) = lower[num_start..cursor].parse::<usize>() {
                if pending_range {
                    if let Some(start) = last_num {
                        // Clamp the expansion: a referenced claim above MAX_CLAIMS
                        // is invalid anyway, and an unclamped range parsed from
                        // untrusted digits (e.g. "claim 1 to 9999999999") would
                        // be an unbounded-loop / OOM DoS.
                        let end = num.min(start.saturating_add(MAX_CLAIMS));
                        for n in (start + 1)..=end {
                            if local.len() >= MAX_CLAIMS {
                                break;
                            }
                            local.push(n);
                        }
                        if num > end {
                            local.push(num); // record the out-of-range ref so it is flagged
                        }
                    }
                    pending_range = false;
                } else {
                    local.push(num);
                }
                last_num = Some(num);
            }
            if local.len() >= MAX_CLAIMS {
                break;
            }
        }
        if local.len() > 1 {
            multi_phrase = true;
        }
        for n in local {
            refs.insert(n);
        }
        if refs.len() >= MAX_CLAIMS {
            break;
        }
        idx = cursor.max(idx + pos + 1);
    }
    let mut refs: Vec<usize> = refs.into_iter().collect();
    refs.sort_unstable();
    let multiple = multi_phrase && refs.len() > 1;
    (refs, multiple)
}

const ANTECEDENT_STOPWORDS: &[&str] = &[
    "the", "a", "an", "said", "wherein", "comprising", "comprises", "including", "includes",
    "of", "to", "and", "or", "for", "with", "in", "on", "at", "by", "from", "as", "is", "are",
    "claim", "claims", "method", "system", "apparatus", "device", "invention", "art", "group",
    "same", "step", "steps", "first", "second", "third", "one", "more", "least", "plurality",
    "according", "preceding", "any", "each", "which", "that", "wherein", "being", "having",
];

fn is_noun_token(w: &str) -> bool {
    !ANTECEDENT_STOPWORDS.contains(&w) && !w.chars().all(|c| c.is_ascii_digit())
}

/// Split a claim into (terms introduced with `a`/`an`, definite uses of
/// `the`/`said X` as `(article, noun)` pairs in order).
fn introduced_and_definite(text: &str) -> (BTreeSet<String>, Vec<(String, String)>) {
    let lower = text.to_ascii_lowercase();
    let words: Vec<String> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect();
    let mut introduced = BTreeSet::new();
    let mut definite = Vec::new();
    for i in 0..words.len() {
        let w = words[i].as_str();
        let next = words.get(i + 1).map(|s| s.as_str());
        if w == "a" || w == "an" {
            if let Some(n) = next.filter(|n| is_noun_token(n)) {
                introduced.insert(n.to_string());
            }
        } else if w == "the" || w == "said" {
            if let Some(n) = next.filter(|n| is_noun_token(n)) {
                definite.push((w.to_string(), n.to_string()));
            }
        }
    }
    (introduced, definite)
}

/// Conservative, advisory antecedent-basis scan over a single claim with a set
/// of terms already introduced by ancestor claims. Flags `the X` / `said X`
/// where `X` was never introduced with `a X` / `an X`.
fn antecedent_findings_with_context(
    claim_number: usize,
    text: &str,
    inherited: &BTreeSet<String>,
) -> Vec<FilingFinding> {
    let (own, definite) = introduced_and_definite(text);
    let mut flagged = BTreeSet::new();
    let mut findings = Vec::new();
    for (article, noun) in definite {
        if own.contains(&noun) || inherited.contains(&noun) || flagged.contains(&noun) {
            continue;
        }
        flagged.insert(noun.clone());
        findings.push(finding(
            "antecedent-basis",
            "warning",
            &format!(
                "Claim {claim_number}: '{article} {noun}' may lack antecedent basis (no earlier 'a {noun}'/'an {noun}'). Advisory heuristic — confirm manually."
            ),
        ));
    }
    findings
}

#[cfg(test)]
pub(crate) fn antecedent_findings(claim_number: usize, text: &str) -> Vec<FilingFinding> {
    antecedent_findings_with_context(claim_number, text, &BTreeSet::new())
}

pub(crate) fn audit_claims(claims: &[String], abstract_text: Option<&str>) -> ClaimAudit {
    let claims: Vec<String> = claims
        .iter()
        .map(|claim| clean_text(claim, MAX_TEXT_LEN))
        .filter(|claim| !claim.is_empty())
        .take(MAX_CLAIMS)
        .collect();
    let total_claims = claims.len();
    let mut independent_claims = 0;
    let mut dependent_claims = 0;
    let mut multiple_dependent_claims = 0;
    let mut findings = Vec::new();
    let mut multi_dependent_positions: Vec<usize> = Vec::new();
    let mut claim_refs: Vec<Vec<usize>> = Vec::with_capacity(total_claims);
    // Terms introduced by each claim plus everything inherited from its valid
    // ancestor chain, so dependent claims do not falsely flag parent terms.
    let mut effective_intro: Vec<BTreeSet<String>> = Vec::with_capacity(total_claims);

    for (index, claim) in claims.iter().enumerate() {
        let claim_number = index + 1;
        let (refs, is_multiple) = parse_claim_dependencies(claim);
        if refs.is_empty() {
            independent_claims += 1;
        } else {
            dependent_claims += 1;
            if is_multiple {
                multiple_dependent_claims += 1;
                multi_dependent_positions.push(claim_number);
            }
            for &referenced in &refs {
                if referenced == 0 || referenced > total_claims {
                    findings.push(finding(
                        "invalid-claim-reference",
                        "blocker",
                        &format!(
                            "Claim {claim_number} references claim {referenced}, which does not exist."
                        ),
                    ));
                } else if referenced >= claim_number {
                    findings.push(finding(
                        "improper-claim-dependency",
                        "blocker",
                        &format!(
                            "Claim {claim_number} depends on claim {referenced}; a claim may only depend on a lower-numbered preceding claim (35 USC 112(d))."
                        ),
                    ));
                }
            }
        }
        let (own_intro, _) = introduced_and_definite(claim);
        let mut inherited = own_intro;
        for &referenced in &refs {
            if referenced >= 1 && referenced < claim_number {
                if let Some(parent) = effective_intro.get(referenced - 1) {
                    inherited.extend(parent.iter().cloned());
                }
            }
        }
        findings.extend(antecedent_findings_with_context(claim_number, claim, &inherited));
        effective_intro.push(inherited);
        claim_refs.push(refs);
    }

    // A multiple dependent claim may not serve as a basis for another
    // multiple dependent claim (35 USC 112(e)).
    for &pos in &multi_dependent_positions {
        let refs = &claim_refs[pos - 1];
        if refs.iter().any(|&r| multi_dependent_positions.contains(&r)) {
            findings.push(finding(
                "multiple-dependent-on-multiple-dependent",
                "blocker",
                &format!(
                    "Claim {pos} is a multiple dependent claim that references another multiple dependent claim, which 35 USC 112(e) prohibits."
                ),
            ));
        }
    }

    if total_claims == 0 {
        findings.push(finding(
            "no-claims",
            "warning",
            "No claims were provided to check.",
        ));
    } else if independent_claims == 0 {
        findings.push(finding(
            "no-independent-claim",
            "blocker",
            "A claim set must contain at least one independent claim.",
        ));
    }
    if independent_claims > 3 {
        findings.push(finding(
            "excess-independent-claims",
            "info",
            &format!(
                "{independent_claims} independent claims: each over 3 carries an excess-claim fee."
            ),
        ));
    }
    if total_claims > 20 {
        findings.push(finding(
            "excess-claims",
            "info",
            &format!("{total_claims} total claims: each over 20 carries an excess-claim fee."),
        ));
    }

    let abstract_word_count = abstract_text.map(|text| {
        let count = text.split_whitespace().count();
        if count > ABSTRACT_WORD_LIMIT {
            findings.push(finding(
                "abstract-too-long",
                "warning",
                &format!(
                    "Abstract is {count} words; 37 CFR 1.72(b) limits it to {ABSTRACT_WORD_LIMIT} words."
                ),
            ));
        }
        count
    });

    ClaimAudit {
        total_claims,
        independent_claims,
        dependent_claims,
        multiple_dependent_claims,
        has_multiple_dependent_claim: multiple_dependent_claims > 0,
        abstract_word_count,
        findings,
    }
}
