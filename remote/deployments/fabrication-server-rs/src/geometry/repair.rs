//! Deterministic mesh repair: vertex welding, degenerate/duplicate culling,
//! winding-consistency unification, hole filling, and a manifold/watertight
//! report. Pure geometry — no I/O, no randomness, no external deps.

use std::collections::HashMap;

use super::mesh::{weld, Mesh};

/// Outcome of a repair pass: every transformation is counted so callers can
/// emit release evidence and learning signals.
#[derive(Clone, Debug, Default)]
pub struct RepairReport {
    pub input_vertices: usize,
    pub input_triangles: usize,
    pub welded_vertices: usize,
    pub removed_degenerate: usize,
    pub removed_duplicate: usize,
    pub flipped_for_consistency: usize,
    pub flipped_global_outward: bool,
    pub boundary_edges_before: usize,
    pub non_manifold_edges: usize,
    pub holes_detected: usize,
    pub holes_filled: usize,
    pub triangles_added_filling: usize,
    pub boundary_edges_after: usize,
    pub watertight: bool,
    pub output_vertices: usize,
    pub output_triangles: usize,
}

/// Undirected edge key with endpoints ordered so `(a,b)` and `(b,a)` collide.
fn ekey(a: usize, b: usize) -> (usize, usize) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

/// Map each undirected edge to the count of triangles that use it.
fn edge_incidence(mesh: &Mesh) -> HashMap<(usize, usize), usize> {
    let mut inc: HashMap<(usize, usize), usize> = HashMap::new();
    for tri in &mesh.triangles {
        for k in 0..3 {
            *inc.entry(ekey(tri[k], tri[(k + 1) % 3])).or_insert(0) += 1;
        }
    }
    inc
}

fn count_boundary_and_nonmanifold(mesh: &Mesh) -> (usize, usize) {
    let mut boundary = 0;
    let mut nonmanifold = 0;
    for &count in edge_incidence(mesh).values() {
        if count == 1 {
            boundary += 1;
        } else if count > 2 {
            nonmanifold += 1;
        }
    }
    (boundary, nonmanifold)
}

/// Drop triangles with a repeated vertex index or near-zero area.
fn remove_degenerate(mesh: &mut Mesh) -> usize {
    let before = mesh.triangles.len();
    let mut kept = Vec::with_capacity(before);
    for t in 0..before {
        let [a, b, c] = mesh.triangles[t];
        if a == b || b == c || a == c {
            continue;
        }
        if mesh.triangle_area(t) <= 1e-12 {
            continue;
        }
        kept.push([a, b, c]);
    }
    mesh.triangles = kept;
    before - mesh.triangles.len()
}

/// Drop triangles that reference the same three vertices (any winding).
fn remove_duplicate(mesh: &mut Mesh) -> usize {
    let before = mesh.triangles.len();
    let mut seen: std::collections::HashSet<(usize, usize, usize)> = std::collections::HashSet::new();
    let mut kept = Vec::with_capacity(before);
    for &tri in &mesh.triangles {
        let mut s = tri;
        s.sort_unstable();
        let key = (s[0], s[1], s[2]);
        if seen.insert(key) {
            kept.push(tri);
        }
    }
    mesh.triangles = kept;
    before - mesh.triangles.len()
}

/// Walk the triangle adjacency graph and flip neighbours so that every shared
/// edge is traversed in opposite directions by its two faces (consistent
/// winding). Returns the number of triangles flipped.
///
/// Standard orientability unification: faces sharing an edge in the *same*
/// direction are mutually reversed; one BFS root anchors each connected
/// component. Non-manifold edges (>2 faces) are skipped — they cannot be made
/// consistent and are reported separately.
fn unify_winding(mesh: &mut Mesh) -> usize {
    let n = mesh.triangles.len();
    if n == 0 {
        return 0;
    }
    // edge -> incident triangles
    let mut edge_tris: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (t, tri) in mesh.triangles.iter().enumerate() {
        for k in 0..3 {
            edge_tris
                .entry(ekey(tri[k], tri[(k + 1) % 3]))
                .or_default()
                .push(t);
        }
    }
    // Does triangle `t` contain the *directed* edge (a -> b)?
    let has_directed = |tri: &[usize; 3], a: usize, b: usize| -> bool {
        (0..3).any(|k| tri[k] == a && tri[(k + 1) % 3] == b)
    };
    let flip = |tri: &mut [usize; 3]| tri.swap(1, 2);

    let mut visited = vec![false; n];
    let mut flips = 0;
    for start in 0..n {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(start);
        while let Some(t) = queue.pop_front() {
            let tri = mesh.triangles[t];
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                let neighbours = match edge_tris.get(&ekey(a, b)) {
                    Some(v) if v.len() == 2 => v.clone(),
                    _ => continue, // boundary or non-manifold edge
                };
                for &nb in &neighbours {
                    if nb == t || visited[nb] {
                        continue;
                    }
                    // `t` traverses a->b. A consistent neighbour must traverse
                    // b->a; if it also has a->b, reverse it.
                    if has_directed(&mesh.triangles[nb], a, b) {
                        flip(&mut mesh.triangles[nb]);
                        flips += 1;
                    }
                    visited[nb] = true;
                    queue.push_back(nb);
                }
            }
        }
    }
    flips
}

/// Trace boundary edges (incidence 1) into oriented loops and fan-triangulate
/// each loop closed. Returns `(loops_detected, loops_filled, triangles_added)`.
///
/// Boundary directed edges are taken in the orientation they appear in their
/// owning face, so the loop runs consistently; the fill fans wind opposite to
/// the boundary so the patched faces face the same way as their neighbours.
fn fill_holes(mesh: &mut Mesh) -> (usize, usize, usize) {
    // Collect boundary directed edges (a -> b) for edges used by one triangle.
    let inc = edge_incidence(mesh);
    let mut next: HashMap<usize, usize> = HashMap::new();
    let mut starts: Vec<usize> = Vec::new();
    for tri in &mesh.triangles {
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            if inc.get(&ekey(a, b)).copied() == Some(1) {
                next.insert(a, b);
                starts.push(a);
            }
        }
    }
    if next.is_empty() {
        return (0, 0, 0);
    }

    let mut detected = 0;
    let mut filled = 0;
    let mut added = 0;
    let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for &s in &starts {
        if consumed.contains(&s) || !next.contains_key(&s) {
            continue;
        }
        // Walk the loop starting at s.
        let mut loop_verts = Vec::new();
        let mut cur = s;
        let mut ok = true;
        loop {
            if !consumed.insert(cur) {
                // Re-entered a vertex already consumed mid-walk: not a clean
                // simple loop; bail on this one.
                ok = loop_verts.first() == Some(&cur);
                break;
            }
            loop_verts.push(cur);
            match next.get(&cur) {
                Some(&nx) => {
                    if nx == s {
                        break; // closed loop
                    }
                    cur = nx;
                }
                None => {
                    ok = false;
                    break;
                }
            }
            if loop_verts.len() > mesh.vertices.len() + 1 {
                ok = false;
                break;
            }
        }
        detected += 1;
        if !ok || loop_verts.len() < 3 {
            continue;
        }
        // Fan triangulate; reverse winding relative to the boundary direction.
        let v0 = loop_verts[0];
        for i in 1..loop_verts.len() - 1 {
            mesh.triangles.push([v0, loop_verts[i + 1], loop_verts[i]]);
            added += 1;
        }
        filled += 1;
    }
    (detected, filled, added)
}

/// Drop vertices no triangle references and compact indices.
fn prune_unused(mesh: &mut Mesh) {
    let mut used = vec![false; mesh.vertices.len()];
    for tri in &mesh.triangles {
        for &i in tri {
            used[i] = true;
        }
    }
    let mut remap = vec![usize::MAX; mesh.vertices.len()];
    let mut compact = Vec::new();
    for (i, &u) in used.iter().enumerate() {
        if u {
            remap[i] = compact.len();
            compact.push(mesh.vertices[i]);
        }
    }
    for tri in &mut mesh.triangles {
        for i in tri.iter_mut() {
            *i = remap[*i];
        }
    }
    mesh.vertices = compact;
}

/// Run the full repair pipeline on `input`, welding within `weld_tol` mm.
/// Returns the repaired mesh and a detailed [`RepairReport`].
pub fn repair(input: &Mesh, weld_tol: f64) -> (Mesh, RepairReport) {
    let mut report = RepairReport {
        input_vertices: input.vertices.len(),
        input_triangles: input.triangles.len(),
        ..Default::default()
    };

    let (mut mesh, merged) = weld(input, weld_tol);
    report.welded_vertices = merged;

    // Pre-repair diagnostic only: the welded-but-uncleaned boundary count.
    let (boundary_before, _) = count_boundary_and_nonmanifold(&mesh);
    report.boundary_edges_before = boundary_before;

    report.removed_degenerate = remove_degenerate(&mut mesh);
    report.removed_duplicate = remove_duplicate(&mut mesh);

    let flips1 = unify_winding(&mut mesh);

    let (detected, filled, added) = fill_holes(&mut mesh);
    report.holes_detected = detected;
    report.holes_filled = filled;
    report.triangles_added_filling = added;

    // Patches may need re-unifying with their neighbours.
    let flips2 = if added > 0 { unify_winding(&mut mesh) } else { 0 };
    report.flipped_for_consistency = flips1 + flips2;

    // Fan fills over collinear boundary runs can introduce zero-area slivers;
    // cull them so the output never carries degenerate faces.
    if added > 0 {
        report.removed_degenerate += remove_degenerate(&mut mesh);
    }

    // Orient outward: if the closed volume came out negative, the whole shell is
    // inside-out — reverse every face.
    if mesh.signed_volume() < 0.0 {
        for tri in &mut mesh.triangles {
            tri.swap(1, 2);
        }
        report.flipped_global_outward = true;
    }

    prune_unused(&mut mesh);

    // Final manifold state — after welding, culling, and filling. This is what
    // the watertight gate and blockers must reflect: removing duplicate faces
    // can eliminate the very non-manifold edges those duplicates created, so a
    // pre-clean count would falsely report a healed mesh as still broken.
    let (boundary_after, nonmanifold_after) = count_boundary_and_nonmanifold(&mesh);
    report.boundary_edges_after = boundary_after;
    report.non_manifold_edges = nonmanifold_after;
    report.watertight = boundary_after == 0 && nonmanifold_after == 0;
    report.output_vertices = mesh.vertices.len();
    report.output_triangles = mesh.triangles.len();

    (mesh, report)
}
