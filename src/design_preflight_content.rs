use serde_json::{json, Value};

use super::{unique_sorted, SupportedDesignFormat, SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn catalog_response(
    contracts: Vec<Value>,
    formats: Vec<SupportedDesignFormat>,
) -> Value {
    let categories = unique_sorted(contracts.iter().filter_map(|contract| {
        contract
            .get("category")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let source_systems = unique_sorted(contracts.iter().filter_map(|contract| {
        contract
            .get("sourceSystem")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));

    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.design-preflight-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /design/preflight/catalog",
            "GET /fabrication/design/preflight/catalog"
        ],
        "relatedRoutes": [
            "GET /fabrication/formats/catalog",
            "GET /fabrication/design/import/catalog",
            "POST /fabrication/design/import/review",
            "POST /fabrication/design/convert/plan",
            "GET /fabrication/mesh-repair/catalog",
            "POST /fabrication/simulation/run",
            "POST /fabrication/quality/result"
        ],
        "formatContractCount": contracts.len(),
        "supportedFormatCount": formats.len(),
        "categories": categories,
        "sourceSystems": source_systems,
        "preflightGroups": [
            {
                "group": "source-identity-and-provenance-state",
                "requiredEvidence": [
                    "file name, source URI, source system, format, revision, and authoring-tool identity",
                    "native CAD ownership, license/export authority, and translator version evidence",
                    "ambiguous .prt/.asm source-system disambiguation for Creo/ProE, NX, SOLIDWORKS, or other native CAD",
                    "redacted URI, checksum, artifact retention, and source-to-neutral lineage"
                ],
                "releaseBlockers": [
                    "notes-only design input without source identity",
                    "ambiguous native CAD extension without source-system or translator evidence",
                    "missing checksum, revision, or provenance for downstream release package"
                ]
            },
            {
                "group": "geometry-units-and-feature-state",
                "requiredEvidence": [
                    "units, scale, coordinate frame, assembly transform, and tolerance basis",
                    "solid/surface/mesh topology, watertightness, normals, wall thickness, and feature preservation",
                    "PMI/GD&T, material/color/body metadata, configuration, and assembly mate review",
                    "neutral export comparison for STEP, IGES, Parasolid, ACIS, JT, STL, 3MF, OBJ, or slicer package outputs"
                ],
                "releaseBlockers": [
                    "unit or scale ambiguity before design generation or toolpath planning",
                    "non-manifold, missing-body, suppressed-feature, or assembly-transform ambiguity",
                    "PMI, tolerance, material, color, or slicer profile metadata not preserved when required"
                ]
            },
            {
                "group": "conversion-simulation-and-learning-state",
                "requiredEvidence": [
                    "worker lane selection and design-conversion request/result subject handoff",
                    "mesh repair, manufacturability, split/combine, and release-boundary review",
                    "simulation, first-article, metrology, or operator/automation signoff for the converted design",
                    "learning observations for translator success, topology drift, split requirement, and human-intervention boundaries"
                ],
                "releaseBlockers": [
                    "conversion worker result missing or failed",
                    "topology, manufacturability, split/combine, or human-intervention boundary unresolved",
                    "simulation, quality, release-package, or learning evidence missing for the exact converted artifact"
                ]
            }
        ],
        "responseSurfaces": [
            "designInputReview.inputs",
            "designInputReview.conversionPlan",
            "designExports.partExports",
            "meshRepairPlan.repairDomains",
            "manufacturabilityPlan.failureBoundaries",
            "machineRelease.releaseBlockers",
            "learningOutcomeDraft.observations"
        ],
        "releasePolicy": [
            "design preflight catalog entries describe CAD/model/slicer evidence required before generation, conversion, or machine-code release",
            "preflight evidence cannot bypass import worker results, mesh/topology review, simulation, setup, quality, or operator/automation signoff",
            "failed design preflight checks should be retained through design import, conversion, mesh repair, quality, and learning outcome routes so DES, MDP/POMDP, and neural workers can learn safer translators and split/combine strategies"
        ],
        "formatContracts": contracts
    })
}
