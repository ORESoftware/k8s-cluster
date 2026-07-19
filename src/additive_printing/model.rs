//! Stable request/response types for additive-manufacturing preflight.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "process", rename_all = "lowercase")]
pub(crate) enum PrintPreflightRequest {
    Fdm {
        #[serde(rename = "requestId")]
        request_id: Option<String>,
        part: PartGeometry,
        machine: FdmMachine,
        material: FdmMaterial,
        profile: FdmProfile,
    },
    Resin {
        #[serde(rename = "requestId")]
        request_id: Option<String>,
        part: PartGeometry,
        machine: ResinMachine,
        material: ResinMaterial,
        profile: ResinProfile,
    },
}

impl PrintPreflightRequest {
    pub(crate) fn request_id(&self) -> Option<&str> {
        match self {
            Self::Fdm { request_id, .. } | Self::Resin { request_id, .. } => request_id.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Dimensions {
    pub(crate) x_mm: f64,
    pub(crate) y_mm: f64,
    pub(crate) z_mm: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PartGeometry {
    pub(crate) dimensions: Dimensions,
    pub(crate) min_wall_mm: f64,
    #[serde(default)]
    pub(crate) max_overhang_degrees: f64,
    #[serde(default)]
    pub(crate) has_enclosed_voids: bool,
    #[serde(default)]
    pub(crate) has_islands: bool,
    #[serde(default)]
    pub(crate) max_cross_section_area_mm2: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FdmMachine {
    pub(crate) build_volume: Dimensions,
    pub(crate) nozzle_diameter_mm: f64,
    pub(crate) max_volumetric_flow_mm3_s: f64,
    #[serde(default)]
    pub(crate) enclosed: bool,
    #[serde(default = "one")]
    pub(crate) max_materials: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FdmMaterial {
    pub(crate) name: String,
    pub(crate) nozzle_temp_min_c: f64,
    pub(crate) nozzle_temp_max_c: f64,
    pub(crate) bed_temp_min_c: f64,
    pub(crate) bed_temp_max_c: f64,
    #[serde(default)]
    pub(crate) drying_required: bool,
    #[serde(default)]
    pub(crate) enclosure_required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FdmProfile {
    pub(crate) layer_height_mm: f64,
    pub(crate) first_layer_height_mm: f64,
    pub(crate) line_width_mm: f64,
    pub(crate) print_speed_mm_s: f64,
    pub(crate) nozzle_temp_c: f64,
    pub(crate) bed_temp_c: f64,
    #[serde(default)]
    pub(crate) supports_enabled: bool,
    #[serde(default = "one")]
    pub(crate) material_count: u16,
    #[serde(default)]
    pub(crate) tool_changes: u32,
    #[serde(default)]
    pub(crate) purge_volume_per_change_mm3: f64,
    pub(crate) dried_hours: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResinMachine {
    pub(crate) build_volume: Dimensions,
    pub(crate) build_plate_area_mm2: f64,
    pub(crate) min_wall_mm: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResinMaterial {
    pub(crate) name: String,
    pub(crate) exposure_min_s: f64,
    pub(crate) exposure_max_s: f64,
    #[serde(default = "default_wash_minutes")]
    pub(crate) minimum_wash_minutes: f64,
    #[serde(default = "default_cure_minutes")]
    pub(crate) minimum_cure_minutes: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResinProfile {
    pub(crate) layer_height_mm: f64,
    pub(crate) exposure_s: f64,
    #[serde(default)]
    pub(crate) supports_enabled: bool,
    #[serde(default)]
    pub(crate) drain_hole_count: u16,
    #[serde(default)]
    pub(crate) wash_minutes: f64,
    #[serde(default)]
    pub(crate) cure_minutes: f64,
    #[serde(default)]
    pub(crate) lift_speed_mm_min: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FindingSeverity {
    Info,
    Warning,
    Blocker,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightFinding {
    pub(crate) code: String,
    pub(crate) severity: FindingSeverity,
    pub(crate) release_gate: bool,
    pub(crate) message: String,
    pub(crate) remediation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrintPreflightResponse {
    pub(crate) schema_version: String,
    pub(crate) request_id: Option<String>,
    pub(crate) process: String,
    pub(crate) release_ready: bool,
    pub(crate) risk_score: u8,
    pub(crate) findings: Vec<PreflightFinding>,
    pub(crate) derived: Value,
}

const fn one() -> u16 {
    1
}

const fn default_wash_minutes() -> f64 {
    3.0
}

const fn default_cure_minutes() -> f64 {
    5.0
}
