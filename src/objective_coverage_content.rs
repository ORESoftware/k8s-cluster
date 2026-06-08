use serde_json::{json, Value};

pub(super) fn matrix() -> Vec<Value> {
    vec![
        json!({
            "requirement": "3d-printing-and-hybrid-intake",
            "covers": "accept fabrication requests for additive printers, slicers, native CAD exports, submitted machine profiles, and hybrid printed/milled/turned assemblies",
            "primaryRoutes": [
                "POST /fabrication/plan",
                "POST /fabrication/design/import/review",
                "POST /fabrication/design/generate",
                "GET /fabrication/design/formats",
                "GET /fabrication/slicers/catalog",
                "GET /fabrication/printers/catalog"
            ],
            "evidenceSurfaces": ["designInputReview", "materialPlan", "slicerProfiles", "defaultMachines", "profileEvidence"],
            "releaseRule": "intake remains advisory until source provenance, units, material, profile, and machine evidence are retained"
        }),
        json!({
            "requirement": "machine-code-and-instruction-generation",
            "covers": "generate draft design packages, toolpaths, slicer plans, G-code, controller programs, and non-G-code job-sheet instructions for printers, mills, routers, lathes, and sheet cutters",
            "primaryRoutes": [
                "POST /fabrication/design/generate",
                "POST /fabrication/instructions/generate",
                "POST /fabrication/machine-code/generate",
                "POST /fabrication/toolpaths/plan",
                "GET /fabrication/machine-code/catalog",
                "GET /fabrication/instructions/generation/catalog"
            ],
            "evidenceSurfaces": ["designPackage", "generatedPrograms", "toolpathPlan", "machineRelease", "releasePackagePlan"],
            "releaseRule": "generated programs stay machineReady=false until controller, simulation, setup, quality, and operator or automation gates clear"
        }),
        json!({
            "requirement": "existing-instruction-validation-and-improvement",
            "covers": "accept existing CNC, additive, slicer, CAM, controller, and text fabrication instructions, validate them, improve them, and retain patches or review findings",
            "primaryRoutes": [
                "POST /fabrication/instructions/analyze",
                "POST /fabrication/instructions/validate",
                "POST /fabrication/instructions/improve",
                "POST /fabrication/instructions/validation/result",
                "POST /fabrication/instructions/review/result",
                "GET /fabrication/instructions/languages"
            ],
            "evidenceSurfaces": ["validationFindings", "failureBoundaries", "patchManifest", "learningObservations", "releaseBlockers"],
            "releaseRule": "improved instructions remain advisory until retained validation, simulation, and release evidence closes every blocker"
        }),
        json!({
            "requirement": "machine-failure-and-human-intervention-boundaries",
            "covers": "surface machine-failure, process-stop, setup, workholding, support-media, material, macro, modal-state, and human-intervention boundaries before unattended release",
            "primaryRoutes": [
                "GET /fabrication/boundaries/catalog",
                "GET /fabrication/boundaries/preflight/catalog",
                "POST /fabrication/boundaries/result",
                "POST /fabrication/interventions/result",
                "POST /fabrication/release/preview"
            ],
            "evidenceSurfaces": ["boundaryAnalysis", "interventionMap", "operatorInterventionPlan", "machineRelease", "priorityDispositions"],
            "releaseRule": "unresolved boundaries force machineReady=false or require split, combine, reroute, remediation, or human/automation handoff"
        }),
        json!({
            "requirement": "operator-observability-and-release-trust",
            "covers": "expose the landing-page, how-it-works, metrics, and Grafana dashboard path operators use to inspect request intake, release blockers, NATS fanout, learning feedback, artifact ledgers, and runtime capacity before trusting generated work",
            "primaryRoutes": [
                "GET /fabrication",
                "GET /fabrication/how-it-works",
                "GET /metrics",
                "GET /grafana/fabrication"
            ],
            "evidenceSurfaces": ["operatorObservability", "operatorDashboardSignals", "dd-fabrication-planner", "releasePackagePlan", "machineRelease"],
            "releaseRule": "operator dashboards and metrics are release evidence context, not machine approval; retained validation, simulation, setup, quality, and signoff gates still decide machineReady"
        }),
        json!({
            "requirement": "split-combine-and-multi-process-learning",
            "covers": "plan one-piece, split-route, recomposed, alternate-machine, and hybrid assemblies that combine printed, milled, turned, routed, cut, or postprocessed parts",
            "primaryRoutes": [
                "POST /fabrication/decomposition/plan",
                "POST /fabrication/assembly/plan",
                "POST /fabrication/interfaces/result",
                "GET /fabrication/hybrid/catalog",
                "POST /fabrication/strategy/recommend",
                "POST /fabrication/learning/outcomes"
            ],
            "evidenceSurfaces": ["decompositionPlan", "assemblyPlan", "interfaceControlPlan", "strategyCandidates", "learningOutcomeDraft"],
            "releaseRule": "split/combine choices require child-route packages, datum-transfer evidence, interface checks, and release-package closure"
        }),
        json!({
            "requirement": "mdp-pomdp-des-neural-learning",
            "covers": "retain rewards, MDP/POMDP/DES model evidence, neural training examples, optimizer outputs, replay gates, and outcome memory so future plans can learn safer routes",
            "primaryRoutes": [
                "GET /fabrication/learning/capabilities",
                "GET /fabrication/learning/models/catalog",
                "GET /fabrication/learning/optimizers/catalog",
                "POST /fabrication/learning/models/result",
                "POST /fabrication/learning/optimizers/result",
                "GET /fabrication/learning/corpus"
            ],
            "evidenceSurfaces": ["mdpRequest", "desMdpSolution", "desPomdpSolution", "neuralPolicy", "neuralTrainingCorpus", "learningOutcomeMemory"],
            "releaseRule": "learned preferences and neural scores remain advisory until replay, simulation, retained artifacts, and release blockers clear"
        }),
    ]
}
