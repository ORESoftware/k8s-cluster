//! Genetic-programming symbolic regression — the Eureqa-style core.
//!
//! Expression trees over a configurable set of analytic building blocks are
//! evolved against the data. Three ideas make the output genuinely analytic and
//! genuinely a trade-off, not a single black box:
//!
//!  * **Linear scaling (Keijzer 2003).** Every candidate `f` is fitted with its
//!    optimal affine wrapper `a·f + b` *in closed form* (least squares) before
//!    its error is measured. This fuses an analytic solve into the evolutionary
//!    loop and makes the search dramatically more effective.
//!  * **A Pareto archive** of non-dominated (complexity, error) models, so the
//!    caller gets the whole accuracy/simplicity front and can pick the equation
//!    that is "simple enough" — exactly Eureqa's workflow.
//!  * **Analytic post-processing**: constant folding + algebraic simplification,
//!    and symbolic differentiation of the chosen equation.
//!
//! Symbolic regression fits the *raw* units (not standardised) so the equations
//! it returns are written in the caller's own variables.

use std::time::Instant;

use crate::linalg::linear_scaling;
use crate::rng::Rng;

// ------------------------------------------------------------------ operators

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unary {
    Neg,
    Sin,
    Cos,
    Exp,
    Log,
    Sqrt,
    Square,
    Tanh,
    Abs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Binary {
    Add,
    Sub,
    Mul,
    Div,
}

impl Unary {
    fn eval(self, v: f64) -> f64 {
        match self {
            Unary::Neg => -v,
            Unary::Sin => v.sin(),
            Unary::Cos => v.cos(),
            // Clamp the exponent so a tail value can't overflow to ±inf.
            Unary::Exp => v.clamp(-30.0, 30.0).exp(),
            // Protected log: smooth, defined for all inputs.
            Unary::Log => (v.abs() + 1e-12).ln(),
            Unary::Sqrt => v.abs().sqrt(),
            Unary::Square => v * v,
            Unary::Tanh => v.tanh(),
            Unary::Abs => v.abs(),
        }
    }

    /// Structural complexity weight (cheap ops cost less, transcendental more).
    fn cost(self) -> usize {
        match self {
            Unary::Neg => 1,
            Unary::Square | Unary::Abs => 2,
            Unary::Sin | Unary::Cos | Unary::Sqrt | Unary::Tanh => 3,
            Unary::Exp | Unary::Log => 4,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Unary::Neg => "neg",
            Unary::Sin => "sin",
            Unary::Cos => "cos",
            Unary::Exp => "exp",
            Unary::Log => "log",
            Unary::Sqrt => "sqrt",
            Unary::Square => "square",
            Unary::Tanh => "tanh",
            Unary::Abs => "abs",
        }
    }
}

impl Binary {
    fn eval(self, a: f64, b: f64) -> f64 {
        match self {
            Binary::Add => a + b,
            Binary::Sub => a - b,
            Binary::Mul => a * b,
            // Protected division: a near-zero denominator yields 1.0.
            Binary::Div => {
                if b.abs() < 1e-9 {
                    1.0
                } else {
                    a / b
                }
            }
        }
    }

    fn cost(self) -> usize {
        match self {
            Binary::Add | Binary::Sub | Binary::Mul => 1,
            Binary::Div => 2,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Binary::Add => "+",
            Binary::Sub => "-",
            Binary::Mul => "*",
            Binary::Div => "/",
        }
    }
}

// ----------------------------------------------------------------- expression

#[derive(Clone)]
pub enum Expr {
    Const(f64),
    Var(usize),
    Unary(Unary, Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
}

impl Expr {
    /// Evaluate at a single input row (protected operators keep this finite for
    /// almost all inputs; genuine overflows surface as non-finite and are
    /// penalised by the fitness function rather than silently swallowed).
    pub fn eval(&self, x: &[f64]) -> f64 {
        match self {
            Expr::Const(c) => *c,
            Expr::Var(i) => x.get(*i).copied().unwrap_or(0.0),
            Expr::Unary(op, a) => op.eval(a.eval(x)),
            Expr::Binary(op, a, b) => op.eval(a.eval(x), b.eval(x)),
        }
    }

    /// Node count (used for crossover/mutation point selection and size caps).
    pub fn size(&self) -> usize {
        match self {
            Expr::Const(_) | Expr::Var(_) => 1,
            Expr::Unary(_, a) => 1 + a.size(),
            Expr::Binary(_, a, b) => 1 + a.size() + b.size(),
        }
    }

    pub fn depth(&self) -> usize {
        match self {
            Expr::Const(_) | Expr::Var(_) => 1,
            Expr::Unary(_, a) => 1 + a.depth(),
            Expr::Binary(_, a, b) => 1 + a.depth().max(b.depth()),
        }
    }

    /// Weighted complexity score — the simplicity axis of the Pareto front.
    pub fn complexity(&self) -> usize {
        match self {
            Expr::Const(_) | Expr::Var(_) => 1,
            Expr::Unary(op, a) => op.cost() + a.complexity(),
            Expr::Binary(op, a, b) => op.cost() + a.complexity() + b.complexity(),
        }
    }

    /// Human-readable infix form using the caller's variable names.
    pub fn to_infix(&self, vars: &[String]) -> String {
        match self {
            Expr::Const(c) => fmt_num(*c),
            Expr::Var(i) => vars
                .get(*i)
                .cloned()
                .unwrap_or_else(|| format!("x{i}")),
            Expr::Unary(op, a) => match op {
                Unary::Neg => format!("(-{})", a.to_infix(vars)),
                Unary::Square => format!("({})^2", a.to_infix(vars)),
                _ => format!("{}({})", op.name(), a.to_infix(vars)),
            },
            Expr::Binary(op, a, b) => {
                format!("({} {} {})", a.to_infix(vars), op.symbol(), b.to_infix(vars))
            }
        }
    }

    // -- preorder addressing for the genetic operators --------------------

    /// Borrow the node at preorder position `target`.
    fn nth(&self, target: usize, counter: &mut usize) -> Option<&Expr> {
        let here = *counter;
        *counter += 1;
        if here == target {
            return Some(self);
        }
        match self {
            Expr::Unary(_, a) => a.nth(target, counter),
            Expr::Binary(_, a, b) => a.nth(target, counter).or_else(|| b.nth(target, counter)),
            _ => None,
        }
    }

    fn subtree_at(&self, target: usize) -> Expr {
        let mut counter = 0;
        self.nth(target, &mut counter).cloned().unwrap_or_else(|| self.clone())
    }

    /// Clone of the tree with the node at preorder position `target` replaced.
    fn replace(&self, target: usize, repl: &Expr, counter: &mut usize) -> Expr {
        let here = *counter;
        *counter += 1;
        if here == target {
            return repl.clone();
        }
        match self {
            Expr::Const(c) => Expr::Const(*c),
            Expr::Var(i) => Expr::Var(*i),
            Expr::Unary(op, a) => Expr::Unary(*op, Box::new(a.replace(target, repl, counter))),
            Expr::Binary(op, a, b) => {
                let na = a.replace(target, repl, counter);
                let nb = b.replace(target, repl, counter);
                Expr::Binary(*op, Box::new(na), Box::new(nb))
            }
        }
    }

    fn replace_at(&self, target: usize, repl: &Expr) -> Expr {
        let mut counter = 0;
        self.replace(target, repl, &mut counter)
    }

    // -- constant extraction for numeric polishing ------------------------

    fn collect_consts(&self, out: &mut Vec<f64>) {
        match self {
            Expr::Const(c) => out.push(*c),
            Expr::Var(_) => {}
            Expr::Unary(_, a) => a.collect_consts(out),
            Expr::Binary(_, a, b) => {
                a.collect_consts(out);
                b.collect_consts(out);
            }
        }
    }

    fn rebuild_consts(&self, vals: &[f64], cursor: &mut usize) -> Expr {
        match self {
            Expr::Const(_) => {
                let v = vals.get(*cursor).copied().unwrap_or(0.0);
                *cursor += 1;
                Expr::Const(v)
            }
            Expr::Var(i) => Expr::Var(*i),
            Expr::Unary(op, a) => Expr::Unary(*op, Box::new(a.rebuild_consts(vals, cursor))),
            Expr::Binary(op, a, b) => {
                let na = a.rebuild_consts(vals, cursor);
                let nb = b.rebuild_consts(vals, cursor);
                Expr::Binary(*op, Box::new(na), Box::new(nb))
            }
        }
    }

    // -- symbolic differentiation -----------------------------------------

    /// Analytic partial derivative with respect to variable `k`, simplified.
    pub fn derivative(&self, k: usize) -> Expr {
        simplify(&self.diff(k))
    }

    fn diff(&self, k: usize) -> Expr {
        match self {
            Expr::Const(_) => Expr::Const(0.0),
            Expr::Var(i) => Expr::Const(if *i == k { 1.0 } else { 0.0 }),
            Expr::Unary(op, a) => {
                let da = a.diff(k);
                match op {
                    Unary::Neg => neg(da),
                    // d/dx sin(a) = cos(a)·a'
                    Unary::Sin => mul(Expr::Unary(Unary::Cos, a.clone()), da),
                    Unary::Cos => neg(mul(Expr::Unary(Unary::Sin, a.clone()), da)),
                    Unary::Exp => mul(Expr::Unary(Unary::Exp, a.clone()), da),
                    // d/dx log(a) = a'/a
                    Unary::Log => bin(Binary::Div, da, (**a).clone()),
                    // d/dx sqrt(a) = a'/(2·sqrt(a))
                    Unary::Sqrt => bin(
                        Binary::Div,
                        da,
                        mul(Expr::Const(2.0), Expr::Unary(Unary::Sqrt, a.clone())),
                    ),
                    // d/dx a^2 = 2·a·a'
                    Unary::Square => mul(mul(Expr::Const(2.0), (**a).clone()), da),
                    // d/dx tanh(a) = (1 - tanh(a)^2)·a'
                    Unary::Tanh => mul(
                        bin(
                            Binary::Sub,
                            Expr::Const(1.0),
                            Expr::Unary(Unary::Square, Box::new(Expr::Unary(Unary::Tanh, a.clone()))),
                        ),
                        da,
                    ),
                    // d/dx |a| = sign(a)·a'  (written a/|a| · a')
                    Unary::Abs => mul(
                        bin(Binary::Div, (**a).clone(), Expr::Unary(Unary::Abs, a.clone())),
                        da,
                    ),
                }
            }
            Expr::Binary(op, a, b) => {
                let da = a.diff(k);
                let db = b.diff(k);
                match op {
                    Binary::Add => bin(Binary::Add, da, db),
                    Binary::Sub => bin(Binary::Sub, da, db),
                    // product rule
                    Binary::Mul => bin(
                        Binary::Add,
                        mul(da, (**b).clone()),
                        mul((**a).clone(), db),
                    ),
                    // quotient rule: (a'b - ab')/b^2
                    Binary::Div => bin(
                        Binary::Div,
                        bin(
                            Binary::Sub,
                            mul(da, (**b).clone()),
                            mul((**a).clone(), db),
                        ),
                        Expr::Unary(Unary::Square, b.clone()),
                    ),
                }
            }
        }
    }
}

// small constructors keeping the diff/simplify code readable
fn neg(a: Expr) -> Expr {
    Expr::Unary(Unary::Neg, Box::new(a))
}
fn mul(a: Expr, b: Expr) -> Expr {
    Expr::Binary(Binary::Mul, Box::new(a), Box::new(b))
}
fn bin(op: Binary, a: Expr, b: Expr) -> Expr {
    Expr::Binary(op, Box::new(a), Box::new(b))
}

/// Format a constant for the analytic output: a few significant digits, no
/// noisy float tails, scientific notation only for very large/small magnitudes.
fn fmt_num(x: f64) -> String {
    if !x.is_finite() {
        return "0".to_string();
    }
    let a = x.abs();
    if a != 0.0 && !(1e-3..1e6).contains(&a) {
        return format!("{x:.3e}");
    }
    let s = format!("{x:.4}");
    // Trim trailing zeros and a dangling decimal point.
    let trimmed = s.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

// --------------------------------------------------------- simplification

/// Constant folding plus a handful of algebraic identities, applied bottom-up.
/// Keeps the reported equations and their derivatives readable without changing
/// their value.
pub fn simplify(e: &Expr) -> Expr {
    match e {
        Expr::Const(c) => Expr::Const(*c),
        Expr::Var(i) => Expr::Var(*i),
        Expr::Unary(op, a) => {
            let sa = simplify(a);
            if let Expr::Const(c) = sa {
                return Expr::Const(op.eval(c));
            }
            // -(-x) = x
            if let (Unary::Neg, Expr::Unary(Unary::Neg, inner)) = (op, &sa) {
                return (**inner).clone();
            }
            Expr::Unary(*op, Box::new(sa))
        }
        Expr::Binary(op, a, b) => {
            let sa = simplify(a);
            let sb = simplify(b);
            if let (Expr::Const(x), Expr::Const(y)) = (&sa, &sb) {
                return Expr::Const(op.eval(*x, *y));
            }
            match op {
                Binary::Add => {
                    if is_zero(&sa) {
                        return sb;
                    }
                    if is_zero(&sb) {
                        return sa;
                    }
                }
                Binary::Sub => {
                    if is_zero(&sb) {
                        return sa;
                    }
                    if is_zero(&sa) {
                        return neg(sb);
                    }
                }
                Binary::Mul => {
                    if is_zero(&sa) || is_zero(&sb) {
                        return Expr::Const(0.0);
                    }
                    if is_one(&sa) {
                        return sb;
                    }
                    if is_one(&sb) {
                        return sa;
                    }
                }
                Binary::Div => {
                    if is_zero(&sa) {
                        return Expr::Const(0.0);
                    }
                    if is_one(&sb) {
                        return sa;
                    }
                }
            }
            Expr::Binary(*op, Box::new(sa), Box::new(sb))
        }
    }
}

fn is_zero(e: &Expr) -> bool {
    matches!(e, Expr::Const(c) if c.abs() < 1e-12)
}
fn is_one(e: &Expr) -> bool {
    matches!(e, Expr::Const(c) if (c - 1.0).abs() < 1e-12)
}

// ----------------------------------------------------------------- fitting

/// Apply the optimal affine wrapper to a candidate and report `(a, b, mse)`.
/// Non-finite predictions yield an infinite error so the individual dies.
fn evaluate_scaled(expr: &Expr, x: &[Vec<f64>], y: &[f64]) -> (f64, f64, f64) {
    let f: Vec<f64> = x.iter().map(|row| expr.eval(row)).collect();
    if f.iter().any(|v| !v.is_finite()) {
        return (1.0, 0.0, f64::INFINITY);
    }
    let (a, b) = linear_scaling(&f, y);
    let n = y.len().max(1) as f64;
    let mut sse = 0.0;
    for i in 0..y.len() {
        let pred = a * f[i] + b;
        let e = pred - y[i];
        sse += e * e;
    }
    let mse = sse / n;
    (a, b, if mse.is_finite() { mse } else { f64::INFINITY })
}

fn scaled_mse(expr: &Expr, x: &[Vec<f64>], y: &[f64]) -> f64 {
    evaluate_scaled(expr, x, y).2
}

/// Fold the fitted affine wrapper into the tree, then simplify, so the reported
/// equation is self-contained and in the caller's units.
fn fold_scaling(expr: &Expr, a: f64, b: f64) -> Expr {
    let scaled = if (a - 1.0).abs() < 1e-9 {
        expr.clone()
    } else {
        mul(Expr::Const(a), expr.clone())
    };
    let shifted = if b.abs() < 1e-9 {
        scaled
    } else {
        bin(Binary::Add, scaled, Expr::Const(b))
    };
    simplify(&shifted)
}

/// Stochastic hill-climb (a (1+1)-ES) on the constants of an expression to
/// squeeze out residual error after evolution — the numeric polish on top of
/// the analytic linear scaling.
fn optimize_constants(expr: &Expr, x: &[Vec<f64>], y: &[f64], rng: &mut Rng, iters: usize) -> Expr {
    let mut consts = Vec::new();
    expr.collect_consts(&mut consts);
    if consts.is_empty() || iters == 0 {
        return expr.clone();
    }
    let eval = |c: &[f64]| -> f64 {
        let mut cursor = 0;
        let candidate = expr.rebuild_consts(c, &mut cursor);
        scaled_mse(&candidate, x, y)
    };
    let mut best = consts.clone();
    let mut best_mse = eval(&best);
    let mut step = 1.0;
    for _ in 0..iters {
        let mut cand = best.clone();
        for v in cand.iter_mut() {
            if rng.chance(0.5) {
                *v += rng.gaussian(0.0, step);
            }
        }
        let m = eval(&cand);
        if m < best_mse {
            best_mse = m;
            best = cand;
        } else {
            step *= 0.95;
            if step < 1e-4 {
                step = 0.5; // restart the step size to escape stagnation
            }
        }
    }
    let mut cursor = 0;
    expr.rebuild_consts(&best, &mut cursor)
}

// --------------------------------------------------------------- generation

pub struct GpConfig {
    pub d: usize,
    pub population: usize,
    pub generations: usize,
    pub max_depth: usize,
    pub max_size: usize,
    pub tournament: usize,
    pub crossover_prob: f64,
    pub mutation_prob: f64,
    pub parsimony: f64,
    pub const_range: f64,
    pub const_prob: f64,
    pub const_opt_iters: usize,
    pub unary: Vec<Unary>,
    pub binary: Vec<Binary>,
}

impl GpConfig {
    /// Defaults for `d` features, optionally restricting the operator set to the
    /// named building blocks (Eureqa lets you choose these).
    pub fn new(d: usize, operators: Option<&[String]>) -> GpConfig {
        let (unary, binary) = match operators {
            Some(names) if !names.is_empty() => parse_operators(names),
            _ => (
                vec![Unary::Sin, Unary::Cos, Unary::Exp, Unary::Log, Unary::Square],
                vec![Binary::Add, Binary::Sub, Binary::Mul, Binary::Div],
            ),
        };
        // Always keep at least one binary op so trees can grow.
        let binary = if binary.is_empty() {
            vec![Binary::Add, Binary::Mul]
        } else {
            binary
        };
        GpConfig {
            d,
            population: 300,
            generations: 40,
            max_depth: 6,
            max_size: 60,
            tournament: 5,
            crossover_prob: 0.7,
            mutation_prob: 0.3,
            parsimony: 0.001,
            const_range: 5.0,
            const_prob: 0.35,
            const_opt_iters: 60,
            unary,
            binary,
        }
    }
}

fn parse_operators(names: &[String]) -> (Vec<Unary>, Vec<Binary>) {
    let mut unary = Vec::new();
    let mut binary = Vec::new();
    for raw in names {
        match raw.trim().to_ascii_lowercase().as_str() {
            "+" | "add" | "plus" => binary.push(Binary::Add),
            "-" | "sub" | "minus" => binary.push(Binary::Sub),
            "*" | "mul" | "times" => binary.push(Binary::Mul),
            "/" | "div" | "divide" => binary.push(Binary::Div),
            "neg" => unary.push(Unary::Neg),
            "sin" => unary.push(Unary::Sin),
            "cos" => unary.push(Unary::Cos),
            "exp" => unary.push(Unary::Exp),
            "log" | "ln" => unary.push(Unary::Log),
            "sqrt" => unary.push(Unary::Sqrt),
            "square" | "sq" | "^2" | "pow2" => unary.push(Unary::Square),
            "tanh" => unary.push(Unary::Tanh),
            "abs" => unary.push(Unary::Abs),
            _ => {}
        }
    }
    (unary, binary)
}

fn random_terminal(rng: &mut Rng, cfg: &GpConfig) -> Expr {
    if cfg.d == 0 || rng.chance(cfg.const_prob) {
        let v = rng.range(-cfg.const_range, cfg.const_range);
        // Snap some constants to integers for nicer-looking equations.
        if rng.chance(0.3) {
            Expr::Const(v.round())
        } else {
            Expr::Const(v)
        }
    } else {
        Expr::Var(rng.below(cfg.d))
    }
}

fn random_node(rng: &mut Rng, cfg: &GpConfig, depth: usize, max_depth: usize, full: bool) -> Expr {
    let at_leaf = depth >= max_depth;
    let pick_terminal = at_leaf || (!full && depth > 0 && rng.chance(0.3));
    if pick_terminal {
        return random_terminal(rng, cfg);
    }
    // Choose between a unary and binary internal node, honouring availability.
    let use_unary = !cfg.unary.is_empty() && (cfg.binary.is_empty() || rng.chance(0.35));
    if use_unary {
        let op = cfg.unary[rng.below(cfg.unary.len())];
        Expr::Unary(op, Box::new(random_node(rng, cfg, depth + 1, max_depth, full)))
    } else {
        let op = cfg.binary[rng.below(cfg.binary.len())];
        let a = random_node(rng, cfg, depth + 1, max_depth, full);
        let b = random_node(rng, cfg, depth + 1, max_depth, full);
        Expr::Binary(op, Box::new(a), Box::new(b))
    }
}

// ----------------------------------------------------------- genetic operators

fn crossover(a: &Expr, b: &Expr, rng: &mut Rng, cfg: &GpConfig) -> Expr {
    let point = rng.below(a.size());
    let donor = b.subtree_at(rng.below(b.size()));
    let child = a.replace_at(point, &donor);
    if child.depth() > cfg.max_depth || child.size() > cfg.max_size {
        a.clone()
    } else {
        child
    }
}

fn mutate(e: &Expr, rng: &mut Rng, cfg: &GpConfig) -> Expr {
    if rng.chance(0.5) {
        // Subtree mutation: replace a random node with a fresh small subtree.
        let point = rng.below(e.size());
        let subtree = random_node(rng, cfg, 0, 2.min(cfg.max_depth), false);
        let child = e.replace_at(point, &subtree);
        if child.depth() > cfg.max_depth || child.size() > cfg.max_size {
            e.clone()
        } else {
            child
        }
    } else {
        // Point mutation: perturb a constant / swap a variable or operator.
        let target = rng.below(e.size());
        let mut counter = 0;
        point_mutate(e, target, rng, cfg, &mut counter)
    }
}

fn point_mutate(e: &Expr, target: usize, rng: &mut Rng, cfg: &GpConfig, counter: &mut usize) -> Expr {
    let here = *counter;
    *counter += 1;
    if here == target {
        return match e {
            Expr::Const(c) => Expr::Const(c + rng.gaussian(0.0, 1.0)),
            Expr::Var(_) => {
                if cfg.d > 1 {
                    Expr::Var(rng.below(cfg.d))
                } else {
                    random_terminal(rng, cfg)
                }
            }
            Expr::Unary(_, a) => {
                let op = if cfg.unary.is_empty() {
                    Unary::Square
                } else {
                    cfg.unary[rng.below(cfg.unary.len())]
                };
                Expr::Unary(op, a.clone())
            }
            Expr::Binary(_, a, b) => {
                let op = cfg.binary[rng.below(cfg.binary.len())];
                Expr::Binary(op, a.clone(), b.clone())
            }
        };
    }
    match e {
        Expr::Const(c) => Expr::Const(*c),
        Expr::Var(i) => Expr::Var(*i),
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(point_mutate(a, target, rng, cfg, counter))),
        Expr::Binary(op, a, b) => {
            let na = point_mutate(a, target, rng, cfg, counter);
            let nb = point_mutate(b, target, rng, cfg, counter);
            Expr::Binary(*op, Box::new(na), Box::new(nb))
        }
    }
}

// -------------------------------------------------------------- evolution loop

struct Scored {
    expr: Expr,
    complexity: usize,
    train_mse: f64,
    penalized: f64,
}

fn score(expr: Expr, cfg: &GpConfig, x: &[Vec<f64>], y: &[f64]) -> Scored {
    let complexity = expr.complexity();
    let mse = scaled_mse(&expr, x, y);
    let penalized = if mse.is_finite() {
        mse * (1.0 + cfg.parsimony * complexity as f64)
    } else {
        f64::INFINITY
    };
    Scored {
        expr,
        complexity,
        train_mse: mse,
        penalized,
    }
}

fn tournament<'a>(pop: &'a [Scored], rng: &mut Rng, k: usize) -> &'a Scored {
    let mut best = &pop[rng.below(pop.len())];
    for _ in 1..k.max(1) {
        let challenger = &pop[rng.below(pop.len())];
        if challenger.penalized < best.penalized {
            best = challenger;
        }
    }
    best
}

/// One non-dominated front member, in raw (pre-fold) form.
struct ParetoRaw {
    expr: Expr,
    complexity: usize,
    train_mse: f64,
}

fn consider(archive: &mut Vec<ParetoRaw>, expr: &Expr, complexity: usize, mse: f64) {
    if !mse.is_finite() {
        return;
    }
    // Already have something at least as simple and at least as accurate?
    for e in archive.iter() {
        if e.complexity <= complexity && e.train_mse <= mse {
            return;
        }
    }
    // Drop anything the newcomer dominates.
    archive.retain(|e| !(complexity <= e.complexity && mse <= e.train_mse));
    archive.push(ParetoRaw {
        expr: expr.clone(),
        complexity,
        train_mse: mse,
    });
}

/// A finished Pareto-front entry, ready for reporting.
pub struct ParetoMember {
    pub expr: Expr,
    pub complexity: usize,
    pub train_mse: f64,
}

pub struct GpResult {
    pub front: Vec<ParetoMember>,
    pub generations: usize,
    /// True if the search or polish phase stopped at the wall-clock budget.
    pub stopped_early: bool,
}

/// Run symbolic regression on raw (un-standardised) data and return the Pareto
/// front of analytic models. The caller chooses which front member to surface
/// (typically by validation error). `deadline` bounds total CPU time: the
/// generation loop and the constant-polish loop both stop cleanly once it
/// passes, returning the best models found so far.
pub fn run(x: &[Vec<f64>], y: &[f64], cfg: &GpConfig, rng: &mut Rng, deadline: Instant) -> GpResult {
    let pop_size = cfg.population.max(8);
    let max_depth = cfg.max_depth.max(2);
    let mut stopped_early = false;

    // Ramped half-and-half initialisation across depths 2..=max_depth.
    let mut population: Vec<Scored> = Vec::with_capacity(pop_size);
    for i in 0..pop_size {
        let depth = 2 + (i % (max_depth - 1));
        let full = i % 2 == 0;
        let expr = random_node(rng, cfg, 0, depth, full);
        population.push(score(expr, cfg, x, y));
    }

    let mut archive: Vec<ParetoRaw> = Vec::new();
    for s in &population {
        consider(&mut archive, &s.expr, s.complexity, s.train_mse);
    }

    let mut gens_run = 0;
    for _ in 0..cfg.generations {
        // Cooperative time budget: stop cleanly with the archive built so far.
        if Instant::now() >= deadline {
            stopped_early = true;
            break;
        }
        let mut next: Vec<Scored> = Vec::with_capacity(pop_size);

        // Elitism: carry the single best penalised individual forward.
        if let Some(best) = population
            .iter()
            .min_by(|a, b| a.penalized.partial_cmp(&b.penalized).unwrap_or(std::cmp::Ordering::Greater))
        {
            next.push(score(best.expr.clone(), cfg, x, y));
        }

        while next.len() < pop_size {
            let child = if rng.chance(cfg.crossover_prob) {
                let a = tournament(&population, rng, cfg.tournament);
                let b = tournament(&population, rng, cfg.tournament);
                crossover(&a.expr, &b.expr, rng, cfg)
            } else {
                let parent = tournament(&population, rng, cfg.tournament);
                parent.expr.clone()
            };
            let child = if rng.chance(cfg.mutation_prob) {
                mutate(&child, rng, cfg)
            } else {
                child
            };
            let scored = score(child, cfg, x, y);
            consider(&mut archive, &scored.expr, scored.complexity, scored.train_mse);
            next.push(scored);
        }

        population = next;
        gens_run += 1;
    }

    // Polish + finalise each archived model: numeric constant hill-climb, fold
    // the analytic scaling in, simplify, and re-derive a clean non-dominated set.
    // Once the deadline passes we skip the (expensive) hill-climb but still fold
    // and keep every remaining model, so the front is never truncated.
    let mut polished: Vec<ParetoRaw> = Vec::new();
    for raw in &archive {
        let iters = if Instant::now() >= deadline {
            stopped_early = true;
            0
        } else {
            cfg.const_opt_iters
        };
        let tuned = optimize_constants(&raw.expr, x, y, rng, iters);
        let (a, b, mse) = evaluate_scaled(&tuned, x, y);
        let folded = fold_scaling(&tuned, a, b);
        consider(&mut polished, &folded, folded.complexity(), mse);
    }

    let mut front: Vec<ParetoMember> = polished
        .into_iter()
        .map(|p| ParetoMember {
            expr: p.expr,
            complexity: p.complexity,
            train_mse: p.train_mse,
        })
        .collect();
    front.sort_by_key(|m| m.complexity);

    GpResult {
        front,
        generations: gens_run,
        stopped_early,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("x{i}")).collect()
    }

    #[test]
    fn eval_and_simplify_constant_fold() {
        // (2 + 3) * x0  -> simplifies the constant fold, evaluates correctly.
        let e = bin(
            Binary::Mul,
            bin(Binary::Add, Expr::Const(2.0), Expr::Const(3.0)),
            Expr::Var(0),
        );
        assert_eq!(e.eval(&[4.0]), 20.0);
        let s = simplify(&e);
        // 5 * x0
        assert_eq!(s.eval(&[4.0]), 20.0);
        assert!(s.to_infix(&vars(1)).contains('5'));
    }

    #[test]
    fn simplify_identities() {
        let x = Expr::Var(0);
        // x + 0 == x
        let e = bin(Binary::Add, x.clone(), Expr::Const(0.0));
        assert_eq!(simplify(&e).to_infix(&vars(1)), "x0");
        // x * 1 == x
        let e = bin(Binary::Mul, x.clone(), Expr::Const(1.0));
        assert_eq!(simplify(&e).to_infix(&vars(1)), "x0");
        // x * 0 == 0
        let e = bin(Binary::Mul, x, Expr::Const(0.0));
        assert_eq!(simplify(&e).to_infix(&vars(1)), "0");
    }

    #[test]
    fn derivative_of_square() {
        // d/dx (x^2) = 2*x
        let e = Expr::Unary(Unary::Square, Box::new(Expr::Var(0)));
        let d = e.derivative(0);
        for &xv in &[-2.0, 0.5, 3.0] {
            assert!((d.eval(&[xv]) - 2.0 * xv).abs() < 1e-9);
        }
    }

    #[test]
    fn derivative_of_sin() {
        // d/dx sin(x) = cos(x)
        let e = Expr::Unary(Unary::Sin, Box::new(Expr::Var(0)));
        let d = e.derivative(0);
        for &xv in &[0.0, 1.0, 2.5] {
            assert!((d.eval(&[xv]) - xv.cos()).abs() < 1e-9);
        }
    }

    #[test]
    fn stops_at_deadline() {
        // An already-expired deadline must short-circuit the search without
        // panicking, still returning a usable (initial) front.
        let x: Vec<Vec<f64>> = (0..20).map(|i| vec![i as f64 * 0.1]).collect();
        let y: Vec<f64> = x.iter().map(|r| r[0] * r[0]).collect();
        let mut cfg = GpConfig::new(1, None);
        cfg.population = 2_000;
        cfg.generations = 200;
        let mut rng = Rng::new(1);
        let result = run(&x, &y, &cfg, &mut rng, Instant::now());
        assert!(result.stopped_early);
        assert_eq!(result.generations, 0);
        assert!(!result.front.is_empty());
    }

    #[test]
    fn recovers_quadratic() {
        // Fit y = 2*x^2 - 3 on a clean grid; expect a near-perfect symbolic fit.
        let mut x = Vec::new();
        let mut y = Vec::new();
        let mut v = -3.0;
        while v <= 3.0 {
            x.push(vec![v]);
            y.push(2.0 * v * v - 3.0);
            v += 0.25;
        }
        let mut cfg = GpConfig::new(1, None);
        cfg.population = 400;
        cfg.generations = 40;
        let mut rng = Rng::new(42);
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        let result = run(&x, &y, &cfg, &mut rng, deadline);
        assert!(!result.front.is_empty());
        // The most accurate member should fit almost perfectly thanks to linear
        // scaling recovering the 2 and -3.
        let best = result
            .front
            .iter()
            .min_by(|a, b| a.train_mse.partial_cmp(&b.train_mse).unwrap())
            .unwrap();
        assert!(best.train_mse < 1e-3, "train_mse was {}", best.train_mse);
    }
}
