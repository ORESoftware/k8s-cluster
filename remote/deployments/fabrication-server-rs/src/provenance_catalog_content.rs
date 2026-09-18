use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    provenance_families: Vec<String>,
    machine_kinds: Vec<String>,
    evidence_scopes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.provenance-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /provenance/catalog", "GET /fabrication/provenance/catalog"],
        "provenanceFamilyCount": entries.len(),
        "provenanceFamilies": provenance_families,
        "machineKinds": machine_kinds,
        "evidenceScopes": evidence_scopes,
        "planningRoutes": [
            "POST /provenance/plan",
            "POST /fabrication/provenance/plan",
            "POST /design/import/review",
            "POST /fabrication/design/import/review",
            "POST /instructions/validate",
            "POST /fabrication/instructions/validate",
            "POST /release/preview",
            "POST /fabrication/release/preview"
        ],
        "reviewRoutes": [
            "GET /jobs/:job_id/release-bundle",
            "GET /fabrication/jobs/:job_id/release-bundle",
            "GET /learning/outcomes",
            "GET /fabrication/learning/outcomes",
            "POST /outcomes",
            "POST /fabrication/outcomes"
        ],
        "responseSurfaces": [
            "designInputReview.conversionPlan",
            "designPackage.parts",
            "materialPlan.routeRequirements",
            "machineCodePackage.programs",
            "qualityPlan.measurementTargets",
            "releasePackagePlan.packages",
            "learning.policySnapshot",
            "learning.outcomes",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "design-package",
            "material-plan",
            "machine-code-package",
            "inspection-report",
            "release-bundle",
            "mdp-request.artifacts.provenanceLedger"
        ],
        "releasePolicy": [
            "provenance catalog entries describe design, material, machine-program, inspection, release, and learning lineage evidence, not certified quality records",
            "machine-ready release remains blocked until source artifacts, material lots, generated or imported programs, inspection results, release bundles, and learning outcomes have traceable hashes, revisions, review status, and signoff evidence",
            "artifact hashes, conversion logs, lot records, controller program digests, inspection dispositions, nonconformance decisions, and learning outcome lineage are retained as MDP/POMDP/neural learning signals"
        ],
        "provenanceContracts": entries
    })
}
