use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{
    unique_sorted, SupportedDesignFormat, FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT,
    FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT, SCHEMA_VERSION, SERVICE_NAME,
};

fn category_counts(formats: &[SupportedDesignFormat]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for format in formats {
        *counts.entry(format.category.clone()).or_insert(0) += 1;
    }
    counts
}

pub(super) fn catalog_response(formats: Vec<SupportedDesignFormat>) -> Value {
    let source_systems = unique_sorted(formats.iter().map(|format| format.source_system.clone()));
    let ecosystems = unique_sorted(formats.iter().map(|format| format.ecosystem.clone()));
    let categories = unique_sorted(formats.iter().map(|format| format.category.clone()));
    let preferred_neutral_exports = unique_sorted(
        formats
            .iter()
            .flat_map(|format| format.preferred_neutral_exports.iter().cloned()),
    );
    let slicer_targets = unique_sorted(
        formats
            .iter()
            .flat_map(|format| format.slicer_targets.iter().cloned()),
    );

    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.design-format-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /design/formats", "GET /fabrication/design/formats"],
        "formatCount": formats.len(),
        "sourceSystems": source_systems,
        "ecosystems": ecosystems,
        "categories": categories,
        "categoryCounts": category_counts(&formats),
        "preferredNeutralExports": preferred_neutral_exports,
        "slicerTargets": slicer_targets,
        "conversionSubjects": {
            "requests": FABRICATION_DESIGN_CONVERSION_REQUESTS_SUBJECT,
            "results": FABRICATION_DESIGN_CONVERSION_RESULTS_SUBJECT,
            "queueGroup": "dd-fabrication-design-conversion"
        },
        "releasePolicy": [
            "native CAD, CAD-kernel, cloud CAD, lightweight PMI, mesh, scan, profile, and slicer project inputs are accepted as source evidence, not certified machine geometry",
            "machine-ready release stays blocked until translator output, topology/scale/profile review, simulation, and operator or automation signoff are attached",
            "prefer STEP or 3MF for mechanical CAD handoff, 3MF/STL/OBJ for mesh handoff, DXF/DWG for sheet profiles, and CAM setup JSON for controller-specific downstream workers"
        ],
        "formats": formats
    })
}
