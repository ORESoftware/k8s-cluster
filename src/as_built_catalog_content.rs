use serde_json::{json, Value};

use super::{SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn response(
    entries: Vec<Value>,
    as_built_families: Vec<String>,
    machine_kinds: Vec<String>,
    evidence_scopes: Vec<String>,
) -> Value {
    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.as-built-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /as-built/catalog", "GET /fabrication/as-built/catalog"],
        "asBuiltFamilyCount": entries.len(),
        "asBuiltFamilies": as_built_families,
        "machineKinds": machine_kinds,
        "evidenceScopes": evidence_scopes,
        "planningRoutes": [
            "POST /quality/result",
            "POST /fabrication/quality/result",
            "POST /handoff/result",
            "POST /fabrication/handoff/result",
            "POST /release/preview",
            "POST /fabrication/release/preview"
        ],
        "reviewRoutes": [
            "GET /quality/catalog",
            "GET /fabrication/quality/catalog",
            "GET /provenance/catalog",
            "GET /fabrication/provenance/catalog",
            "GET /learning/outcomes",
            "GET /fabrication/learning/outcomes"
        ],
        "responseSurfaces": [
            "qualityResult.measurements",
            "toolpathResult.simulationChecks",
            "releasePackagePlan.requiredArtifacts",
            "machineRelease.blockers",
            "decompositionPlan.parts",
            "interfaceControlPlan.controls",
            "handoffResult.evidence",
            "learning.outcomes"
        ],
        "artifactSurfaces": [
            "as-built-deviation-map",
            "as-built-scan-mesh",
            "as-built-cmm-report",
            "as-built-interface-fit-record",
            "as-built-learning-observations",
            "mdp-request.artifacts.asBuilt"
        ],
        "releasePolicy": [
            "as-built catalog entries describe actual geometry evidence contracts, not certified metrology acceptance",
            "machine-ready release remains blocked while scan, CMM, deviation-map, datum-alignment, interface-fit, or as-built lineage artifacts are absent or unresolved",
            "split/combine and hybrid-route release requires as-built interface evidence showing the recomposed actual geometry still satisfies the intended design package",
            "as-built deviations are retained for MDP/POMDP/neural workers so future planning can learn when to add inspection, split parts, change machines, reroute features, or require human signoff"
        ],
        "asBuiltContracts": entries
    })
}
