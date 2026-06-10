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
        "schemaVersion": "dd.fabrication.workholding-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /workholding/catalog", "GET /fabrication/workholding/catalog"],
        "workholdingFamilyCount": entries.len(),
        "workholdingFamilies": workholding_families,
        "machineKinds": machine_kinds,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan", "POST /setup/plan", "POST /fabrication/setup/plan", "POST /workholding/plan", "POST /fabrication/workholding/plan"],
        "reviewRoutes": [
            "POST /instructions/analyze",
            "POST /fabrication/instructions/analyze",
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate",
            "POST /simulation/run",
            "POST /fabrication/simulation/run",
            "POST /quality/plan",
            "POST /fabrication/quality/plan"
        ],
        "responseSurfaces": [
            "toolingPlan.requirements.workholding",
            "fixturePlan.setups",
            "fixturePlan.setups.requiredEvidence",
            "fixturePlan.setups.clearanceChecks",
            "fixturePlan.setups.workholding",
            "fixturePlan.datumTransfers",
            "simulation.riskProfile.programRisks",
            "operatorInterventionPlan.requiredOperatorActions",
            "interfaceControlPlan.interfaces",
            "decompositionPlan.parts",
            "assemblyPlan.requiredEvidence",
            "releasePackagePlan.requiredArtifacts",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "fixture-plan",
            "tooling-plan",
            "setup-plan",
            "simulation-report",
            "assembly-plan",
            "interface-control-plan",
            "mdp-request.artifacts.fixturePlan"
        ],
        "releasePolicy": [
            "workholding catalog entries describe evidence contracts for stock, build, fixture, support, retention, and recomposition holding, not certified fixture designs",
            "machine-ready release remains blocked while build-surface, clamp, vacuum, chuck, support, tab, nest, datum-transfer, or split/combine fixture evidence is unresolved",
            "workholding failures and successful fixture choices are retained as MDP/POMDP/neural learning signals so future planners can split jobs, change fixtures, add probes, or require human intervention earlier"
        ],
        "workholdingFamiliesDetailed": entries
    })
}
