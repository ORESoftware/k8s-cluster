use serde_json::{json, Value};

pub(super) fn landing_page() -> Value {
    json!({
        "label": "Human fabrication overview",
        "routes": ["/landing", "/fabrication", "/fabrication/landing"],
        "describes": [
            "request intake for CAD, mesh, slicer, CAM, CNC, and text instructions",
            "design, instruction, machine-code, toolpath, simulation, and release evidence flow",
            "machine-failure boundaries, human-intervention gates, split/combine planning, and learning handoffs"
        ]
    })
}

pub(super) fn start_here() -> Value {
    json!({
        "humanOverview": "/fabrication",
        "workflowOverview": "/fabrication/how-it-works",
        "capabilities": "/fabrication/capabilities",
        "requestSchema": "/fabrication/schema",
        "examples": "/fabrication/examples",
        "apiDocs": "/api/docs",
        "operatorDashboard": "/grafana/fabrication",
        "operatorDashboardSignals": [
            "request intake",
            "release blockers",
            "NATS fanout",
            "learning feedback",
            "artifact ledgers",
            "runtime capacity"
        ]
    })
}

pub(super) fn capabilities() -> Value {
    json!([
        "hybrid additive/subtractive/turning process planning",
        "draft G-code and operator instruction generation",
        "standalone generated machine-program and instruction package creation",
        "existing instruction validation and improvement hints",
        "retained external instruction validation results for imported and generated programs",
        "standalone instruction improvement review and patch-manifest generation",
        "standalone instruction boundary review for machine-failure, human-intervention, automation, and split/combine gates",
        "simulation and dry-run catalog discovery for machine-envelope, clearance, collision, and remediation evidence",
        "CAD, organic-model, neutral mesh, and slicer project input review",
        "CAD/model/slicer import worker-lane discovery and release blockers",
        "standalone CAD/model/slicer import review and conversion-plan validation",
        "bounded machine profile evidence intake for calibration, tooling, fixtures, materials, support media, and blockers",
        "bounded job and artifact inspection",
        "fabrication outcome reward ingestion and policy snapshots",
        "machine-failure and human-intervention boundary detection",
        "machine-release blocker and release-package preview for fabrication intents",
        "hybrid make strategy candidate, learned preference, and MDP/POMDP policy handoff discovery",
        "learned hybrid strategy recommendation previews for open fabrication intents",
        "end-to-end fabrication workflow route, evidence, and learning handoff planning",
        "MDP/POMDP/DES/neural policy feature contract"
    ])
}
