use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    gate_contracts: Vec<Value>,
    gate_types: Vec<String>,
    package_kinds: Vec<Value>,
    package_kind_names: Vec<String>,
    required_artifacts: Vec<&'static str>,
    blocker_sources: Vec<Value>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.release-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /release/preflight/catalog",
            "GET /fabrication/release/preflight/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/release/catalog",
            "GET /fabrication/design/preflight/catalog",
            "GET /fabrication/controllers/preflight/catalog",
            "GET /fabrication/workholding/preflight/catalog",
            "GET /fabrication/assembly/preflight/catalog",
            "GET /fabrication/quality/preflight/catalog",
            "POST /fabrication/release/preview",
            "POST /fabrication/release/result",
            "POST /fabrication/learning/outcomes"
        ],
        "packageKindCount": package_kinds.len(),
        "gateCount": gate_contracts.len(),
        "packageKinds": package_kind_names,
        "gateTypes": gate_types,
        "preflightGroups": [
            {
                "group": "manifest-artifact-and-checksum-state",
                "requiredEvidence": [
                    "design package, generated or imported program, machine code, controller plan, setup, fixture, simulation, quality, and release-package artifact URIs",
                    "checksums, revision IDs, material lot, machine profile, worker version, route package ID, and retained source request evidence",
                    "trace from printed, milled, turned, cut, EDM, postprocessed, and recomposed child artifacts into the release bundle"
                ],
                "releaseBlockers": [
                    "release bundle lacks artifact URI, checksum, revision, route package, or retained source request evidence",
                    "generated and improved instructions are mixed without explicit provenance and acceptance evidence",
                    "split/combine child packages cannot be traced to final assembly, inspection, and disposition artifacts"
                ]
            },
            {
                "group": "machine-controller-simulation-and-process-state",
                "requiredEvidence": [
                    "controller/postprocessor compatibility, machine envelope, modal state, coordinate frame, tool/nozzle/spindle state, and dry-run or simulation evidence",
                    "workholding, setup, calibration, consumables, process recipe, utility, environment, safety, maintenance, and monitoring release gates",
                    "machine-failure, human-intervention, hidden-state, and POMDP belief evidence for uncertain release boundaries"
                ],
                "releaseBlockers": [
                    "machine-ready release requested before controller, calibration, setup, simulation, monitoring, or process support gates clear",
                    "failure boundary predicts collision, out-of-envelope motion, missing consumable, unsafe process state, or hidden manual recovery",
                    "operator, automation, or robot handoff lacks a retained stop point and release owner"
                ]
            },
            {
                "group": "quality-disposition-signoff-and-learning-state",
                "requiredEvidence": [
                    "quality inspection, final fit, surface finish, cleanliness, first article, material witness, and acceptance-band evidence",
                    "nonconformance, rework, waiver, scrap/remake, split/combine redesign, and release-owner disposition evidence",
                    "learning outcome draft with release blockers, cleared gates, reward hint, route risk, and future planning signals"
                ],
                "releaseBlockers": [
                    "quality, cleanliness, final fit, disposition, or release owner evidence is missing before machine-ready handoff",
                    "failed gate is accepted without reinspection, waiver, or disposition authority",
                    "release outcome is not available to DES, MDP/POMDP, and neural workers for future route selection"
                ]
            }
        ],
        "responseSurfaces": [
            "releasePackagePlan.packages",
            "releasePackagePlan.releaseGates",
            "releasePackagePlan.requiredArtifacts",
            "releaseReadinessResult.manifestArtifacts",
            "releaseReadinessResult.decisions",
            "releaseReadinessResult.blockers",
            "machineRelease.blockers",
            "validation.failureBoundaries",
            "qualityResult.findings",
            "dispositionResult.decisions",
            "learningOutcome.observations"
        ],
        "releasePolicy": [
            "release preflight entries describe evidence required before machine-ready handoff, not certified equipment safety or production approval",
            "machine-ready release remains blocked while manifest, checksum, controller, simulation, workholding, quality, disposition, split/combine, signoff, or learning evidence is absent",
            "failed release preflight checks should feed DES, MDP/POMDP, and neural workers so future plans can add evidence gates, split jobs, reroute manufacturing, or require human intervention earlier"
        ],
        "requiredArtifacts": required_artifacts,
        "blockerSources": blocker_sources,
        "packageKindContracts": package_kinds,
        "gateContracts": gate_contracts
    })
}
