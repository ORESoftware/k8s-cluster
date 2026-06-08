use serde_json::{json, Value};

use super::{unique_sorted, SCHEMA_VERSION, SERVICE_NAME};

pub(super) fn export_contracts() -> Vec<Value> {
    vec![
        json!({
            "format": "dd-parametric-csg-json",
            "consumer": "design-agent",
            "sourceSurface": "designPackage.parts.primitive",
            "artifactSurface": "parametric-design",
            "purpose": "authoritative editable planning primitive with coordinate frames and model intent",
            "releaseGate": "draft until model regeneration, simulation, quality, and machine-release evidence clear"
        }),
        json!({
            "format": "3MF",
            "consumer": "slicer",
            "sourceSurface": "designPackage.parts.exportTargets",
            "artifactSurface": "design-export-bundle.partExports",
            "purpose": "slicer-ready mesh package with material and orientation metadata",
            "releaseGate": "draft until slicer profile, support/orientation, mesh, and first-layer evidence clear"
        }),
        json!({
            "format": "STL",
            "consumer": "mesh-review",
            "sourceSurface": "designPackage.parts.exportTargets",
            "artifactSurface": "generated-design-export",
            "purpose": "neutral mesh handoff for additive review",
            "releaseGate": "draft until watertight/manifold/normals/wall-thickness review clears"
        }),
        json!({
            "format": "STEP",
            "consumer": "cam",
            "sourceSurface": "designPackage.parts.exportTargets",
            "artifactSurface": "generated-design-export",
            "purpose": "B-rep solid handoff for CAM feature recognition",
            "releaseGate": "draft until CAM regeneration, datum, simulation, and controller review clear"
        }),
        json!({
            "format": "DXF",
            "consumer": "sheet-cam",
            "sourceSurface": "designPackage.parts.exportTargets",
            "artifactSurface": "generated-design-export",
            "purpose": "2D sheet profile with kerf, lead-in, pierce, and tab metadata",
            "releaseGate": "draft until kerf, pierce, fume/support media, and part-retention evidence clear"
        }),
        json!({
            "format": "dd-cam-setup-json",
            "consumer": "cam-setup-agent",
            "sourceSurface": "designExports.partExports.content.camSetup",
            "artifactSurface": "design-export-bundle",
            "purpose": "datum, stock, fixture, tolerance, and operation setup handoff",
            "releaseGate": "draft until fixture/workholding, tool, and simulation evidence clear"
        }),
        json!({
            "format": "dd-sheet-nesting-json",
            "consumer": "nesting-agent",
            "sourceSurface": "designExports.partExports.content.nesting",
            "artifactSurface": "design-export-bundle",
            "purpose": "sheet nesting, kerf coupon, retained-tab, and support-media handoff",
            "releaseGate": "draft until nesting, cut recipe, and part-retention gates clear"
        }),
        json!({
            "format": "STEP-assembly",
            "consumer": "cad-cam-assembly",
            "sourceSurface": "designPackage.assemblyExports",
            "artifactSurface": "designExports.assemblyExports",
            "purpose": "neutral assembly handoff with part transforms and join references",
            "releaseGate": "draft until interface-control, dry-fit, datum transfer, and final metrology clear"
        }),
        json!({
            "format": "dd-assembly-graph-json",
            "consumer": "assembly-planner",
            "sourceSurface": "assembly.assemblyGraph",
            "artifactSurface": "designExports.assemblyExports",
            "purpose": "machine-readable join graph and split/combine design intent",
            "releaseGate": "draft until split/combine reviews and recomposition release gates clear"
        }),
        json!({
            "format": "operator-review-packet",
            "consumer": "operator",
            "sourceSurface": "manufacturingHandoff.parts",
            "artifactSurface": "manufacturing-handoff",
            "purpose": "special-process drawing, setup, inspection, and acceptance review packet",
            "releaseGate": "draft until operator signoff and machine-release blockers clear"
        }),
    ]
}

pub(super) fn handoff_contracts() -> Vec<Value> {
    vec![
        json!({
            "surface": "designPackage",
            "schemaVersion": "dd.fabrication.design-package.v1",
            "fields": ["representation", "units", "releaseState", "parts", "assemblyExports", "exportTargets", "blockers"],
            "usedFor": ["CAD/CAM/slicer export targets", "part coordinate frames", "model intent", "assembly export contracts"]
        }),
        json!({
            "surface": "designExports",
            "schemaVersion": "dd.fabrication.design-export-bundle.v1",
            "fields": ["partExports", "assemblyExports", "summary", "notes"],
            "usedFor": ["deterministic draft export payloads", "format/media-type dispatch", "blocked export accounting"]
        }),
        json!({
            "surface": "designInputReview",
            "schemaVersion": "design input review payload",
            "fields": ["inputs", "conversionPlan", "supportedFormats", "reviewRequiredCount"],
            "usedFor": ["source CAD/mesh/slicer review", "conversion worker dispatch", "release blockers for unsupported or ambiguous inputs"]
        }),
        json!({
            "surface": "manufacturingHandoff",
            "schemaVersion": "dd.fabrication.manufacturing-handoff.v1",
            "fields": ["machineReady", "reviewRequired", "parts", "releaseGates"],
            "usedFor": ["part-level geometry envelopes", "stock/datum/fixture setup", "program and process-node links", "release gates"]
        }),
        json!({
            "surface": "processGraph",
            "schemaVersion": "process graph response payload",
            "fields": ["nodes", "dependencies", "gates", "releaseState"],
            "usedFor": ["operation graph links", "generated program links", "release-gate propagation", "hybrid route dependencies"]
        }),
        json!({
            "surface": "hybridMakePlan",
            "schemaVersion": "hybrid make plan response payload",
            "fields": ["partRoutes", "joinOperations", "splitCombineDecisions", "learningObservations"],
            "usedFor": ["printed/milled/turned route combinations", "join planning", "split/combine learning"]
        }),
    ]
}

pub(super) fn catalog_response() -> Value {
    let export_contracts = export_contracts();
    let handoff_contracts = handoff_contracts();
    let export_formats = unique_sorted(export_contracts.iter().filter_map(|item| {
        item.get("format")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let consumers = unique_sorted(export_contracts.iter().filter_map(|item| {
        item.get("consumer")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));

    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.design-generation-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /design/generation/catalog", "GET /fabrication/design/generation/catalog"],
        "generationRoutes": ["POST /design/generate", "POST /fabrication/design/generate"],
        "exportContractCount": export_contracts.len(),
        "handoffContractCount": handoff_contracts.len(),
        "exportFormats": export_formats,
        "consumers": consumers,
        "planningRoutes": ["POST /plan", "POST /fabrication/plan"],
        "designInputRoutes": [
            "GET /design/formats",
            "GET /fabrication/design/formats",
            "GET /formats/catalog",
            "GET /fabrication/formats/catalog",
            "GET /design/import/catalog",
            "GET /fabrication/design/import/catalog"
        ],
        "responseSurfaces": [
            "designPackage",
            "designPackage.parts",
            "designPackage.parts.coordinateFrame",
            "designPackage.parts.primitive",
            "designPackage.parts.exportTargets",
            "designPackage.assemblyExports",
            "designExports",
            "designExports.partExports",
            "designExports.assemblyExports",
            "designExports.summary",
            "designInputReview",
            "designInputReview.conversionPlan",
            "manufacturingHandoff",
            "manufacturingHandoff.parts",
            "manufacturingHandoff.releaseGates",
            "processGraph.nodes",
            "processGraph.gates",
            "hybridMakePlan.splitCombineDecisions",
            "machineRelease.blockers",
            "releasePackagePlan.requiredArtifacts"
        ],
        "artifactSurfaces": [
            "design-summary",
            "parametric-design",
            "design-package",
            "design-export-bundle",
            "design-input-review",
            "generated-design-export",
            "generated-assembly-design-export",
            "manufacturing-handoff",
            "process-graph",
            "hybrid-make-plan",
            "mdp-request.artifacts.designPackage",
            "mdp-request.artifacts.designExports"
        ],
        "learningSurfaces": [
            "hybridMakePlan.learningObservations",
            "decompositionPlan.learningObservations",
            "interfaceControlPlan.learningObservations",
            "neuralTrainingCorpus.examples",
            "learning.interventionSignals"
        ],
        "releasePolicy": [
            "design generation catalog entries describe deterministic draft payloads and handoff contracts, not certified CAD, mesh, CAM, or controller output",
            "machine-ready release remains blocked while generated exports are blocked, design input conversion is unresolved, machine release is blocked, or manufacturing handoff gates require review",
            "design, export, handoff, and split/combine observations are emitted for MDP/POMDP/neural workers so future planning can learn when to regenerate geometry, split parts, combine assemblies, or choose alternate machines"
        ],
        "schemas": [
            "dd.fabrication.design-package.v1",
            "dd.fabrication.design-export-bundle.v1",
            "dd.fabrication.generated-design-export.v1",
            "dd.fabrication.generated-assembly-export.v1",
            "dd.fabrication.parametric-design.v1",
            "dd.fabrication.manufacturing-handoff.v1"
        ],
        "exportContracts": export_contracts,
        "handoffContracts": handoff_contracts
    })
}
