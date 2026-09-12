use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    contracts: Vec<Value>,
    families: Vec<String>,
    family_counts: BTreeMap<String, usize>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.boundary-remediation-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /remediation/catalog", "GET /fabrication/remediation/catalog"],
        "boundaryCount": contracts.len(),
        "families": families,
        "familyCounts": family_counts,
        "sourceCatalogRoutes": [
            "GET /boundaries/catalog",
            "GET /fabrication/boundaries/catalog",
            "GET /improvements/catalog",
            "GET /fabrication/improvements/catalog",
            "GET /decomposition/catalog",
            "GET /fabrication/decomposition/catalog",
            "GET /interventions/catalog",
            "GET /fabrication/interventions/catalog",
            "GET /release/catalog",
            "GET /fabrication/release/catalog"
        ],
        "analysisRoutes": ["POST /instructions/analyze", "POST /fabrication/instructions/analyze"],
        "validationRoutes": ["POST /instructions/validate", "POST /fabrication/instructions/validate"],
        "improvementRoutes": ["POST /instructions/improve", "POST /fabrication/instructions/improve"],
        "boundaryReviewRoutes": [
            "POST /instructions/boundaries/review",
            "POST /fabrication/instructions/boundaries/review"
        ],
        "responseSurfaces": [
            "validation.failureBoundaries",
            "boundarySummary",
            "resolutionPlan.steps",
            "interventionMap",
            "operatorInterventionPlan",
            "improvedPrograms.patchManifest",
            "decompositionPlan",
            "interfaceControlPlan",
            "machineRelease.blockers",
            "releasePackagePlan.requiredArtifacts",
            "learning.interventionSignals",
            "neuralTrainingCorpus.examples"
        ],
        "releasePolicy": [
            "boundary remediation catalog entries rank review lanes for generated and imported fabrication instructions; they do not certify corrected controller output",
            "machineReady=false remains mandatory until remediation evidence, validation, simulation or dry-run evidence, controller/postprocessor review, split/combine review, and operator or automation signoff clear",
            "remediation learning signals feed MDP/POMDP/neural workers so future requests can choose safer machines, split/combine routes, regenerated instructions, or human checkpoints before hardware execution"
        ],
        "remediationContracts": contracts
    })
}
