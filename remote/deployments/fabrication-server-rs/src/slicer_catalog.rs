use serde_json::{json, Value};

pub(super) fn machine_code_handoff_compatibility() -> Value {
    json!({
        "schemaVersion": "dd.fabrication.slicer-machine-code-handoff.v1",
        "handoffFamilies": [
            {
                "family": "fdm-printer-gcode",
                "generatedKinds": ["marlin-gcode", "reprap-gcode", "klipper-gcode", "bambu-gcode"],
                "requiredEvidence": [
                    "printer firmware and kinematic limits",
                    "material/nozzle/bed/chamber profile",
                    "extrusion mode, volumetric mode, and E-axis reset",
                    "support/orientation/first-layer review",
                    "dry-run or simulation risk review"
                ]
            },
            {
                "family": "resin-printer-package",
                "generatedKinds": ["ctb-resin-job", "photon-resin-job", "lychee-resin-job", "chitubox-resin-job"],
                "requiredEvidence": [
                    "printer and slicer package version",
                    "resin exposure/layer manifest",
                    "island, hollowing, drain, and support review",
                    "peel/lift/recoat and vat/refill state",
                    "wash/cure/PPE postprocess release"
                ]
            },
            {
                "family": "slicer-project-or-profile",
                "generatedKinds": ["3mf-project", "prusaslicer-project", "orcaslicer-project", "cura-project", "bambu-studio-project"],
                "requiredEvidence": [
                    "project/profile checksum and source-system identity",
                    "printer/material/profile compatibility",
                    "mesh unit/scale/topology review",
                    "plate fit, support, and first-layer preview",
                    "retained generated G-code or package artifact URI/checksum"
                ]
            }
        ],
        "blockerKinds": [
            "slicer-profile-boundary",
            "slicer-orientation-support-boundary",
            "slicer-first-layer-boundary",
            "slicer-mesh-topology-boundary",
            "slicer-high-speed-kinematics-boundary",
            "non-gcode-job-sheet-evidence"
        ],
        "releasePolicy": [
            "slicer machine-code handoffs remain draft printer instructions until compatibility evidence, simulation or dry-run review, and operator or automation signoff clear",
            "non-G-code resin and slicer package jobs must retain package manifest and postprocess evidence before machine-ready release"
        ]
    })
}
