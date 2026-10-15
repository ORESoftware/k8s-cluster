# USA-ACC control constitution baseline

Status: **engineering baseline; not legal authorization or pilot approval**  
Tracks: DEN-912, DEN-935, DEN-961

The source of truth is `policy/action-registry.v1.json`. `src/control_constitution.rs` parses that registry and evaluates every proposed administrative action under a default-deny rule.

## Non-negotiable boundary

Software may assist with registered administrative and logistics decisions only. It must not predict, recommend, optimize, or learn legal merits, credibility, guilt, liability, verdict direction, remedy severity, punishment, protected-trait outcomes, political timing, public/media salience, quorum weights, voting thresholds, appeal rights, notice rules, recusal rules, or other governance authority.

A learned policy may propose an action that already exists in the registry. It may not add or alter registry entries, approvals, preconditions, governance weights, rights, or rollback rules.

## Decision record

Every evaluation returns:

- registry schema and policy version;
- requested action ID;
- allow or deny outcome;
- each satisfied observation, approval, and precondition;
- every denial reason;
- required approvals;
- governing issue or policy references.

Unknown actions and observations deny. Missing approvals and false or absent preconditions deny. Registry mutation and governance-weight mutation deny. The evaluator never guesses approval or silently drops a failed constraint.

## Initial registered surfaces

The baseline covers:

1. intake routing;
2. priority or expedite review;
3. evidence-task routing;
4. judge-pool capacity advice;
5. staff-capacity advice;
6. hearing scheduling;
7. conflict-cleared feasible-slate construction;
8. commit-reveal public lottery execution;
9. quorum recovery;
10. notification scheduling;
11. policy deployment.

Final judge or panel selection is not an optimizer action. Optimization may produce only an auditable feasible slate; a separately approved public lottery selects from that slate.

## Promotion and rollback

A candidate policy remains advisory until its registered approvals and preconditions pass. Deployment requires signed provenance, offline/shadow evidence, rights/security review, tested rollback, and a frozen policy version for active matters. A throughput or reward improvement cannot waive a registry constraint.

## Review ownership

Legal, governance, privacy, accessibility, security, and community reviewers should annotate registry sources and requirements. They must not need to modify model code. Legal feasibility remains blocked on DEN-934; this baseline defines a safe software boundary but does not establish that USA-ACC may exercise governmental or adjudicative authority.
