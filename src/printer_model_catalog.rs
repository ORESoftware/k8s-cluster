//! Named FDM printer models the service explicitly supports.
//!
//! The default fleet and most planning flows describe machines structurally
//! (kind, controller, envelope). This catalog pins the vendor models we commit
//! to supporting so callers can reference them by name ("creality-k1") and the
//! FDM printer catalog can advertise their real envelopes and limits.

use serde_json::{json, Value};

pub(crate) struct FdmPrinterModelSpec {
    /// Normalized model token (`normalize_token` form), e.g. "creality-k1".
    pub(crate) model: &'static str,
    pub(crate) vendor: &'static str,
    pub(crate) display_name: &'static str,
    /// Extra normalized tokens that resolve to this model.
    pub(crate) aliases: &'static [&'static str],
    /// Canonical machine kind the model maps to in fleet/profile terms.
    pub(crate) machine_kind: &'static str,
    pub(crate) controller: &'static str,
    pub(crate) kinematics: &'static str,
    pub(crate) work_envelope_mm: [f64; 3],
    pub(crate) max_print_speed_mm_s: f64,
    pub(crate) max_acceleration_mm_s2: f64,
    pub(crate) max_nozzle_temp_c: f64,
    pub(crate) max_bed_temp_c: f64,
    pub(crate) max_volumetric_flow_mm3_s: f64,
    pub(crate) enclosed: bool,
    pub(crate) max_materials: u16,
    pub(crate) materials: &'static [&'static str],
    pub(crate) operations: &'static [&'static str],
    pub(crate) notes: &'static str,
}

pub(crate) const FDM_PRINTER_MODEL_SPECS: &[FdmPrinterModelSpec] = &[
    FdmPrinterModelSpec {
        model: "creality-k1",
        vendor: "Creality",
        display_name: "Creality K1",
        aliases: &["k1"],
        machine_kind: "fdm-printer",
        controller: "klipper",
        kinematics: "corexy",
        work_envelope_mm: [220.0, 220.0, 250.0],
        max_print_speed_mm_s: 600.0,
        max_acceleration_mm_s2: 20_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 100.0,
        max_volumetric_flow_mm3_s: 32.0,
        enclosed: true,
        max_materials: 1,
        materials: &["pla", "petg", "abs", "tpu"],
        operations: &["additive-print"],
        notes: "High-speed enclosed CoreXY running Creality OS (Klipper-based); \
                accepts klipper-flavored G-code.",
    },
    FdmPrinterModelSpec {
        model: "creality-k2",
        vendor: "Creality",
        display_name: "Creality K2",
        aliases: &["k2", "creality-k2-combo", "k2-combo"],
        machine_kind: "multi-material-fdm-printer",
        controller: "klipper",
        kinematics: "corexy",
        work_envelope_mm: [260.0, 260.0, 260.0],
        max_print_speed_mm_s: 600.0,
        max_acceleration_mm_s2: 20_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 100.0,
        max_volumetric_flow_mm3_s: 32.0,
        enclosed: true,
        max_materials: 16,
        materials: &[
            "pla",
            "petg",
            "abs",
            "asa",
            "tpu",
            "pva",
            "hips",
            "support-material",
        ],
        operations: &[
            "additive-print",
            "multi-material-fdm-print",
            "ams-mmu-print",
            "material-map",
            "purge-tower",
            "wipe-tower",
            "filament-change-automation",
        ],
        notes: "Enclosed CoreXY running Creality OS (Klipper-based); CFS \
                multi-material up to 16 spools when units are chained.",
    },
    FdmPrinterModelSpec {
        model: "bambu-a1",
        vendor: "Bambu Lab",
        display_name: "Bambu Lab A1",
        aliases: &["a1", "bambu-lab-a1"],
        machine_kind: "fdm-printer",
        controller: "bambu",
        kinematics: "cartesian",
        work_envelope_mm: [256.0, 256.0, 256.0],
        max_print_speed_mm_s: 500.0,
        max_acceleration_mm_s2: 10_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 100.0,
        max_volumetric_flow_mm3_s: 28.0,
        enclosed: false,
        max_materials: 1,
        materials: &["pla", "petg", "tpu"],
        operations: &["additive-print"],
        notes: "Open-frame Cartesian bed-slinger running Bambu firmware; accepts \
                Bambu-flavored G-code from Bambu Studio / OrcaSlicer. The open frame \
                is validated for PLA/PETG/TPU, not ABS/ASA.",
    },
    FdmPrinterModelSpec {
        model: "bambu-a1-combo",
        vendor: "Bambu Lab",
        display_name: "Bambu Lab A1 Combo",
        aliases: &["a1-combo", "bambu-lab-a1-combo", "a1-ams-lite", "bambu-a1-ams"],
        machine_kind: "multi-material-fdm-printer",
        controller: "bambu",
        kinematics: "cartesian",
        work_envelope_mm: [256.0, 256.0, 256.0],
        max_print_speed_mm_s: 500.0,
        max_acceleration_mm_s2: 10_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 100.0,
        max_volumetric_flow_mm3_s: 28.0,
        enclosed: false,
        max_materials: 4,
        materials: &["pla", "petg", "tpu", "pva", "support-material"],
        operations: &[
            "additive-print",
            "multi-material-fdm-print",
            "ams-mmu-print",
            "material-map",
            "purge-tower",
            "wipe-tower",
            "filament-change-automation",
        ],
        notes: "A1 with a single AMS Lite (4-slot, not chainable) for multi-material; \
                open-frame Cartesian bed-slinger running Bambu firmware/G-code. The open \
                frame limits it to PLA/PETG/TPU plus PVA/support interfaces.",
    },
    FdmPrinterModelSpec {
        model: "bambu-a1-mini",
        vendor: "Bambu Lab",
        display_name: "Bambu Lab A1 mini",
        aliases: &["a1-mini", "bambu-lab-a1-mini"],
        machine_kind: "fdm-printer",
        controller: "bambu",
        kinematics: "cartesian",
        work_envelope_mm: [180.0, 180.0, 180.0],
        max_print_speed_mm_s: 500.0,
        max_acceleration_mm_s2: 10_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 80.0,
        max_volumetric_flow_mm3_s: 28.0,
        enclosed: false,
        max_materials: 1,
        materials: &["pla", "petg", "tpu"],
        operations: &["additive-print"],
        notes: "Compact open-frame Cartesian bed-slinger (180 mm cube, bed to 80 °C) \
                running Bambu firmware; accepts Bambu-flavored G-code.",
    },
    FdmPrinterModelSpec {
        model: "bambu-a1-mini-combo",
        vendor: "Bambu Lab",
        display_name: "Bambu Lab A1 mini Combo",
        aliases: &["a1-mini-combo", "bambu-lab-a1-mini-combo"],
        machine_kind: "multi-material-fdm-printer",
        controller: "bambu",
        kinematics: "cartesian",
        work_envelope_mm: [180.0, 180.0, 180.0],
        max_print_speed_mm_s: 500.0,
        max_acceleration_mm_s2: 10_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 80.0,
        max_volumetric_flow_mm3_s: 28.0,
        enclosed: false,
        max_materials: 4,
        materials: &["pla", "petg", "tpu", "pva", "support-material"],
        operations: &[
            "additive-print",
            "multi-material-fdm-print",
            "ams-mmu-print",
            "material-map",
            "purge-tower",
            "wipe-tower",
            "filament-change-automation",
        ],
        notes: "A1 mini with a single AMS Lite (4-slot) for multi-material; compact \
                open-frame Cartesian bed-slinger running Bambu firmware/G-code.",
    },
    FdmPrinterModelSpec {
        model: "bambu-p1s",
        vendor: "Bambu Lab",
        display_name: "Bambu Lab P1S",
        aliases: &["p1s", "bambu-lab-p1s", "p1s-combo", "bambu-p1s-combo"],
        machine_kind: "multi-material-fdm-printer",
        controller: "bambu",
        kinematics: "corexy",
        work_envelope_mm: [256.0, 256.0, 256.0],
        max_print_speed_mm_s: 500.0,
        max_acceleration_mm_s2: 20_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 100.0,
        max_volumetric_flow_mm3_s: 32.0,
        enclosed: true,
        max_materials: 16,
        materials: &[
            "pla",
            "petg",
            "abs",
            "asa",
            "tpu",
            "pva",
            "hips",
            "pc",
            "support-material",
        ],
        operations: &[
            "additive-print",
            "multi-material-fdm-print",
            "ams-mmu-print",
            "material-map",
            "purge-tower",
            "wipe-tower",
            "filament-change-automation",
        ],
        notes: "Enclosed CoreXY running Bambu firmware; AMS multi-material up to 16 \
                spools when four AMS units are chained. The enclosure enables ABS/ASA/PC. \
                Accepts Bambu-flavored G-code.",
    },
    FdmPrinterModelSpec {
        model: "bambu-x1-carbon",
        vendor: "Bambu Lab",
        display_name: "Bambu Lab X1 Carbon",
        aliases: &["x1-carbon", "x1c", "bambu-x1c", "bambu-lab-x1-carbon", "x1-carbon-combo"],
        machine_kind: "multi-material-fdm-printer",
        controller: "bambu",
        kinematics: "corexy",
        work_envelope_mm: [256.0, 256.0, 256.0],
        max_print_speed_mm_s: 500.0,
        max_acceleration_mm_s2: 20_000.0,
        max_nozzle_temp_c: 300.0,
        max_bed_temp_c: 110.0,
        max_volumetric_flow_mm3_s: 32.0,
        enclosed: true,
        max_materials: 16,
        materials: &[
            "pla",
            "petg",
            "abs",
            "asa",
            "tpu",
            "pva",
            "hips",
            "pc",
            "pa",
            "support-material",
        ],
        operations: &[
            "additive-print",
            "multi-material-fdm-print",
            "ams-mmu-print",
            "material-map",
            "purge-tower",
            "wipe-tower",
            "filament-change-automation",
        ],
        notes: "Enclosed CoreXY flagship (lidar + AMS) running Bambu firmware; AMS \
                multi-material up to 16 spools; hardened-nozzle option for abrasive \
                PA/PC composites; bed to 110 °C. Accepts Bambu-flavored G-code.",
    },
];

/// Resolve a machine kind or model reference to a supported printer model.
/// Accepts raw input; matching is on the normalized token.
pub(crate) fn fdm_printer_model_for_token(value: &str) -> Option<&'static FdmPrinterModelSpec> {
    let token = super::normalize_token(value);
    FDM_PRINTER_MODEL_SPECS
        .iter()
        .find(|spec| spec.model == token || spec.aliases.iter().any(|alias| *alias == token))
}

pub(super) fn fdm_printer_models_json() -> Value {
    Value::Array(
        FDM_PRINTER_MODEL_SPECS
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
                    "kinematics": spec.kinematics,
                    "workEnvelopeMm": spec.work_envelope_mm,
                    "maxPrintSpeedMmS": spec.max_print_speed_mm_s,
                    "maxAccelerationMmS2": spec.max_acceleration_mm_s2,
                    "maxNozzleTempC": spec.max_nozzle_temp_c,
                    "maxBedTempC": spec.max_bed_temp_c,
                    "maxVolumetricFlowMm3S": spec.max_volumetric_flow_mm3_s,
                    "enclosed": spec.enclosed,
                    "maxMaterials": spec.max_materials,
                    "materials": spec.materials,
                    "operations": spec.operations,
                    "notes": spec.notes,
                })
            })
            .collect(),
    )
}
