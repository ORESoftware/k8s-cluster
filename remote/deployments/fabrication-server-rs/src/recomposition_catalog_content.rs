use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response() -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.recomposition-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /recomposition/catalog", "GET /fabrication/recomposition/catalog"],
        "relatedDiscoveryRoutes": [
            "GET /fabrication/decomposition/catalog",
            "GET /fabrication/assembly/catalog",
            "GET /fabrication/interfaces/catalog",
            "GET /fabrication/interfaces/preflight/catalog",
            "GET /fabrication/joining/catalog",
            "GET /fabrication/quality/catalog",
            "GET /fabrication/release/catalog",
            "GET /fabrication/learning/engines/catalog"
        ],
        "recompositionStages": [
            {
                "stage": "child-route-package-intake",
                "requiredEvidence": [
                    "printed, milled, turned, sheet-cut, EDM, molded, cast, electronics, or special-process child route package IDs",
                    "child artifact URI, checksum, revision, material, process, disposition, and release status",
                    "single-piece fallback or split/combine rationale retained from decomposition and manufacturability review"
                ],
                "blockedSurfaces": [
                    "decompositionPlan.targets",
                    "releasePackagePlan.packages",
                    "machineRelease.blockers"
                ]
            },
            {
                "stage": "interface-control-and-datum-transfer",
                "requiredEvidence": [
                    "interface-control ID, datum-transfer map, mating orientation, keying, clocking, and anti-rotation proof",
                    "probe, scan, CMM, dry-fit, fixture repeatability, work-offset, or measured coordinate transfer evidence",
                    "critical-to-function dimensions, tolerance stack, clearance/interference, and postprocess allowance evidence"
                ],
                "blockedSurfaces": [
                    "interfaceControlPlan.controls",
                    "qualityPlan.measurementTargets",
                    "assemblyPlan.joinSteps"
                ]
            },
            {
                "stage": "joining-and-final-release",
                "requiredEvidence": [
                    "joining method selection, recipe, access path, fixture, clamp, torque, weld, adhesive, insert, fastener, or serviceability evidence",
                    "post-join metrology, functional proof, leak/torque/pull/runout/electrical check, nonconformance disposition, and signoff evidence",
                    "learning outcome with split/combine decision, recomposition result, reward hint, and retained artifacts"
                ],
                "blockedSurfaces": [
                    "joiningCatalog.joiningFamilies",
                    "qualityResult.metrologyChecks",
                    "releaseReadiness.releaseBlockers",
                    "learningOutcome.observations"
                ]
            }
        ],
        "boundarySignals": [
            "recomposition-child-package-boundary",
            "recomposition-interface-control-boundary",
            "recomposition-joining-release-boundary",
            "split-combine-interface-control-boundary",
            "human-intervention-boundary"
        ],
        "responseSurfaces": [
            "decompositionPlan.recompositionInterfaces",
            "interfaceControlPlan.releaseGates",
            "assemblyPlan.joinSteps",
            "hybridMakePlan.splitCombineDecisions",
            "releasePackagePlan.packages",
            "learningOutcome.observations"
        ],
        "planningRoutes": [
            "POST /fabrication/decomposition/plan",
            "POST /fabrication/assembly/plan",
            "POST /fabrication/release/preview",
            "POST /fabrication/workflow/plan"
        ],
        "resultRoutes": [
            "POST /fabrication/decomposition/result",
            "POST /fabrication/assembly/result",
            "POST /fabrication/interfaces/result",
            "POST /fabrication/joining/result",
            "POST /fabrication/release/result",
            "POST /fabrication/learning/outcomes"
        ],
        "releasePolicy": [
            "recomposition catalog entries describe how separately fabricated child parts become one releasable object; they are evidence contracts, not released assembly work instructions",
            "machine-ready release remains blocked until child route packages, interface-control evidence, datum transfer, fit stackup, joining method, final inspection, retained artifacts, and operator or automation signoff are attached to release surfaces",
            "recomposition outcomes should feed DES, MDP/POMDP, and neural workers so future plans can compare single-piece fabrication, split/combine routes, joining methods, rework loops, and human-intervention checkpoints"
        ]
    })
}
