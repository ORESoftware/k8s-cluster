use std::collections::BTreeSet;

use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LatheModelSpec {
    pub model: &'static str,
    pub vendor: &'static str,
    pub display_name: &'static str,
    pub aliases: &'static [&'static str],
    pub machine_kind: &'static str,
    pub controller: &'static str,
    pub control_name: &'static str,
    /// Planning envelope `[maximum cutting diameter, maximum cutting length]` in millimetres.
    pub work_envelope_mm: [f64; 2],
    pub spindle_speed_rpm: [u32; 2],
    pub max_bar_capacity_mm: Option<f64>,
    pub chuck_size_mm: Option<f64>,
    pub tool_positions: Option<u8>,
    pub materials: &'static [&'static str],
    pub operations: &'static [&'static str],
    pub notes: &'static [&'static str],
}

pub(crate) const LATHE_MODEL_SPECS: &[LatheModelSpec] = &[
    LatheModelSpec {
        model: "haas-st-20",
        vendor: "Haas Automation",
        display_name: "Haas ST-20",
        aliases: &["st-20", "st20", "haas-st20", "haas-st-20-series"],
        machine_kind: "lathe",
        controller: "haas-gcode",
        control_name: "Haas NGC",
        work_envelope_mm: [330.0, 572.0],
        spindle_speed_rpm: [1, 4_000],
        max_bar_capacity_mm: Some(64.0),
        chuck_size_mm: Some(210.0),
        tool_positions: Some(12),
        materials: &[
            "aluminum",
            "steel",
            "stainless-steel",
            "brass",
            "titanium",
            "plastic",
        ],
        operations: &[
            "turn",
            "face",
            "bore",
            "thread",
            "groove",
            "part-off",
            "tailstock-support",
            "bar-feed",
        ],
        notes: &[
            "The 330 mm diameter assumes the BOT turret; other turret configurations reduce the published cutting diameter.",
            "Maximum cutting length varies with workholding.",
            "Bar stock extending behind the spindle requires an approved support or bar feeder.",
        ],
    },
    LatheModelSpec {
        model: "tormach-15l-slant-pro",
        vendor: "Tormach",
        display_name: "Tormach 15L Slant-PRO",
        aliases: &[
            "15l",
            "15l-slant-pro",
            "tormach-15l",
            "slant-pro",
            "tormach-slant-pro",
        ],
        machine_kind: "lathe",
        controller: "linuxcnc",
        control_name: "PathPilot",
        // The 15L publishes 10 in X travel and 12 in Z travel. Daedalus keeps
        // the conservative nominal planning envelope in the same two-value
        // shape used by the existing lathe preflight path.
        work_envelope_mm: [254.0, 305.0],
        spindle_speed_rpm: [180, 3_500],
        max_bar_capacity_mm: Some(28.575),
        chuck_size_mm: Some(152.4),
        tool_positions: Some(8),
        materials: &[
            "aluminum",
            "steel",
            "stainless-steel",
            "brass",
            "plastic",
        ],
        operations: &[
            "turn",
            "face",
            "bore",
            "thread",
            "groove",
            "part-off",
            "5c-collet-workholding",
            "gang-tool-turning",
            "turret-turning",
        ],
        notes: &[
            "Spindle speed is workholding-dependent: the published 5C range is 250-3500 rpm and the 6-inch chuck range is 180-2500 rpm.",
            "The 28.575 mm stock limit represents the published 1.125-inch 5C workholding guidance, not unrestricted unsupported bar stock.",
            "PathPilot jobs use the existing LinuxCNC postprocessor family and still require an exact machine/controller verification artifact.",
        ],
    },
];

pub(crate) fn lathe_model_for_token(token: &str) -> Option<&'static LatheModelSpec> {
    LATHE_MODEL_SPECS.iter().find(|spec| {
        spec.model == token || spec.aliases.iter().any(|alias| *alias == token)
    })
}

pub(crate) fn lathe_models_json() -> Value {
    json!(LATHE_MODEL_SPECS
        .iter()
        .map(|spec| {
            json!({
                "model": spec.model,
                "vendor": spec.vendor,
                "displayName": spec.display_name,
                "aliases": spec.aliases,
                "machineKind": spec.machine_kind,
                "controller": spec.controller,
                "controlName": spec.control_name,
                "workEnvelopeMm": spec.work_envelope_mm,
                "spindleSpeedRpm": {
                    "minimum": spec.spindle_speed_rpm[0],
                    "maximum": spec.spindle_speed_rpm[1],
                },
                "maxBarCapacityMm": spec.max_bar_capacity_mm,
                "chuckSizeMm": spec.chuck_size_mm,
                "toolPositions": spec.tool_positions,
                "materials": spec.materials,
                "operations": spec.operations,
                "notes": spec.notes,
            })
        })
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_and_common_aliases_resolve() {
        assert_eq!(
            lathe_model_for_token("haas-st-20").map(|spec| spec.controller),
            Some("haas-gcode")
        );
        assert_eq!(
            lathe_model_for_token("st20").map(|spec| spec.model),
            Some("haas-st-20")
        );
        assert_eq!(
            lathe_model_for_token("15l-slant-pro").map(|spec| spec.controller),
            Some("linuxcnc")
        );
        assert_eq!(
            lathe_model_for_token("tormach-slant-pro").map(|spec| spec.model),
            Some("tormach-15l-slant-pro")
        );
    }

    #[test]
    fn model_tokens_are_unique_and_envelopes_are_positive() {
        let mut tokens = BTreeSet::new();
        for spec in LATHE_MODEL_SPECS {
            assert!(tokens.insert(spec.model), "duplicate model token {}", spec.model);
            for alias in spec.aliases {
                assert!(tokens.insert(alias), "duplicate alias {alias}");
            }
            assert!(spec.work_envelope_mm.iter().all(|value| *value > 0.0));
            assert!(spec.spindle_speed_rpm[0] > 0);
            assert!(spec.spindle_speed_rpm[1] >= spec.spindle_speed_rpm[0]);
            assert!(!spec.materials.is_empty());
            assert!(!spec.operations.is_empty());
        }
    }

    #[test]
    fn json_catalog_preserves_controller_and_release_caveats() {
        let catalog = lathe_models_json();
        let entries = catalog.as_array().expect("lathe model catalog array");
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| {
            entry["model"] == "haas-st-20"
                && entry["controller"] == "haas-gcode"
                && entry["maxBarCapacityMm"] == 64.0
        }));
        assert!(entries.iter().any(|entry| {
            entry["model"] == "tormach-15l-slant-pro"
                && entry["controlName"] == "PathPilot"
                && entry["notes"].as_array().is_some_and(|notes| !notes.is_empty())
        }));
    }
}
