use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    consumable_families: Vec<String>,
    machine_kinds: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.consumables-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /consumables/catalog", "GET /fabrication/consumables/catalog"],
        "consumableFamilyCount": entries.len(),
        "consumableFamilies": consumable_families,
        "machineKinds": machine_kinds,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan", "POST /materials/plan", "POST /fabrication/materials/plan"],
        "reviewRoutes": [
            "POST /materials/result",
            "POST /fabrication/materials/result",
            "POST /utilities/result",
            "POST /fabrication/utilities/result",
            "POST /toolpaths/result",
            "POST /fabrication/toolpaths/result",
            "POST /telemetry/result",
            "POST /fabrication/telemetry/result"
        ],
        "responseSurfaces": [
            "materialPlan.routeRequirements",
            "toolingPlan.requirements.consumables",
            "utilitiesResult.checks",
            "supportStrategyPlan.requirements",
            "monitoringPlan.alerts",
            "qualityResult.measurements",
            "postprocessPlan.requiredArtifacts",
            "provenanceResult.lineage",
            "machineRelease.blockers",
            "learning.outcomes"
        ],
        "artifactSurfaces": [
            "consumable-lot-record",
            "tool-life-record",
            "wear-inspection-record",
            "kerf-coupon-record",
            "powder-reuse-record",
            "purge-prime-record",
            "consumables-learning-observations",
            "mdp-request.artifacts.consumables"
        ],
        "releasePolicy": [
            "consumables catalog entries describe material, tool, support-media, and process-consumable evidence contracts, not certified inventory, tooling, or hazardous-material approval",
            "machine-ready and unattended release remain blocked when material quantity, lot, shelf-life, dry state, tool life, wear, nozzle, gas, abrasive, coolant, wire, resin, powder, binder, solvent, media, or postprocess consumable evidence is stale or missing",
            "consumable outcomes feed MDP/POMDP/neural workers so future planners can learn tool-life risk, material capacity, support-media depletion, split/combine reroutes, and operator refill checkpoints"
        ],
        "consumableContracts": entries
    })
}
