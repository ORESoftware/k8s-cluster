use crate::{
    claims::audit_claims,
    deadlines::analyze_deadlines,
    fees::{estimate_fees, Entity},
    state::Config,
    types::{
        finding, AttachmentEvidence, AttorneyHandoff, ChecklistItem, DraftSection, KnownPriorArt,
        PatentIntakeRequest, PatentMatterPackage, ProvisionalDraft, ReadinessReview, SearchPlan,
        SearchQuery, SearchSource, UiPackageForm,
    },
    util::{clean_optional, clean_text, normalize_track, now_ms, request_id, slugify, split_lines},
    MAX_LIST_ITEMS, MAX_SHORT_TEXT_LEN, MAX_TEXT_LEN, SCHEMA_VERSION,
};

pub(crate) fn intake_from_form(form: UiPackageForm) -> PatentIntakeRequest {
    let known_prior_art = split_lines(&form.known_prior_art.unwrap_or_default())
        .into_iter()
        .map(|line| KnownPriorArt {
            title: line,
            url: None,
            notes: None,
        })
        .collect();
    let attachments = split_lines(&form.attachments.unwrap_or_default())
        .into_iter()
        .map(|line| AttachmentEvidence {
            name: line,
            kind: Some("figure-or-evidence".to_string()),
            url: None,
            notes: None,
        })
        .collect();

    PatentIntakeRequest {
        request_id: None,
        schema_version: Some(SCHEMA_VERSION.to_string()),
        title: clean_text(&form.title, MAX_SHORT_TEXT_LEN),
        inventor_names: split_lines(&form.inventor_names),
        applicant: clean_optional(form.applicant, MAX_SHORT_TEXT_LEN),
        invention_summary: clean_text(&form.invention_summary, MAX_TEXT_LEN),
        technical_field: clean_text(
            &form.technical_field.unwrap_or_default(),
            MAX_SHORT_TEXT_LEN,
        ),
        problem: clean_text(&form.problem, MAX_TEXT_LEN),
        solution: clean_text(&form.solution, MAX_TEXT_LEN),
        novelty_claims: split_lines(&form.novelty_claims),
        embodiments: split_lines(&form.embodiments.unwrap_or_default()),
        alternatives: split_lines(&form.alternatives.unwrap_or_default()),
        advantages: split_lines(&form.advantages.unwrap_or_default()),
        public_disclosure_date: clean_optional(form.public_disclosure_date, 64),
        provisional_filing_date: clean_optional(form.provisional_filing_date, 64),
        foreign_priority_date: clean_optional(form.foreign_priority_date, 64),
        target_filing: clean_optional(form.target_filing, 64),
        entity_status: clean_optional(form.entity_status, 32),
        desired_claim_count: Some(8),
        attorney_review: Some(form.attorney_review.is_some()),
        known_prior_art,
        attachments,
        notes: None,
    }
}

pub(crate) fn validate_intake(request: &PatentIntakeRequest) -> Result<(), String> {
    for (label, value) in [
        ("title", request.title.as_str()),
        ("inventionSummary", request.invention_summary.as_str()),
        ("problem", request.problem.as_str()),
        ("solution", request.solution.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(format!("{label} must not be empty"));
        }
    }
    if request.title.len() > MAX_SHORT_TEXT_LEN {
        return Err(format!("title must be at most {MAX_SHORT_TEXT_LEN} bytes"));
    }
    if request.invention_summary.len() > MAX_TEXT_LEN
        || request.problem.len() > MAX_TEXT_LEN
        || request.solution.len() > MAX_TEXT_LEN
    {
        return Err(format!(
            "long text fields must be at most {MAX_TEXT_LEN} bytes"
        ));
    }
    if request.inventor_names.len() > MAX_LIST_ITEMS
        || request.novelty_claims.len() > MAX_LIST_ITEMS
        || request.embodiments.len() > MAX_LIST_ITEMS
        || request.alternatives.len() > MAX_LIST_ITEMS
        || request.advantages.len() > MAX_LIST_ITEMS
        || request.known_prior_art.len() > MAX_LIST_ITEMS
        || request.attachments.len() > MAX_LIST_ITEMS
    {
        return Err(format!(
            "list fields may contain at most {MAX_LIST_ITEMS} items"
        ));
    }
    // Cap individual list-item lengths. The form path already does this via
    // clean_text; the JSON path must enforce it too so a single oversized item
    // cannot bloat a stored package or amplify the AI prompt.
    let short_lists = [
        ("inventorNames", &request.inventor_names),
        ("noveltyClaims", &request.novelty_claims),
        ("embodiments", &request.embodiments),
        ("alternatives", &request.alternatives),
        ("advantages", &request.advantages),
    ];
    for (label, items) in short_lists {
        if items.iter().any(|item| item.len() > MAX_SHORT_TEXT_LEN) {
            return Err(format!(
                "each {label} item must be at most {MAX_SHORT_TEXT_LEN} bytes"
            ));
        }
    }
    for art in &request.known_prior_art {
        if art.title.len() > MAX_SHORT_TEXT_LEN
            || art.url.as_ref().is_some_and(|v| v.len() > MAX_SHORT_TEXT_LEN)
            || art.notes.as_ref().is_some_and(|v| v.len() > MAX_TEXT_LEN)
        {
            return Err("knownPriorArt entries are too long".to_string());
        }
    }
    for attachment in &request.attachments {
        if attachment.name.len() > MAX_SHORT_TEXT_LEN
            || attachment.url.as_ref().is_some_and(|v| v.len() > MAX_SHORT_TEXT_LEN)
            || attachment.notes.as_ref().is_some_and(|v| v.len() > MAX_TEXT_LEN)
        {
            return Err("attachment entries are too long".to_string());
        }
    }
    Ok(())
}

pub(crate) fn evaluate_readiness(request: &PatentIntakeRequest) -> ReadinessReview {
    let mut score = 0u16;
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let mut strengths = Vec::new();
    let mut next_actions = Vec::new();

    if request.title.trim().len() >= 8 {
        score += 8;
    } else {
        blockers.push(finding(
            "missing-title",
            "blocker",
            "Add a descriptive invention title.",
        ));
    }

    if !request.inventor_names.is_empty() {
        score += 8;
        strengths.push(format!(
            "{} inventor(s) captured",
            request.inventor_names.len()
        ));
    } else {
        blockers.push(finding(
            "missing-inventors",
            "blocker",
            "Capture every likely inventor before filing.",
        ));
        next_actions.push("Confirm inventorship with counsel.".to_string());
    }

    if request.invention_summary.chars().count() >= 120 {
        score += 14;
        strengths.push("Invention summary is long enough for drafting context.".to_string());
    } else {
        blockers.push(finding(
            "thin-summary",
            "blocker",
            "Expand the invention summary with structure, operation, and use cases.",
        ));
        next_actions.push("Add concrete component and workflow detail.".to_string());
    }

    if request.problem.chars().count() >= 40 {
        score += 8;
    } else {
        warnings.push(finding(
            "thin-problem",
            "warning",
            "The problem statement is too short for a strong background section.",
        ));
    }

    if request.solution.chars().count() >= 60 {
        score += 12;
    } else {
        blockers.push(finding(
            "thin-solution",
            "blocker",
            "Describe the solution in enough detail to support enablement review.",
        ));
    }

    if request.novelty_claims.len() >= 2 {
        score += 16;
        strengths.push(format!(
            "{} novelty points captured",
            request.novelty_claims.len()
        ));
    } else if request.novelty_claims.len() == 1 {
        score += 8;
        warnings.push(finding(
            "single-novelty-point",
            "warning",
            "Only one novelty point is captured; add alternatives or dependent features.",
        ));
    } else {
        blockers.push(finding(
            "missing-novelty",
            "blocker",
            "List the technical features believed to be new.",
        ));
    }

    if !request.technical_field.trim().is_empty() {
        score += 5;
    } else {
        warnings.push(finding(
            "missing-technical-field",
            "warning",
            "Add a technical field to focus searching and drafting.",
        ));
    }

    if !request.embodiments.is_empty() {
        score += 8;
    } else {
        warnings.push(finding(
            "missing-embodiments",
            "warning",
            "Add at least one implementation embodiment.",
        ));
    }

    if !request.alternatives.is_empty() {
        score += 5;
    } else {
        next_actions
            .push("Capture alternate implementations to avoid a narrow disclosure.".to_string());
    }

    if !request.advantages.is_empty() {
        score += 5;
    }

    if !request.attachments.is_empty() {
        score += 6;
        strengths.push(format!(
            "{} figure/evidence attachment(s) listed",
            request.attachments.len()
        ));
    } else {
        warnings.push(finding(
            "missing-figures",
            "warning",
            "Prepare at least a system diagram or method flow drawing.",
        ));
    }

    if !request.known_prior_art.is_empty() {
        score += 5;
        strengths.push("Known prior art captured for attorney review.".to_string());
    } else {
        warnings.push(finding(
            "no-known-prior-art",
            "warning",
            "No known prior art was provided; run a search before final filing decisions.",
        ));
    }

    if request.public_disclosure_date.is_some() {
        warnings.push(finding(
            "public-disclosure-date",
            "warning",
            "A public disclosure date is present; review filing deadlines and non-US rights.",
        ));
        next_actions.push("Have counsel evaluate public disclosure timing.".to_string());
    }

    if request.attorney_review.unwrap_or(false) {
        score += 3;
    } else {
        next_actions.push("Route the draft package to patent counsel before filing.".to_string());
    }

    let score = score.min(100) as u8;
    let status = if !blockers.is_empty() {
        "needs-invention-detail"
    } else if score >= 82 {
        "ready-for-attorney-review"
    } else if score >= 65 {
        "draftable"
    } else {
        "needs-invention-detail"
    }
    .to_string();

    if next_actions.is_empty() {
        next_actions.push(
            "Review draft claims, figures, and prior-art search results with counsel.".to_string(),
        );
    }

    ReadinessReview {
        score,
        status,
        blockers,
        warnings,
        strengths,
        next_actions,
    }
}

fn build_draft(request: &PatentIntakeRequest) -> ProvisionalDraft {
    let field = if request.technical_field.trim().is_empty() {
        "the relevant technical field".to_string()
    } else {
        request.technical_field.trim().to_string()
    };
    let embodiments = if request.embodiments.is_empty() {
        "At least one implementation should describe the components, data flow, control flow, and operating environment in enough detail for a skilled person to reproduce the invention.".to_string()
    } else {
        request
            .embodiments
            .iter()
            .enumerate()
            .map(|(index, item)| format!("Embodiment {}: {}", index + 1, item))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let alternatives = if request.alternatives.is_empty() {
        "Alternative implementations may vary component placement, sequence of operations, data structures, materials, integration surfaces, or user interaction while preserving the inventive concept.".to_string()
    } else {
        request
            .alternatives
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let advantages = if request.advantages.is_empty() {
        "Potential advantages should be validated against prior systems and quantified where possible.".to_string()
    } else {
        request
            .advantages
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let novelty = if request.novelty_claims.is_empty() {
        "Novel technical features remain to be identified.".to_string()
    } else {
        request
            .novelty_claims
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    ProvisionalDraft {
        title: request.title.clone(),
        abstract_draft: abstract_draft(request),
        sections: vec![
            DraftSection {
                heading: "Technical Field".to_string(),
                body: format!("The disclosure relates to {field}."),
            },
            DraftSection {
                heading: "Background".to_string(),
                body: request.problem.clone(),
            },
            DraftSection {
                heading: "Summary".to_string(),
                body: format!(
                    "{}\n\nThe proposed solution includes: {}",
                    request.invention_summary, request.solution
                ),
            },
            DraftSection {
                heading: "Potentially Novel Features".to_string(),
                body: novelty,
            },
            DraftSection {
                heading: "Detailed Description".to_string(),
                body: embodiments,
            },
            DraftSection {
                heading: "Alternative Implementations".to_string(),
                body: alternatives,
            },
            DraftSection {
                heading: "Advantages".to_string(),
                body: advantages,
            },
        ],
        claim_seeds: claim_seeds(request),
        drawing_plan: drawing_plan(request),
    }
}

fn abstract_draft(request: &PatentIntakeRequest) -> String {
    let mut parts = vec![request.invention_summary.trim().to_string()];
    if !request.solution.trim().is_empty() {
        parts.push(format!(
            "The invention addresses the problem by {}.",
            sentence_fragment(&request.solution)
        ));
    }
    if let Some(first) = request.novelty_claims.first() {
        parts.push(format!(
            "In some implementations, the system includes {}.",
            sentence_fragment(first)
        ));
    }
    parts
        .join(" ")
        .split_whitespace()
        .take(120)
        .collect::<Vec<_>>()
        .join(" ")
}

fn sentence_fragment(value: &str) -> String {
    value.trim().trim_end_matches(['.', ';', ':']).to_string()
}

fn claim_seeds(request: &PatentIntakeRequest) -> Vec<String> {
    let mut seeds = Vec::new();
    let noun = if request.title.trim().is_empty() {
        "invention"
    } else {
        request.title.trim()
    };
    let count = request.desired_claim_count.unwrap_or(8).clamp(3, 20);
    seeds.push(format!(
        "A {noun} comprising elements configured to perform the solution described in the specification."
    ));
    for novelty in request.novelty_claims.iter().take(count.saturating_sub(1)) {
        seeds.push(format!(
            "The {noun} of claim 1, wherein {}.",
            sentence_fragment(novelty)
        ));
    }
    if !request.alternatives.is_empty() && seeds.len() < count {
        seeds.push(format!(
            "The {noun} of claim 1, wherein the same result is achieved through at least one disclosed alternative implementation."
        ));
    }
    while seeds.len() < count.min(5) {
        seeds.push(format!(
            "A method of using the {noun} to solve the identified technical problem."
        ));
    }
    seeds
}

fn drawing_plan(request: &PatentIntakeRequest) -> Vec<String> {
    let mut plan = vec![
        "Figure 1: system or architecture overview showing major components and interfaces."
            .to_string(),
        "Figure 2: method flow showing the primary operating sequence.".to_string(),
    ];
    for (index, embodiment) in request.embodiments.iter().take(4).enumerate() {
        plan.push(format!(
            "Figure {}: embodiment detail for {}.",
            index + 3,
            sentence_fragment(embodiment)
        ));
    }
    if request.attachments.is_empty() {
        plan.push(
            "Evidence needed: sketches, screenshots, CAD, data-flow diagrams, or lab notes."
                .to_string(),
        );
    } else {
        for attachment in request.attachments.iter().take(6) {
            plan.push(format!("Existing evidence: {}", attachment.name));
        }
    }
    plan
}

pub(crate) fn build_search_plan(request: &PatentIntakeRequest) -> SearchPlan {
    let mut queries = Vec::new();
    let field = if request.technical_field.trim().is_empty() {
        request.title.trim()
    } else {
        request.technical_field.trim()
    };
    queries.push(SearchQuery {
        label: "core invention".to_string(),
        query: format!("\"{}\" patent", request.title.trim()),
        intent: "Find close title and phrase matches.".to_string(),
    });
    if !field.is_empty() {
        queries.push(SearchQuery {
            label: "technical field".to_string(),
            query: format!("{field} {}", request.problem.trim()),
            intent: "Map the problem space and common terminology.".to_string(),
        });
    }
    for (index, novelty) in request.novelty_claims.iter().take(6).enumerate() {
        queries.push(SearchQuery {
            label: format!("novelty point {}", index + 1),
            query: format!("{} {}", request.title.trim(), novelty),
            intent: "Check whether a claimed feature appears in earlier publications.".to_string(),
        });
    }
    let mut classification_hints = Vec::new();
    if !request.technical_field.trim().is_empty() {
        classification_hints.push(format!(
            "Start CPC/USPC exploration around {}.",
            request.technical_field.trim()
        ));
    }
    classification_hints.push(
        "Record patent families, earliest priority dates, claim overlap, and non-patent literature."
            .to_string(),
    );

    SearchPlan {
        queries,
        classification_hints,
        sources: vec![
            SearchSource {
                name: "USPTO Patent Public Search".to_string(),
                url: "https://ppubs.uspto.gov/pubwebapp/".to_string(),
                use_case: "US patent and published application search.".to_string(),
            },
            SearchSource {
                name: "Patent Center".to_string(),
                url: "https://patentcenter.uspto.gov/".to_string(),
                use_case: "Operator filing handoff and application management.".to_string(),
            },
            SearchSource {
                name: "Google Patents".to_string(),
                url: "https://patents.google.com/".to_string(),
                use_case: "Broad keyword and family exploration.".to_string(),
            },
            SearchSource {
                name: "Espacenet".to_string(),
                url: "https://worldwide.espacenet.com/".to_string(),
                use_case: "International patent family and classification review.".to_string(),
            },
            SearchSource {
                name: "WIPO PATENTSCOPE".to_string(),
                url: "https://patentscope.wipo.int/".to_string(),
                use_case: "PCT and international publication search.".to_string(),
            },
        ],
        review_notes: vec![
            "Preserve search strings and reviewed references for counsel.".to_string(),
            "Compare every close reference against the novelty list and embodiments.".to_string(),
        ],
    }
}

fn build_checklist(
    config: &Config,
    request: &PatentIntakeRequest,
    readiness: &ReadinessReview,
) -> Vec<ChecklistItem> {
    let mut items = vec![
        ChecklistItem {
            label: "Invention disclosure intake".to_string(),
            status: if readiness.blockers.is_empty() { "complete" } else { "open" }.to_string(),
            owner: "inventor".to_string(),
            notes: "Title, inventors, problem, solution, novelty, and embodiments.".to_string(),
        },
        ChecklistItem {
            label: "Specification draft".to_string(),
            status: "draft".to_string(),
            owner: "service".to_string(),
            notes: "Generated sections are drafting support, not final legal text.".to_string(),
        },
        ChecklistItem {
            label: "Drawings and figures".to_string(),
            status: if request.attachments.is_empty() { "open" } else { "draft" }.to_string(),
            owner: "inventor".to_string(),
            notes: "Prepare clean figures from the drawing plan before filing.".to_string(),
        },
        ChecklistItem {
            label: "Prior-art search notes".to_string(),
            status: if request.known_prior_art.is_empty() { "open" } else { "draft" }.to_string(),
            owner: "operator".to_string(),
            notes: "Use search plan results to brief counsel.".to_string(),
        },
        ChecklistItem {
            label: "Attorney review".to_string(),
            status: if request.attorney_review.unwrap_or(false) { "requested" } else { "open" }.to_string(),
            owner: "counsel".to_string(),
            notes: "Review inventorship, enablement, claim strategy, disclosure timing, and filing type.".to_string(),
        },
        ChecklistItem {
            label: "Patent Center filing handoff".to_string(),
            status: "operator-action".to_string(),
            owner: "operator".to_string(),
            notes: format!("Use configured handoff URL: {}", config.patent_center_url),
        },
    ];
    if request.target_filing.as_deref() == Some("non-provisional") {
        items.push(ChecklistItem {
            label: "Oath/declaration and ADS".to_string(),
            status: "open".to_string(),
            owner: "counsel".to_string(),
            notes: "Non-provisional filings usually need formal forms and claim review."
                .to_string(),
        });
    }
    items
}

fn build_handoff(
    request: &PatentIntakeRequest,
    draft: &ProvisionalDraft,
    search_plan: &SearchPlan,
) -> AttorneyHandoff {
    AttorneyHandoff {
        summary: format!(
            "{} inventor(s), {} novelty point(s), {} draft section(s), {} search query group(s).",
            request.inventor_names.len(),
            request.novelty_claims.len(),
            draft.sections.len(),
            search_plan.queries.len()
        ),
        questions: vec![
            "Are all named contributors legally inventors for at least one claim?".to_string(),
            "Does the disclosure enable a skilled person to make and use the invention?"
                .to_string(),
            "Should this be filed as provisional, non-provisional, design, or PCT-related work?"
                .to_string(),
            "Do public disclosures or offers for sale create deadline pressure?".to_string(),
            "Which claim seeds should become independent claims?".to_string(),
        ],
        package_manifest: vec![
            "invention-intake.json".to_string(),
            "readiness-review.json".to_string(),
            "draft-specification.md".to_string(),
            "claim-seeds.md".to_string(),
            "drawing-plan.md".to_string(),
            "prior-art-search-plan.md".to_string(),
            "claim-audit.json".to_string(),
            "uspto-fee-estimate.json".to_string(),
            "filing-deadlines.json".to_string(),
            "filing-checklist.md".to_string(),
        ],
    }
}

pub(crate) fn build_package(
    config: &Config,
    request: PatentIntakeRequest,
) -> Result<PatentMatterPackage, String> {
    validate_intake(&request)?;
    let request_id = request_id(request.request_id.as_ref(), "patent-package");
    let generated_at_ms = now_ms();
    let readiness = evaluate_readiness(&request);
    let draft = build_draft(&request);
    let search_plan = build_search_plan(&request);
    let filing_checklist = build_checklist(config, &request, &readiness);
    let attorney_handoff = build_handoff(&request, &draft, &search_plan);
    let filing_track = normalize_track(request.target_filing.as_ref());
    let claim_audit = audit_claims(&draft.claim_seeds, Some(&draft.abstract_draft));
    let entity = Entity::parse(request.entity_status.as_deref());
    let fee_estimate = estimate_fees(
        entity,
        &filing_track,
        claim_audit.total_claims,
        claim_audit.independent_claims,
        claim_audit.has_multiple_dependent_claim,
    );
    let deadlines = analyze_deadlines(
        request.provisional_filing_date.as_deref(),
        request.public_disclosure_date.as_deref(),
        request.foreign_priority_date.as_deref(),
        None,
    );
    let mut warnings = readiness
        .warnings
        .iter()
        .map(|finding| finding.message.clone())
        .collect::<Vec<_>>();
    for milestone in deadlines.milestones.iter().filter(|m| m.status == "past") {
        warnings.push(format!(
            "Deadline likely missed: {} (due {}).",
            milestone.label, milestone.due_date
        ));
    }
    warnings.push("This package is preparation support only; it does not file with the USPTO or replace legal advice.".to_string());
    let matter_id = format!("pf-{}-{generated_at_ms}", slugify(&request.title));
    Ok(PatentMatterPackage {
        ok: true,
        matter_id,
        request_id,
        schema_version: SCHEMA_VERSION,
        generated_at_ms,
        filing_track,
        title: request.title,
        applicant: request.applicant,
        inventor_names: request.inventor_names,
        readiness,
        draft,
        search_plan,
        claim_audit,
        fee_estimate,
        deadlines,
        filing_checklist,
        attorney_handoff,
        warnings,
    })
}
