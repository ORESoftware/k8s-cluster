use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    strategy_families: Vec<String>,
    machine_kinds: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.support-strategy-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /support-strategies/catalog", "GET /fabrication/support-strategies/catalog"],
        "supportStrategyFamilyCount": entries.len(),
        "strategyFamilies": strategy_families,
        "machineKinds": machine_kinds,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan", "POST /decomposition/plan", "POST /fabrication/decomposition/plan"],
        "reviewRoutes": [
            "POST /instructions/analyze",
            "POST /fabrication/instructions/analyze",
            "POST /design/generate",
            "POST /fabrication/design/generate",
            "POST /simulation/run",
            "POST /fabrication/simulation/run",
            "POST /assembly/plan",
            "POST /fabrication/assembly/plan"
        ],
        "responseSurfaces": [
            "designInputReview.manufacturabilityEvidence",
            "slicerProfileCatalog",
            "fixturePlan.setups",
            "toolingPlan.requirements.workholding",
            "decompositionPlan.parts",
            "interfaceControlPlan.interfaces",
            "interventionMap.requiredInterventions",
            "interventionMap.splitCombineDecisions",
            "executionPlan.stopPoints",
            "postprocessPlan.requiredArtifacts",
            "qualityPlan.measurementTargets",
            "learning.outcomes",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "decomposition-plan",
            "interface-control-plan",
            "fixture-plan",
            "tooling-plan",
            "simulation-report",
            "assembly-plan",
            "learning-outcomes",
            "mdp-request.artifacts.decompositionPlan"
        ],
        "releasePolicy": [
            "support strategy catalog entries describe orientation, support, sacrificial-holding, tab, bridge, split/combine, and support-removal evidence contracts, not certified manufacturing instructions",
            "machine-ready release remains blocked while orientation, supports, tabs, bridges, sacrificial stock, support removal, postprocess access, or split/combine interface evidence is unresolved",
            "orientation, support, split/combine, and intervention outcomes are retained as MDP/POMDP/neural learning signals so future planners can choose one-piece, split, combine, or alternate-machine routes earlier"
        ],
        "supportStrategies": entries
    })
}
