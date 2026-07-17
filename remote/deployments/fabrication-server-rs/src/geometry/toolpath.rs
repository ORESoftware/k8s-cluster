//! Toolpath generation by planar slicing.
//!
//! Intersects the mesh with a stack of horizontal planes, stitches the per-layer
//! line segments into closed perimeter contours, and reports path length / layer
//! statistics. A capped G-code sample (G0/G1 perimeter motion) can be emitted for
//! inspection. Pure geometry — additive (FDM-style) perimeter planning; the same
//! contour output is the basis for the cost model's motion-time estimate.

use std::collections::HashMap;

use super::mesh::{Mesh, Vec3};

/// Upper bound on slice layers. A hostile (tiny) layer height on a tall part
/// would otherwise drive unbounded work; callers get a clear error instead.
const MAX_SLICE_LAYERS: usize = 100_000;

/// Upper bound on `layers * triangles` scan operations for one slice request,
/// the dominant cost of slicing. Caps the multiplicative DoS that body-size
/// limits alone do not constrain.
const MAX_SLICE_OPS: u64 = 400_000_000;

/// Upper bound on total cross-section segments retained across all layers, so
/// pathological geometry cannot exhaust memory inside the body limit.
const MAX_TOTAL_SEGMENTS: usize = 4_000_000;

/// A single closed (or open, if stitching failed) loop at a given Z height.
#[derive(Clone, Debug)]
pub struct Contour {
    pub z: f64,
    pub points: Vec<(f64, f64)>,
    pub closed: bool,
    pub length: f64,
}

/// Full slice result across all layers.
#[derive(Clone, Debug, Default)]
pub struct SliceResult {
    pub layer_height: f64,
    pub z_min: f64,
    pub z_max: f64,
    pub layer_count: usize,
    pub contours: Vec<Contour>,
    pub total_path_length: f64,
    pub closed_contours: usize,
    pub open_contours: usize,
}

const STITCH_EPS: f64 = 1e-4;

/// Intersection point of segment p->q with plane z=`z` (caller guarantees the
/// segment straddles the plane).
fn plane_cross(p: Vec3, q: Vec3, z: f64) -> (f64, f64) {
    let t = (z - p.z) / (q.z - p.z);
    (p.x + t * (q.x - p.x), p.y + t * (q.y - p.y))
}

/// All line segments produced by slicing `mesh` at height `z`.
fn segments_at(mesh: &Mesh, z: f64) -> Vec<((f64, f64), (f64, f64))> {
    let mut segs = Vec::new();
    for t in 0..mesh.triangles.len() {
        let (a, b, c) = mesh.triangle_points(t);
        let verts = [a, b, c];
        let mut hits: Vec<(f64, f64)> = Vec::new();
        for k in 0..3 {
            let p = verts[k];
            let q = verts[(k + 1) % 3];
            // Strict straddle avoids double-counting a vertex that lies exactly
            // on the plane.
            if (p.z - z) * (q.z - z) < 0.0 {
                hits.push(plane_cross(p, q, z));
            }
        }
        if hits.len() == 2 {
            segs.push((hits[0], hits[1]));
        }
    }
    segs
}

type Point2 = (f64, f64);
type Segment2 = (Point2, Point2);

fn dist(a: Point2, b: Point2) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Quantize a point to a STITCH_EPS grid cell for spatial bucketing.
fn cell(p: Point2) -> (i64, i64) {
    (
        (p.0 / STITCH_EPS).round() as i64,
        (p.1 / STITCH_EPS).round() as i64,
    )
}

/// Stitch unordered segments into contours by chaining matching endpoints
/// within `STITCH_EPS`. Endpoints are indexed in a spatial hash so chaining is
/// near-linear in the segment count rather than O(segments^2) — this removes a
/// quadratic-time DoS for layers with many cross-section segments.
fn stitch(segments: &[Segment2], z: f64) -> Vec<Contour> {
    // Bucket every segment endpoint by grid cell.
    let mut buckets: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, seg) in segments.iter().enumerate() {
        buckets.entry(cell(seg.0)).or_default().push(i);
        buckets.entry(cell(seg.1)).or_default().push(i);
    }
    // Find an unused segment with an endpoint within STITCH_EPS of `tail`,
    // searching the 3x3 cell neighborhood so matches near a cell boundary are
    // not missed. Returns the segment index and its far endpoint.
    let find = |tail: Point2, used: &[bool]| -> Option<(usize, Point2)> {
        let (cx, cy) = cell(tail);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(cands) = buckets.get(&(cx + dx, cy + dy)) {
                    for &c in cands {
                        if used[c] {
                            continue;
                        }
                        if dist(segments[c].0, tail) <= STITCH_EPS {
                            return Some((c, segments[c].1));
                        } else if dist(segments[c].1, tail) <= STITCH_EPS {
                            return Some((c, segments[c].0));
                        }
                    }
                }
            }
        }
        None
    };

    let mut used = vec![false; segments.len()];
    let mut contours = Vec::new();
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut pts = vec![segments[start].0, segments[start].1];
        // Extend from the tail until no segment connects.
        loop {
            let tail = *pts.last().unwrap();
            match find(tail, &used) {
                Some((c, far)) => {
                    used[c] = true;
                    pts.push(far);
                }
                None => break,
            }
        }
        let closed = pts.len() > 2 && dist(*pts.first().unwrap(), *pts.last().unwrap()) <= STITCH_EPS;
        if closed {
            pts.pop(); // drop duplicate closing point
        }
        let mut length = 0.0;
        for w in pts.windows(2) {
            length += dist(w[0], w[1]);
        }
        if closed && pts.len() > 1 {
            length += dist(*pts.last().unwrap(), pts[0]);
        }
        contours.push(Contour {
            z,
            points: pts,
            closed,
            length,
        });
    }
    contours
}

/// Slice `mesh` into perimeter contours at the given `layer_height` (mm).
/// Layers are sampled at half-height offsets to avoid coplanar-face ambiguity.
pub fn slice(mesh: &Mesh, layer_height: f64) -> Result<SliceResult, String> {
    if !layer_height.is_finite() || layer_height <= 0.0 {
        return Err("layerHeightMm must be > 0".into());
    }
    if mesh.triangles.is_empty() {
        return Err("mesh has no triangles to slice".into());
    }
    let (min, max) = mesh.bounding_box();
    let height = max.z - min.z;
    if height <= layer_height {
        return Err(format!(
            "part height {:.4}mm is not taller than one layer ({:.4}mm)",
            height, layer_height
        ));
    }
    let layer_count = (height / layer_height).floor() as usize;
    // Hard cap so a tiny layer height on a tall part cannot exhaust memory.
    if layer_count > MAX_SLICE_LAYERS {
        return Err(format!(
            "slicing would produce {} layers (limit {}); raise layerHeightMm",
            layer_count, MAX_SLICE_LAYERS
        ));
    }
    // Bound the multiplicative scan cost (layers * triangles) that the request
    // body size alone does not constrain.
    let ops = (layer_count as u64).saturating_mul(mesh.triangles.len() as u64);
    if ops > MAX_SLICE_OPS {
        return Err(format!(
            "slicing would require {} plane tests (limit {}); raise layerHeightMm or simplify the mesh",
            ops, MAX_SLICE_OPS
        ));
    }
    let mut result = SliceResult {
        layer_height,
        z_min: min.z,
        z_max: max.z,
        layer_count,
        ..Default::default()
    };
    let mut total_segments = 0usize;
    for i in 0..layer_count {
        let z = min.z + (i as f64 + 0.5) * layer_height;
        let segs = segments_at(mesh, z);
        if segs.is_empty() {
            continue;
        }
        total_segments = total_segments.saturating_add(segs.len());
        if total_segments > MAX_TOTAL_SEGMENTS {
            return Err(format!(
                "slice produced more than {} cross-section segments; simplify the mesh or raise layerHeightMm",
                MAX_TOTAL_SEGMENTS
            ));
        }
        for contour in stitch(&segs, z) {
            result.total_path_length += contour.length;
            if contour.closed {
                result.closed_contours += 1;
            } else {
                result.open_contours += 1;
            }
            result.contours.push(contour);
        }
    }
    Ok(result)
}

/// Emit a capped G-code sample (perimeter G0/G1 motion). `max_layers` limits how
/// many distinct Z layers are written so responses stay bounded; the returned
/// flag reports whether output was truncated.
pub fn to_gcode(slice: &SliceResult, feedrate_mm_per_min: f64, max_layers: usize) -> (String, bool) {
    let mut out = String::new();
    out.push_str("; dd-fabrication-server planar slice (sample)\n");
    out.push_str(&format!(
        "; layerHeight={:.4} layers={} feedrate={:.1}\n",
        slice.layer_height, slice.layer_count, feedrate_mm_per_min
    ));
    out.push_str("G21 ; mm\nG90 ; absolute\n");
    let mut emitted_layers = 0usize;
    let mut last_z = f64::NAN;
    let mut truncated = false;
    for contour in &slice.contours {
        if contour.z != last_z {
            if emitted_layers >= max_layers {
                truncated = true;
                break;
            }
            last_z = contour.z;
            emitted_layers += 1;
            out.push_str(&format!("G0 Z{:.3}\n", contour.z));
        }
        if let Some(&(x0, y0)) = contour.points.first() {
            out.push_str(&format!("G0 X{:.3} Y{:.3}\n", x0, y0));
            for &(x, y) in &contour.points[1..] {
                out.push_str(&format!("G1 X{:.3} Y{:.3} F{:.0}\n", x, y, feedrate_mm_per_min));
            }
            if contour.closed {
                out.push_str(&format!("G1 X{:.3} Y{:.3} F{:.0}\n", x0, y0, feedrate_mm_per_min));
            }
        }
    }
    (out, truncated)
}
