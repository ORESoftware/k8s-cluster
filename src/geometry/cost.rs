//! Manufacturing cost estimation from mesh geometry plus a slice/toolpath.
//!
//! Cost = material + machine time + setup, scaled by an overhead factor. The
//! material term uses the watertight part volume (times an infill fraction for
//! additive); the machine-time term derives from the sliced perimeter path
//! length at the planning feedrate, plus a per-layer change overhead. Every
//! input is explicit so estimates are deterministic and auditable.

use super::mesh::Vec3;

/// Tunable economic and process inputs. Callers may override any field; the API
/// layer supplies sensible per-process defaults.
#[derive(Clone, Copy, Debug)]
pub struct CostInputs {
    pub material_density_g_cm3: f64,
    pub material_price_per_kg: f64,
    pub machine_rate_per_hour: f64,
    pub setup_cost: f64,
    /// Fraction of the part volume actually filled with material (0..=1).
    pub infill_fraction: f64,
    pub feedrate_mm_per_min: f64,
    /// Seconds of non-cutting overhead added per layer (travel, layer change).
    pub layer_change_seconds: f64,
    /// Multiplicative overhead/margin applied to the subtotal (e.g. 0.2 = 20%).
    pub overhead_fraction: f64,
}

impl Default for CostInputs {
    fn default() -> Self {
        // Defaults model a desktop FDM job in PLA, mid-range shop rate.
        CostInputs {
            material_density_g_cm3: 1.24,
            material_price_per_kg: 25.0,
            machine_rate_per_hour: 30.0,
            setup_cost: 15.0,
            infill_fraction: 0.2,
            feedrate_mm_per_min: 3000.0,
            layer_change_seconds: 1.5,
            overhead_fraction: 0.15,
        }
    }
}

/// Itemized cost estimate, all monetary values in the same (caller-defined)
/// currency. Times are hours.
#[derive(Clone, Debug)]
pub struct CostEstimate {
    pub part_volume_cm3: f64,
    pub bbox_volume_cm3: f64,
    pub material_mass_g: f64,
    pub material_cost: f64,
    pub machine_time_hours: f64,
    pub machine_cost: f64,
    pub setup_cost: f64,
    pub subtotal: f64,
    pub overhead: f64,
    pub total: f64,
}

/// Estimate cost from the absolute part volume (mm^3), bounding box, sliced
/// perimeter `path_length_mm`, `layer_count`, and economic `inputs`.
pub fn estimate(
    abs_volume_mm3: f64,
    bbox: (Vec3, Vec3),
    path_length_mm: f64,
    layer_count: usize,
    inputs: &CostInputs,
) -> CostEstimate {
    let part_volume_cm3 = abs_volume_mm3 / 1000.0;
    let (min, max) = bbox;
    let bbox_volume_cm3 =
        ((max.x - min.x) * (max.y - min.y) * (max.z - min.z)).abs() / 1000.0;

    let infill = inputs.infill_fraction.clamp(0.0, 1.0);
    let material_mass_g = part_volume_cm3 * infill * inputs.material_density_g_cm3;
    let material_cost = (material_mass_g / 1000.0) * inputs.material_price_per_kg;

    let feed = if inputs.feedrate_mm_per_min > 0.0 {
        inputs.feedrate_mm_per_min
    } else {
        1.0
    };
    let cutting_hours = (path_length_mm / feed) / 60.0;
    let overhead_hours = (layer_count as f64 * inputs.layer_change_seconds) / 3600.0;
    let machine_time_hours = cutting_hours + overhead_hours;
    let machine_cost = machine_time_hours * inputs.machine_rate_per_hour;

    let subtotal = material_cost + machine_cost + inputs.setup_cost;
    let overhead = subtotal * inputs.overhead_fraction.max(0.0);
    let total = subtotal + overhead;

    CostEstimate {
        part_volume_cm3,
        bbox_volume_cm3,
        material_mass_g,
        material_cost,
        machine_time_hours,
        machine_cost,
        setup_cost: inputs.setup_cost,
        subtotal,
        overhead,
        total,
    }
}
