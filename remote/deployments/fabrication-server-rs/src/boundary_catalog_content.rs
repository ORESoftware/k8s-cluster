use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    catalog: Vec<Value>,
    families: Vec<String>,
    family_counts: BTreeMap<String, usize>,
    decision_matrix: Vec<Value>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.boundary-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /boundaries/catalog", "GET /fabrication/boundaries/catalog"],
        "boundaryCount": catalog.len(),
        "families": families,
        "familyCounts": family_counts,
        "analysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "responseSurfaces": [
            "validation.failureBoundaries",
            "boundarySummary",
            "resolutionPlan",
            "interventionMap",
            "operatorInterventionPlan",
            "releaseProbePlan",
            "decompositionPlan",
            "releasePackagePlan"
        ],
        "decisionMatrix": decision_matrix,
        "releasePolicy": [
            "boundary catalog entries describe analyzer coverage and release evidence, not controller-certified safety",
            "machine-ready release remains blocked while any cataloged machine-failure, human-intervention, split/combine, automation, postprocess, inspection, profile, or material boundary is unresolved",
            "boundary kinds are converted into MDP/POMDP/neural observations so workers can learn which jobs need regeneration, split/combine, automation proof, or human intervention"
        ],
        "boundaries": catalog
    })
}
