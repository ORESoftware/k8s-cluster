use serde_json::{json, Value};

pub(super) fn model_families() -> Value {
    json!([
        {
            "family": "mdp-policy-snapshot",
            "primitive": "des_engine::des::decision::solve_mdp",
            "artifactKinds": ["mdp-request", "mdp-value-table", "policy-action-map", "learning-policy-snapshot"],
            "trainedFrom": ["learning.outcomes", "reward_terms", "validation.failureBoundaries", "releasePackagePlan.releaseGates"],
            "usedFor": [
                "fabrication route action ranking",
                "machine/process selection previews",
                "failure-boundary remediation scoring"
            ],
            "promotionGates": [
                "reward evidence is complete and redacted",
                "policy was replayed against retained validation and release blockers",
                "machine-ready release still requires simulation, quality, setup, and operator evidence"
            ]
        },
        {
            "family": "pomdp-belief-policy",
            "primitive": "des_engine::des::decision::solve_pomdp_underlying",
            "artifactKinds": ["pomdp-belief-state", "hidden-intervention-risk", "probe-priority-map"],
            "trainedFrom": ["learning.observations", "operatorInterventionPlan.requiredOperatorActions", "executionTelemetryResult.machineStops"],
            "usedFor": [
                "human-intervention uncertainty estimation",
                "probe and inspection prioritization",
                "uncertain split/combine or restart-state review"
            ],
            "promotionGates": [
                "belief-state assumptions are attached to the retained job evidence",
                "hidden-state risk cannot certify unattended release",
                "probe or operator gates remain open until direct evidence clears them"
            ]
        },
        {
            "family": "des-studio-surrogate",
            "primitive": "des_engine::des::studio::analyze_model_spec",
            "artifactKinds": ["desScheduleModel", "desInstructionModel", "queue-capacity-analysis", "worker-lane-surrogate"],
            "trainedFrom": ["machineSchedule", "instructionAnalysis.reviewQueue", "worker result timings", "simulationResult.findings"],
            "usedFor": [
                "machine-lane capacity planning",
                "instruction-review bottleneck detection",
                "hybrid cell and worker-lane dispatch previews"
            ],
            "promotionGates": [
                "queue graph validates against the DES Studio schema",
                "surrogate output is tied to observed cycle-time or queue evidence",
                "runtime telemetry must confirm any schedule policy before automatic reuse"
            ]
        },
        {
            "family": "bounded-neural-policy-sketch",
            "primitive": "des_engine::des::general::neural_network::FeedForwardNetwork",
            "artifactKinds": ["neural-training-corpus", "feature-vector-map", "advisory-action-score", "model-card"],
            "trainedFrom": ["learning.outcomes", "state/action features", "quality buckets", "false-positive and false-negative boundary labels"],
            "usedFor": [
                "feature-to-action ranking previews",
                "boundary-risk classifier experiments",
                "split/combine strategy hints"
            ],
            "promotionGates": [
                "training corpus is versioned, bounded, and linked to retained artifacts",
                "model card records features, labels, reward terms, and known failure modes",
                "neural scores are advisory and subordinate to deterministic validation and release gates"
            ]
        }
    ])
}

pub(super) fn neural_feature_contract(parameter_count: usize) -> Value {
    json!({
        "schemaVersion": "dd.fabrication.neural-feature-contract.v1",
        "engine": "des_engine::des::general::neural_network::FeedForwardNetwork",
        "networkKind": "deterministic-single-layer-sigmoid-policy-head",
        "inputDimension": 9,
        "outputDimension": 4,
        "parameterCount": parameter_count,
        "featureNames": [
            "objective-embedding",
            "material-family",
            "stock-envelope",
            "machine-envelope",
            "toolpath-token-sequence",
            "simulated-force-temperature-vibration",
            "inspection-error-vector",
            "automation-requirement-vector",
            "resolution-step-policy-state"
        ],
        "featureSources": [
            "design and part count normalization",
            "manufacturing-method diversity",
            "human-intervention process steps",
            "validation finding density",
            "human-boundary count",
            "instruction-improvement count",
            "minimum tolerance pressure",
            "automation requirement count",
            "resolution-plan step count"
        ],
        "outputLabels": [
            "split-combine",
            "human-intervention",
            "machine-failure",
            "automation-gap"
        ],
        "retainedSurfaces": [
            "learning.neuralPolicy.engineInference",
            "learning.neuralTrainingCorpus.featureNames",
            "learning.neuralTrainingCorpus.inferenceCandidates",
            "learningCorpus.neuralTrainingExamples",
            "learningModelResult.modelCardCompatibility"
        ],
        "compatibilityChecks": [
            "model-card featureNames must match neuralTrainingCorpus.featureNames in order",
            "model-card inputDimension and outputDimension must match this contract",
            "model-card outputLabels must cover split-combine, human-intervention, machine-failure, and automation-gap",
            "model-card normalization must preserve bounded 0..1 feature vectors",
            "model artifact URI and checksum must be retained before promotion review"
        ],
        "releasePolicy": [
            "feature vectors are bounded 0..1 and must match neuralTrainingCorpus.featureNames before model-card compatibility can pass",
            "output labels are advisory action-risk scores for planning and replay, not controller approval",
            "machine-ready release remains blocked by deterministic validation, simulation, setup, quality, telemetry, and human or automation gates"
        ]
    })
}
