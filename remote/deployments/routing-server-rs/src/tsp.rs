//! Pure VRP/TSP core: geometry, multi-start construction, and 2-opt local search.
//!
//! A *solution* is a set of routes; each route is a cyclic sequence of stop indices.
//! For a pure TSP (no depot) there is a single route over every stop. For a VRP the
//! depot is included once in each vehicle's route, so a route's cyclic distance is
//! exactly `depot -> c1 -> ... -> ck -> depot`. No I/O, no NATS — deterministic from
//! a `u64` seed so the master's local fallback and the worker pods agree.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

pub const MAX_STOPS: usize = 1_000;
pub const MAX_VEHICLES: usize = 64;
pub const MAX_LOCAL_PASSES: usize = 60;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stop {
    pub id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub stops: Vec<Stop>,
    /// Depot index for VRP. `None` => single-tour TSP.
    #[serde(default)]
    pub depot_index: Option<usize>,
    #[serde(default = "one")]
    pub vehicles: usize,
}

fn one() -> usize {
    1
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    pub routes: Vec<Vec<usize>>,
    pub distance: f64,
}

impl Problem {
    pub fn validate(&self) -> Result<(), String> {
        if self.stops.len() < 2 {
            return Err("problem requires at least 2 stops".into());
        }
        if self.stops.len() > MAX_STOPS {
            return Err(format!("problem exceeds max {MAX_STOPS} stops"));
        }
        for stop in &self.stops {
            if !stop.x.is_finite() || !stop.y.is_finite() {
                return Err("stop coordinates must be finite".into());
            }
        }
        if let Some(depot) = self.depot_index {
            if depot >= self.stops.len() {
                return Err("depotIndex out of range".into());
            }
        }
        Ok(())
    }

    #[inline]
    pub fn dist(&self, a: usize, b: usize) -> f64 {
        let p = &self.stops[a];
        let q = &self.stops[b];
        ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt()
    }

    pub fn route_distance(&self, route: &[usize]) -> f64 {
        if route.len() < 2 {
            return 0.0;
        }
        let mut total = 0.0;
        for i in 0..route.len() {
            total += self.dist(route[i], route[(i + 1) % route.len()]);
        }
        total
    }

    pub fn solution_distance(&self, routes: &[Vec<usize>]) -> f64 {
        routes.iter().map(|r| self.route_distance(r)).sum()
    }
}

/// SplitMix64 — same deterministic generator used by the GA core.
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
}

/// One multi-start restart: randomized construction + 2-opt refinement.
pub fn solve_restart(problem: &Problem, seed: u64, local_passes: usize) -> Solution {
    let mut rng = Rng::new(seed);
    let passes = local_passes.clamp(1, MAX_LOCAL_PASSES);

    let routes = match problem.depot_index {
        Some(depot) => {
            let customers: Vec<usize> = (0..problem.stops.len()).filter(|&i| i != depot).collect();
            let vehicles = problem.vehicles.clamp(1, MAX_VEHICLES).min(customers.len().max(1));
            sweep_assign(problem, &customers, depot, vehicles, &mut rng)
                .into_iter()
                .map(|group| {
                    let mut route = vec![depot];
                    if !group.is_empty() {
                        route.extend(nearest_neighbor(problem, &group, rng.below(group.len())));
                    }
                    two_opt(problem, &mut route, passes);
                    rotate_to_front(&mut route, depot);
                    route
                })
                .collect()
        }
        None => {
            let all: Vec<usize> = (0..problem.stops.len()).collect();
            let mut route = nearest_neighbor(problem, &all, rng.below(all.len()));
            two_opt(problem, &mut route, passes);
            vec![route]
        }
    };

    let distance = problem.solution_distance(&routes);
    Solution { routes, distance }
}

/// Greedy nearest-neighbour ordering of a node set from a chosen start position.
fn nearest_neighbor(problem: &Problem, nodes: &[usize], start_pos: usize) -> Vec<usize> {
    let mut remaining = nodes.to_vec();
    if remaining.is_empty() {
        return remaining;
    }
    let mut route = Vec::with_capacity(remaining.len());
    let mut current = remaining.swap_remove(start_pos % remaining.len());
    route.push(current);
    while !remaining.is_empty() {
        let mut best = 0usize;
        let mut best_dist = f64::INFINITY;
        for (idx, &node) in remaining.iter().enumerate() {
            let d = problem.dist(current, node);
            if d < best_dist {
                best_dist = d;
                best = idx;
            }
        }
        current = remaining.swap_remove(best);
        route.push(current);
    }
    route
}

/// Sweep heuristic: order customers by polar angle around the depot (with a random
/// rotation for restart diversity), then split into `vehicles` contiguous arcs.
fn sweep_assign(
    problem: &Problem,
    customers: &[usize],
    depot: usize,
    vehicles: usize,
    rng: &mut Rng,
) -> Vec<Vec<usize>> {
    let (dx, dy) = (problem.stops[depot].x, problem.stops[depot].y);
    let offset = rng.unit() * std::f64::consts::TAU;
    let angle = |node: usize| -> f64 {
        let a = (problem.stops[node].y - dy).atan2(problem.stops[node].x - dx) + offset;
        a.rem_euclid(std::f64::consts::TAU)
    };
    let mut sorted = customers.to_vec();
    sorted.sort_by(|&a, &b| angle(a).partial_cmp(&angle(b)).unwrap_or(Ordering::Equal));

    let n = sorted.len();
    let v = vehicles.max(1);
    let chunk = n.div_ceil(v).max(1);
    let mut groups = vec![Vec::new(); v];
    for (i, node) in sorted.into_iter().enumerate() {
        groups[(i / chunk).min(v - 1)].push(node);
    }
    groups
}

/// Classic full-neighbourhood 2-opt on a cyclic route, capped at `max_passes` sweeps.
fn two_opt(problem: &Problem, route: &mut Vec<usize>, max_passes: usize) {
    let n = route.len();
    if n < 4 {
        return;
    }
    let mut improved = true;
    let mut passes = 0;
    while improved && passes < max_passes {
        improved = false;
        passes += 1;
        for i in 0..n - 1 {
            for j in i + 1..n {
                let a = route[i];
                let b = route[(i + 1) % n];
                let c = route[j];
                let d = route[(j + 1) % n];
                if a == c || a == d || b == c {
                    continue;
                }
                let before = problem.dist(a, b) + problem.dist(c, d);
                let after = problem.dist(a, c) + problem.dist(b, d);
                if after + 1e-9 < before {
                    route[i + 1..=j].reverse();
                    improved = true;
                }
            }
        }
    }
}

fn rotate_to_front(route: &mut [usize], node: usize) {
    if let Some(pos) = route.iter().position(|&n| n == node) {
        route.rotate_left(pos);
    }
}

/// Deterministically generate a random problem instance inside a `width x height`
/// box with the depot at index 0. Used by the dashboard's "generate" action.
pub fn generate_problem(count: usize, vehicles: usize, width: f64, height: f64, seed: u64) -> Problem {
    let mut rng = Rng::new(seed);
    let stops = (0..count.max(2))
        .map(|i| Stop {
            id: if i == 0 { "depot".to_string() } else { format!("s{i}") },
            x: rng.unit() * width,
            y: rng.unit() * height,
        })
        .collect();
    Problem {
        stops,
        depot_index: if vehicles > 1 { Some(0) } else { None },
        vehicles: vehicles.clamp(1, MAX_VEHICLES),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(n: usize) -> Problem {
        // Points on a circle: the optimal TSP tour is the circle perimeter.
        let stops = (0..n)
            .map(|i| {
                let t = (i as f64 / n as f64) * std::f64::consts::TAU;
                Stop {
                    id: format!("s{i}"),
                    x: 100.0 * t.cos(),
                    y: 100.0 * t.sin(),
                }
            })
            .collect();
        Problem {
            stops,
            depot_index: None,
            vehicles: 1,
        }
    }

    #[test]
    fn deterministic_for_a_fixed_seed() {
        let problem = generate_problem(40, 4, 1000.0, 600.0, 7);
        let a = solve_restart(&problem, 99, 20);
        let b = solve_restart(&problem, 99, 20);
        assert_eq!(a.routes, b.routes);
        assert!((a.distance - b.distance).abs() < 1e-9);
    }

    #[test]
    fn two_opt_finds_circle_perimeter() {
        let problem = ring(24);
        let solution = solve_restart(&problem, 1, 40);
        // Perimeter of a 24-gon on r=100 ~ circumference 2*pi*100 ~ 628.
        assert!(solution.distance < 660.0, "got {}", solution.distance);
        assert_eq!(solution.routes.len(), 1);
        assert_eq!(solution.routes[0].len(), 24);
    }

    #[test]
    fn vrp_partitions_across_vehicles_with_depot() {
        let problem = generate_problem(30, 3, 1000.0, 600.0, 5);
        let solution = solve_restart(&problem, 2, 25);
        assert_eq!(solution.routes.len(), 3);
        for route in &solution.routes {
            assert_eq!(route[0], 0, "each VRP route starts at the depot");
        }
        let visited: usize = solution.routes.iter().map(|r| r.len() - 1).sum();
        assert_eq!(visited, 29, "all customers covered exactly once");
    }
}
