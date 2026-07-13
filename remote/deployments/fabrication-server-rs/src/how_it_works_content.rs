use serde_json::{json, Value};

pub(super) fn start_here_workflow() -> Value {
    json!([
        {
            "step": "discover",
            "summary": "Inspect capabilities, machines, materials, templates, schemas, and API docs before selecting a worker lane.",
            "primaryRoutes": [
                "GET /fabrication/capabilities",
                "GET /fabrication/machines/catalog",
                "GET /fabrication/materials/catalog",
                "GET /fabrication/templates/catalog",
                "GET /api/docs"
            ],
            "evidenceGoal": "choose the right printer, mill, lathe, cutter, material, and request template before submitting work"
        },
        {
            "step": "import-or-generate",
            "summary": "Submit source geometry, slicer/CAM/controller inputs, or a design-generation request and keep every output as draft evidence.",
            "primaryRoutes": [
                "GET /fabrication/design/import/catalog",
                "GET /fabrication/design/generation/catalog",
                "GET /fabrication/instructions/languages",
                "GET /fabrication/machine-code/catalog",
                "POST /fabrication/design/generate"
            ],
            "evidenceGoal": "retain source provenance, generated design exports, instruction language, and controller/postprocessor targets"
        },
        {
            "step": "validate-and-improve",
            "summary": "Analyze imported or generated instructions, find release blockers, and create reviewable remediation or improvement drafts.",
            "primaryRoutes": [
                "GET /fabrication/instructions/validation/catalog",
                "GET /fabrication/boundaries/catalog",
                "GET /fabrication/remediation/catalog",
                "GET /fabrication/improvements/catalog",
                "POST /fabrication/instructions/validate"
            ],
            "evidenceGoal": "surface machine-failure, human-intervention, split/combine, simulation, and quality blockers before release"
        },
        {
            "step": "split-combine-release",
            "summary": "Decide whether to decompose, assemble, or release the retained package after setup, simulation, quality, and signoff evidence clears.",
            "primaryRoutes": [
                "GET /fabrication/decomposition/catalog",
                "GET /fabrication/assembly/catalog",
                "GET /fabrication/release/catalog",
                "GET /fabrication/artifacts/catalog",
                "POST /fabrication/release/preview"
            ],
            "evidenceGoal": "prove interfaces, joins, release gates, retained artifacts, and machineReady blockers before downstream approval"
        },
        {
            "step": "learn-from-results",
            "summary": "Feed completed, failed, remediated, or blocked outcomes back into DES/MDP/POMDP/neural policy memory.",
            "primaryRoutes": [
                "GET /fabrication/learning/engines/catalog",
                "GET /fabrication/learning/rewards/catalog",
                "GET /fabrication/learning/replay/catalog",
                "GET /fabrication/learning/outcomes",
                "POST /fabrication/learning/outcomes"
            ],
            "evidenceGoal": "keep learned preferences advisory until replay, simulation, retained evidence, and release blockers clear"
        }
    ])
}

pub(super) fn flow() -> Value {
    json!([
        {
            "step": "discover",
            "summary": "Find supported machines, CAD/model/slicer formats, instruction languages, templates, capabilities, and schemas.",
            "routes": [
                "GET /fabrication/capabilities",
                "GET /fabrication/formats/catalog",
                "GET /fabrication/instructions/languages",
                "GET /fabrication/templates/catalog",
                "GET /fabrication/schema"
            ],
            "releaseGate": "discovery data is advisory and does not certify machine readiness"
        },
        {
            "step": "intake",
            "summary": "Attach fabrication intent, CAD or mesh sources, slicer/CAM/controller programs, text job sheets, machine profiles, materials, and evidence.",
            "routes": [
                "POST /fabrication/design/import/review",
                "POST /fabrication/instructions/import/review",
                "POST /fabrication/plan"
            ],
            "releaseGate": "native CAD, ambiguous formats, imported programs, and text instructions stay blocked until translator, controller, setup, and source-system evidence is retained"
        },
        {
            "step": "generate",
            "summary": "Draft design packages, machine-code or printer instructions, toolpaths, setup sheets, schedules, decomposition plans, and assembly/interface plans.",
            "routes": [
                "POST /fabrication/design/generate",
                "POST /fabrication/machine-code/generate",
                "POST /fabrication/instructions/generate",
                "POST /fabrication/decomposition/plan",
                "POST /fabrication/assembly/plan",
                "POST /fabrication/schedule/plan"
            ],
            "releaseGate": "generated outputs are deterministic draft evidence, not certified controller, slicer, CAD, or fixture output"
        },
        {
            "step": "validate",
            "summary": "Analyze imported or generated instructions for machine-failure, human-intervention, automation, split/combine, simulation, quality, and postprocess boundaries.",
            "routes": [
                "POST /fabrication/instructions/analyze",
                "POST /fabrication/instructions/validate",
                "POST /fabrication/instructions/improve",
                "POST /fabrication/simulation/run",
                "GET /fabrication/boundaries/catalog"
            ],
            "releaseGate": "machineReady remains false while failure boundaries, human intervention, simulation, setup, controller, postprocess, or split/combine blockers remain unresolved"
        },
        {
            "step": "release",
            "summary": "Assemble retained artifacts, release probes, quality evidence, operator or automation signoff, and bundle manifests for review.",
            "routes": [
                "POST /fabrication/release/preview",
                "GET /fabrication/release/catalog",
                "GET /fabrication/jobs",
                "GET /fabrication/jobs/:job_id/release-bundle"
            ],
            "releaseGate": "release previews are compact review packets and do not by themselves approve a real machine run"
        },
        {
            "step": "learn",
            "summary": "Record outcomes, rewards, remediation decisions, replay evidence, and policy snapshots so future MDP/POMDP/DES/neural recommendations can improve.",
            "routes": [
                "POST /fabrication/learning/outcomes",
                "GET /fabrication/learning/outcomes",
                "GET /fabrication/learning/replay/catalog",
                "GET /fabrication/learning/optimizers/catalog",
                "GET /fabrication/learning/policy"
            ],
            "releaseGate": "learned preferences remain advisory until replay, simulation, retained evidence, and release blockers clear"
        }
    ])
}

pub(super) fn machine_families() -> Value {
    json!([
        "3D printers and slicer-driven additive systems",
        "vertical mills, horizontal mills, routers, five-axis, mill-turn, Swiss, and lathe systems",
        "laser, waterjet, plasma, wire EDM, sinker EDM, grinding, inspection, postprocess, and assembly cells",
        "hybrid split/combine routes that join printed, milled, turned, cut, inspected, or postprocessed parts"
    ])
}

pub(super) fn release_gate_matrix() -> Value {
    json!([
        {
            "gateId": "source-provenance",
            "label": "Source provenance",
            "proves": "CAD, mesh, CAM, slicer, controller, macro, and text job-sheet origins are identified before interpretation.",
            "blocks": ["translator release", "controller review", "machine-ready release"],
            "evidenceRoutes": ["POST /fabrication/design/import/review", "POST /fabrication/instructions/import/review"]
        },
        {
            "gateId": "machine-envelope",
            "label": "Machine envelope",
            "proves": "machine axes, fixtures, work offsets, tool length, stock, supports, and controller modal state fit the selected route.",
            "blocks": ["toolpath release", "machine-code release", "unattended run release"],
            "evidenceRoutes": ["POST /fabrication/machines/select", "POST /fabrication/toolpaths/result", "GET /fabrication/machines/catalog"]
        },
        {
            "gateId": "process-readiness",
            "label": "Process readiness",
            "proves": "thermal, extrusion, spindle, feed, coolant, dust, gas, abrasive, resin, powder, or filament state is ready for the operation.",
            "blocks": ["printer instruction release", "subtractive cutting release", "sheet-cutting release"],
            "evidenceRoutes": ["POST /fabrication/instructions/validate", "POST /fabrication/materials/result", "POST /fabrication/utilities/result"]
        },
        {
            "gateId": "simulation-evidence",
            "label": "Simulation evidence",
            "proves": "dry-run, collision, reach, support, quality, postprocess, and release-bundle reviews cleared known blockers.",
            "blocks": ["release preview", "machine-ready release", "learning promotion"],
            "evidenceRoutes": ["POST /fabrication/simulation/run", "POST /fabrication/quality/result", "POST /fabrication/release/preview"]
        },
        {
            "gateId": "human-or-automation-handoff",
            "label": "Human or automation handoff",
            "proves": "operator interventions, automation handoffs, split/combine joins, restart steps, and signoffs are retained.",
            "blocks": ["restart release", "split/combine release", "machine-ready release"],
            "evidenceRoutes": ["POST /fabrication/interventions/result", "POST /fabrication/assembly/result", "GET /fabrication/jobs/:job_id/release-bundle"]
        },
        {
            "gateId": "learning-disposition",
            "label": "Learning disposition",
            "proves": "MDP/POMDP/DES/neural recommendations are tied to retained outcomes and remain advisory until promotion evidence clears.",
            "blocks": ["policy promotion", "learned-route preference release", "unattended repeat-run release"],
            "evidenceRoutes": ["POST /fabrication/learning/outcomes", "GET /fabrication/learning/replay/catalog", "GET /fabrication/learning/policy"]
        }
    ])
}

pub(super) fn learning_contract() -> Value {
    json!({
        "preferredPrimitiveSource": "remote/submodules/discrete-event-system.rs des_engine",
        "methods": ["MDP", "POMDP", "DES", "neural policy evidence", "reward replay"],
        "promotionRule": "policy recommendations cannot promote to machine-ready release without retained evidence and cleared blockers"
    })
}

pub(super) fn priority_disposition_contract() -> Value {
    json!({
        "responseSurface": "priorityDispositions",
        "appliesTo": [
            "worker result review routes",
            "machine-code, simulation, remediation, interface, release, and learning handoffs",
            "human-intervention and split/combine boundary closure"
        ],
        "dispositions": ["blocked", "needs-review", "closed", "pending-blocker-resolution", "ready-for-learning"],
        "learningObservationShape": "<family>:<priority>:<disposition>",
        "releaseRule": "blocked and pending-blocker-resolution priority dispositions keep machineReady false until retained evidence clears the matching release gate"
    })
}

pub(super) fn operator_observability() -> Value {
    json!({
        "dashboard": "/grafana/fabrication",
        "grafanaUid": "dd-fabrication-planner",
        "signals": [
            "request intake",
            "release blockers",
            "NATS fanout",
            "learning feedback",
            "artifact ledgers",
            "runtime capacity"
        ],
        "releaseRule": "operators should inspect dashboard signals and retained release evidence before trusting generated or imported machine work"
    })
}
