use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    catalog: Vec<Value>,
    families: Vec<String>,
    family_counts: BTreeMap<String, usize>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.boundary-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /boundaries/preflight/catalog",
            "GET /fabrication/boundaries/preflight/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/boundaries/catalog",
            "GET /fabrication/remediation/catalog",
            "GET /fabrication/release/preflight/catalog",
            "GET /fabrication/interventions/catalog",
            "GET /fabrication/decomposition/catalog",
            "POST /fabrication/instructions/analyze",
            "POST /fabrication/instructions/validate",
            "POST /fabrication/instructions/boundaries/review",
            "POST /fabrication/remediation/plan",
            "POST /fabrication/learning/outcomes"
        ],
        "boundaryCount": catalog.len(),
        "families": families,
        "familyCounts": family_counts,
        "preflightGroups": [
            {
                "group": "machine-failure-boundary-evidence-state",
                "requiredEvidence": [
                    "machine envelope, axis travel, spindle/nozzle/tool state, fixture clearance, material-machine compatibility, controller modal state, and dry-run or simulation trace evidence",
                    "failure boundary ID, source program ID, line/range, severity, machine-failure risk, release blocker, and recommended remediation evidence",
                    "release probe, hidden-state/POMDP belief, and retained artifact checksum evidence when boundary confidence is uncertain"
                ],
                "releaseBlockers": [
                    "machine-failure boundary lacks source line, detection source, simulation/probe evidence, or retained remediation action",
                    "machine-ready release requested while collision, envelope, controller, material, postprocess, fixture, or profile blocker remains unresolved",
                    "failure boundary is accepted without release-owner signoff and learning feedback"
                ]
            },
            {
                "group": "human-intervention-and-automation-gap-state",
                "requiredEvidence": [
                    "operator checkpoint, manual setup/recovery action, robot/automation capability, monitoring trigger, and safe stop/resume evidence",
                    "handoff instructions, intervention owner, release authority, and artifact links for each hidden manual step",
                    "learning observation that records whether human intervention recovered, failed, or changed the route"
                ],
                "releaseBlockers": [
                    "program requires manual recovery or automation fallback without an explicit intervention plan",
                    "human handoff is expected but no owner, stop point, instruction, or release authority is retained",
                    "automation capability gap is hidden inside generated instructions instead of surfaced as a release blocker"
                ]
            },
            {
                "group": "split-combine-and-remediation-boundary-state",
                "requiredEvidence": [
                    "split boundary, combine/assembly boundary, interface-control, decomposition target, datum transfer, recomposition, and quality evidence",
                    "remediation plan that names regeneration, instruction improvement, route split, assembly recomposition, rework, waiver, or human intervention",
                    "DES/MDP/POMDP/neural learning signals for the final boundary outcome and future route selection"
                ],
                "releaseBlockers": [
                    "single-piece route is infeasible but split/combine, interface, or assembly evidence is missing",
                    "combine or recomposition boundary lacks datum, fit, quality, workholding, or release-package evidence",
                    "remediation outcome is not tied to learning observations before release"
                ]
            }
        ],
        "responseSurfaces": [
            "validation.failureBoundaries",
            "boundarySummary.machineFailureRisks",
            "interventionMap.requiredActions",
            "operatorInterventionPlan.requiredOperatorActions",
            "releaseProbePlan.probes",
            "decompositionPlan.targets",
            "interfaceControlPlan.controls",
            "boundaryRemediationPlan.actions",
            "releasePackagePlan.releaseGates",
            "learningOutcome.observations"
        ],
        "releasePolicy": [
            "boundary preflight entries describe evidence required before trusting machine-failure, human-intervention, automation-gap, and split/combine decisions; they are not controller-certified safety results",
            "machine-ready release remains blocked while boundary source evidence, remediation action, release probe, intervention owner, split/combine route, or learning feedback is absent",
            "failed boundary preflight checks should feed DES, MDP/POMDP, and neural workers so future plans can reroute manufacturing, split parts, regenerate instructions, or require human intervention earlier"
        ],
        "boundaries": catalog
    })
}
