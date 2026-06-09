use serde_json::{json, Value};

use super::{unique_sorted, SCHEMA_VERSION, SERVICE_NAME};

fn entries() -> Vec<Value> {
    vec![
        json!({
            "toolpathFamily": "additive-slicer-path-and-extrusion-plan",
            "machineKinds": ["fdm-printer", "multi-material-fdm-printer", "pellet-fgf-printer", "robotic-additive-cell", "resin-printer", "material-jetting-printer"],
            "pathEvidence": [
                "slice path, extrusion/jet/exposure segments, layer order, seam, support, purge/prime, and tool/material change evidence",
                "temperature wait, bed/chamber state, first-layer, fan/cooling, acceleration, volumetric flow, and retraction mode evidence",
                "resume, homing, z-offset, material map, and postprocess handoff checks before machine-ready release"
            ],
            "releaseBlockers": [
                "positive extrusion, jetting, or exposure appears before temperature, material, positioning, and profile evidence",
                "tool/material change, pause/resume, or extrusion-mode transition is not followed by reset, purge/prime, or verification evidence",
                "path exceeds printer envelope, violates support strategy, or requires split/combine without interface-control evidence"
            ],
            "responseSurfaces": ["slicerPlan.profileEvidence", "supportStrategyPlan", "simulation.programs", "postprocessPlan.steps", "machineRelease.blockers"],
            "artifactSurfaces": ["slicer-profile", "generated-machine-program", "simulation-report", "release-package-plan"],
            "learningSignals": ["toolpath:additive", "extrusion-state:*", "temperature-wait:*", "support-removal:*"]
        }),
        json!({
            "toolpathFamily": "subtractive-cam-rough-finish-and-fixture-clearance",
            "machineKinds": ["vertical-mill", "horizontal-mill", "five-axis-mill", "rotary-indexer-mill", "cnc-router"],
            "pathEvidence": [
                "roughing, finishing, drilling, contouring, ramp/plunge, lead-in/out, rest-machining, and stock-to-leave evidence",
                "fixture, clamp, vise, vacuum, tab, work-offset, tool-length, cutter compensation, and arc-plane evidence",
                "feeds/speeds, spindle/coolant/chip evacuation state, tool-life envelope, and dry-run simulation evidence"
            ],
            "releaseBlockers": [
                "cutting or rapid plunge reaches stock before spindle, feed, workholding, datum, tool-length, or process-media evidence",
                "arc, cutter-comp, canned-cycle, coordinate-transform, or incremental modal state cannot be released safely",
                "fixture/clamp clearance, envelope, tool-life, or chip evacuation risk requires operator intervention"
            ],
            "responseSurfaces": ["toolingPlan.requirements", "fixturePlan.setups", "controllerPlan.requiredControllerChecks", "simulation.failureBoundaries", "machineRelease.blockers"],
            "artifactSurfaces": ["tooling-plan", "fixture-plan", "controller-plan", "simulation-report", "machine-code-programs"],
            "learningSignals": ["toolpath:subtractive", "fixture-clearance:*", "feed-speed:*", "modal-state:*"]
        }),
        json!({
            "toolpathFamily": "turning-millturn-threading-and-transfer",
            "machineKinds": ["lathe", "mill-turn-center", "swiss-turning-center"],
            "pathEvidence": [
                "turning, facing, grooving, boring, threading, part-off, live-tool, transfer, and pickoff path evidence",
                "chuck/collet/guide-bushing/tailstock/subspindle/catcher support, stickout, runout, and spindle-state evidence",
                "CSS/RPM cap, feed-per-rev, tool-nose compensation, turret tool change, and synchronization review"
            ],
            "releaseBlockers": [
                "threading, part-off, transfer, or live-tool path lacks feed-mode, support, spindle-state, or synchronization evidence",
                "tool-nose compensation, turret change, CSS, or spindle direction state cannot be released safely",
                "bar stock, part catcher, subspindle, or tailstock support requires human intervention before completion"
            ],
            "responseSurfaces": ["fixturePlan.setups", "toolingPlan.requirements", "controllerPlan.requiredControllerChecks", "executionPlan.stopPoints", "machineRelease.blockers"],
            "artifactSurfaces": ["lathe-program", "mill-turn-program", "simulation-report", "quality-plan"],
            "learningSignals": ["toolpath:turning", "threading-sync:*", "partoff-support:*", "spindle-transfer:*"]
        }),
        json!({
            "toolpathFamily": "sheet-cut-nesting-kerf-pierce-and-retention",
            "machineKinds": ["laser-sheet-cutter", "waterjet-sheet-cutter", "plasma-sheet-cutter", "wire-edm-sheet-cutter", "hot-wire-foam-cutter"],
            "pathEvidence": [
                "nest layout, common-line, kerf, pierce/thread, lead-in/out, cut order, tab/bridge, and slug/drop retention evidence",
                "assist gas, fume extraction, abrasive/pump pressure, dielectric/flushing, work clamp, table/slat/support media, and coupon evidence",
                "material thickness, cut chart, heat affected zone, edge quality, skeleton handling, and part traceability review"
            ],
            "releaseBlockers": [
                "feed cutting starts before process-media, pierce/thread, kerf, and cut-chart evidence",
                "support media stops before continued cutting without restart verification",
                "slug/drop, skeleton, tab, or part traceability risk requires operator intervention"
            ],
            "responseSurfaces": ["nestingResult", "processRecipeCatalog", "consumablesResult", "qualityPlan.measurementTargets", "machineRelease.blockers"],
            "artifactSurfaces": ["dd-sheet-nesting-json", "process-recipe-result", "simulation-report", "release-package-plan"],
            "learningSignals": ["toolpath:sheet-cutting", "kerf:*", "support-media:*", "drop-control:*"]
        }),
        json!({
            "toolpathFamily": "hybrid-split-combine-interface-and-recomposition-path",
            "machineKinds": ["hybrid-cell", "robotic-assembly-cell", "vertical-mill", "lathe", "fdm-printer", "laser-sheet-cutter"],
            "pathEvidence": [
                "per-part route path, interface datum, assembly/recomposition sequence, fixture handoff, and inspection checkpoint evidence",
                "printed/milled/turned/sheet-cut subpart traceability, kit labels, tolerance stackup, and joining or fastening path evidence",
                "operator stop points, automation handoffs, machine sequencing, and release bundle cross-links"
            ],
            "releaseBlockers": [
                "one-piece path cannot complete and split/combine interface evidence is absent",
                "recomposition, datum transfer, kit traceability, or inspection path requires human intervention",
                "machine sequence, fixture handoff, or release package lacks retained evidence for all subparts"
            ],
            "responseSurfaces": ["decompositionPlan.parts", "interfaceControlPlan.interfaces", "assemblyPlan.steps", "executionPlan.stopPoints", "releasePackagePlan.packages"],
            "artifactSurfaces": ["decomposition-result", "assembly-result", "toolpath-result", "release-bundle"],
            "learningSignals": ["toolpath:hybrid", "split-combine:*", "interface-datum:*", "recomposition:*"]
        }),
    ]
}

pub(super) fn response() -> Value {
    let entries = entries();
    let toolpath_families = unique_sorted(entries.iter().filter_map(|entry| {
        entry
            .get("toolpathFamily")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    }));
    let machine_kinds = unique_sorted(entries.iter().flat_map(|entry| {
        entry
            .get("machineKinds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
    }));

    json!({
        "ok": true,
        "service": SERVICE_NAME,
        "schemaVersion": "dd.fabrication.toolpath-catalog.v1",
        "serviceSchemaVersion": SCHEMA_VERSION,
        "routes": ["GET /toolpaths/catalog", "GET /fabrication/toolpaths/catalog"],
        "toolpathFamilyCount": entries.len(),
        "toolpathFamilies": toolpath_families,
        "machineKinds": machine_kinds,
        "planningRoutes": ["POST /toolpaths/plan", "POST /fabrication/toolpaths/plan"],
        "reviewRoutes": ["POST /toolpaths/result", "POST /fabrication/toolpaths/result"],
        "relatedCatalogRoutes": [
            "GET /fabrication/machine-code/catalog",
            "GET /fabrication/process-recipes/catalog",
            "GET /fabrication/nesting/catalog",
            "GET /fabrication/workholding/catalog",
            "GET /fabrication/simulation/catalog",
            "GET /fabrication/release/catalog"
        ],
        "responseSurfaces": [
            "toolpathPlan",
            "toolpathPlan.simulationTrace",
            "generatedPrograms",
            "controllerPlan.requiredControllerChecks",
            "fixturePlan.setups",
            "executionPlan.stopPoints",
            "releasePackagePlan.packages",
            "machineRelease.blockers"
        ],
        "artifactSurfaces": [
            "generated-machine-program",
            "machine-code-programs",
            "toolpath-result",
            "simulation-report",
            "release-package-plan",
            "mdp-request.artifacts.toolpathCatalog"
        ],
        "releasePolicy": [
            "toolpath catalog entries describe CAM, slicer, sheet-cut, turning, and hybrid path evidence contracts, not certified machine programs",
            "machine-ready release remains blocked until path geometry, feeds/speeds or process parameters, workholding/datum, controller modal state, simulation, and operator or automation handoff evidence clears",
            "toolpath planning and result observations are retained for MDP/POMDP/neural workers so future jobs can choose safer machines, split parts, adjust feeds, add fixtures, or insert human intervention earlier"
        ],
        "toolpathContracts": entries
    })
}
