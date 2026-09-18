use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(entries: Vec<Value>, maintenance_families: Vec<String>) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.maintenance-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /maintenance/catalog", "GET /fabrication/maintenance/catalog"],
        "maintenanceFamilyCount": entries.len(),
        "maintenanceFamilies": maintenance_families,
        "reviewRoutes": [
            "POST /setup/result",
            "POST /fabrication/setup/result",
            "POST /calibration/result",
            "POST /fabrication/calibration/result",
            "POST /utilities/result",
            "POST /fabrication/utilities/result",
            "POST /monitoring/result",
            "POST /fabrication/monitoring/result",
            "POST /telemetry/result",
            "POST /fabrication/telemetry/result"
        ],
        "responseSurfaces": [
            "machineProfile.evidence.maintenance",
            "setupResult.datumReviews",
            "calibrationResult.probeReviews",
            "utilitiesResult.checks",
            "monitoringResult.alerts",
            "telemetryResult.boundaryCorrelations",
            "safetyResult.interlocks",
            "machineRelease.blockers",
            "learning.outcomes"
        ],
        "artifactSurfaces": [
            "maintenance-release-record",
            "service-work-order",
            "lockout-clearance-proof",
            "post-service-dry-run",
            "sensor-calibration-certificate",
            "safety-channel-test",
            "maintenance-learning-observations",
            "mdp-request.artifacts.maintenance"
        ],
        "releasePolicy": [
            "maintenance catalog entries describe service-readiness evidence contracts, not certified machine maintenance approval or regulatory lockout/tagout procedure",
            "machine-ready, unattended, and customer-ready release remain blocked when lockout, service, wear, calibration, sensor, process-support, or safety-channel evidence is stale or missing",
            "maintenance outcomes feed MDP/POMDP/neural workers so future planners can avoid brittle machines, add operator checkpoints, split work across healthier equipment, or schedule service before release"
        ],
        "maintenanceContracts": entries
    })
}
