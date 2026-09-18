use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    kinematic_families: Vec<String>,
    machine_kinds: Vec<String>,
    axes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.kinematics-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /kinematics/catalog", "GET /fabrication/kinematics/catalog"],
        "kinematicFamilyCount": entries.len(),
        "kinematicFamilies": kinematic_families,
        "machineKinds": machine_kinds,
        "axes": axes,
        "reviewRoutes": [
            "POST /simulation/run",
            "POST /fabrication/simulation/run",
            "POST /toolpaths/plan",
            "POST /fabrication/toolpaths/plan",
            "POST /machine-code/generate",
            "POST /fabrication/machine-code/generate",
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate"
        ],
        "responseSurfaces": [
            "simulation.axisExtents",
            "simulation.riskProfile.programRisks",
            "controllerPlan.requiredControllerChecks",
            "fixturePlan.setups.requiredEvidence",
            "monitoringPlan.monitorPoints",
            "releaseProbePlan.probes",
            "machineRelease.blockers"
        ],
        "releasePolicy": [
            "kinematics catalog entries describe required axis, coordinate-mode, TCP/frame, envelope, fixture-clearance, synchronization, and probe evidence, not certified kinematic calibration records",
            "machine-ready release remains blocked until homing, units, coordinate state, axis envelope, rotary/robot frame, fixture clearance, simulation, and operator or automation signoff evidence clear",
            "axis-envelope, coordinate-mode, TCP/frame, external-axis, spindle-sync, and clearance observations are retained as MDP/POMDP/neural learning signals for future program generation and validation"
        ],
        "kinematics": entries
    })
}
