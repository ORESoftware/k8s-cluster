use std::sync::{Arc, RwLock};

use crate::{
    ai::{ai_output_schema, intake_brief, parse_ai_draft, AiDraft},
    claims::{antecedent_findings, audit_claims, parse_claim_dependencies},
    deadlines::{analyze_deadlines, CivilDate},
    fees::{estimate_fees, Entity},
    handlers::review_package,
    package::{build_package, evaluate_readiness, validate_intake},
    state::{AppState, Config, Metrics, PatentStore},
    types::{example_request, PackageReviewRequest, PatentIntakeRequest, PatentMatterPackageInput},
    MAX_CLAIMS, MAX_SHORT_TEXT_LEN,
};

fn test_config() -> Config {
    Config {
        server_auth_secret: Some("secret".to_string()),
        allow_unauthenticated: false,
        patent_center_url: "https://patentcenter.uspto.gov/".to_string(),
        max_matters: 10,
        anthropic_api_key: None,
        anthropic_base_url: "https://api.anthropic.com".to_string(),
        ai_model: "claude-opus-4-8".to_string(),
        ai_max_concurrency: 4,
    }
}

#[test]
fn complete_intake_scores_ready_for_attorney_review() {
    let request = example_request();
    let review = evaluate_readiness(&request);
    assert!(review.score >= 82, "score was {}", review.score);
    assert_eq!(review.status, "ready-for-attorney-review");
    assert!(review.blockers.is_empty());
}

#[test]
fn thin_intake_has_blockers() {
    let request = PatentIntakeRequest {
        title: "Idea".to_string(),
        invention_summary: "too short".to_string(),
        problem: "unknown".to_string(),
        solution: "unknown".to_string(),
        ..example_request()
    };
    let mut request = request;
    request.inventor_names.clear();
    request.novelty_claims.clear();
    let review = evaluate_readiness(&request);
    assert!(review.score < 65);
    assert!(review
        .blockers
        .iter()
        .any(|finding| finding.code == "missing-inventors"));
    assert!(review
        .blockers
        .iter()
        .any(|finding| finding.code == "missing-novelty"));
}

#[test]
fn package_contains_claim_seeds_drawings_and_checklist() {
    let config = test_config();
    let package = build_package(&config, example_request()).expect("package");
    assert!(!package.draft.claim_seeds.is_empty());
    assert!(package
        .draft
        .drawing_plan
        .iter()
        .any(|item| item.contains("Figure 1")));
    assert!(package
        .filing_checklist
        .iter()
        .any(|item| item.label == "Patent Center filing handoff"));
}

#[test]
fn package_review_blocks_low_readiness() {
    let state = AppState {
        config: Arc::new(test_config()),
        metrics: Arc::new(Metrics::default()),
        store: Arc::new(RwLock::new(PatentStore::default())),
        http: reqwest::Client::new(),
        ai_permits: Arc::new(tokio::sync::Semaphore::new(4)),
    };
    let response = review_package(
        &state,
        PackageReviewRequest {
            matter_id: None,
            package: Some(PatentMatterPackageInput {
                readiness_score: Some(40),
                blocker_count: Some(2),
                section_count: Some(3),
                checklist_open_count: Some(3),
            }),
        },
    );
    assert_eq!(response.release_gate, "blocked");
    assert!(response
        .findings
        .iter()
        .any(|finding| finding.severity == "blocker"));
}

#[test]
fn civil_date_add_months_clamps_and_roundtrips() {
    let jan31 = CivilDate::parse("2025-01-31").unwrap();
    assert_eq!(jan31.add_months(1).format(), "2025-02-28");
    let leap = CivilDate::parse("2024-01-31").unwrap();
    assert_eq!(leap.add_months(1).format(), "2024-02-29");
    let d = CivilDate::parse("2025-06-09").unwrap();
    assert_eq!(CivilDate::from_days(d.to_days()), d);
    assert_eq!(d.add_months(12).format(), "2026-06-09");
    assert!(CivilDate::parse("2025-13-01").is_none());
    assert!(CivilDate::parse("2025-02-30").is_none());
}

#[test]
fn fee_scaling_matches_published_2025_schedule() {
    // Large-entity nonprovisional with 5 independent + 25 total claims.
    let large = estimate_fees(Entity::Large, "non-provisional", 25, 5, false);
    // basic 350 + search 770 + exam 880 + 2*600 + 5*200 = 4200
    assert_eq!(large.total_usd as u64, 4200);
    let micro = estimate_fees(Entity::Micro, "non-provisional", 25, 5, false);
    // micro = 20% of each line: 70 + 154 + 176 + 2*120 + 5*40 = 840
    assert_eq!(micro.total_usd as u64, 840);
    let prov = estimate_fees(Entity::Small, "provisional", 30, 9, true);
    assert_eq!(prov.total_usd as u64, 130); // small provisional filing fee only
    assert_eq!(prov.line_items.len(), 1);
}

#[test]
fn deadlines_flag_missed_and_upcoming() {
    let report = analyze_deadlines(Some("2024-01-01"), None, None, Some("2025-06-01"));
    let np = report
        .milestones
        .iter()
        .find(|m| m.code == "nonprovisional-from-provisional")
        .unwrap();
    assert_eq!(np.due_date, "2025-01-01");
    assert!(np.days_remaining < 0);
    assert_eq!(np.status, "past");

    let fresh = analyze_deadlines(Some("2025-05-15"), None, None, Some("2025-06-01"));
    let np = fresh
        .milestones
        .iter()
        .find(|m| m.code == "nonprovisional-from-provisional")
        .unwrap();
    assert_eq!(np.due_date, "2026-05-15");
    assert_eq!(np.status, "ok");
}

#[test]
fn disclosure_warns_about_foreign_rights() {
    let report = analyze_deadlines(None, Some("2025-01-01"), None, Some("2025-06-01"));
    assert!(report
        .milestones
        .iter()
        .any(|m| m.code == "us-grace-period-bar"));
    assert!(report.warnings.iter().any(|w| w.contains("absolute novelty")));
}

#[test]
fn claim_audit_detects_independence_and_bad_dependency() {
    let claims = vec![
        "A widget comprising a frame and a sensor coupled to the frame.".to_string(),
        "The widget of claim 1, wherein the sensor is thermal.".to_string(),
        "The widget of claim 5, wherein the frame is metal.".to_string(),
    ];
    let audit = audit_claims(&claims, None);
    assert_eq!(audit.total_claims, 3);
    assert_eq!(audit.independent_claims, 1);
    assert_eq!(audit.dependent_claims, 2);
    assert!(audit
        .findings
        .iter()
        .any(|f| f.code == "invalid-claim-reference"));
}

#[test]
fn claim_audit_detects_multiple_dependent_and_forward_reference() {
    let claims = vec![
        "A method comprising sensing a value.".to_string(),
        "The method of claim 1, further comprising logging.".to_string(),
        "The method of any of claims 1 or 2, wherein the value is temperature.".to_string(),
    ];
    let audit = audit_claims(&claims, None);
    assert!(audit.has_multiple_dependent_claim);
    assert_eq!(audit.multiple_dependent_claims, 1);
    // Self/forward reference is rejected.
    let bad = vec!["The system of claim 2.".to_string(), "A system.".to_string()];
    let audit = audit_claims(&bad, None);
    assert!(audit
        .findings
        .iter()
        .any(|f| f.code == "improper-claim-dependency"));
}

#[test]
fn dependent_claims_inherit_parent_antecedents() {
    let claims = vec![
        "A gadget comprising a housing and a motor in the housing.".to_string(),
        "The gadget of claim 1, wherein the motor is electric.".to_string(),
        "The gadget of any of claims 1 or 2, wherein the housing is sealed.".to_string(),
    ];
    let audit = audit_claims(&claims, None);
    assert!(
        !audit.findings.iter().any(|f| f.code == "antecedent-basis"),
        "parent-introduced terms must not be flagged in dependent claims: {:?}",
        audit.findings
    );
    // A term that no ancestor introduced is still flagged.
    let novel = vec![
        "A gadget comprising a housing.".to_string(),
        "The gadget of claim 1, wherein the flywheel is balanced.".to_string(),
    ];
    let audit = audit_claims(&novel, None);
    assert!(audit
        .findings
        .iter()
        .any(|f| f.code == "antecedent-basis" && f.message.contains("flywheel")));
}

#[test]
fn validate_intake_rejects_oversized_list_items() {
    let mut request = example_request();
    request.novelty_claims = vec!["x".repeat(MAX_SHORT_TEXT_LEN + 1)];
    let err = validate_intake(&request).unwrap_err();
    assert!(err.contains("noveltyClaims"), "unexpected error: {err}");
    // A normal-sized item passes.
    let mut ok = example_request();
    ok.novelty_claims = vec!["a reasonable novelty point".to_string()];
    assert!(validate_intake(&ok).is_ok());
}

#[test]
fn audit_handles_multibyte_utf8_without_panicking() {
    // parse_claim_dependencies walks byte offsets over the lowercased text and
    // slices `lower[cursor..]`; multibyte UTF-8 next to "claim"/numbers is where
    // a char-boundary panic would surface. This must not panic.
    let claims = vec![
        "A café système comprising a naïve wîdget, 日本語.".to_string(),
        "The système of claim 1, wherein the wîdget café is 設計 — 1 to 3.".to_string(),
        "Claim™ café of any of claims 1 or 2, naïve 日本.".to_string(),
    ];
    let audit = audit_claims(&claims, Some("Abstract with café 日本語 ™ characters."));
    assert_eq!(audit.total_claims, 3);
    let (refs, _) = parse_claim_dependencies("The wîdget café of claims 1–2, 日本語.");
    assert!(refs.len() <= MAX_CLAIMS + 1);
}

#[test]
fn claim_range_expansion_is_bounded() {
    // A huge range parsed from untrusted digits must not blow up.
    let (refs, _) = parse_claim_dependencies("The system of claims 1 to 9999999999.");
    assert!(refs.len() <= MAX_CLAIMS + 1, "refs unbounded: {}", refs.len());
    // The out-of-range endpoint is still recorded so it gets flagged.
    assert!(refs.iter().any(|&r| r > MAX_CLAIMS));
    // And auditing such a claim terminates and flags it.
    let audit = audit_claims(
        &[
            "A system comprising a part.".to_string(),
            "The system of claims 1 to 9999999999.".to_string(),
        ],
        None,
    );
    assert!(audit
        .findings
        .iter()
        .any(|f| f.code == "invalid-claim-reference"));
}

#[test]
fn abstract_over_limit_is_flagged() {
    let long_abstract = "word ".repeat(180);
    let audit = audit_claims(&["A device.".to_string()], Some(&long_abstract));
    assert_eq!(audit.abstract_word_count, Some(180));
    assert!(audit.findings.iter().any(|f| f.code == "abstract-too-long"));
}

#[test]
fn antecedent_basis_flags_unintroduced_term() {
    let findings = antecedent_findings(1, "A device wherein the rotor spins.");
    assert!(findings.iter().any(|f| f.code == "antecedent-basis"));
    // Properly introduced term is not flagged.
    let ok = antecedent_findings(1, "A device comprising a rotor, wherein the rotor spins.");
    assert!(ok.is_empty());
}

#[test]
fn parse_ai_draft_handles_plain_and_fenced_json() {
    let plain = r#"{"abstract":"An abstract.","claims":["A device."],"sections":[{"heading":"Field","body":"..."}]}"#;
    let draft = parse_ai_draft(plain).expect("plain json");
    assert_eq!(draft.abstract_text, "An abstract.");
    assert_eq!(draft.claims.len(), 1);
    assert_eq!(draft.sections.len(), 1);

    let fenced = "```json\n{\"abstract\":\"X\",\"claims\":[\"A method.\"],\"sections\":[]}\n```";
    let draft = parse_ai_draft(fenced).expect("fenced json");
    assert_eq!(draft.claims, vec!["A method.".to_string()]);

    assert!(parse_ai_draft("not json at all").is_err());
}

#[test]
fn ai_output_schema_is_well_formed() {
    let schema = ai_output_schema();
    assert_eq!(schema["additionalProperties"], serde_json::json!(false));
    assert!(schema["properties"]["claims"]["items"]["type"] == "string");
    // Generated drafts feed straight back into the deterministic auditor.
    let draft = AiDraft {
        abstract_text: "An abstract.".to_string(),
        claims: vec![
            "A widget comprising a frame.".to_string(),
            "The widget of claim 1, wherein the frame is metal.".to_string(),
        ],
        sections: vec![],
    };
    let audit = audit_claims(&draft.claims, Some(&draft.abstract_text));
    assert_eq!(audit.independent_claims, 1);
    assert_eq!(audit.dependent_claims, 1);
}

#[test]
fn intake_brief_includes_core_fields() {
    let brief = intake_brief(&example_request());
    assert!(brief.contains("Adaptive thermal sensor array"));
    assert!(brief.contains("Novelty points"));
    assert!(brief.contains("Problem:"));
}

#[test]
fn generated_package_includes_fee_deadline_and_claim_audit() {
    let config = test_config();
    let mut request = example_request();
    request.provisional_filing_date = Some("2025-01-01".to_string());
    let package = build_package(&config, request).expect("package");
    assert_eq!(package.fee_estimate.entity, "micro");
    assert!(package.fee_estimate.total_usd > 0.0);
    assert!(!package.claim_audit.findings.is_empty() || package.claim_audit.total_claims > 0);
    assert!(package
        .deadlines
        .milestones
        .iter()
        .any(|m| m.code == "nonprovisional-from-provisional"));
}
