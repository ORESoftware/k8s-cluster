//! Pure genetic-algorithm core: deterministic RNG, benchmark fitness functions,
//! and single-island evolution. No NATS, no I/O — so it is trivially unit-testable
//! and can run identically in the master's local-fallback path or in a worker pod.

use std::cmp::Ordering;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Maximum genome dimension, population, and generation counts. These bound a
/// single island job so one malformed request cannot pin a worker indefinitely.
pub const MAX_DIMENSION: usize = 256;
pub const MAX_POPULATION: usize = 4_000;
pub const MAX_GENERATIONS: usize = 5_000;

/// Built-in continuous minimization benchmarks. All have a global minimum of 0;
/// `sphere` is convex, the others are multimodal (good migration stress tests).
pub fn known_functions() -> &'static [&'static str] {
    &["sphere", "rosenbrock", "rastrigin", "ackley"]
}

pub fn is_known_function(name: &str) -> bool {
    known_functions().contains(&name)
}

/// Fitness to minimize. Unknown names fall back to `sphere`.
pub fn evaluate(function: &str, x: &[f64]) -> f64 {
    match function {
        "rosenbrock" => x
            .windows(2)
            .map(|w| 100.0 * (w[1] - w[0] * w[0]).powi(2) + (1.0 - w[0]).powi(2))
            .sum(),
        "rastrigin" => {
            10.0 * x.len() as f64
                + x.iter()
                    .map(|v| v * v - 10.0 * (std::f64::consts::TAU * v).cos())
                    .sum::<f64>()
        }
        "ackley" => {
            let n = x.len().max(1) as f64;
            let sum_sq = x.iter().map(|v| v * v).sum::<f64>();
            let sum_cos = x.iter().map(|v| (std::f64::consts::TAU * v).cos()).sum::<f64>();
            -20.0 * (-0.2 * (sum_sq / n).sqrt()).exp() - (sum_cos / n).exp()
                + 20.0
                + std::f64::consts::E
        }
        // "sphere" and anything unrecognized.
        _ => x.iter().map(|v| v * v).sum(),
    }
}

/// SplitMix64 — small, fast, fully deterministic from a u64 seed. Avoids pulling
/// in the `rand` crate, matching the dependency-light style of the solver fleet.
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

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    /// Standard-normal sample via Box-Muller (one of the pair).
    pub fn gaussian(&mut self) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemSpec {
    pub function: String,
    pub dimension: usize,
    pub lower_bound: f64,
    pub upper_bound: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GaParams {
    pub population_size: usize,
    pub generations: usize,
    pub mutation_rate: f64,
    /// Mutation step size as a fraction of the search range (sigma = range * scale).
    pub mutation_scale: f64,
    pub crossover_rate: f64,
    pub elite_count: usize,
    pub tournament_size: usize,
}

/// One epoch of evolution work for a single island.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandJob {
    pub solve_id: String,
    pub request_id: String,
    pub epoch: usize,
    pub island_id: usize,
    pub problem: ProblemSpec,
    pub params: GaParams,
    pub seed: u64,
    /// Carried-in population (sorted best-first). Empty → the island seeds randomly.
    #[serde(default)]
    pub population: Vec<Vec<f64>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IslandResult {
    pub solve_id: String,
    pub request_id: String,
    pub epoch: usize,
    pub island_id: usize,
    pub worker_node: String,
    /// Final population, fitness-sorted ascending (best individual first).
    pub population: Vec<Vec<f64>>,
    pub best_genome: Vec<f64>,
    pub best_fitness: f64,
    pub evaluations: u64,
    pub elapsed_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IslandJob {
    /// Evolve this island's subpopulation for `params.generations` generations and
    /// return the fitness-sorted result. Deterministic given `seed`.
    pub fn run(&self, worker_node: &str) -> IslandResult {
        let started = Instant::now();
        let mut rng = Rng::new(self.seed);
        let problem = &self.problem;
        let params = &self.params;

        let mut population = self.population.clone();
        if population.is_empty() {
            population = (0..params.population_size.max(2))
                .map(|_| {
                    (0..problem.dimension)
                        .map(|_| rng.range(problem.lower_bound, problem.upper_bound))
                        .collect()
                })
                .collect();
        }
        let pop_size = population.len().max(2);
        let sigma = (problem.upper_bound - problem.lower_bound).abs() * params.mutation_scale;

        let mut evaluations = 0u64;
        let mut scored = score_and_sort(&problem.function, population, &mut evaluations);

        for _ in 0..params.generations {
            let elite = params.elite_count.min(scored.len());
            let mut next: Vec<Vec<f64>> =
                scored.iter().take(elite).map(|(_, g)| g.clone()).collect();
            while next.len() < pop_size {
                let parent_a = &scored[tournament(&scored, params.tournament_size, &mut rng)].1;
                let mut child = if rng.unit() < params.crossover_rate {
                    let parent_b =
                        &scored[tournament(&scored, params.tournament_size, &mut rng)].1;
                    blend_crossover(parent_a, parent_b, &mut rng)
                } else {
                    parent_a.clone()
                };
                mutate(
                    &mut child,
                    params.mutation_rate,
                    sigma,
                    problem.lower_bound,
                    problem.upper_bound,
                    &mut rng,
                );
                next.push(child);
            }
            scored = score_and_sort(&problem.function, next, &mut evaluations);
        }

        let (best_fitness, best_genome) = scored
            .first()
            .map(|(f, g)| (*f, g.clone()))
            .unwrap_or((f64::INFINITY, Vec::new()));
        let population = scored.into_iter().map(|(_, g)| g).collect();

        IslandResult {
            solve_id: self.solve_id.clone(),
            request_id: self.request_id.clone(),
            epoch: self.epoch,
            island_id: self.island_id,
            worker_node: worker_node.to_string(),
            population,
            best_genome,
            best_fitness,
            evaluations,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            error: None,
        }
    }
}

fn score_and_sort(
    function: &str,
    population: Vec<Vec<f64>>,
    evaluations: &mut u64,
) -> Vec<(f64, Vec<f64>)> {
    let mut scored: Vec<(f64, Vec<f64>)> = population
        .into_iter()
        .map(|g| (evaluate(function, &g), g))
        .collect();
    *evaluations += scored.len() as u64;
    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
    scored
}

/// k-way tournament selection; returns the index of the fittest contestant.
fn tournament(scored: &[(f64, Vec<f64>)], k: usize, rng: &mut Rng) -> usize {
    let mut best = rng.below(scored.len());
    for _ in 1..k.max(1) {
        let challenger = rng.below(scored.len());
        if scored[challenger].0 < scored[best].0 {
            best = challenger;
        }
    }
    best
}

/// BLX-0.25 blend crossover per gene.
fn blend_crossover(a: &[f64], b: &[f64], rng: &mut Rng) -> Vec<f64> {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let beta = rng.range(-0.25, 1.25);
            x + beta * (y - x)
        })
        .collect()
}

/// Per-gene Gaussian mutation with reflection-free clamping to the search box.
fn mutate(genome: &mut [f64], rate: f64, sigma: f64, lo: f64, hi: f64, rng: &mut Rng) {
    for gene in genome.iter_mut() {
        if rng.unit() < rate {
            *gene += sigma * rng.gaussian();
        }
        if lo <= hi {
            *gene = gene.clamp(lo, hi);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(function: &str, generations: usize) -> IslandJob {
        IslandJob {
            solve_id: "s".into(),
            request_id: "r".into(),
            epoch: 0,
            island_id: 0,
            problem: ProblemSpec {
                function: function.into(),
                dimension: 4,
                lower_bound: -5.0,
                upper_bound: 5.0,
            },
            params: GaParams {
                population_size: 40,
                generations,
                mutation_rate: 0.2,
                mutation_scale: 0.1,
                crossover_rate: 0.9,
                elite_count: 2,
                tournament_size: 3,
            },
            seed: 42,
            population: Vec::new(),
        }
    }

    #[test]
    fn deterministic_for_a_fixed_seed() {
        let a = job("rastrigin", 30).run("node");
        let b = job("rastrigin", 30).run("node");
        assert_eq!(a.best_fitness, b.best_fitness);
        assert_eq!(a.best_genome, b.best_genome);
    }

    #[test]
    fn improves_over_generations() {
        let none = job("sphere", 0).run("node");
        let many = job("sphere", 60).run("node");
        assert!(many.best_fitness < none.best_fitness);
        assert!(many.best_fitness < 1.0, "sphere should converge near 0");
    }

    #[test]
    fn population_stays_sorted_and_sized() {
        let result = job("ackley", 10).run("node");
        assert_eq!(result.population.len(), 40);
        for pair in result.population.windows(2) {
            let a = evaluate("ackley", &pair[0]);
            let b = evaluate("ackley", &pair[1]);
            assert!(a <= b);
        }
    }
}
