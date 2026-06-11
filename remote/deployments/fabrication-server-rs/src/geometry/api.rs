//! JSON request/response layer for the geometry engine.
//!
//! Each builder decodes submitted STL geometry, runs the relevant pure-geometry
//! pipeline, and returns a deterministic [`serde_json::Value`] carrying both the
//! computed result and release evidence (`status`, `machineReady`, `blockers`,
//! learning policy) consistent with the rest of the fabrication server.

use serde::Deserialize;
use serde_json::{json, Value};

use super::cost::{self, CostInputs};
use super::mesh::{self, Mesh};
use super::repair::{self, RepairReport};
use super::toolpath;

/// STL geometry payload shared by every geometry endpoint. Exactly one of
/// `stlBase64` (binary or ASCII bytes, base64-encoded) or `stlAscii` (inline
/// ASCII STL text) must be supplied.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeometryPayload {
    pub stl_base64: Option<String>,
    pub stl_ascii: Option<String>,
    /// Vertex-weld grid size in mm (default 1e-3).
    pub weld_tolerance_mm: Option<f64>,
}

fn round6(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    (v * 1_000_000.0).round() / 1_000_000.0
}

/// Hard ceiling on G-code sample layers emitted in a response, independent of
/// the caller-supplied `maxGcodeLayers`, so the JSON body stays bounded.
const MAX_GCODE_LAYERS: usize = 2_000;

/// Reject NaN/Inf so malformed numbers never reach the geometry math (where
/// they would silently serialize as JSON `null`).
fn require_finite(name: &str, v: f64) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("{} must be a finite number", name))
    }
}

fn require_positive(name: &str, v: f64) -> Result<f64, String> {
    let v = require_finite(name, v)?;
    if v > 0.0 {
        Ok(v)
    } else {
        Err(format!("{} must be greater than 0", name))
    }
}

fn require_non_negative(name: &str, v: f64) -> Result<f64, String> {
    let v = require_finite(name, v)?;
    if v >= 0.0 {
        Ok(v)
    } else {
        Err(format!("{} must not be negative", name))
    }
}

/// Validate and clamp a 0..=1 fraction.
fn require_fraction(name: &str, v: f64) -> Result<f64, String> {
    let v = require_finite(name, v)?;
    if (0.0..=1.0).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{} must be between 0 and 1", name))
    }
}

/// Decode the STL bytes and parse them into a mesh, returning the raw bytes too
/// (used to derive a deterministic request id).
fn load_mesh(payload: &GeometryPayload) -> Result<(Mesh, Vec<u8>), String> {
    let bytes: Vec<u8> = match (&payload.stl_base64, &payload.stl_ascii) {
        (Some(b64), None) => mesh::decode_base64(b64)?,
        (None, Some(ascii)) => ascii.clone().into_bytes(),
        (Some(_), Some(_)) => {
            return Err("provide either stlBase64 or stlAscii, not both".into())
        }
        (None, None) => return Err("missing geometry: supply stlBase64 or stlAscii".into()),
    };
    let mesh = mesh::parse_stl(&bytes)?;
    Ok((mesh, bytes))
}

fn weld_tol(payload: &GeometryPayload) -> f64 {
    payload.weld_tolerance_mm.filter(|t| *t > 0.0).unwrap_or(1e-3)
}

const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_CURRENCY_LEN: usize = 8;

fn derive_request_id(
    provided: &Option<String>,
    bytes: &[u8],
    prefix: &str,
) -> Result<String, String> {
    match provided {
        Some(id) => {
            if id.is_empty() || id.len() > MAX_REQUEST_ID_LEN {
                return Err(format!(
                    "requestId must be 1..={} characters",
                    MAX_REQUEST_ID_LEN
                ));
            }
            Ok(id.clone())
        }
        None => Ok(format!("{}-{:016x}", prefix, mesh::fnv1a_64(bytes))),
    }
}

/// Accept a short ASCII currency code (e.g. "USD"); otherwise fall back to USD
/// rather than echoing an arbitrary caller-controlled string.
fn sanitize_currency(provided: &Option<String>) -> String {
    match provided {
        Some(c)
            if !c.is_empty()
                && c.len() <= MAX_CURRENCY_LEN
                && c.chars().all(|ch| ch.is_ascii_alphanumeric()) =>
        {
            c.to_ascii_uppercase()
        }
        _ => "USD".to_string(),
    }
}

fn report_json(r: &RepairReport) -> Value {
    json!({
        "inputVertices": r.input_vertices,
        "inputTriangles": r.input_triangles,
        "weldedVertices": r.welded_vertices,
        "removedDegenerateTriangles": r.removed_degenerate,
        "removedDuplicateTriangles": r.removed_duplicate,
        "flippedForConsistency": r.flipped_for_consistency,
        "flippedGlobalOutward": r.flipped_global_outward,
        "boundaryEdgesBefore": r.boundary_edges_before,
        "nonManifoldEdges": r.non_manifold_edges,
        "holesDetected": r.holes_detected,
        "holesFilled": r.holes_filled,
        "trianglesAddedFilling": r.triangles_added_filling,
        "boundaryEdgesAfter": r.boundary_edges_after,
        "watertight": r.watertight,
        "outputVertices": r.output_vertices,
        "outputTriangles": r.output_triangles,
    })
}

fn metrics_json(m: &Mesh) -> Value {
    let (min, max) = m.bounding_box();
    json!({
        "triangles": m.triangles.len(),
        "vertices": m.vertices.len(),
        "surfaceAreaMm2": round6(m.surface_area()),
        "volumeMm3": round6(m.signed_volume().abs()),
        "boundingBoxMm": {
            "min": [round6(min.x), round6(min.y), round6(min.z)],
            "max": [round6(max.x), round6(max.y), round6(max.z)],
            "size": [round6(max.x - min.x), round6(max.y - min.y), round6(max.z - min.z)],
        }
    })
}

// ---------------------------------------------------------------------------
// POST /mesh-repair/plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRepairPlanRequest {
    pub request_id: Option<String>,
    #[serde(flatten)]
    pub geometry: GeometryPayload,
    /// When true, the base64 binary STL of the repaired shell is returned.
    pub return_repaired_stl: Option<bool>,
}

pub fn mesh_repair_plan_response(req: MeshRepairPlanRequest) -> Result<Value, String> {
    let (input, bytes) = load_mesh(&req.geometry)?;
    let request_id = derive_request_id(&req.request_id, &bytes, "mesh-repair-plan")?;
    let (repaired, report) = repair::repair(&input, weld_tol(&req.geometry));

    let mut blockers: Vec<String> = Vec::new();
    if report.non_manifold_edges > 0 {
        blockers.push(format!(
            "{} non-manifold edge(s) require manual topology review",
            report.non_manifold_edges
        ));
    }
    if report.boundary_edges_after > 0 {
        blockers.push(format!(
            "{} boundary edge(s) remain after hole filling; mesh is not watertight",
            report.boundary_edges_after
        ));
    }
    let machine_ready = report.watertight;
    let status = if machine_ready {
        "mesh-repair-watertight"
    } else if report.non_manifold_edges > 0 {
        "mesh-repair-non-manifold-blocked"
    } else {
        "mesh-repair-residual-boundary"
    };

    let mut response = json!({
        "ok": true,
        "requestId": request_id,
        "capability": "mesh-repair",
        "status": status,
        "machineReady": machine_ready,
        "weldToleranceMm": weld_tol(&req.geometry),
        "repair": report_json(&report),
        "metricsBefore": metrics_json(&input),
        "metricsAfter": metrics_json(&repaired),
        "blockers": blockers,
        "learning": {
            "policy": "deterministic-geometry",
            "signals": ["holesFilled", "flippedForConsistency", "watertight"],
        },
        "routes": ["POST /mesh-repair/plan", "POST /fabrication/mesh-repair/plan"],
        "resultRoutes": ["POST /mesh-repair/result", "POST /fabrication/mesh-repair/result"],
        "catalogRoutes": ["GET /mesh-repair/catalog", "GET /fabrication/mesh-repair/catalog"],
    });

    if req.return_repaired_stl.unwrap_or(false) {
        let stl = repaired.to_binary_stl();
        response["repairedStl"] = json!({
            "format": "stl-binary",
            "encoding": "base64",
            "byteLength": stl.len(),
            "data": mesh::encode_base64(&stl),
        });
    }
    Ok(response)
}

// ---------------------------------------------------------------------------
// POST /toolpaths/generate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolpathGenerateRequest {
    pub request_id: Option<String>,
    #[serde(flatten)]
    pub geometry: GeometryPayload,
    pub layer_height_mm: Option<f64>,
    pub feedrate_mm_per_min: Option<f64>,
    pub emit_gcode: Option<bool>,
    pub max_gcode_layers: Option<usize>,
}

pub fn toolpath_generate_response(req: ToolpathGenerateRequest) -> Result<Value, String> {
    let (input, bytes) = load_mesh(&req.geometry)?;
    let request_id = derive_request_id(&req.request_id, &bytes, "toolpath-generate")?;
    let layer_height = require_positive("layerHeightMm", req.layer_height_mm.unwrap_or(0.2))?;
    let feedrate = require_positive("feedrateMmPerMin", req.feedrate_mm_per_min.unwrap_or(3000.0))?;

    // Heal first so slicing operates on a watertight shell.
    let (repaired, report) = repair::repair(&input, weld_tol(&req.geometry));
    let slice = toolpath::slice(&repaired, layer_height)?;

    let mut blockers: Vec<String> = Vec::new();
    if !report.watertight {
        blockers.push(
            "input mesh was not watertight after repair; toolpaths may have gaps".into(),
        );
    }
    if slice.open_contours > 0 {
        blockers.push(format!(
            "{} open contour(s) detected; perimeters are not all closed",
            slice.open_contours
        ));
    }
    let machine_ready = report.watertight && slice.open_contours == 0 && slice.closed_contours > 0;
    let status = if machine_ready {
        "toolpaths-generated-closed-perimeters"
    } else {
        "toolpaths-generated-with-blockers"
    };

    let mut layers = json!({
        "layerHeightMm": round6(slice.layer_height),
        "layerCount": slice.layer_count,
        "closedContours": slice.closed_contours,
        "openContours": slice.open_contours,
        "totalPathLengthMm": round6(slice.total_path_length),
        "zMinMm": round6(slice.z_min),
        "zMaxMm": round6(slice.z_max),
    });

    let mut response = json!({
        "ok": true,
        "requestId": request_id,
        "capability": "toolpath-generation",
        "method": "planar-slice",
        "status": status,
        "machineReady": machine_ready,
        "feedrateMmPerMin": round6(feedrate),
        "repair": report_json(&report),
        "blockers": blockers,
        "learning": {
            "policy": "deterministic-geometry",
            "signals": ["totalPathLengthMm", "openContours", "layerCount"],
        },
        "routes": ["POST /toolpaths/generate", "POST /fabrication/toolpaths/generate"],
        "planRoutes": ["POST /toolpaths/plan", "POST /fabrication/toolpaths/plan"],
        "catalogRoutes": ["GET /toolpaths/catalog", "GET /fabrication/toolpaths/catalog"],
    });

    if req.emit_gcode.unwrap_or(false) {
        let max_layers = req.max_gcode_layers.unwrap_or(50).clamp(1, MAX_GCODE_LAYERS);
        let (gcode, truncated) = toolpath::to_gcode(&slice, feedrate, max_layers);
        layers["gcodeSample"] = json!({
            "dialect": "rs274-subset",
            "maxLayersEmitted": max_layers,
            "truncated": truncated,
            "text": gcode,
        });
    }
    response["toolpaths"] = layers;
    Ok(response)
}

// ---------------------------------------------------------------------------
// POST /costing/estimate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimateRequest {
    pub request_id: Option<String>,
    #[serde(flatten)]
    pub geometry: GeometryPayload,
    pub currency: Option<String>,
    pub material_density_g_cm3: Option<f64>,
    pub material_price_per_kg: Option<f64>,
    pub machine_rate_per_hour: Option<f64>,
    pub setup_cost: Option<f64>,
    pub infill_fraction: Option<f64>,
    pub feedrate_mm_per_min: Option<f64>,
    pub layer_height_mm: Option<f64>,
    pub layer_change_seconds: Option<f64>,
    pub overhead_fraction: Option<f64>,
}

pub fn cost_estimate_response(req: CostEstimateRequest) -> Result<Value, String> {
    let (input, bytes) = load_mesh(&req.geometry)?;
    let request_id = derive_request_id(&req.request_id, &bytes, "cost-estimate")?;
    let layer_height = require_positive("layerHeightMm", req.layer_height_mm.unwrap_or(0.2))?;

    let (repaired, report) = repair::repair(&input, weld_tol(&req.geometry));
    let slice = toolpath::slice(&repaired, layer_height)?;

    let defaults = CostInputs::default();
    let inputs = CostInputs {
        material_density_g_cm3: require_positive(
            "materialDensityGCm3",
            req.material_density_g_cm3.unwrap_or(defaults.material_density_g_cm3),
        )?,
        material_price_per_kg: require_non_negative(
            "materialPricePerKg",
            req.material_price_per_kg.unwrap_or(defaults.material_price_per_kg),
        )?,
        machine_rate_per_hour: require_non_negative(
            "machineRatePerHour",
            req.machine_rate_per_hour.unwrap_or(defaults.machine_rate_per_hour),
        )?,
        setup_cost: require_non_negative(
            "setupCost",
            req.setup_cost.unwrap_or(defaults.setup_cost),
        )?,
        infill_fraction: require_fraction(
            "infillFraction",
            req.infill_fraction.unwrap_or(defaults.infill_fraction),
        )?,
        feedrate_mm_per_min: require_positive(
            "feedrateMmPerMin",
            req.feedrate_mm_per_min.unwrap_or(defaults.feedrate_mm_per_min),
        )?,
        layer_change_seconds: require_non_negative(
            "layerChangeSeconds",
            req.layer_change_seconds.unwrap_or(defaults.layer_change_seconds),
        )?,
        overhead_fraction: require_non_negative(
            "overheadFraction",
            req.overhead_fraction.unwrap_or(defaults.overhead_fraction),
        )?,
    };

    let volume = repaired.signed_volume().abs();
    let est = cost::estimate(
        volume,
        repaired.bounding_box(),
        slice.total_path_length,
        slice.layer_count,
        &inputs,
    );

    // Watertight geometry yields a trustworthy volume; otherwise flag low
    // confidence rather than refusing to estimate.
    let confidence = if report.watertight { "high" } else { "low" };
    let currency = sanitize_currency(&req.currency);

    let response = json!({
        "ok": true,
        "requestId": request_id,
        "capability": "cost-estimation",
        "status": if report.watertight { "cost-estimated-watertight" } else { "cost-estimated-unverified-volume" },
        "machineReady": report.watertight,
        "confidence": confidence,
        "currency": currency,
        "inputs": {
            "materialDensityGCm3": inputs.material_density_g_cm3,
            "materialPricePerKg": inputs.material_price_per_kg,
            "machineRatePerHour": inputs.machine_rate_per_hour,
            "setupCost": inputs.setup_cost,
            "infillFraction": inputs.infill_fraction,
            "feedrateMmPerMin": inputs.feedrate_mm_per_min,
            "layerHeightMm": layer_height,
            "layerChangeSeconds": inputs.layer_change_seconds,
            "overheadFraction": inputs.overhead_fraction,
        },
        "estimate": {
            "partVolumeCm3": round6(est.part_volume_cm3),
            "boundingBoxVolumeCm3": round6(est.bbox_volume_cm3),
            "materialMassG": round6(est.material_mass_g),
            "materialCost": round6(est.material_cost),
            "machineTimeHours": round6(est.machine_time_hours),
            "machineCost": round6(est.machine_cost),
            "setupCost": round6(est.setup_cost),
            "subtotal": round6(est.subtotal),
            "overhead": round6(est.overhead),
            "total": round6(est.total),
        },
        "toolpathBasis": {
            "totalPathLengthMm": round6(slice.total_path_length),
            "layerCount": slice.layer_count,
        },
        "repair": report_json(&report),
        "learning": {
            "policy": "deterministic-geometry",
            "signals": ["total", "machineTimeHours", "materialMassG"],
        },
        "routes": ["POST /costing/estimate", "POST /fabrication/costing/estimate"],
        "resultRoutes": ["POST /costing/result", "POST /fabrication/costing/result"],
        "catalogRoutes": ["GET /costing/catalog", "GET /fabrication/costing/catalog"],
    });
    Ok(response)
}
