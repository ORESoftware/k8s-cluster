use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    contracts: Vec<Value>,
    families: Vec<String>,
    release_gates: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.assembly-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /assembly/preflight/catalog",
            "GET /fabrication/assembly/preflight/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/assembly/catalog",
            "GET /fabrication/decomposition/catalog",
            "GET /fabrication/interfaces/preflight/catalog",
            "GET /fabrication/workholding/preflight/catalog",
            "GET /fabrication/quality/preflight/catalog",
            "POST /fabrication/assembly/plan",
            "POST /fabrication/assembly/result",
            "POST /fabrication/release/result",
            "POST /fabrication/learning/outcomes"
        ],
        "assemblyFamilyCount": families.len(),
        "assemblyFamilies": families,
        "releaseGateKinds": release_gates,
        "preflightGroups": [
            {
                "group": "child-route-package-and-interface-state",
                "requiredEvidence": [
                    "printed, milled, turned, sheet-cut, EDM, or special-process child route package IDs",
                    "interface-control IDs, datum transfer, orientation keys, tolerance stackups, and dry-fit evidence",
                    "retained child-route artifacts, checksums, material trace, and disposition status before recomposition"
                ],
                "releaseBlockers": [
                    "child route package is missing, stale, failed, or not traceable to retained artifacts",
                    "interface datum, tolerance, or orientation evidence is absent before parts are combined",
                    "split/combine boundary expects human fit correction without an explicit intervention and disposition plan"
                ]
            },
            {
                "group": "join-recipe-fixture-and-process-state",
                "requiredEvidence": [
                    "assembly fixture, recomposition nest, robot cell, press, weld, fastener, adhesive, cure, torque, or heat-set recipe evidence",
                    "workholding/preflight clearance, clamp or gripper force, access, collision, and process-window verification",
                    "operator or automation ownership for each manual fit, robot handoff, joining, cure, rework, or recovery stop"
                ],
                "releaseBlockers": [
                    "join operation lacks fixture, process, controller, cure, torque, or recipe evidence",
                    "robotic, press, weld, adhesive, or fastener operation can collide, overconstrain, undercure, or damage child parts",
                    "assembly process requires manual recovery but no planned stop, instruction, or release owner is retained"
                ]
            },
            {
                "group": "final-fit-quality-release-and-learning-state",
                "requiredEvidence": [
                    "final metrology, functional fit, leak/torque/pull/electrical test, visual inspection, and acceptance-band evidence",
                    "nonconformance, rework, remake, waiver, split/combine redesign, or scrap disposition evidence",
                    "learning observations for successful and failed recomposition paths, hidden human intervention, and route reliability"
                ],
                "releaseBlockers": [
                    "assembled object lacks final fit, quality, or functional proof before release-package handoff",
                    "out-of-tolerance interface or join result is not tied to disposition and reinspection evidence",
                    "outcome learning is absent for a split/combine decision that changed route, fixture, operator, or process risk"
                ]
            }
        ],
        "responseSurfaces": [
            "assembly.assemblyGraph",
            "hybridMakePlan.joinOperations",
            "hybridMakePlan.splitCombineDecisions",
            "interfaceControlPlan.controls",
            "fixturePlan.setups",
            "workholdingPreflightCatalog.preflightGroups",
            "qualityPlan.inspectionPoints",
            "releasePackagePlan.releaseGates",
            "machineRelease.releaseBlockers",
            "learningOutcome.observations"
        ],
        "releasePolicy": [
            "assembly preflight entries describe evidence required before child fabrication routes are combined into one released object; they are not certified assembly, robot-cell, or joining instructions",
            "machine-ready release remains blocked while child route packages, interface controls, join recipes, fixtures, final fit, quality, disposition, or operator/automation signoff evidence is absent",
            "failed assembly preflight checks should feed DES, MDP/POMDP, and neural workers so future plans can split, combine, recompose, redesign, reroute, or require human intervention earlier"
        ],
        "contracts": contracts
    })
}
