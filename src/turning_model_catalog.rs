//! Named CNC turning-center models the service explicitly supports.
//!
//! The generic turning pipeline remains authoritative for safety and release.
//! These entries provide conservative planning profiles, aliases, controller
//! expectations, and published machine limits; they never certify a live asset.

use serde_json::{json, Value};

pub(crate) struct TurningMachineModelSpec {
    /// Normalized model token (`normalize_token` form).
    pub(crate) model: &'static str,
    pub(crate) vendor: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) aliases: &'static [&'static str],
    /// Canonical machine kind used by the generic turning pipeline.
    pub(crate) machine_kind: &'static str,
    /// Controller dialect used by the derived default fleet profile.
    pub(crate) controller: &'static str,
    /// Published controller families for this model/series.
    pub(crate) controller_options: &'static [&'static str],
    /// Whether release must prove the exact installed control before execution.
    pub(crate) requires_controller_confirmation: bool,
    /// Maximum cutting diameter × maximum cutting length in millimetres.
    pub(crate) work_envelope_mm: [f64; 2],
    pub(crate) axes: u8,
    pub(crate) chuck_size_mm: f64,
    pub(crate) bar_capacity_mm: Option<f64>,
    pub(crate) max_spindle_speed_rpm: u32,
    pub(crate) spindle_power_kw: f64,
    pub(crate) max_spindle_torque_nm: f64,
    pub(crate) tool_stations: u8,
    pub(crate) materials: &'static [&'static str],
    pub(crate) operations: &'static [&'static str],
    pub(crate) release_requirements: &'static [&'static str],
    pub(crate) notes: &'static str,
    pub(crate) source_url: &'static str,
}

pub(crate) const TURNING_MACHINE_MODEL_SPECS: &[TurningMachineModelSpec] = &[
    TurningMachineModelSpec {
        model: "haas-st-20",
        vendor: "Haas Automation",
        display_name: "Haas ST-20",
        aliases: &["st-20", "st20", "haas-st20", "haas-st-series-20"],
        machine_kind: "lathe",
        controller: "haas-gcode",
        controller_options: &["haas-gcode"],
        requires_controller_confirmation: true,
        work_envelope_mm: [330.0, 572.0],
        axes: 2,
        chuck_size_mm: 210.0,
        bar_capacity_mm: Some(64.0),
        max_spindle_speed_rpm: 4_000,
        spindle_power_kw: 14.9,
        max_spindle_torque_nm: 203.0,
        tool_stations: 12,
        materials: &[
            "aluminum",
            "steel",
            "stainless-steel",
            "brass",
            "plastic",
            "titanium",
        ],
        operations: &[
            "turn",
            "face",
            "bore",
            "thread",
            "groove",
            "part-off",
            "bar-fed-turning",
            "rigid-tap",
        ],
        release_requirements: &[
            "confirm the exact Haas control generation, enabled options, parameter set, and postprocessor",
            "verify chuck or collet, jaw pressure, stock stick-out, runout, tailstock or bar support, and part catcher",
            "verify turret station, tool geometry and wear offsets, G50 spindle limit, CSS/fixed-RPM mode, and feed-per-revolution state",
            "simulate or dry-run threading, grooving, and part-off moves and retain first-article dimensional evidence",
        ],
        notes: "Reference profile for the standard two-axis Haas ST-20 with an 8.3-inch chuck. Optional Y-axis, sub-spindle, live tooling, APL, auto door, and alternate turret configurations require an explicit submitted machine profile rather than silently inheriting this base profile.",
        source_url: "https://www.haascnc.com/machines/lathes/st/models/standard/st-20.html",
    },
    TurningMachineModelSpec {
        model: "dn-solutions-lynx-2100b-fanuc",
        vendor: "DN Solutions",
        display_name: "DN Solutions Lynx 2100B (Fanuc configuration)",
        aliases: &[
            "lynx-2100b",
            "lynx2100b",
            "lynx-2100b-fanuc",
            "dn-lynx-2100b",
            "dn-solutions-lynx-2100b",
            "doosan-lynx-2100b",
        ],
        machine_kind: "lathe",
        controller: "fanuc-gcode",
        controller_options: &["fanuc-gcode", "siemens-sinumerik"],
        requires_controller_confirmation: true,
        work_envelope_mm: [350.0, 330.0],
        axes: 2,
        chuck_size_mm: 203.2,
        bar_capacity_mm: None,
        max_spindle_speed_rpm: 4_500,
        spindle_power_kw: 15.0,
        max_spindle_torque_nm: 169.0,
        tool_stations: 12,
        materials: &[
            "aluminum",
            "steel",
            "stainless-steel",
            "brass",
            "plastic",
            "titanium",
        ],
        operations: &[
            "turn",
            "face",
            "bore",
            "thread",
            "groove",
            "part-off",
            "bar-fed-turning",
        ],
        release_requirements: &[
            "confirm whether the installed control is Fanuc i Plus or Siemens S828D; this derived fleet entry is Fanuc-only",
            "confirm regional machine variant, chuck, spindle, turret, optional tailstock, bar feeder, catcher, and conveyor configuration",
            "verify workholding pressure, stock support, tool station and offset table, spindle/feed modes, thread pitch synchronization, and recovery state",
            "simulate or dry-run threading, grooving, and cutoff paths and retain first-piece inspection evidence",
        ],
        notes: "Conservative reference profile for the 8-inch, two-axis Lynx 2100B. DN Solutions offers multiple controls and regional configurations; the generic model aliases resolve to the Fanuc reference profile only for planning, and machine-ready release remains blocked until the exact installed control and options are confirmed.",
        source_url: "https://www.dn-solutions.com/global/product/turning-center/2-axis-horizontal/lynx-2100-b.do",
    },
];

/// Resolve a raw model reference or alias to a supported turning model.
pub(crate) fn turning_machine_model_for_token(
    value: &str,
) -> Option<&'static TurningMachineModelSpec> {
    let token = super::normalize_token(value);
    TURNING_MACHINE_MODEL_SPECS
        .iter()
        .find(|spec| spec.model == token || spec.aliases.iter().any(|alias| *alias == token))
}

pub(super) fn turning_machine_models_json() -> Value {
    Value::Array(
        TURNING_MACHINE_MODEL_SPECS
            .iter()
            .map(|spec| {
                json!({
                    "model": spec.model,
                    "vendor": spec.vendor,
                    "displayName": spec.display_name,
                    "aliases": spec.aliases,
                    "machineKind": spec.machine_kind,
                    "fleetMachineId": format!("{}-1", spec.model),
                    "controller": spec.controller,
                    "controllerOptions": spec.controller_options,
                    "requiresControllerConfirmation": spec.requires_controller_confirmation,
                    "workEnvelopeMm": spec.work_envelope_mm,
                    "axes": spec.axes,
                    "chuckSizeMm": spec.chuck_size_mm,
                    "barCapacityMm": spec.bar_capacity_mm,
                    "maxSpindleSpeedRpm": spec.max_spindle_speed_rpm,
                    "spindlePowerKw": spec.spindle_power_kw,
                    "maxSpindleTorqueNm": spec.max_spindle_torque_nm,
                    "toolStations": spec.tool_stations,
                    "materials": spec.materials,
                    "operations": spec.operations,
                    "releaseRequirements": spec.release_requirements,
                    "notes": spec.notes,
                    "sourceUrl": spec.source_url,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_turning_models_resolve_canonical_and_legacy_aliases() {
        let haas = turning_machine_model_for_token("ST 20").expect("Haas ST-20 alias");
        assert_eq!(haas.model, "haas-st-20");
        assert_eq!(haas.controller, "haas-gcode");

        let lynx =
            turning_machine_model_for_token("Doosan Lynx 2100B").expect("legacy Doosan Lynx alias");
        assert_eq!(lynx.model, "dn-solutions-lynx-2100b-fanuc");
        assert_eq!(lynx.controller, "fanuc-gcode");
        assert!(lynx.controller_options.contains(&"siemens-sinumerik"));
        assert!(lynx.requires_controller_confirmation);
    }

    #[test]
    fn named_turning_models_join_the_default_fleet_and_catalogs() {
        let machines = crate::default_machines();
        for expected in ["haas-st-20-1", "dn-solutions-lynx-2100b-fanuc-1"] {
            assert!(
                machines.iter().any(|machine| machine.id == expected),
                "default fleet missing {expected}"
            );
        }

        assert_eq!(
            crate::machine_class("haas-st20"),
            crate::MachineClass::Lathe
        );
        assert_eq!(
            crate::machine_class("lynx-2100b"),
            crate::MachineClass::Lathe
        );

        let turning = crate::turning_catalog_response();
        assert_eq!(
            turning
                .get("supportedTurningModelCount")
                .and_then(Value::as_u64),
            Some(2)
        );
        let models = turning
            .get("supportedTurningModels")
            .and_then(Value::as_array)
            .expect("supportedTurningModels array");
        assert!(models.iter().any(|model| model["model"] == "haas-st-20"));
        assert!(models
            .iter()
            .any(|model| model["model"] == "dn-solutions-lynx-2100b-fanuc"));

        let lathe = crate::lathe_catalog_response();
        let lathe_machines = lathe["latheMachines"]
            .as_array()
            .expect("latheMachines array");
        let haas_machine = lathe_machines
            .iter()
            .find(|machine| machine["id"] == "haas-st-20-1")
            .expect("Haas fleet entry");
        assert!(haas_machine["acceptedInstructionLanguages"]
            .as_array()
            .is_some_and(|languages| languages.iter().any(|language| language == "haas-gcode")));
        let lynx_machine = lathe_machines
            .iter()
            .find(|machine| machine["id"] == "dn-solutions-lynx-2100b-fanuc-1")
            .expect("Lynx fleet entry");
        assert!(lynx_machine["acceptedInstructionLanguages"]
            .as_array()
            .is_some_and(|languages| languages.iter().any(|language| language == "fanuc-gcode")));
    }

    #[test]
    fn model_catalog_preserves_published_limits_and_release_gates() {
        let value = turning_machine_models_json();
        let models = value.as_array().expect("model array");
        let st20 = models
            .iter()
            .find(|model| model["model"] == "haas-st-20")
            .expect("ST-20 model");
        assert_eq!(st20["workEnvelopeMm"], json!([330.0, 572.0]));
        assert_eq!(st20["barCapacityMm"], 64.0);
        assert_eq!(st20["maxSpindleSpeedRpm"], 4_000);

        let lynx = models
            .iter()
            .find(|model| model["model"] == "dn-solutions-lynx-2100b-fanuc")
            .expect("Lynx model");
        assert_eq!(lynx["workEnvelopeMm"], json!([350.0, 330.0]));
        assert_eq!(lynx["maxSpindleSpeedRpm"], 4_500);
        assert_eq!(lynx["toolStations"], 12);
        assert_eq!(lynx["requiresControllerConfirmation"], true);
    }
}
