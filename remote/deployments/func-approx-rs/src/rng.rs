//! Small dependency-free PRNG: SplitMix64 with Box–Muller normals.
//!
//! Mirrors the seeded-RNG approach used across the sibling compute servers
//! (monte-carlo, evolution) so every fit is reproducible from a seed without
//! pulling in the `rand` crate.

pub struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
            spare: None,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }

    /// Uniform integer in [0, n); returns 0 when n == 0 so callers never panic.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    /// Bernoulli draw — true with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.uniform() < p
    }

    /// Standard normal via Box–Muller with one cached spare.
    pub fn normal(&mut self) -> f64 {
        if let Some(spare) = self.spare.take() {
            return spare;
        }
        let mut u1 = self.uniform();
        if u1 < 1e-12 {
            u1 = 1e-12;
        }
        let u2 = self.uniform();
        let mag = (-2.0 * u1.ln()).sqrt();
        self.spare = Some(mag * (std::f64::consts::TAU * u2).sin());
        mag * (std::f64::consts::TAU * u2).cos()
    }

    /// Normal with the given mean and standard deviation.
    pub fn gaussian(&mut self, mean: f64, std: f64) -> f64 {
        mean + std * self.normal()
    }
}
