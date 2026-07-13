use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    workholding_families: Vec<String>,
    machine_kinds: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.workholding-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /workholding/preflight/catalog",
            "GET /fabrication/workholding/preflight/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/workholding/catalog",
            "GET /fabrication/setup/catalog",
            "GET /fabrication/tooling/catalog",
            "GET /fabrication/interfaces/preflight/catalog",
            "GET /fabrication/quality/preflight/catalog",
            "POST /fabrication/workholding/result",
            "POST /fabrication/simulation/run",
            "POST /fabrication/release/result",
            "POST /fabrication/learning/outcomes"
        ],
        "workholdingFamilyCount": entries.len(),
        "workholdingFamilies": workholding_families,
        "machineKinds": machine_kinds,
        "preflightGroups": [
            {
                "group": "stock-build-surface-and-primary-hold-state",
                "requiredEvidence": [
                    "build plate, vacuum table, vise, chuck, collet, pallet, fixture, nest, or support surface evidence",
                    "clamp force, adhesive tape, tab, brim, raft, tailstock, steady rest, or pin/support proof before the first material-removal or deposition move",
                    "stock stick-out, overhang, collision envelope, thermal drift, chip/debris clearance, and access-for-inspection evidence"
                ],
                "releaseBlockers": [
                    "machine motion begins before primary hold and support evidence is retained",
                    "part, stock, printed layer, slug, or cutoff can shift, tip, lift, vibrate, or collide without a retained mitigation",
                    "operator is expected to recover or re-clamp without a planned stop, re-probe, and release-owner signoff"
                ]
            },
            {
                "group": "datum-transfer-reprobe-and-clearance-state",
                "requiredEvidence": [
                    "datum scheme, work offset, probe/touch-off, fixture coordinate frame, orientation key, and setup-revision evidence",
                    "tool, nozzle, head, gripper, spindle, jaw, clamp, fixture, support, and robot path clearance proof",
                    "re-probe, re-zero, thermal soak, fixture-change, material-change, and machine-pause restart evidence"
                ],
                "releaseBlockers": [
                    "datum or work offset changes after setup without re-probe or operator/automation confirmation",
                    "toolpath, printhead, spindle, cutter, robot, slug, or transfer path can intersect the fixture or support hardware",
                    "restart after pause, tool change, media change, or fixture adjustment lacks retained verification"
                ]
            },
            {
                "group": "split-combine-fixture-and-human-intervention-state",
                "requiredEvidence": [
                    "assembly jig, bond clamp, press fixture, heat-set support, datum-transfer fixture, or recomposition nest proof",
                    "interface control, dry-fit, torque, cure, adhesive, vision/fiducial, gripper, and final metrology evidence for recomposed parts",
                    "human intervention stop point, operator instruction, hold/release authority, and learning observation for recoveries or failures"
                ],
                "releaseBlockers": [
                    "printed, milled, cut, or postprocessed pieces are combined before fixture and datum-transfer evidence",
                    "bond, press, torque, cure, heat-set, or robotic assembly operation lacks a retained hold and inspection plan",
                    "split/combine recovery or manual fit is expected but not represented as an intervention, disposition, and learning outcome"
                ]
            }
        ],
        "responseSurfaces": [
            "fixturePlan.setups",
            "fixturePlan.setups.requiredEvidence",
            "fixturePlan.setups.clearanceChecks",
            "fixturePlan.datumTransfers",
            "toolingPlan.requirements.workholding",
            "simulation.riskProfile.programRisks",
            "operatorInterventionPlan.requiredOperatorActions",
            "interfaceControlPlan.interfaces",
            "assemblyPlan.requiredEvidence",
            "releasePackagePlan.requiredArtifacts",
            "machineRelease.releaseBlockers",
            "learningOutcome.observations"
        ],
        "releasePolicy": [
            "workholding preflight entries describe evidence required before machine-ready release, unattended motion, recomposition, or human handoff; they are not certified fixture designs",
            "release remains blocked while stock, build plate, clamp, vacuum, chuck, support, datum transfer, clearance, or split/combine fixture evidence is absent",
            "failed workholding preflight checks should feed DES, MDP/POMDP, and neural workers so future plans can split jobs, add fixtures, insert re-probes, or require human intervention earlier"
        ],
        "workholdingFamiliesDetailed": entries
    })
}
