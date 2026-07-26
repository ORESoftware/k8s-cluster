//! Pure FDM and resin release-gate analysis.

use std::{error::Error, fmt};

use serde_json::json;

use super::{
    model::{
        Dimensions, FdmMachine, FdmMaterial, FdmProfile, FindingSeverity, PartGeometry,
        PreflightFinding, PrintPreflightRequest, PrintPreflightResponse, ResinMachine,
        ResinMaterial, ResinProfile,
    },
    ADDITIVE_PREFLIGHT_SCHEMA,
};

pub(crate) fn analyze(
    request: PrintPreflightRequest,
) -> Result<PrintPreflightResponse, PreflightError> {
    validate_request_id(request.request_id())?;
    match request {
        PrintPreflightRequest::Fdm {
            request_id,
            part,
            machine,
            material,
            profile,
        } => analyze_fdm(request_id, part, machine, material, profile),
        PrintPreflightRequest::Resin {
            request_id,
            part,
            machine,
            material,
            profile,
        } => analyze_resin(request_id, part, machine, material, profile),
    }
}

fn analyze_fdm(
    request_id: Option<String>,
    part: PartGeometry,
    machine: FdmMachine,
    material: FdmMaterial,
    profile: FdmProfile,
) -> Result<PrintPreflightResponse, PreflightError> {
    validate_part(&part)?;
    validate_dimensions("machine.buildVolume", &machine.build_volume)?;
    validate_positive("machine.nozzleDiameterMm", machine.nozzle_diameter_mm)?;
    validate_positive(
        "machine.maxVolumetricFlowMm3S",
        machine.max_volumetric_flow_mm3_s,
    )?;
    validate_nonempty("material.name", &material.name)?;
    for (name, value) in [
        ("profile.layerHeightMm", profile.layer_height_mm),
        ("profile.firstLayerHeightMm", profile.first_layer_height_mm),
        ("profile.lineWidthMm", profile.line_width_mm),
        ("profile.printSpeedMmS", profile.print_speed_mm_s),
        ("profile.nozzleTempC", profile.nozzle_temp_c),
        ("profile.bedTempC", profile.bed_temp_c),
    ] {
        validate_positive(name, value)?;
    }
    validate_range(
        "material.nozzle temperature",
        material.nozzle_temp_min_c,
        material.nozzle_temp_max_c,
    )?;
    validate_range(
        "material.bed temperature",
        material.bed_temp_min_c,
        material.bed_temp_max_c,
    )?;

    let mut findings = Vec::new();
    check_build_fit(&part.dimensions, &machine.build_volume, &mut findings);
    let layer_ratio = profile.layer_height_mm / machine.nozzle_diameter_mm;
    if layer_ratio > 0.8 {
        findings.push(blocker(
            "fdm.layer-height-too-large",
            "Layer height exceeds 80% of nozzle diameter, so layer bonding and extrusion geometry are not release-safe.",
            "Reduce layer height or select a larger nozzle, then re-slice and re-run preflight.",
        ));
    } else if layer_ratio < 0.2 {
        findings.push(warning(
            "fdm.layer-height-very-fine",
            "Layer height is below 20% of nozzle diameter and may add heat dwell, time, and repeatability risk.",
            "Validate the nozzle, motion system, cooling, and material profile at this layer height.",
        ));
    }
    let first_layer_ratio = profile.first_layer_height_mm / machine.nozzle_diameter_mm;
    if first_layer_ratio > 1.0 {
        findings.push(blocker(
            "fdm.first-layer-exceeds-nozzle",
            "First-layer height exceeds nozzle diameter.",
            "Set first-layer height at or below the nozzle diameter and recalibrate Z offset.",
        ));
    }
    let line_width_ratio = profile.line_width_mm / machine.nozzle_diameter_mm;
    if !(0.8..=1.8).contains(&line_width_ratio) {
        findings.push(warning(
            "fdm.line-width-outside-window",
            "Extrusion line width is outside the 0.8–1.8 nozzle-diameter review window.",
            "Use a validated line width for the nozzle and confirm dimensional/extrusion calibration.",
        ));
    }
    let volumetric_flow =
        profile.layer_height_mm * profile.line_width_mm * profile.print_speed_mm_s;
    if volumetric_flow > machine.max_volumetric_flow_mm3_s {
        findings.push(blocker(
            "fdm.volumetric-flow-exceeded",
            "Requested volumetric flow exceeds the machine/hotend capability.",
            "Reduce speed, layer height, or line width, or use a validated higher-flow hotend.",
        ));
    }
    if !within(
        profile.nozzle_temp_c,
        material.nozzle_temp_min_c,
        material.nozzle_temp_max_c,
    ) {
        findings.push(blocker(
            "fdm.nozzle-temperature-outside-material-window",
            "Nozzle temperature is outside the material's validated range.",
            "Select a validated material profile and temperature before release.",
        ));
    }
    if !within(
        profile.bed_temp_c,
        material.bed_temp_min_c,
        material.bed_temp_max_c,
    ) {
        findings.push(blocker(
            "fdm.bed-temperature-outside-material-window",
            "Bed temperature is outside the material's validated range.",
            "Correct the bed profile and verify first-layer adhesion.",
        ));
    }
    if part.max_overhang_degrees > 50.0 && !profile.supports_enabled {
        findings.push(blocker(
            "fdm.unsupported-overhang",
            "The model contains overhangs above 50 degrees without support generation.",
            "Reorient the part, add validated supports, or split the model for fabrication.",
        ));
    }
    if part.min_wall_mm < profile.line_width_mm * 2.0 {
        findings.push(blocker(
            "fdm.wall-below-two-lines",
            "Minimum wall thickness cannot retain two requested extrusion lines.",
            "Increase wall thickness, use a smaller nozzle/line width, or document a single-wall qualification.",
        ));
    }
    if material.enclosure_required && !machine.enclosed {
        findings.push(blocker(
            "fdm.enclosure-required",
            "The selected material requires a controlled enclosure but the machine is open.",
            "Route to an enclosed machine or select a material validated for the available environment.",
        ));
    }
    if material.drying_required && profile.dried_hours.unwrap_or_default() <= 0.0 {
        findings.push(blocker(
            "fdm.material-conditioning-missing",
            "The material requires drying, but no drying evidence was supplied.",
            "Dry the lot using its validated time/temperature profile and attach conditioning evidence.",
        ));
    }
    if profile.material_count > machine.max_materials {
        findings.push(blocker(
            "fdm.material-count-exceeds-machine",
            "The profile requests more materials than the machine can feed.",
            "Reduce material count, split the job, or route to a compatible multi-material machine.",
        ));
    }
    if profile.material_count > 1
        && profile.tool_changes > 0
        && profile.purge_volume_per_change_mm3 < 10.0
    {
        findings.push(blocker(
            "fdm.multi-material-purge-insufficient",
            "Multi-material tool changes do not include enough purge volume to qualify transitions.",
            "Calibrate purge volume by material pair and regenerate the purge tower or wipe strategy.",
        ));
    }

    finish(
        request_id,
        "fdm",
        findings,
        json!({
            "layerToNozzleRatio": rounded(layer_ratio),
            "firstLayerToNozzleRatio": rounded(first_layer_ratio),
            "lineWidthToNozzleRatio": rounded(line_width_ratio),
            "volumetricFlowMm3S": rounded(volumetric_flow),
            "volumetricFlowUtilization": rounded(volumetric_flow / machine.max_volumetric_flow_mm3_s),
            "material": material.name,
        }),
    )
}

fn analyze_resin(
    request_id: Option<String>,
    part: PartGeometry,
    machine: ResinMachine,
    material: ResinMaterial,
    profile: ResinProfile,
) -> Result<PrintPreflightResponse, PreflightError> {
    validate_part(&part)?;
    validate_dimensions("machine.buildVolume", &machine.build_volume)?;
    validate_positive("machine.buildPlateAreaMm2", machine.build_plate_area_mm2)?;
    validate_positive("machine.minWallMm", machine.min_wall_mm)?;
    validate_nonempty("material.name", &material.name)?;
    validate_range(
        "material.exposure",
        material.exposure_min_s,
        material.exposure_max_s,
    )?;
    for (name, value) in [
        ("profile.layerHeightMm", profile.layer_height_mm),
        ("profile.exposureS", profile.exposure_s),
    ] {
        validate_positive(name, value)?;
    }

    let mut findings = Vec::new();
    check_build_fit(&part.dimensions, &machine.build_volume, &mut findings);
    if !(0.02..=0.1).contains(&profile.layer_height_mm) {
        findings.push(warning(
            "resin.layer-height-review",
            "Layer height is outside the common 0.02–0.10 mm qualification window.",
            "Confirm the printer, resin, exposure, and motion profile for this layer height.",
        ));
    }
    if part.min_wall_mm < machine.min_wall_mm {
        findings.push(blocker(
            "resin.wall-below-machine-minimum",
            "Minimum wall thickness is below the machine/resin qualification limit.",
            "Thicken the wall or route to a process with a validated smaller feature limit.",
        ));
    }
    if !within(
        profile.exposure_s,
        material.exposure_min_s,
        material.exposure_max_s,
    ) {
        findings.push(blocker(
            "resin.exposure-outside-material-window",
            "Normal-layer exposure is outside the resin's validated range.",
            "Use a calibrated resin profile and confirm exposure with a validation coupon.",
        ));
    }
    if part.has_enclosed_voids && profile.drain_hole_count < 2 {
        findings.push(blocker(
            "resin.enclosed-volume-drainage",
            "An enclosed resin volume has fewer than two drain/vent openings.",
            "Add appropriately placed drain and vent holes, or remove the enclosed cavity.",
        ));
    }
    if part.has_islands && !profile.supports_enabled {
        findings.push(blocker(
            "resin.unsupported-islands",
            "Slice analysis reports islands without supports.",
            "Reorient the part or add supports and re-run island detection.",
        ));
    }
    if part.max_overhang_degrees > 45.0 && !profile.supports_enabled {
        findings.push(blocker(
            "resin.unsupported-overhang",
            "The orientation contains release-risk overhangs without supports.",
            "Reorient or support the model, then inspect contact and peel forces.",
        ));
    }
    if profile.wash_minutes < material.minimum_wash_minutes {
        findings.push(blocker(
            "resin.wash-cycle-insufficient",
            "The wash cycle is shorter than the resin's minimum validated duration.",
            "Apply the full validated wash process and retain solvent/bath condition evidence.",
        ));
    }
    if profile.cure_minutes < material.minimum_cure_minutes {
        findings.push(blocker(
            "resin.cure-cycle-insufficient",
            "The cure cycle is shorter than the resin's minimum validated duration.",
            "Apply the validated UV cure time/temperature and record the post-cure lot evidence.",
        ));
    }
    let cross_section_utilization = part.max_cross_section_area_mm2 / machine.build_plate_area_mm2;
    if cross_section_utilization > 0.65 && profile.lift_speed_mm_min > 80.0 {
        findings.push(blocker(
            "resin.peel-force-risk",
            "Large cross-section utilization and lift speed create excessive peel/suction risk.",
            "Reorient or hollow the part, add drainage, and reduce lift speed using a validated profile.",
        ));
    }

    finish(
        request_id,
        "resin",
        findings,
        json!({
            "crossSectionUtilization": rounded(cross_section_utilization),
            "exposureS": profile.exposure_s,
            "drainHoleCount": profile.drain_hole_count,
            "washMinutes": profile.wash_minutes,
            "cureMinutes": profile.cure_minutes,
            "material": material.name,
        }),
    )
}

fn check_build_fit(part: &Dimensions, build: &Dimensions, findings: &mut Vec<PreflightFinding>) {
    if part.x_mm + 2.0 > build.x_mm || part.y_mm + 2.0 > build.y_mm || part.z_mm + 1.0 > build.z_mm
    {
        findings.push(blocker(
            "printing.build-volume-exceeded",
            "Part dimensions plus release margin exceed the configured build volume.",
            "Reorient or split the model, or route it to a machine with a larger validated envelope.",
        ));
    }
}

fn finish(
    request_id: Option<String>,
    process: &str,
    mut findings: Vec<PreflightFinding>,
    derived: serde_json::Value,
) -> Result<PrintPreflightResponse, PreflightError> {
    if findings.is_empty() {
        findings.push(PreflightFinding {
            code: format!("{process}.preflight-clear"),
            severity: FindingSeverity::Info,
            release_gate: false,
            message: "No modeled additive preflight blockers were found.".to_string(),
            remediation: "Continue with slicing, simulation, first-article inspection, and operator release gates.".to_string(),
        });
    }
    let blocker_count = findings
        .iter()
        .filter(|finding| finding.severity == FindingSeverity::Blocker)
        .count();
    let warning_count = findings
        .iter()
        .filter(|finding| finding.severity == FindingSeverity::Warning)
        .count();
    let risk_score = (blocker_count * 25 + warning_count * 8).min(100) as u8;

    Ok(PrintPreflightResponse {
        schema_version: ADDITIVE_PREFLIGHT_SCHEMA.to_string(),
        request_id,
        process: process.to_string(),
        release_ready: blocker_count == 0,
        risk_score,
        findings,
        derived,
    })
}

fn blocker(code: &str, message: &str, remediation: &str) -> PreflightFinding {
    finding(code, FindingSeverity::Blocker, true, message, remediation)
}

fn warning(code: &str, message: &str, remediation: &str) -> PreflightFinding {
    finding(code, FindingSeverity::Warning, false, message, remediation)
}

fn finding(
    code: &str,
    severity: FindingSeverity,
    release_gate: bool,
    message: &str,
    remediation: &str,
) -> PreflightFinding {
    PreflightFinding {
        code: code.to_string(),
        severity,
        release_gate,
        message: message.to_string(),
        remediation: remediation.to_string(),
    }
}

fn validate_request_id(request_id: Option<&str>) -> Result<(), PreflightError> {
    if request_id.is_some_and(|value| value.trim().is_empty() || value.len() > 128) {
        return Err(PreflightError(
            "requestId must contain 1–128 characters when supplied".to_string(),
        ));
    }
    Ok(())
}

fn validate_part(part: &PartGeometry) -> Result<(), PreflightError> {
    validate_dimensions("part.dimensions", &part.dimensions)?;
    validate_positive("part.minWallMm", part.min_wall_mm)?;
    validate_nonnegative("part.maxOverhangDegrees", part.max_overhang_degrees)?;
    validate_nonnegative(
        "part.maxCrossSectionAreaMm2",
        part.max_cross_section_area_mm2,
    )
}

fn validate_dimensions(name: &str, dimensions: &Dimensions) -> Result<(), PreflightError> {
    validate_positive(&format!("{name}.xMm"), dimensions.x_mm)?;
    validate_positive(&format!("{name}.yMm"), dimensions.y_mm)?;
    validate_positive(&format!("{name}.zMm"), dimensions.z_mm)
}

fn validate_positive(name: &str, value: f64) -> Result<(), PreflightError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(PreflightError(format!(
            "{name} must be a positive finite number"
        )));
    }
    Ok(())
}

fn validate_nonnegative(name: &str, value: f64) -> Result<(), PreflightError> {
    if !value.is_finite() || value < 0.0 {
        return Err(PreflightError(format!(
            "{name} must be a non-negative finite number"
        )));
    }
    Ok(())
}

fn validate_range(name: &str, minimum: f64, maximum: f64) -> Result<(), PreflightError> {
    validate_positive(&format!("{name} minimum"), minimum)?;
    validate_positive(&format!("{name} maximum"), maximum)?;
    if minimum > maximum {
        return Err(PreflightError(format!(
            "{name} minimum must not exceed maximum"
        )));
    }
    Ok(())
}

fn validate_nonempty(name: &str, value: &str) -> Result<(), PreflightError> {
    if value.trim().is_empty() || value.len() > 128 {
        return Err(PreflightError(format!(
            "{name} must contain 1–128 characters"
        )));
    }
    Ok(())
}

fn within(value: f64, minimum: f64, maximum: f64) -> bool {
    (minimum..=maximum).contains(&value)
}

fn rounded(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreflightError(pub(crate) String);

impl fmt::Display for PreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for PreflightError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimensions(x: f64, y: f64, z: f64) -> Dimensions {
        Dimensions {
            x_mm: x,
            y_mm: y,
            z_mm: z,
        }
    }

    fn part() -> PartGeometry {
        PartGeometry {
            dimensions: dimensions(50.0, 40.0, 30.0),
            min_wall_mm: 1.2,
            max_overhang_degrees: 35.0,
            has_enclosed_voids: false,
            has_islands: false,
            max_cross_section_area_mm2: 1_000.0,
        }
    }

    fn fdm_request() -> PrintPreflightRequest {
        PrintPreflightRequest::Fdm {
            request_id: Some("fdm-safe".to_string()),
            part: part(),
            machine: FdmMachine {
                build_volume: dimensions(220.0, 220.0, 250.0),
                nozzle_diameter_mm: 0.4,
                max_volumetric_flow_mm3_s: 15.0,
                enclosed: true,
                max_materials: 2,
            },
            material: FdmMaterial {
                name: "PETG".to_string(),
                nozzle_temp_min_c: 225.0,
                nozzle_temp_max_c: 250.0,
                bed_temp_min_c: 70.0,
                bed_temp_max_c: 90.0,
                drying_required: true,
                enclosure_required: false,
            },
            profile: FdmProfile {
                layer_height_mm: 0.2,
                first_layer_height_mm: 0.24,
                line_width_mm: 0.44,
                print_speed_mm_s: 60.0,
                nozzle_temp_c: 240.0,
                bed_temp_c: 80.0,
                supports_enabled: true,
                material_count: 1,
                tool_changes: 0,
                purge_volume_per_change_mm3: 0.0,
                dried_hours: Some(4.0),
            },
        }
    }

    fn resin_request() -> PrintPreflightRequest {
        PrintPreflightRequest::Resin {
            request_id: Some("resin-safe".to_string()),
            part: part(),
            machine: ResinMachine {
                build_volume: dimensions(130.0, 80.0, 160.0),
                build_plate_area_mm2: 10_400.0,
                min_wall_mm: 0.8,
            },
            material: ResinMaterial {
                name: "tough-resin".to_string(),
                exposure_min_s: 2.0,
                exposure_max_s: 3.0,
                minimum_wash_minutes: 3.0,
                minimum_cure_minutes: 5.0,
            },
            profile: ResinProfile {
                layer_height_mm: 0.05,
                exposure_s: 2.5,
                supports_enabled: true,
                drain_hole_count: 2,
                wash_minutes: 5.0,
                cure_minutes: 10.0,
                lift_speed_mm_min: 60.0,
            },
        }
    }

    fn creality_k1_request() -> PrintPreflightRequest {
        PrintPreflightRequest::Fdm {
            request_id: Some("creality-k1".to_string()),
            part: part(),
            machine: FdmMachine {
                build_volume: dimensions(220.0, 220.0, 250.0),
                nozzle_diameter_mm: 0.4,
                max_volumetric_flow_mm3_s: 32.0,
                enclosed: true,
                max_materials: 1,
            },
            material: FdmMaterial {
                name: "PETG".to_string(),
                nozzle_temp_min_c: 225.0,
                nozzle_temp_max_c: 260.0,
                bed_temp_min_c: 70.0,
                bed_temp_max_c: 90.0,
                drying_required: true,
                enclosure_required: false,
            },
            profile: FdmProfile {
                layer_height_mm: 0.2,
                first_layer_height_mm: 0.24,
                line_width_mm: 0.42,
                print_speed_mm_s: 300.0,
                nozzle_temp_c: 250.0,
                bed_temp_c: 80.0,
                supports_enabled: true,
                material_count: 1,
                tool_changes: 0,
                purge_volume_per_change_mm3: 0.0,
                dried_hours: Some(6.0),
            },
        }
    }

    fn creality_k2_request() -> PrintPreflightRequest {
        PrintPreflightRequest::Fdm {
            request_id: Some("creality-k2".to_string()),
            part: part(),
            machine: FdmMachine {
                build_volume: dimensions(260.0, 260.0, 260.0),
                nozzle_diameter_mm: 0.4,
                max_volumetric_flow_mm3_s: 32.0,
                enclosed: true,
                max_materials: 16,
            },
            material: FdmMaterial {
                name: "PLA".to_string(),
                nozzle_temp_min_c: 190.0,
                nozzle_temp_max_c: 230.0,
                bed_temp_min_c: 50.0,
                bed_temp_max_c: 65.0,
                drying_required: false,
                enclosure_required: false,
            },
            profile: FdmProfile {
                layer_height_mm: 0.2,
                first_layer_height_mm: 0.24,
                line_width_mm: 0.42,
                print_speed_mm_s: 300.0,
                nozzle_temp_c: 220.0,
                bed_temp_c: 60.0,
                supports_enabled: true,
                material_count: 4,
                tool_changes: 24,
                purge_volume_per_change_mm3: 45.0,
                dried_hours: None,
            },
        }
    }

    #[test]
    fn creality_k1_high_speed_profile_is_release_ready() {
        let response = analyze(creality_k1_request()).expect("FDM analysis");
        assert!(response.release_ready, "findings: {:?}", response.findings);
        assert_eq!(response.derived["volumetricFlowMm3S"], 25.2);
    }

    #[test]
    fn creality_k2_cfs_multi_material_profile_is_release_ready() {
        let response = analyze(creality_k2_request()).expect("FDM analysis");
        assert!(response.release_ready, "findings: {:?}", response.findings);
    }

    fn bambu_a1_request() -> PrintPreflightRequest {
        PrintPreflightRequest::Fdm {
            request_id: Some("bambu-a1".to_string()),
            part: part(),
            machine: FdmMachine {
                build_volume: dimensions(256.0, 256.0, 256.0),
                nozzle_diameter_mm: 0.4,
                max_volumetric_flow_mm3_s: 28.0,
                enclosed: false,
                max_materials: 1,
            },
            material: FdmMaterial {
                name: "PLA".to_string(),
                nozzle_temp_min_c: 190.0,
                nozzle_temp_max_c: 230.0,
                bed_temp_min_c: 45.0,
                bed_temp_max_c: 60.0,
                drying_required: false,
                enclosure_required: false,
            },
            profile: FdmProfile {
                layer_height_mm: 0.2,
                first_layer_height_mm: 0.24,
                line_width_mm: 0.42,
                print_speed_mm_s: 250.0,
                nozzle_temp_c: 220.0,
                bed_temp_c: 55.0,
                supports_enabled: true,
                material_count: 1,
                tool_changes: 0,
                purge_volume_per_change_mm3: 0.0,
                dried_hours: None,
            },
        }
    }

    fn bambu_a1_combo_request() -> PrintPreflightRequest {
        PrintPreflightRequest::Fdm {
            request_id: Some("bambu-a1-combo".to_string()),
            part: part(),
            machine: FdmMachine {
                build_volume: dimensions(256.0, 256.0, 256.0),
                nozzle_diameter_mm: 0.4,
                max_volumetric_flow_mm3_s: 28.0,
                enclosed: false,
                max_materials: 4,
            },
            material: FdmMaterial {
                name: "PLA".to_string(),
                nozzle_temp_min_c: 190.0,
                nozzle_temp_max_c: 230.0,
                bed_temp_min_c: 45.0,
                bed_temp_max_c: 60.0,
                drying_required: false,
                enclosure_required: false,
            },
            profile: FdmProfile {
                layer_height_mm: 0.2,
                first_layer_height_mm: 0.24,
                line_width_mm: 0.42,
                print_speed_mm_s: 250.0,
                nozzle_temp_c: 220.0,
                bed_temp_c: 55.0,
                supports_enabled: true,
                material_count: 4,
                tool_changes: 24,
                purge_volume_per_change_mm3: 45.0,
                dried_hours: None,
            },
        }
    }

    fn bambu_x1_carbon_request() -> PrintPreflightRequest {
        PrintPreflightRequest::Fdm {
            request_id: Some("bambu-x1-carbon".to_string()),
            part: part(),
            machine: FdmMachine {
                build_volume: dimensions(256.0, 256.0, 256.0),
                nozzle_diameter_mm: 0.4,
                max_volumetric_flow_mm3_s: 32.0,
                enclosed: true,
                max_materials: 16,
            },
            material: FdmMaterial {
                name: "ABS".to_string(),
                nozzle_temp_min_c: 240.0,
                nozzle_temp_max_c: 270.0,
                bed_temp_min_c: 90.0,
                bed_temp_max_c: 110.0,
                drying_required: false,
                enclosure_required: true,
            },
            profile: FdmProfile {
                layer_height_mm: 0.2,
                first_layer_height_mm: 0.24,
                line_width_mm: 0.42,
                print_speed_mm_s: 200.0,
                nozzle_temp_c: 250.0,
                bed_temp_c: 100.0,
                supports_enabled: true,
                material_count: 4,
                tool_changes: 20,
                purge_volume_per_change_mm3: 60.0,
                dried_hours: None,
            },
        }
    }

    #[test]
    fn bambu_a1_open_frame_profile_is_release_ready() {
        let response = analyze(bambu_a1_request()).expect("FDM analysis");
        assert!(response.release_ready, "findings: {:?}", response.findings);
        assert_eq!(response.derived["volumetricFlowMm3S"], 21.0);
    }

    #[test]
    fn bambu_a1_combo_ams_lite_multi_material_profile_is_release_ready() {
        let response = analyze(bambu_a1_combo_request()).expect("FDM analysis");
        assert!(response.release_ready, "findings: {:?}", response.findings);
    }

    #[test]
    fn bambu_x1_carbon_enclosed_abs_profile_is_release_ready() {
        // ABS demands an enclosure; the enclosed CoreXY machine satisfies it, so the
        // enclosure boundary must not fire.
        let response = analyze(bambu_x1_carbon_request()).expect("FDM analysis");
        assert!(response.release_ready, "findings: {:?}", response.findings);
    }

    #[test]
    fn safe_fdm_profile_remains_release_ready() {
        let response = analyze(fdm_request()).expect("FDM analysis");
        assert!(response.release_ready);
        assert_eq!(response.risk_score, 0);
        assert_eq!(response.derived["volumetricFlowMm3S"], 5.28);
    }

    #[test]
    fn fdm_flow_and_layer_geometry_are_release_gates() {
        let mut request = fdm_request();
        let PrintPreflightRequest::Fdm {
            machine, profile, ..
        } = &mut request
        else {
            unreachable!()
        };
        machine.max_volumetric_flow_mm3_s = 3.0;
        profile.layer_height_mm = 0.36;

        let response = analyze(request).expect("FDM analysis");
        assert!(!response.release_ready);
        assert!(response
            .findings
            .iter()
            .any(|finding| finding.code == "fdm.volumetric-flow-exceeded"));
        assert!(response
            .findings
            .iter()
            .any(|finding| finding.code == "fdm.layer-height-too-large"));
    }

    #[test]
    fn fdm_material_conditioning_enclosure_and_purge_are_checked() {
        let mut request = fdm_request();
        let PrintPreflightRequest::Fdm {
            machine,
            material,
            profile,
            ..
        } = &mut request
        else {
            unreachable!()
        };
        machine.enclosed = false;
        material.enclosure_required = true;
        profile.dried_hours = None;
        profile.material_count = 2;
        profile.tool_changes = 12;
        profile.purge_volume_per_change_mm3 = 2.0;

        let response = analyze(request).expect("FDM analysis");
        for code in [
            "fdm.enclosure-required",
            "fdm.material-conditioning-missing",
            "fdm.multi-material-purge-insufficient",
        ] {
            assert!(response.findings.iter().any(|finding| finding.code == code));
        }
    }

    #[test]
    fn safe_resin_profile_remains_release_ready() {
        let response = analyze(resin_request()).expect("resin analysis");
        assert!(response.release_ready);
        assert_eq!(response.process, "resin");
    }

    #[test]
    fn resin_void_island_wash_and_cure_checks_block_release() {
        let mut request = resin_request();
        let PrintPreflightRequest::Resin { part, profile, .. } = &mut request else {
            unreachable!()
        };
        part.has_enclosed_voids = true;
        part.has_islands = true;
        profile.supports_enabled = false;
        profile.drain_hole_count = 1;
        profile.wash_minutes = 1.0;
        profile.cure_minutes = 1.0;

        let response = analyze(request).expect("resin analysis");
        for code in [
            "resin.enclosed-volume-drainage",
            "resin.unsupported-islands",
            "resin.wash-cycle-insufficient",
            "resin.cure-cycle-insufficient",
        ] {
            assert!(response.findings.iter().any(|finding| finding.code == code));
        }
    }

    #[test]
    fn resin_cross_section_and_lift_speed_detect_peel_risk() {
        let mut request = resin_request();
        let PrintPreflightRequest::Resin {
            part,
            machine,
            profile,
            ..
        } = &mut request
        else {
            unreachable!()
        };
        part.max_cross_section_area_mm2 = machine.build_plate_area_mm2 * 0.8;
        profile.lift_speed_mm_min = 120.0;

        let response = analyze(request).expect("resin analysis");
        assert!(response
            .findings
            .iter()
            .any(|finding| finding.code == "resin.peel-force-risk"));
    }

    #[test]
    fn invalid_numeric_input_is_rejected_before_release_logic() {
        let mut request = fdm_request();
        let PrintPreflightRequest::Fdm { profile, .. } = &mut request else {
            unreachable!()
        };
        profile.print_speed_mm_s = f64::NAN;

        assert_eq!(
            analyze(request).expect_err("invalid input").to_string(),
            "profile.printSpeedMmS must be a positive finite number"
        );
    }
}
