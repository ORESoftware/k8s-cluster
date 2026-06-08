use serde_json::{json, Value};

pub(super) fn patch_release_checklist() -> Value {
    json!([
        {
            "gate": "immutable-source-retention",
            "requiredEvidence": [
                "original imported or generated instruction artifact URI and checksum",
                "patch manifest references original program id and operation ids",
                "patched preview is retained as a separate improved-program artifact"
            ],
            "blocks": ["releasePackagePlan.requiredArtifacts", "improvedPrograms.patchManifest"]
        },
        {
            "gate": "post-patch-validation",
            "requiredEvidence": [
                "validation rerun against patched instructions",
                "simulation, dry-run, backplot, or slicer/package review for changed motion or process state",
                "controller, firmware, slicer, or postprocessor compatibility review"
            ],
            "blocks": ["validation.findings", "simulation.failureBoundaries", "controllerPlan.releaseGates"]
        },
        {
            "gate": "human-or-automation-signoff",
            "requiredEvidence": [
                "reviewer disposition for human-review patch operations",
                "operator or automation signoff for added stops, setup checks, or split/combine handoffs",
                "learning outcome draft retains patch action, blocker, artifact, and reward hints"
            ],
            "blocks": ["operatorInterventionPlan.requiredOperatorActions", "machineRelease.blockers", "learning.outcomeDraft"]
        }
    ])
}
