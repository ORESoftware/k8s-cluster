use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    tolerance_families: Vec<String>,
    machine_kinds: Vec<String>,
    geometry_scopes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.tolerance-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /tolerances/catalog", "GET /fabrication/tolerances/catalog"],
        "toleranceFamilyCount": entries.len(),
        "toleranceFamilies": tolerance_families,
        "machineKinds": machine_kinds,
        "geometryScopes": geometry_scopes,
        "planningRoutes": [
            "POST /quality/plan",
            "POST /fabrication/quality/plan",
            "POST /decomposition/plan",
            "POST /fabrication/decomposition/plan",
            "POST /assembly/plan",
            "POST /fabrication/assembly/plan"
        ],
        "reviewRoutes": [
            "POST /quality/result",
            "POST /fabrication/quality/result",
            "POST /release/preview",
            "POST /fabrication/release/preview",
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate"
        ],
        "responseSurfaces": [
            "designInputReview.pmi",
            "materialPlan.routeRequirements",
            "slicerPlan.profileEvidence",
            "fixturePlan.datumTransfers",
            "decompositionPlan.parts",
            "interfaceControlPlan.interfaces",
            "assemblyPlan.requiredEvidence",
            "qualityPlan.measurementTargets",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "quality-plan",
            "interface-control-plan",
            "assembly-plan",
            "inspection-report",
            "release-package-plan",
            "mdp-request.artifacts.toleranceEvidence"
        ],
        "releasePolicy": [
            "tolerance catalog entries describe dimensional, GD&T/PMI, fit, kerf, datum-transfer, and interface-control evidence, not certified inspection plans",
            "machine-ready release remains blocked until tolerance-critical features have material/process allowance, datum, metrology, inspection, and operator or automation signoff evidence",
            "coupon measurements, first-article results, gauge outcomes, kerf offsets, fit-up interventions, and split/combine stackups are retained as MDP/POMDP/neural learning signals"
        ],
        "toleranceContracts": entries
    })
}
