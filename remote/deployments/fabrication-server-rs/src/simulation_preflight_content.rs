use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    risk_contracts: Vec<Value>,
    dry_run_contracts: Vec<Value>,
    risk_types: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.simulation-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /simulation/preflight/catalog",
            "GET /fabrication/simulation/preflight/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/simulation/catalog",
            "POST /fabrication/simulation/run",
            "POST /fabrication/simulation/result",
            "GET /fabrication/toolpaths/catalog",
            "GET /fabrication/machine-code/catalog",
            "GET /fabrication/release/preflight/catalog",
            "GET /fabrication/learning/preflight/catalog",
            "POST /fabrication/learning/outcomes"
        ],
        "riskContractCount": risk_contracts.len(),
        "dryRunContractCount": dry_run_contracts.len(),
        "riskTypes": risk_types,
        "preflightGroups": [
            {
                "group": "machine-envelope-fixture-and-datum-state",
                "requiredEvidence": [
                    "selected machine work envelope, axis limits, rotary/index limits, fixture and stock envelope, and datum/work-offset proof",
                    "tool/nozzle/beam/jet/end-effector clearance model, safe retract height, clamp/tab/hold-down state, and collision sweep",
                    "split/combine child-route envelope checks when a single machine cannot safely cover the full object"
                ],
                "releaseBlockers": [
                    "generated or imported program lacks machine envelope, fixture, work-offset, stock, datum, or clearance evidence before dry-run",
                    "axis extents, rotary sweep, robot reach, or sheet nesting exceed the selected machine or fixture envelope",
                    "simulation would hide a required human intervention, re-fixture, child-part split, or assembly recomposition gate"
                ]
            },
            {
                "group": "controller-process-and-program-state",
                "requiredEvidence": [
                    "controller/postprocessor dialect, units, coordinate mode, tool/nozzle/process state, feed/speed, and modal defaults for each simulated program",
                    "process-start evidence for spindle, beam, jet, heater, extrusion, assist media, coolant, chip evacuation, or equivalent process support",
                    "arc, canned-cycle, compensation, tool-length, threading, additive thermal, and sheet-cutting support-media assumptions surfaced before dry-run"
                ],
                "releaseBlockers": [
                    "simulation input omits controller dialect, modal state, units, work offset, feed/speed, or postprocessor evidence",
                    "simulated cutting, extrusion, beam, jet, or process feed occurs before process-start or support-media evidence",
                    "program trace cannot map findings back to source lines, generated programs, machine-code artifacts, or validation boundaries"
                ]
            },
            {
                "group": "dry-run-release-and-learning-state",
                "requiredEvidence": [
                    "retained simulation report, dry-run artifact URI/checksum, risk profile, findings, failure boundaries, and operator or automation signoff",
                    "release-package links to simulation, execution plan, machine release, quality plan, remediation, and required human interventions",
                    "DES, MDP/POMDP, and neural learning observations for reroute, split/combine, clearance, intervention, and reward updates"
                ],
                "releaseBlockers": [
                    "machine-ready release requested while simulation risk is blocked or review-required, dry-run artifacts are missing, or signoff remains open",
                    "risk findings are not promoted into failure boundaries, remediation actions, release probes, or operator intervention plans",
                    "learning workers cannot observe whether dry-run evidence reduced risk, forced split/combine, or required human intervention"
                ]
            }
        ],
        "responseSurfaces": [
            "simulation.programs",
            "simulation.programs.axisExtents",
            "simulation.programs.safeClearanceObserved",
            "simulation.programs.spindleOrHeatupObserved",
            "simulation.riskProfile",
            "simulation.riskProfile.programRisks",
            "simulation.riskProfile.learningObservations",
            "simulation.findings",
            "simulation.failureBoundaries",
            "validation.failureBoundaries",
            "machineRelease.blockers",
            "executionPlan.stopPoints",
            "releaseProbePlan.probes",
            "operatorInterventionPlan.requiredOperatorActions",
            "releasePackagePlan.requiredArtifacts"
        ],
        "artifactSurfaces": [
            "simulation-report",
            "analysis-simulation-report",
            "dry-run-or-simulation-report",
            "rotary-clearance-simulation-report",
            "robot-path-or-fixture-simulation-report",
            "mdp-request.artifacts.simulation",
            "mdp-request.artifacts.releaseProbePlan"
        ],
        "releasePolicy": [
            "simulation preflight entries describe required evidence before dry-run or simulation results can influence release; they do not certify machine execution",
            "machineReady remains false while envelope, fixture, datum, controller, process-start, dry-run artifact, risk, signoff, release-package, or learning evidence is missing",
            "failed simulation preflight checks should feed DES, MDP/POMDP, and neural workers so future plans can reroute, split or combine work, add clearance, regenerate programs, or require human intervention earlier"
        ]
    })
}
