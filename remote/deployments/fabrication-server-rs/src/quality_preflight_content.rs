use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    inspection_contracts: Vec<Value>,
    measurement_contracts: Vec<Value>,
    families: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.quality-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /quality/preflight/catalog",
            "GET /fabrication/quality/preflight/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/quality/catalog",
            "GET /fabrication/cleanliness/preflight/catalog",
            "GET /fabrication/release/catalog",
            "POST /fabrication/quality/result",
            "POST /fabrication/dispositions/result",
            "POST /fabrication/release/result",
            "POST /fabrication/learning/outcomes"
        ],
        "inspectionContractCount": inspection_contracts.len(),
        "measurementContractCount": measurement_contracts.len(),
        "families": families,
        "preflightGroups": [
            {
                "group": "metrology-instrument-and-datum-state",
                "requiredEvidence": [
                    "calibrated instrument, probe, gauge, fixture, vision scale, CMM program, or scan setup evidence",
                    "datum scheme, coordinate frame, temperature soak, uncertainty, measurement units, and acceptance-band evidence",
                    "measurement artifact URI, checksum, feature map, revision, and operator or automation owner"
                ],
                "releaseBlockers": [
                    "measurement taken with expired calibration, missing datum reference, or implicit acceptance criteria",
                    "metrology artifact missing checksum, feature ID, tolerance source, or retained raw/result data",
                    "hidden interface, internal channel, lattice, or assembled feature not inspectable by the proposed method"
                ]
            },
            {
                "group": "first-article-final-fit-and-surface-state",
                "requiredEvidence": [
                    "first-article, in-process, final-fit, surface-finish, edge-quality, cleanliness, and material-process witness evidence",
                    "interface fit, hole/thread gauge, bearing/seal land, bondline, torque, leak, pull, functional, or visual acceptance evidence",
                    "sampling plan, critical-to-quality features, disposition owner, and reinspection trigger evidence"
                ],
                "releaseBlockers": [
                    "machine-ready release requested before first-article or final-fit evidence",
                    "surface finish, support scar, burr, FOD, residue, edge quality, or process witness evidence missing",
                    "assembly, packaging, coating, or human handoff before required quality gates clear"
                ]
            },
            {
                "group": "nonconformance-disposition-and-learning-state",
                "requiredEvidence": [
                    "nonconformance finding with measured deviation, affected feature, root cause, and disposition authority",
                    "rework, reinspect, scrap/remake, waiver, split/combine redesign, or human-intervention plan",
                    "learning observation for quality gate, measurement target, route risk, split/combine outcome, and recovered release"
                ],
                "releaseBlockers": [
                    "failed quality gate without disposition, reinspection, or release-owner evidence",
                    "rework route would violate material allowance, interface, strength, thermal, or surface requirements",
                    "split/combine or human-fit recovery expected but not planned as an intervention"
                ]
            }
        ],
        "responseSurfaces": [
            "qualityPlan.inspectionPoints",
            "qualityPlan.measurementTargets",
            "qualityResult.measurements",
            "qualityResult.findings",
            "dispositionResult.decisions",
            "releasePackagePlan.releaseGates",
            "machineRelease.releaseBlockers",
            "learningOutcome.observations"
        ],
        "releasePolicy": [
            "quality preflight entries describe evidence required before machine-ready release, assembly, packaging, or human handoff; they are not certified acceptance results",
            "quality preflight evidence cannot bypass retained measurements, calibrated instruments, cleanliness checks, disposition authority, release packages, or operator/automation signoff",
            "failed quality preflight checks should feed DES, MDP/POMDP, and neural workers so future plans can add inspection, split parts, reroute manufacturing, or require human intervention earlier"
        ],
        "inspectionContracts": inspection_contracts,
        "measurementContracts": measurement_contracts
    })
}
