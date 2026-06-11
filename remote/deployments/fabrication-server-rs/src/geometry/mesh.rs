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
        (0..self.triangles.len()).map(|t| self.triangle_area(t)).sum()
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
        let n =
            u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
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
        .checked_add(50usize.checked_mul(n).ok_or("binary stl triangle count overflow")?)
        .ok_or("binary stl size overflow")?;
    if bytes.len() < needed {
        return Err(format!(
            "binary stl truncated: declares {} triangles ({} bytes) but only {} present",
            n, needed, bytes.len()
        ));
    }
    if n > MAX_TRIANGLES {
        return Err(format!(
            "binary stl has {} triangles, exceeding the {} limit",
            n, MAX_TRIANGLES
        ));
    }
    let read_f32 = |off: usize| -> f64 {
        f32::from_le_bytes([
            bytes[off],
            bytes[off + 1],
            bytes[off + 2],
            bytes[off + 3],
        ]) as f64
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
            if !v.is_finite() {
                return Err(format!("binary stl triangle {} has non-finite vertex", i));
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
                *slot = raw
                    .parse::<f64>()
                    .map_err(|_| format!("ascii stl vertex has non-numeric coordinate '{}'", raw))?;
            }
            coords.extend_from_slice(&v);
        }
    }
    if coords.is_empty() || coords.len() % 9 != 0 {
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
            if !v.is_finite() {
                return Err(format!("ascii stl triangle {} has non-finite vertex", i));
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
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
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
    let mut map: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut welded = Mesh::default();
    let mut remap: Vec<usize> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let key = (quantize(v.x, tol), quantize(v.y, tol), quantize(v.z, tol));
        let idx = *map.entry(key).or_insert_with(|| {
            welded.vertices.push(*v);
            welded.vertices.len() - 1
        });
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
