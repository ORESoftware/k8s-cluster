# Legacy MDP, learning, and simulation disposition map

Status: **migration baseline; source documents still require section-by-section annotation**  
Tracks: DEN-961  
Canonical dependencies: DEN-912, DEN-914, DEN-923, DEN-926, DEN-934, DEN-935

This map prevents earlier research language from being mistaken for approved live architecture. The machine-readable action registry is authoritative for software execution. Legacy prose may remain as historical or negative-control material only when its status is explicit.

## Classification vocabulary

- **A — registered administrative:** eligible only through a registered action with required constraints and approvals.
- **B — constrained/human approval:** research or recommendation may be useful, but execution requires external hard constraints and named human approval.
- **C — simulation only:** may exist in synthetic research but cannot enter live observations, rewards, or execution.
- **D — legally unresolved:** blocked pending DEN-934 and independent review.
- **E — prohibited:** merits, credibility, outcome direction, protected rights, political/publicity pressure, or governance authority.
- **F — superseded:** duplicate prose replaced by a canonical registry entry or issue.

## High-risk concept dispositions

| Legacy concept | Class | Canonical disposition |
|---|---:|---|
| Automated `reject_frivolous` or merits-like intake rejection | E | Software may route a filing for human review; it may not decide legal frivolousness or merits. |
| Public/media salience as priority or expedite input | E | `public_media_salience` is globally prohibited. Expedite must use registered legal/safety deadlines and dual review. |
| Scalar judge or witness credibility scores | E | Both are prohibited observations and cannot influence assignment, evidence handling, voting, or review. |
| Latent truth, guilt/liability, witness reliability, expected verdict, remedy, punishment | E | Excluded from live and policy-training state. Synthetic negative controls must be isolated and labeled. |
| Verdict confidence, majority conformity, appeal outcome, ideological agreement rewards | E | No merits or conformity reward is eligible. Administrative rewards require DEN-926 and rights constraints. |
| Preference or inverse learning from historical merits decisions | E | Historical merits behavior cannot teach live policy preferences. |
| Learned panel size, quorum, threshold, notice, response, appeal, or recusal rules | E | Governance weights are immutable to models and require a reviewed registry/policy revision. |
| Individual productivity score or outcome quota | E | Capacity planning may use aggregate safe-workload inputs; retaliatory or outcome-linked individual scoring is prohibited. |
| Optimizer directly chooses final judge or panel | E | `assignment.build_slate` may build a feasible, conflict-cleared slate only; `assignment.public_lottery` performs final selection. |
| Intake routing by jurisdiction, language, accessibility, representation, filing type | A | Registered as `intake.route`; unknown features or missing approvals deny. |
| Hearing scheduling by availability, accessibility, language, deadlines, and capacity | B | Registered as `hearing.schedule`; notice and non-digital fallback are hard preconditions with human approval. |
| Judge/staff capacity forecasts | B | Registered advisory actions; qualification, safe workload, and nonbinding labels are mandatory. |
| Commit-reveal public lottery | B/D | Registered as a critical human-executed mechanism, but institutional/legal authority remains unresolved pending DEN-934. |
| Policy shadowing, canary, rollback | A/B | Registered as `policy.deploy`; promotion requires signed provenance, rights/security approvals, shadow evidence, and tested rollback. |

## Source documents to annotate

The following documents and overlapping generated pages must be audited section by section when their source repositories are available:

- `docs/mdp-corruption-courts.md`;
- `docs/anti-corruption-learning.md`;
- `docs/corruption-court-platform-sim.md`;
- `docs/anti-corruption-court-plan.md`;
- generated Astro pages derived from those drafts;
- MDP/POMDP state schemas, reward tables, assignment rules, simulation agents, and decision logs.

Each retained section needs a stable issue/requirement ID, classification, owner, current/superseded badge, constraints, approval path, evaluation ticket, and deployment gate. Generated pages must be checked against `policy/action-registry.v1.json` so a stale page cannot present a prohibited or unresolved concept as live capability.

## Akrion boundary

Reusable Akrion assets are domain-neutral Rust interfaces, deterministic clocks and replay, generic constraint and planning machinery, benchmarks, simulation infrastructure, and evaluation methods. Soccer observations, rewards, learned weights, tactics, data, and benchmark success do not validate a court procedure and must never be imported as policy evidence.

## CI follow-up

The baseline evaluator already fails closed on unregistered actions, unregistered observations, prohibited features, governance mutation, missing approvals, and missing preconditions. Follow-up CI should extract every live action/feature/reward reference from configuration and generated documentation, compare it with the registry, and fail on drift or unclassified content.
