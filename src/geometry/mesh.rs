//! Pure-Rust triangle-mesh core: vector math, STL (binary + ASCII) parsing and
//! writing, base64 decode/encode, and watertight-volume / surface metrics.
//!
//! Intentionally dependency-free (std only) so the geometry engine stays
//! consistent with this crate's minimal-dependency posture and can be verified
//! in isolation. Higher layers (`repair`, `toolpath`, `cost`) build on these
//! primitives; `api` adds the serde/JSON glue.

use std::collections::HashMap;

/// Hard ceiling on triangles accepted from a single STL payload. The global
/// 512 KiB request-body limit already bounds binary STL to ~10k triangles; this
/// is defense-in-depth so a raised body limit (or a hostile declared count)
/// cannot drive an unbounded allocation or O(n) blow-up downstream.
pub const MAX_TRIANGLES: usize = 2_000_000;

/// Clamp range (mm) for the vertex-weld grid. Below the floor, welding is a
/// no-op and float noise survives; above the ceiling, distinct features would
/// collapse and corrupt topology.
pub const MIN_WELD_TOL: f64 = 1e-6;
pub const MAX_WELD_TOL: f64 = 5.0;

/// Maximum absolute coordinate (mm) accepted on any axis — a 10 km envelope,
/// far beyond any real mm-scale part. Bounding magnitude keeps the weld
/// quantizer from saturating `i64` (which would alias distant vertices), keeps
/// volume/area/bbox math from overflowing to non-finite, and rejects random
/// binary blobs whose bytes decode to absurd floats.
pub const MAX_ABS_COORD: f64 = 1.0e7;

/// True when a vertex is finite and inside the accepted coordinate envelope.
fn coord_ok(v: Vec3) -> bool {
    v.is_finite()
        && v.x.abs() <= MAX_ABS_COORD
        && v.y.abs() <= MAX_ABS_COORD
        && v.z.abs() <= MAX_ABS_COORD
}

/// A 3D point / vector in millimetres (the service's canonical unit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }

    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn scale(self, s: f64) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }

    pub fn dot(self, o: Vec3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l <= f64::EPSILON {
            Vec3::new(0.0, 0.0, 0.0)
        } else {
            self.scale(1.0 / l)
        }
    }

    /// Finite-ness guard so malformed STL floats (NaN/Inf) cannot poison metrics.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// An indexed triangle mesh. Triangles reference `vertices` by index.
///
/// Freshly parsed STL is a "soup" (one unshared vertex per triangle corner);
/// [`crate::geometry::repair`] welds it into a shared-vertex topology.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[usize; 3]>,
}

impl Mesh {
    pub fn triangle_points(&self, tri: usize) -> (Vec3, Vec3, Vec3) {
        let [a, b, c] = self.triangles[tri];
        (self.vertices[a], self.vertices[b], self.vertices[c])
    }

    /// Geometric (area-weighted) face normal; unit length, zero for degenerates.
    pub fn triangle_normal(&self, tri: usize) -> Vec3 {
        let (a, b, c) = self.triangle_points(tri);
        b.sub(a).cross(c.sub(a)).normalized()
    }

    pub fn triangle_area(&self, tri: usize) -> f64 {
        let (a, b, c) = self.triangle_points(tri);
        b.sub(a).cross(c.sub(a)).length() * 0.5
    }

    /// Total surface area (mm^2).
    pub fn surface_area(&self) -> f64 {
        (0..self.triangles.len())
            .map(|t| self.triangle_area(t))
            .sum()
    }

    /// Signed volume (mm^3) via the divergence/tetrahedron sum. The sign encodes
    /// global winding: positive when triangles wind counter-clockwise as seen
    /// from outside (outward normals). Magnitude is meaningful only when the
    /// mesh is watertight, but it is a robust orientation oracle regardless.
    pub fn signed_volume(&self) -> f64 {
        let mut acc = 0.0;
        for t in 0..self.triangles.len() {
            let (a, b, c) = self.triangle_points(t);
            acc += a.dot(b.cross(c));
        }
        acc / 6.0
    }

    /// Axis-aligned bounding box `(min, max)`; zero box for empty meshes.
    pub fn bounding_box(&self) -> (Vec3, Vec3) {
        if self.vertices.is_empty() {
            return (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0));
        }
        let mut min = self.vertices[0];
        let mut max = self.vertices[0];
        for v in &self.vertices[1..] {
            min.x = min.x.min(v.x);
            min.y = min.y.min(v.y);
            min.z = min.z.min(v.z);
            max.x = max.x.max(v.x);
            max.y = max.y.max(v.y);
            max.z = max.z.max(v.z);
        }
        (min, max)
    }

    /// Serialize to a little-endian binary STL (recomputed face normals).
    pub fn to_binary_stl(&self) -> Vec<u8> {
        let n = self.triangles.len();
        let mut out = Vec::with_capacity(84 + 50 * n);
        out.extend_from_slice(&[0u8; 80]); // header
        out.extend_from_slice(&(n as u32).to_le_bytes());
        for t in 0..n {
            let normal = self.triangle_normal(t);
            let (a, b, c) = self.triangle_points(t);
            for comp in [normal, a, b, c] {
                out.extend_from_slice(&(comp.x as f32).to_le_bytes());
                out.extend_from_slice(&(comp.y as f32).to_le_bytes());
                out.extend_from_slice(&(comp.z as f32).to_le_bytes());
            }
            out.extend_from_slice(&[0u8, 0u8]); // attribute byte count
        }
        out
    }
}

/// Parse an STL blob, auto-detecting binary vs. ASCII.
///
/// Detection prefers the exact binary size law (`84 + 50*n`) because some
/// binary exporters write a header that begins with the ASCII token `solid`.
pub fn parse_stl(bytes: &[u8]) -> Result<Mesh, String> {
    if bytes.len() < 15 {
        return Err("stl payload too short to be valid".into());
    }
    if bytes.len() >= 84 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        if 84usize.checked_add(50usize.saturating_mul(n)) == Some(bytes.len()) {
            return parse_binary_stl(bytes);
        }
    }
    let head: String = bytes
        .iter()
        .take(256)
        .map(|&b| b as char)
        .collect::<String>()
        .trim_start()
        .to_ascii_lowercase();
    if head.starts_with("solid") {
        return parse_ascii_stl(bytes);
    }
    // Last resort: trust the binary triangle count if it fits the buffer.
    parse_binary_stl(bytes)
}

fn parse_binary_stl(bytes: &[u8]) -> Result<Mesh, String> {
    if bytes.len() < 84 {
        return Err("binary stl missing 84-byte header".into());
    }
    let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let needed = 84usize
        .checked_add(
            50usize
                .checked_mul(n)
                .ok_or("binary stl triangle count overflow")?,
        )
        .ok_or("binary stl size overflow")?;
    if bytes.len() < needed {
        return Err(format!(
            "binary stl truncated: declares {} triangles ({} bytes) but only {} present",
            n,
            needed,
            bytes.len()
        ));
    }
    if n > MAX_TRIANGLES {
        return Err(format!(
            "binary stl has {} triangles, exceeding the {} limit",
            n, MAX_TRIANGLES
        ));
    }
    let read_f32 = |off: usize| -> f64 {
        f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]) as f64
    };
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(n * 3);
    mesh.triangles.reserve(n);
    for i in 0..n {
        let base = 84 + 50 * i + 12; // skip stored normal
        let mut tri = [0usize; 3];
        for (k, slot) in tri.iter_mut().enumerate() {
            let off = base + k * 12;
            let v = Vec3::new(read_f32(off), read_f32(off + 4), read_f32(off + 8));
            if !coord_ok(v) {
                return Err(format!(
                    "binary stl triangle {} has a non-finite or out-of-range vertex (limit {:e} mm)",
                    i, MAX_ABS_COORD
                ));
            }
            *slot = mesh.vertices.len();
            mesh.vertices.push(v);
        }
        mesh.triangles.push(tri);
    }
    if mesh.triangles.is_empty() {
        return Err("binary stl contains no triangles".into());
    }
    Ok(mesh)
}

fn parse_ascii_stl(bytes: &[u8]) -> Result<Mesh, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "ascii stl is not valid utf-8".to_string())?;
    let mut coords: Vec<f64> = Vec::new();
    let mut tokens = text.split_whitespace();
    while let Some(tok) = tokens.next() {
        if tok.eq_ignore_ascii_case("vertex") {
            let mut v = [0.0f64; 3];
            for slot in v.iter_mut() {
                let raw = tokens
                    .next()
                    .ok_or("ascii stl vertex truncated mid-coordinate")?;
                *slot = raw.parse::<f64>().map_err(|_| {
                    format!("ascii stl vertex has non-numeric coordinate '{}'", raw)
                })?;
            }
            coords.extend_from_slice(&v);
        }
    }
    if coords.is_empty() || !coords.len().is_multiple_of(9) {
        return Err(format!(
            "ascii stl produced {} vertex coordinates (not a whole number of triangles)",
            coords.len()
        ));
    }
    let tri_count = coords.len() / 9;
    if tri_count > MAX_TRIANGLES {
        return Err(format!(
            "ascii stl has {} triangles, exceeding the {} limit",
            tri_count, MAX_TRIANGLES
        ));
    }
    let mut mesh = Mesh::default();
    for i in 0..tri_count {
        let mut tri = [0usize; 3];
        for (k, slot) in tri.iter_mut().enumerate() {
            let o = i * 9 + k * 3;
            let v = Vec3::new(coords[o], coords[o + 1], coords[o + 2]);
            if !coord_ok(v) {
                return Err(format!(
                    "ascii stl triangle {} has a non-finite or out-of-range vertex (limit {:e} mm)",
                    i, MAX_ABS_COORD
                ));
            }
            *slot = mesh.vertices.len();
            mesh.vertices.push(v);
        }
        mesh.triangles.push(tri);
    }
    Ok(mesh)
}

/// Decode standard (RFC 4648) base64, tolerating embedded whitespace/newlines
/// and optional `=` padding. Used to accept binary STL inside JSON requests.
pub fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in input.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = val(c).ok_or_else(|| format!("invalid base64 character: 0x{:02x}", c))?;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    if out.is_empty() {
        return Err("base64 payload decoded to zero bytes".into());
    }
    Ok(out)
}

/// Encode bytes as standard base64 (with padding). Used to return repaired STL.
pub fn encode_base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18 & 0x3f) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Stable 64-bit FNV-1a hash, used to derive deterministic request ids from
/// geometry content when the caller omits one.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Quantize a coordinate to an integer grid cell for vertex welding.
pub(crate) fn quantize(value: f64, tol: f64) -> i64 {
    (value / tol).round() as i64
}

/// Build a welded copy of `mesh`: vertices within `tol` (a positive grid size)
/// collapse to a single index. Returns the new mesh and the number of vertex
/// references that were merged away.
pub(crate) fn weld(mesh: &Mesh, tol: f64) -> (Mesh, usize) {
    // Reject NaN/Inf and clamp into a sane grid range so a hostile or fat-
    // fingered tolerance cannot collapse the whole mesh or be a no-op.
    let tol = if tol.is_finite() && tol > 0.0 {
        tol.clamp(MIN_WELD_TOL, MAX_WELD_TOL)
    } else {
        1e-3
    };
    // Bucket welded vertices by grid cell (cell size = tol). For each incoming
    // vertex, search the 3x3x3 neighborhood so a pair within `tol` but split
    // across a cell boundary still merges — exact-cell hashing alone would leave
    // such seams as spurious boundary edges and report a closed mesh as open.
    let mut cells: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    let mut welded = Mesh::default();
    let mut remap: Vec<usize> = Vec::with_capacity(mesh.vertices.len());
    let tol_sq = tol * tol;
    for v in &mesh.vertices {
        let key = (quantize(v.x, tol), quantize(v.y, tol), quantize(v.z, tol));
        let mut found: Option<usize> = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = cells.get(&(key.0 + dx, key.1 + dy, key.2 + dz)) {
                        for &wi in list {
                            let w = welded.vertices[wi];
                            let d = w.sub(*v);
                            if d.dot(d) <= tol_sq {
                                found = Some(wi);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        let idx = match found {
            Some(wi) => wi,
            None => {
                welded.vertices.push(*v);
                let ni = welded.vertices.len() - 1;
                cells.entry(key).or_default().push(ni);
                ni
            }
        };
        remap.push(idx);
    }
    for tri in &mesh.triangles {
        welded
            .triangles
            .push([remap[tri[0]], remap[tri[1]], remap[tri[2]]]);
    }
    let merged = mesh.vertices.len().saturating_sub(welded.vertices.len());
    (welded, merged)
}

#[cfg(test)]
mod mesh_unit_tests {
    use super::*;

    /// Unit tetrahedron at the origin with outward-wound faces.
    /// Volume = 1/6, surface area = 3 * 0.5 + sqrt(3)/2.
    fn tetra() -> Mesh {
        Mesh {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0), // 0: o
                Vec3::new(1.0, 0.0, 0.0), // 1: a
                Vec3::new(0.0, 1.0, 0.0), // 2: b
                Vec3::new(0.0, 0.0, 1.0), // 3: c
            ],
            triangles: vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
        }
    }

    #[test]
    fn tetra_volume_area_bbox() {
        let m = tetra();
        let expected_area = 1.5 + 3.0f64.sqrt() / 2.0;
        assert!((m.signed_volume() - 1.0 / 6.0).abs() < 1e-12);
        assert!((m.surface_area() - expected_area).abs() < 1e-12);
        let (min, max) = m.bounding_box();
        assert_eq!(min, Vec3::new(0.0, 0.0, 0.0));
        assert_eq!(max, Vec3::new(1.0, 1.0, 1.0));

        // Reversing every winding must exactly negate the signed volume.
        let mut flipped = m.clone();
        for tri in &mut flipped.triangles {
            tri.swap(1, 2);
        }
        assert!((flipped.signed_volume() + 1.0 / 6.0).abs() < 1e-12);
    }

    #[test]
    fn binary_stl_round_trip_preserves_geometry() {
        let m = tetra();
        let stl = m.to_binary_stl();
        assert_eq!(stl.len(), 84 + 50 * m.triangles.len());
        let parsed = parse_stl(&stl).expect("round-tripped binary STL must parse");
        // Parsed STL is a vertex soup: one vertex per triangle corner.
        assert_eq!(parsed.triangles.len(), 4);
        assert_eq!(parsed.vertices.len(), 12);
        // f32 storage loses precision, but these coordinates are exact in f32.
        assert!((parsed.signed_volume() - 1.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn ascii_stl_parses_and_rejects_truncation() {
        let text = "solid t\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid t\n";
        let mesh = parse_stl(text.as_bytes()).expect("valid ascii stl");
        assert_eq!(mesh.triangles.len(), 1);
        assert!((mesh.triangle_area(0) - 0.5).abs() < 1e-12);

        // Dropping the last coordinate leaves a non-whole number of triangles.
        let truncated = "solid t\n facet\n outer loop\n vertex 0 0 0\n vertex 1 0 0\n vertex 0 1\n";
        assert!(parse_stl(truncated.as_bytes()).is_err());

        // Non-finite coordinates are rejected, not propagated into metrics.
        let nan = "solid t\n vertex 0 0 0 vertex 1 0 0 vertex 0 1 NaN endsolid";
        assert!(parse_stl(nan.as_bytes()).is_err());
    }

    #[test]
    fn base64_round_trip_whitespace_and_errors() {
        assert_eq!(encode_base64(b"hello"), "aGVsbG8=");
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        // Embedded whitespace/newlines are tolerated.
        assert_eq!(decode_base64(" aGVs\nbG8= ").unwrap(), b"hello");
        // All byte values survive an encode/decode round trip.
        let bytes: Vec<u8> = (0u8..=255).collect();
        assert_eq!(decode_base64(&encode_base64(&bytes)).unwrap(), bytes);
        // Invalid characters and empty payloads fail closed.
        assert!(decode_base64("aGV!bG8=").is_err());
        assert!(decode_base64("").is_err());
    }

    #[test]
    fn fnv1a_matches_reference_vectors() {
        // Published FNV-1a 64-bit test vectors.
        assert_eq!(fnv1a_64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn weld_merges_within_tolerance_only() {
        // Two triangles sharing an edge, but the shared corners are offset by
        // float noise far below the weld tolerance.
        let eps = 1e-5;
        let m = Mesh {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(1.0 + eps, 0.0, 0.0),
                Vec3::new(0.0, 1.0 + eps, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
            ],
            triangles: vec![[0, 1, 2], [3, 5, 4]],
        };
        let (welded, merged) = weld(&m, 1e-3);
        assert_eq!(merged, 2);
        assert_eq!(welded.vertices.len(), 4);
        // Distinct features (0.1 mm apart) must NOT merge at a 1e-3 tolerance.
        let far = Mesh {
            vertices: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.1, 0.0, 0.0)],
            triangles: vec![],
        };
        let (w2, merged2) = weld(&far, 1e-3);
        assert_eq!(merged2, 0);
        assert_eq!(w2.vertices.len(), 2);
    }
}
