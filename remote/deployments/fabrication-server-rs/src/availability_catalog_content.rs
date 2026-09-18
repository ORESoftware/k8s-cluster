use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(entries: Vec<Value>, availability_families: Vec<String>) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.availability-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /availability/catalog", "GET /fabrication/availability/catalog"],
        "availabilityFamilyCount": entries.len(),
        "availabilityFamilies": availability_families,
        "reviewRoutes": [
            "POST /machines/select",
            "POST /fabrication/machines/select",
            "POST /schedule/result",
            "POST /fabrication/schedule/result",
            "POST /utilities/result",
            "POST /fabrication/utilities/result",
            "GET /maintenance/catalog",
            "GET /fabrication/maintenance/catalog",
            "POST /learning/outcomes",
            "POST /fabrication/learning/outcomes"
        ],
        "responseSurfaces": [
            "machineSelection.candidates",
            "machineSchedule.machineLanes",
            "scheduleResult.holds",
            "materialPlan.routeRequirements",
            "toolingPlan.requirements",
            "utilitiesResult.checks",
            "maintenanceContracts",
            "operatorInterventionPlan.requiredOperatorActions",
            "machineRelease.blockers",
            "learning.outcomes"
        ],
        "artifactSurfaces": [
            "availability-snapshot",
            "machine-capacity-window",
            "queue-depth-report",
            "operator-coverage-plan",
            "fallback-machine-plan",
            "split-combine-capacity-model",
            "availability-learning-observations",
            "mdp-request.artifacts.availability"
        ],
        "releasePolicy": [
            "availability catalog entries describe capacity and readiness evidence contracts, not certified shop scheduling authority or guaranteed machine uptime",
            "machine-ready and unattended release remain blocked when live machine state, queue, material, tooling, fixture, utility, maintenance, operator, or automation capacity evidence is stale or missing",
            "availability outcomes feed DES, MDP, POMDP, and neural workers so future planners can learn fallback machines, split/combine capacity, queue-delay risk, and reliable unattended windows"
        ],
        "availabilityContracts": entries
    })
}
