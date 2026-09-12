use serde_json::{json, Value};

use super::{
    design_import_release_blockers_for_category, design_import_required_evidence_for_category,
    design_import_review_gates_for_category, design_import_worker_lane_for_category,
    design_strings, unique_sorted, DESIGN_FORMAT_SPECS,
    FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP,
    FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT, FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT,
    SCHEMA_VERSION, SERVICE_NAME,
};

pub(super) fn catalog_contracts() -> Vec<Value> {
    DESIGN_FORMAT_SPECS
        .iter()
        .map(|spec| {
            json!({
                "normalizedFormat": spec.normalized_format,
                "sourceSystem": spec.source_system,
                "ecosystem": spec.ecosystem,
                "category": spec.category,
                "acceptedExtensions": design_strings(spec.extensions),
                "aliases": design_strings(spec.aliases),
                "workerLane": design_import_worker_lane_for_category(spec.category),
                "requestSubject": FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT,
                "resultSubject": FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT,
                "queueGroup": FABRICATION_DESIGN_CONVERSION_REQUESTS_QUEUE_GROUP,
                "importStrategy": spec.import_strategy,
                "preferredNeutralExports": design_strings(spec.preferred_neutral_exports),
                "slicerTargets": design_strings(spec.slicer_targets),
                "requiredEvidence": design_import_required_evidence_for_category(spec.category),
                "reviewGates": design_import_review_gates_for_category(spec.category),
                "releaseBlockers": design_import_release_blockers_for_category(spec.category),
                "notes": [spec.note]
            })
        })
        .collect()
}

pub(super) fn translator_readiness_checklist() -> Vec<Value> {
    vec![
        json!({
            "checkId": "native-cad-translator-provenance",
            "appliesTo": ["PTC Creo / Pro/ENGINEER", "SOLIDWORKS", "Autodesk Fusion", "Siemens NX", "CATIA", "Onshape"],
            "requiredEvidence": [
                "source system and version",
                "translator or export worker identity",
                "configuration, assembly, and suppressed-feature review",
                "neutral export checksum or conversion result artifact"
            ],
            "blocks": ["designInputReview.conversionPlan", "machineRelease.blockers"],
            "learningSignals": ["cad-translator:*", "native-cad-version:*", "suppressed-feature-review:*"]
        }),
        json!({
            "checkId": "neutral-kernel-and-pmi-preservation",
            "appliesTo": ["STEP", "IGES", "Parasolid", "ACIS", "JT"],
            "requiredEvidence": [
                "units, coordinate frame, and body count",
                "PMI/GD&T and tolerance preservation review",
                "kernel version or schema version",
                "topology healing and B-rep comparison result"
            ],
            "blocks": ["designExports.partExports", "manufacturingHandoff.parts"],
            "learningSignals": ["neutral-kernel:*", "pmi-preservation:*", "topology-healing:*"]
        }),
        json!({
            "checkId": "mesh-slicer-profile-readiness",
            "appliesTo": ["STL", "3MF", "OBJ", "AMF", "PrusaSlicer", "OrcaSlicer", "Cura", "Bambu Studio", "Lychee Slicer", "Chitubox"],
            "requiredEvidence": [
                "mesh scale, watertightness, normals, and wall-thickness review",
                "material, color, or resin profile preservation when present",
                "slicer machine/material profile checksum",
                "support, orientation, first-layer or exposure review"
            ],
            "blocks": ["designExports.partExports", "generated-machine-program", "machineRelease.blockers"],
            "learningSignals": ["mesh-readiness:*", "slicer-profile:*", "support-orientation:*"]
        }),
        json!({
            "checkId": "sheet-profile-and-cam-handoff",
            "appliesTo": ["DXF", "DWG", "CAM setup JSON", "APT/CLDATA"],
            "requiredEvidence": [
                "drawing units, layer purpose, and revision",
                "closed contour, kerf, lead-in/out, tab, and nesting review",
                "stock thickness and sheet process recipe",
                "postprocessor/controller target or CAM setup lineage"
            ],
            "blocks": ["designExports.partExports", "machine-code-generation", "machineRelease.blockers"],
            "learningSignals": ["sheet-profile:*", "cam-handoff:*", "kerf-nesting:*"]
        }),
    ]
}

pub(super) fn catalog_response() -> Value {
    let contracts = catalog_contracts();
    let worker_lanes = unique_sorted(contracts.iter().filter_map(|contract| {
        contract
            .get("workerLane")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));
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
        "schemaVersion": "dd.fabrication.design-import-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": [
            "GET /formats/catalog",
            "GET /fabrication/formats/catalog",
            "GET /design/import/catalog",
            "GET /fabrication/design/import/catalog"
        ],
        "formatContractCount": contracts.len(),
        "workerLanes": worker_lanes,
        "categories": categories,
        "sourceSystems": source_systems,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "relatedCatalogRoutes": [
            "GET /design/formats",
            "GET /fabrication/design/formats",
            "GET /design/generation/catalog",
            "GET /fabrication/design/generation/catalog",
            "GET /handoff/catalog",
            "GET /fabrication/handoff/catalog"
        ],
        "conversionSubjects": {
            "requests": FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT,
            "results": FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT,
            "queueGroup": "dd-fabrication-design-conversion"
        },
        "responseSurfaces": [
            "designInputReview.inputs",
            "designInputReview.conversionPlan",
            "designPackage.parts.exportTargets",
            "designExports.partExports",
            "manufacturingHandoff.parts",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "design-input-review",
            "design-package",
            "design-export-bundle",
            "generated-design-export",
            "parametric-design",
            "mdp-request"
        ],
        "ambiguityPolicy": [
            "source identity must include fileName, sourceUri, format, or sourceSystem; notes alone do not authorize import",
            "sourceUri values are retained without userinfo, query strings, or fragments",
            "ambiguous native extensions such as .prt or .asm stay release-blocked until sourceSystem, translator, or neutral-export evidence is attached"
        ],
        "translatorReadinessChecklist": translator_readiness_checklist(),
        "releasePolicy": [
            "CAD/model/slicer import contracts describe review and conversion worker lanes, not certified fabrication geometry",
            "machine-ready release remains blocked until conversion results, topology/scale/profile review, neutral export checksums, simulation, and operator or automation signoff are retained",
            "conversion outcomes feed designInputReview, machineRelease blockers, and MDP/POMDP/neural training surfaces so future plans can learn which import lanes unblock or fail"
        ],
        "formatContracts": contracts
    })
}
