//! Neuroevolution: a self-adaptive (μ/μ_I, λ) Evolution Strategy that optimises
//! the *weights of an MLP* without gradients. This is the "combine evolutionary
//! programming with neural nets" path — the same network the backprop learner
//! uses, but searched evolutionarily, which copes with rugged or
//! non-differentiable error surfaces where gradient descent stalls.
//!
//! Each offspring carries its own log-normally mutated step size, so the search
//! anneals its exploration automatically. The fitness-best-on-validation genome
//! is returned to guard against overfitting the training fold.

// Genome vectors are indexed by position when recombining; range loops are the
// clearest idiom for the element-wise math.
#![allow(clippy::needless_range_loop)]

use std::time::Instant;

use crate::nn::Mlp;
use crate::rng::Rng;

pub struct EsConfig {
    pub population: usize,
    pub parents: usize,
    pub generations: usize,
    pub sigma0: f64,
}

impl Default for EsConfig {
    fn default() -> Self {
        EsConfig {
            population: 48,
            parents: 12,
            generations: 60,
            sigma0: 0.3,
        }
    }
}

struct Candidate {
    genome: Vec<f64>,
    sigma: f64,
    fitness: f64, // training MSE (lower is better)
}

/// Evolve the parameters of `template` against standardised data. The returned
/// network is `template`'s architecture with the best evolved weights. Returns
/// `(network, generations_run)`.
#[allow(clippy::too_many_arguments)]
pub fn evolve(
    template: &Mlp,
    train_x: &[Vec<f64>],
    train_y: &[f64],
    val_x: &[Vec<f64>],
    val_y: &[f64],
    cfg: &EsConfig,
    rng: &mut Rng,
    deadline: Instant,
) -> (Mlp, usize) {
    let dim = template.parameter_count();
    let lambda = cfg.population.max(4);
    let mu = cfg.parents.clamp(1, lambda);
    // Step-size learning rate for the log-normal self-adaptation.
    let tau = 1.0 / (2.0 * dim as f64).sqrt();

    let mut scratch = template.clone();
    let evaluate = |scratch: &mut Mlp, genome: &[f64], xs: &[Vec<f64>], ys: &[f64]| -> f64 {
        scratch.set_flat(genome);
        scratch.mse(xs, ys)
    };

    let mut mean = template.to_flat();
    let mut sigma = cfg.sigma0.max(1e-4);

    let mut best_genome = mean.clone();
    let mut best_val = evaluate(&mut scratch, &mean, val_x, val_y);

    let mut gens_run = 0;
    for _ in 0..cfg.generations {
        // Cooperative time budget: stop cleanly and keep the best-on-val genome.
        if Instant::now() >= deadline {
            break;
        }
        let mut offspring: Vec<Candidate> = Vec::with_capacity(lambda);
        for _ in 0..lambda {
            // Per-candidate budget check: one generation evaluates up to λ nets,
            // so a coarse per-generation check could overshoot badly on large
            // nets/datasets. Bound the overshoot to a single evaluation.
            if Instant::now() >= deadline {
                break;
            }
            let child_sigma = (sigma * (tau * rng.normal()).exp()).clamp(1e-5, 10.0);
            let mut genome = vec![0.0; dim];
            for k in 0..dim {
                genome[k] = mean[k] + child_sigma * rng.normal();
            }
            let fitness = evaluate(&mut scratch, &genome, train_x, train_y);
            offspring.push(Candidate {
                genome,
                sigma: child_sigma,
                fitness,
            });
        }
        // A partial generation can leave fewer than μ offspring (or none if the
        // budget elapsed immediately); recombine over what we actually have.
        if offspring.is_empty() {
            break;
        }

        // Select the μ fittest and recombine by (intermediate) averaging.
        offspring.sort_by(|a, b| {
            a.fitness
                .partial_cmp(&b.fitness)
                .unwrap_or(std::cmp::Ordering::Greater)
        });
        offspring.truncate(mu.min(offspring.len()));

        let count = offspring.len();
        let mut new_mean = vec![0.0; dim];
        let mut new_sigma = 0.0;
        for cand in &offspring {
            for k in 0..dim {
                new_mean[k] += cand.genome[k];
            }
            new_sigma += cand.sigma;
        }
        let inv = 1.0 / count as f64;
        for k in 0..dim {
            new_mean[k] *= inv;
        }
        mean = new_mean;
        sigma = (new_sigma * inv).clamp(1e-5, 10.0);

        // Keep whichever recombined mean generalises best.
        let val = evaluate(&mut scratch, &mean, val_x, val_y);
        if val < best_val {
            best_val = val;
            best_genome = mean.clone();
        }
        gens_run += 1;
    }

    let mut net = template.clone();
    net.set_flat(&best_genome);
    (net, gens_run)
}
