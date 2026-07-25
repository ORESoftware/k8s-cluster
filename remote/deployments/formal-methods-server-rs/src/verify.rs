use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
};

use regex::Regex;

use crate::{
    annotations::{AnnotatedExpr, AnnotationBlock, ParsedSource, VarDecl},
    expr::{collect_vars, expr_to_smt, parse_expr, Expr, SortHint},
    smt::{declarations_for, declarations_with_extras, run_z3, SmtResult, SmtStatus},
    state::Config,
    types::{Finding, FindingKind, Severity},
};

// ---------------------------------------------------------------------------
// verification engine
// ---------------------------------------------------------------------------

pub(crate) struct VerifyContext<'a> {
    pub(crate) config: &'a Config,
}

impl<'a> VerifyContext<'a> {
    fn finding(
        &self,
        kind: FindingKind,
        severity: Severity,
        file: &str,
        line: usize,
        end_line: usize,
        message: String,
        detail: Option<String>,
        goal: Option<String>,
        smt: Option<&SmtResult>,
        smt_query: Option<String>,
        reasoning: &'static str,
    ) -> Finding {
        Finding {
            kind,
            severity,
            file: file.to_string(),
            line,
            end_line,
            message,
            detail,
            goal,
            counterexample: smt.map(|r| r.model.clone()).filter(|m| !m.is_empty()),
            smt_query,
            solver_status: smt.map(|r| smt_status_label(&r.status).to_string()),
            reasoning: Some(reasoning),
        }
    }

    async fn check_unsat(&self, script: &str) -> SmtResult {
        match run_z3(self.config, script).await {
            Ok(result) => result,
            Err(message) => SmtResult {
                status: SmtStatus::Error,
                model: BTreeMap::new(),
                raw: message,
            },
        }
    }
}

fn smt_status_label(status: &SmtStatus) -> &'static str {
    match status {
        SmtStatus::Sat => "sat",
        SmtStatus::Unsat => "unsat",
        SmtStatus::Unknown => "unknown",
        SmtStatus::Error => "error",
    }
}

pub(crate) async fn verify_block(
    ctx: &VerifyContext<'_>,
    block: &AnnotationBlock,
    z3_calls: &AtomicU64,
    z3_failures: &AtomicU64,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    let mut assumption_smt: Vec<String> = Vec::new();
    let mut assumption_vars: HashSet<String> = HashSet::new();

    let mut parsed_assume: Vec<(AnnotatedExpr, Expr)> = Vec::new();
    for ann in block.assumes.iter().chain(block.requires.iter()) {
        match parse_expr(&ann.raw) {
            Ok(expr) => {
                collect_vars(&expr, &mut assumption_vars);
                match expr_to_smt(&expr) {
                    Ok(smt) => {
                        assumption_smt.push(smt);
                        parsed_assume.push((ann.clone(), expr));
                    }
                    Err(err) => {
                        findings.push(Finding {
                            kind: FindingKind::UnsupportedExpression,
                            severity: Severity::Warning,
                            file: block.file.clone(),
                            line: ann.line,
                            end_line: ann.line,
                            message: format!("could not encode assumption: {err}"),
                            detail: Some(ann.raw.clone()),
                            goal: None,
                            counterexample: None,
                            smt_query: None,
                            solver_status: None,
                            reasoning: Some("encoding"),
                        });
                    }
                }
            }
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not parse expression: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("parser"),
                });
            }
        }
    }

    // unsatisfiable preconditions: the function body is unreachable as specified.
    if !block.requires.is_empty() || !block.assumes.is_empty() {
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &assumption_vars));
        for smt in &assumption_smt {
            script.push_str(&format!("(assert {smt})\n"));
        }
        script.push_str("(check-sat)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        if matches!(result.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        if result.status == SmtStatus::Unsat {
            let last_line = block
                .requires
                .last()
                .or_else(|| block.assumes.last())
                .map(|a| a.line)
                .unwrap_or(block.end_line);
            findings.push(ctx.finding(
                FindingKind::UnsatisfiablePrecondition,
                Severity::Error,
                &block.file,
                block.start_line,
                last_line,
                "the conjunction of @requires/@assume is unsatisfiable; this contract can never be entered".to_string(),
                Some("Z3 proved that no values of the declared variables satisfy all assumptions.".to_string()),
                None,
                Some(&result),
                Some(script),
                "deduction: ⊢ ⊥ from ⋀ requires",
            ));
        }
    }

    // ensures and asserts: try to falsify the goal.
    let mut goal_units: Vec<(
        &'static str,
        FindingKind,
        Severity,
        &AnnotatedExpr,
        &'static str,
    )> = Vec::new();
    for ann in &block.ensures {
        goal_units.push((
            "ensures",
            FindingKind::PostconditionViolation,
            Severity::Error,
            ann,
            "deduction: search for ⋀ assumptions ∧ ¬ ensures",
        ));
    }
    for ann in &block.asserts {
        goal_units.push((
            "assert",
            FindingKind::AssertionViolation,
            Severity::Error,
            ann,
            "deduction: search for ⋀ assumptions ∧ ¬ assert",
        ));
    }

    for (label, kind, severity, ann, reasoning) in goal_units {
        let expr = match parse_expr(&ann.raw) {
            Ok(expr) => expr,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not parse @{label}: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("parser"),
                });
                continue;
            }
        };
        let mut vars = assumption_vars.clone();
        collect_vars(&expr, &mut vars);
        let goal_smt = match expr_to_smt(&expr) {
            Ok(smt) => smt,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not encode @{label}: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("encoding"),
                });
                continue;
            }
        };
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &vars));
        for smt in &assumption_smt {
            script.push_str(&format!("(assert {smt})\n"));
        }
        script.push_str(&format!("(assert (not {goal_smt}))\n"));
        script.push_str("(check-sat)\n(get-model)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        match result.status {
            SmtStatus::Sat => {
                findings.push(ctx.finding(
                    kind.clone(),
                    severity.clone(),
                    &block.file,
                    ann.line,
                    ann.line,
                    format!("@{label} can be violated under the declared assumptions"),
                    Some("Z3 found a model that satisfies all @requires/@assume but falsifies the goal.".to_string()),
                    Some(ann.raw.clone()),
                    Some(&result),
                    Some(script),
                    reasoning,
                ));
            }
            SmtStatus::Unsat => {
                // proved -- intentionally no finding.
            }
            SmtStatus::Unknown => {
                findings.push(
                    ctx.finding(
                        FindingKind::SolverUnknown,
                        Severity::Info,
                        &block.file,
                        ann.line,
                        ann.line,
                        format!("solver returned unknown for @{label}"),
                        Some(
                            "Z3 could not prove or refute this goal within the configured budget."
                                .to_string(),
                        ),
                        Some(ann.raw.clone()),
                        Some(&result),
                        Some(script),
                        reasoning,
                    ),
                );
            }
            SmtStatus::Error => {
                z3_failures.fetch_add(1, Ordering::Relaxed);
                findings.push(ctx.finding(
                    FindingKind::SolverUnknown,
                    Severity::Warning,
                    &block.file,
                    ann.line,
                    ann.line,
                    format!("solver error while checking @{label}"),
                    Some(result.raw.chars().take(300).collect()),
                    Some(ann.raw.clone()),
                    Some(&result),
                    Some(script),
                    reasoning,
                ));
            }
        }
    }

    // loop invariants: prove that requires entails invariant (initialisation).
    for ann in &block.invariants {
        let expr = match parse_expr(&ann.raw) {
            Ok(expr) => expr,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not parse @invariant: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("parser"),
                });
                continue;
            }
        };
        let mut vars = assumption_vars.clone();
        collect_vars(&expr, &mut vars);
        let goal_smt = match expr_to_smt(&expr) {
            Ok(smt) => smt,
            Err(err) => {
                findings.push(Finding {
                    kind: FindingKind::UnsupportedExpression,
                    severity: Severity::Warning,
                    file: block.file.clone(),
                    line: ann.line,
                    end_line: ann.line,
                    message: format!("could not encode @invariant: {err}"),
                    detail: Some(ann.raw.clone()),
                    goal: None,
                    counterexample: None,
                    smt_query: None,
                    solver_status: None,
                    reasoning: Some("encoding"),
                });
                continue;
            }
        };
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &vars));
        for smt in &assumption_smt {
            script.push_str(&format!("(assert {smt})\n"));
        }
        script.push_str(&format!("(assert (not {goal_smt}))\n"));
        script.push_str("(check-sat)\n(get-model)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        if matches!(result.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        match result.status {
            SmtStatus::Sat => {
                findings.push(
                    ctx.finding(
                        FindingKind::LoopInvariantNotEstablished,
                        Severity::Error,
                        &block.file,
                        ann.line,
                        ann.line,
                        "loop @invariant does not follow from the preceding @requires/@assume"
                            .to_string(),
                        Some(
                            "Induction base step: the invariant must hold on loop entry."
                                .to_string(),
                        ),
                        Some(ann.raw.clone()),
                        Some(&result),
                        Some(script),
                        "induction: base-step refutation",
                    ),
                );
            }
            SmtStatus::Unknown => {
                findings.push(ctx.finding(
                    FindingKind::SolverUnknown,
                    Severity::Info,
                    &block.file,
                    ann.line,
                    ann.line,
                    "solver returned unknown for @invariant".to_string(),
                    None,
                    Some(ann.raw.clone()),
                    Some(&result),
                    Some(script),
                    "induction: base-step",
                ));
            }
            _ => {}
        }
    }

    // variant must be non-negative under invariants & assumptions; this is a
    // lightweight termination sanity check (full preservation step requires a
    // primed-state encoding which we don't infer from comments).
    for variant in &block.variants {
        let expr = match parse_expr(&variant.raw) {
            Ok(expr) => expr,
            Err(_) => continue,
        };
        let mut vars = assumption_vars.clone();
        collect_vars(&expr, &mut vars);
        let smt = match expr_to_smt(&expr) {
            Ok(smt) => smt,
            Err(_) => continue,
        };
        let mut invariant_smt = Vec::new();
        for inv in &block.invariants {
            if let Ok(parsed) = parse_expr(&inv.raw) {
                collect_vars(&parsed, &mut vars);
                if let Ok(s) = expr_to_smt(&parsed) {
                    invariant_smt.push(s);
                }
            }
        }
        let mut script = String::new();
        script.push_str(&declarations_with_extras(&block.decls, &vars));
        for s in assumption_smt.iter().chain(invariant_smt.iter()) {
            script.push_str(&format!("(assert {s})\n"));
        }
        script.push_str(&format!("(assert (< {smt} 0))\n"));
        script.push_str("(check-sat)\n(get-model)\n");

        z3_calls.fetch_add(1, Ordering::Relaxed);
        let result = ctx.check_unsat(&script).await;
        if result.status == SmtStatus::Sat {
            findings.push(
                ctx.finding(
                    FindingKind::LoopVariantNotDecreasing,
                    Severity::Warning,
                    &block.file,
                    variant.line,
                    variant.line,
                    "@variant can be negative under the declared invariants".to_string(),
                    Some(
                        "Termination measures must remain non-negative on entry to each iteration."
                            .to_string(),
                    ),
                    Some(variant.raw.clone()),
                    Some(&result),
                    Some(script),
                    "induction: termination measure",
                ),
            );
        }
    }

    findings
}

// ---------------------------------------------------------------------------
// heuristic checks over plain source lines (no annotations required)
// ---------------------------------------------------------------------------

fn if_condition_pattern() -> Regex {
    Regex::new(r"\bif\s*\(([^()]+(?:\([^()]*\)[^()]*)*)\)").expect("if regex")
}

fn current_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

pub(crate) async fn heuristic_checks(
    ctx: &VerifyContext<'_>,
    parsed: &ParsedSource,
    decls_lookup: &HashMap<String, SortHint>,
    z3_calls: &AtomicU64,
    z3_failures: &AtomicU64,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let re = if_condition_pattern();

    // path conditions of currently open if-blocks: (indent, smt_condition, line)
    let mut stack: Vec<(usize, String, usize)> = Vec::new();

    for (line_no, raw_line) in &parsed.plain_lines {
        let line = raw_line.trim_end();
        let indent = current_indent(raw_line);

        while let Some((top_indent, _, _)) = stack.last() {
            if indent <= *top_indent && !line.is_empty() {
                stack.pop();
            } else {
                break;
            }
        }

        let cap = match re.captures(line) {
            Some(c) => c,
            None => continue,
        };
        let cond_raw = cap.get(1).unwrap().as_str().trim();
        if cond_raw.is_empty() {
            continue;
        }

        let expr = match parse_expr(cond_raw) {
            Ok(expr) => expr,
            Err(_) => continue,
        };
        let mut vars = HashSet::new();
        collect_vars(&expr, &mut vars);
        if vars.is_empty() {
            continue;
        }
        if !vars.iter().all(|v| decls_lookup.contains_key(v.as_str())) {
            continue;
        }
        let smt = match expr_to_smt(&expr) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let decls: Vec<VarDecl> = decls_lookup
            .iter()
            .filter(|(name, _)| vars.contains(name.as_str()))
            .map(|(name, sort)| VarDecl {
                name: name.clone(),
                sort: sort.clone(),
                line: *line_no,
            })
            .collect();

        // always-true check: assert (not cond) and look for sat.
        let mut script = String::new();
        script.push_str(&declarations_for(&decls));
        for (_, parent, _) in &stack {
            script.push_str(&format!("(assert {parent})\n"));
        }
        script.push_str(&format!("(assert (not {smt}))\n"));
        script.push_str("(check-sat)\n");
        z3_calls.fetch_add(1, Ordering::Relaxed);
        let r_true = ctx.check_unsat(&script).await;
        if matches!(r_true.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        let always_true = r_true.status == SmtStatus::Unsat;

        // always-false check: assert cond and look for sat.
        let mut script_false = String::new();
        script_false.push_str(&declarations_for(&decls));
        for (_, parent, _) in &stack {
            script_false.push_str(&format!("(assert {parent})\n"));
        }
        script_false.push_str(&format!("(assert {smt})\n"));
        script_false.push_str("(check-sat)\n");
        z3_calls.fetch_add(1, Ordering::Relaxed);
        let r_false = ctx.check_unsat(&script_false).await;
        if matches!(r_false.status, SmtStatus::Error) {
            z3_failures.fetch_add(1, Ordering::Relaxed);
        }
        let always_false = r_false.status == SmtStatus::Unsat;

        if always_true && !stack.is_empty() {
            findings.push(ctx.finding(
                FindingKind::TautologyAlwaysTrue,
                Severity::Warning,
                &parsed.file,
                *line_no,
                *line_no,
                format!("`if ({cond_raw})` is implied by the surrounding conditions"),
                Some(
                    "All enclosing if-branch conditions imply this one, so the test is redundant."
                        .to_string(),
                ),
                Some(cond_raw.to_string()),
                Some(&r_true),
                Some(script.clone()),
                "deduction: ⋀ outer ⊢ cond",
            ));
        } else if always_true {
            findings.push(ctx.finding(
                FindingKind::TautologyAlwaysTrue,
                Severity::Warning,
                &parsed.file,
                *line_no,
                *line_no,
                format!("`if ({cond_raw})` is always true for declared variables"),
                Some("This condition is a tautology over the declared variable sorts.".to_string()),
                Some(cond_raw.to_string()),
                Some(&r_true),
                Some(script.clone()),
                "deduction: ⊢ cond",
            ));
        } else if always_false && !stack.is_empty() {
            findings.push(ctx.finding(
                FindingKind::DeadNestedBranch,
                Severity::Error,
                &parsed.file,
                *line_no,
                *line_no,
                format!("nested `if ({cond_raw})` is unreachable from outer branch"),
                Some(
                    "The conjunction of outer path conditions contradicts this guard.".to_string(),
                ),
                Some(cond_raw.to_string()),
                Some(&r_false),
                Some(script_false.clone()),
                "deduction: ⋀ outer ∧ cond ⊢ ⊥",
            ));
        } else if always_false {
            findings.push(
                ctx.finding(
                    FindingKind::TautologyAlwaysFalse,
                    Severity::Warning,
                    &parsed.file,
                    *line_no,
                    *line_no,
                    format!("`if ({cond_raw})` is always false for declared variables"),
                    Some(
                        "This condition is a contradiction over the declared variable sorts."
                            .to_string(),
                    ),
                    Some(cond_raw.to_string()),
                    Some(&r_false),
                    Some(script_false.clone()),
                    "deduction: cond ⊢ ⊥",
                ),
            );
        }

        if !always_true && !always_false {
            stack.push((indent, smt, *line_no));
        }
    }

    findings
}
