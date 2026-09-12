use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(entries: Vec<Value>, energy_families: Vec<String>) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.energy-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /energy/catalog", "GET /fabrication/energy/catalog"],
        "energyFamilyCount": entries.len(),
        "energyFamilies": energy_families,
        "planningRoutes": [
            "POST /plan",
            "POST /fabrication/plan",
            "POST /costing/result",
            "POST /fabrication/costing/result",
            "POST /utilities/result",
            "POST /fabrication/utilities/result",
            "POST /availability/result",
            "POST /fabrication/availability/result",
            "POST /learning/observe",
            "POST /fabrication/learning/observe"
        ],
        "responseSurfaces": [
            "scheduleResult.lanes",
            "costingResult.estimateFamilies",
            "utilitiesResult.checks",
            "availabilityResult.capacityWindows",
            "monitoringPlan.alerts",
            "telemetryResult.channels",
            "machineRelease.blockers",
            "learning.outcomes"
        ],
        "artifactSurfaces": [
            "energy-budget-record",
            "power-load-record",
            "thermal-load-record",
            "ups-recovery-record",
            "carbon-window-record",
            "energy-learning-observations",
            "mdp-request.artifacts.energy"
        ],
        "releasePolicy": [
            "energy catalog entries describe machine, process, and facility power evidence contracts, not utility billing, certified electrical design, or carbon-compliance approval",
            "machine-ready release remains blocked while heater, spindle, axis, beam, jet, pump, compressor, chiller, UPS, facility circuit, or thermal-load evidence is missing for the selected route",
            "energy outcomes are retained as costing, availability, schedule, maintenance, and MDP/POMDP/neural learning signals so future planners can split, combine, defer, or reroute brittle fabrication work"
        ],
        "energyContracts": entries
    })
}
