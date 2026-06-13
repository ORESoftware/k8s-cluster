//! The analytic-formulation engine: turning the genetic-programming output into
//! the cleanest *analytic* form we can.
//!
//! Symbolic regression evolves correct-but-messy expression trees — `(x + x)`
//! instead of `2·x`, `a·b + a·c` instead of `a·(b + c)`, un-folded constants.
//! The hand-rolled `gp::simplify` only reaches a few local identities in one
//! bottom-up pass. This module replaces that with **equality saturation** over an
//! e-graph (the `egg` crate, the same engine [Herbie] uses to find accurate
//! analytic forms): we plant the expression, apply value-preserving rewrites
//! until fixpoint or a hard node/iteration/time budget, then extract the
//! lowest-cost member under the *same* complexity weights the Pareto front
//! reports.
//!
//! Two invariants make this safe to drop into the fit loop:
//!
//!  * **Value preservation under the engine's own arithmetic.** The constant
//!    folder uses the *protected* operator semantics from [`crate::gp`]
//!    (`Unary::eval` / `Binary::eval`), so a canonicalised expression evaluates
//!    identically to the original — including at the protected branches
//!    (`a/0 → 1`, `log(|a|)`, `sqrt(|a|)`). The rewrite rules are likewise sound
//!    under those semantics (e.g. `a/a → 1` holds because protected division
//!    returns 1 at a near-zero denominator too).
//!  * **It can never break a fit.** Every entry point is total: on any internal
//!    failure, an empty result, or a pathological size blow-up it returns the
//!    input unchanged. The `egg` runner is bounded on nodes, iterations *and*
//!    wall-clock, so it cannot pin a core regardless of input.
//!
//! [Herbie]: https://herbie.uwplse.org/

use std::time::Duration;

use egg::{
    define_language, merge_option, rewrite, Analysis, CostFunction, DidMerge, EGraph, Extractor,
    Id, Language, RecExpr, Rewrite, Runner, Symbol,
};
use ordered_float::NotNan;

use crate::gp::{Binary, Expr, Unary};

/// Hashable/orderable f64 constant for the e-graph.
type Constant = NotNan<f64>;

define_language! {
    /// The e-graph mirror of [`crate::gp::Expr`]. Variables become `v{i}` symbols
    /// so they round-trip back to `Expr::Var(i)` by index, independent of the
    /// caller's variable names.
    pub enum MathLang {
        "+" = Add([Id; 2]),
        "-" = Sub([Id; 2]),
        "*" = Mul([Id; 2]),
        "/" = Div([Id; 2]),
        "neg" = ENeg([Id; 1]),
        "sin" = Sin([Id; 1]),
        "cos" = Cos([Id; 1]),
        "exp" = Exp([Id; 1]),
        "log" = Log([Id; 1]),
        "sqrt" = Sqrt([Id; 1]),
        "square" = Square([Id; 1]),
        "tanh" = Tanh([Id; 1]),
        "abs" = Abs([Id; 1]),
        Constant(Constant),
        Symbol(Symbol),
    }
}

/// Constant-folding analysis using the *protected* semantics of [`crate::gp`], so
/// the simplified expression is numerically equivalent to the original under the
/// engine's own arithmetic (not textbook arithmetic).
#[derive(Default)]
struct ConstFold;

/// Keep a folded value only when it is finite (a non-finite fold is not a useful
/// constant and `NotNan` cannot hold NaN).
fn finite(v: f64) -> Option<Constant> {
    NotNan::new(v).ok().filter(|x| x.is_finite())
}

impl Analysis<MathLang> for ConstFold {
    type Data = Option<Constant>;

    fn make(egraph: &mut EGraph<MathLang, Self>, enode: &MathLang, _id: Id) -> Self::Data {
        let val = |id: &Id| egraph[*id].data;
        let unary = |op: Unary, a: &Id| val(a).and_then(|x| finite(op.eval(x.into_inner())));
        let binary = |op: Binary, a: &Id, b: &Id| match (val(a), val(b)) {
            (Some(x), Some(y)) => finite(op.eval(x.into_inner(), y.into_inner())),
            _ => None,
        };
        match enode {
            MathLang::Constant(c) => Some(*c),
            MathLang::Add([a, b]) => binary(Binary::Add, a, b),
            MathLang::Sub([a, b]) => binary(Binary::Sub, a, b),
            MathLang::Mul([a, b]) => binary(Binary::Mul, a, b),
            MathLang::Div([a, b]) => binary(Binary::Div, a, b),
            MathLang::ENeg([a]) => unary(Unary::Neg, a),
            MathLang::Sin([a]) => unary(Unary::Sin, a),
            MathLang::Cos([a]) => unary(Unary::Cos, a),
            MathLang::Exp([a]) => unary(Unary::Exp, a),
            MathLang::Log([a]) => unary(Unary::Log, a),
            MathLang::Sqrt([a]) => unary(Unary::Sqrt, a),
            MathLang::Square([a]) => unary(Unary::Square, a),
            MathLang::Tanh([a]) => unary(Unary::Tanh, a),
            MathLang::Abs([a]) => unary(Unary::Abs, a),
            MathLang::Symbol(_) => None,
        }
    }

    fn merge(&mut self, a: &mut Self::Data, b: Self::Data) -> DidMerge {
        // Both children of a union should fold to the same constant; keep `a`.
        merge_option(a, b, |_, _| DidMerge(false, false))
    }

    fn modify(egraph: &mut EGraph<MathLang, Self>, id: Id) {
        if let Some(c) = egraph[id].data {
            let folded = egraph.add(MathLang::Constant(c));
            egraph.union(id, folded);
        }
    }
}

/// Value-preserving rewrite rules. Where a rule looks arithmetic-unsound at a
/// boundary it is in fact sound under the *protected* semantics: `a/a → 1`
/// because protected division returns 1 even at a≈0.
fn rules() -> Vec<Rewrite<MathLang, ConstFold>> {
    vec![
        rewrite!("comm-add"; "(+ ?a ?b)" => "(+ ?b ?a)"),
        rewrite!("comm-mul"; "(* ?a ?b)" => "(* ?b ?a)"),
        rewrite!("assoc-add"; "(+ (+ ?a ?b) ?c)" => "(+ ?a (+ ?b ?c))"),
        rewrite!("assoc-mul"; "(* (* ?a ?b) ?c)" => "(* ?a (* ?b ?c))"),
        rewrite!("add-0"; "(+ ?a 0)" => "?a"),
        rewrite!("sub-0"; "(- ?a 0)" => "?a"),
        rewrite!("mul-1"; "(* ?a 1)" => "?a"),
        rewrite!("mul-0"; "(* ?a 0)" => "0"),
        rewrite!("div-1"; "(/ ?a 1)" => "?a"),
        rewrite!("sub-self"; "(- ?a ?a)" => "0"),
        rewrite!("div-self"; "(/ ?a ?a)" => "1"),
        rewrite!("add-same"; "(+ ?a ?a)" => "(* 2 ?a)"),
        rewrite!("sq-fold"; "(* ?a ?a)" => "(square ?a)"),
        rewrite!("sq-expand"; "(square ?a)" => "(* ?a ?a)"),
        rewrite!("neg-neg"; "(neg (neg ?a))" => "?a"),
        rewrite!("sub-to-negadd"; "(- 0 ?a)" => "(neg ?a)"),
        rewrite!("add-neg"; "(+ ?a (neg ?b))" => "(- ?a ?b)"),
        // Subtraction has no protected branch, so a−b == a+(−b) holds exactly.
        // Re-expressing it as a sum lets the commutativity/associativity/fold
        // rules float scattered constants together — e.g. the linear-scaling
        // wrapper a·(f − c) + b collapses its constant tail to a single term.
        rewrite!("sub-as-addneg"; "(- ?a ?b)" => "(+ ?a (neg ?b))"),
        // Factoring shrinks; distribution is its inverse. Offering both lets the
        // cost-based extractor pick whichever member is simpler.
        rewrite!("factor"; "(+ (* ?a ?b) (* ?a ?c))" => "(* ?a (+ ?b ?c))"),
        rewrite!("distribute"; "(* ?a (+ ?b ?c))" => "(+ (* ?a ?b) (* ?a ?c))"),
        // Idempotent compositions, all exact under the protected ops: square(a)≥0
        // so sqrt(square(a)) = |a|, and (sqrt(|a|))² = |a|; |·| and the sign of a
        // squared/abs'd argument are irrelevant.
        rewrite!("sqrt-of-square"; "(sqrt (square ?a))" => "(abs ?a)"),
        rewrite!("square-of-sqrt"; "(square (sqrt ?a))" => "(abs ?a)"),
        rewrite!("abs-of-abs"; "(abs (abs ?a))" => "(abs ?a)"),
        rewrite!("abs-of-neg"; "(abs (neg ?a))" => "(abs ?a)"),
        rewrite!("square-of-neg"; "(square (neg ?a))" => "(square ?a)"),
        rewrite!("square-of-abs"; "(square (abs ?a))" => "(square ?a)"),
        // Trig/tanh parity (exact): sin and tanh are odd, cos is even.
        rewrite!("sin-odd"; "(sin (neg ?a))" => "(neg (sin ?a))"),
        rewrite!("tanh-odd"; "(tanh (neg ?a))" => "(neg (tanh ?a))"),
        rewrite!("cos-even"; "(cos (neg ?a))" => "(cos ?a)"),
    ]
}

/// Extraction cost mirrors [`crate::gp::Expr::complexity`] so the canonical form
/// is "simplest" under the very metric the Pareto front reports.
struct OpCost;
impl CostFunction<MathLang> for OpCost {
    type Cost = f64;
    fn cost<F: FnMut(Id) -> Self::Cost>(&mut self, enode: &MathLang, mut child: F) -> Self::Cost {
        let weight = match enode {
            MathLang::Constant(_) | MathLang::Symbol(_) => 1.0,
            MathLang::Add(_) | MathLang::Sub(_) | MathLang::Mul(_) | MathLang::ENeg(_) => 1.0,
            MathLang::Div(_) => 2.0,
            MathLang::Square(_) | MathLang::Abs(_) => 2.0,
            MathLang::Sin(_) | MathLang::Cos(_) | MathLang::Sqrt(_) | MathLang::Tanh(_) => 3.0,
            MathLang::Exp(_) | MathLang::Log(_) => 4.0,
        };
        enode.fold(weight, |sum, id| sum + child(id))
    }
}

fn to_rec(e: &Expr, rec: &mut RecExpr<MathLang>) -> Id {
    match e {
        Expr::Const(c) => {
            let n = NotNan::new(*c).unwrap_or_else(|_| NotNan::new(0.0).unwrap());
            rec.add(MathLang::Constant(n))
        }
        Expr::Var(i) => rec.add(MathLang::Symbol(Symbol::from(format!("v{i}")))),
        Expr::Unary(op, a) => {
            let ca = to_rec(a, rec);
            rec.add(match op {
                Unary::Neg => MathLang::ENeg([ca]),
                Unary::Sin => MathLang::Sin([ca]),
                Unary::Cos => MathLang::Cos([ca]),
                Unary::Exp => MathLang::Exp([ca]),
                Unary::Log => MathLang::Log([ca]),
                Unary::Sqrt => MathLang::Sqrt([ca]),
                Unary::Square => MathLang::Square([ca]),
                Unary::Tanh => MathLang::Tanh([ca]),
                Unary::Abs => MathLang::Abs([ca]),
            })
        }
        Expr::Binary(op, a, b) => {
            let ca = to_rec(a, rec);
            let cb = to_rec(b, rec);
            rec.add(match op {
                Binary::Add => MathLang::Add([ca, cb]),
                Binary::Sub => MathLang::Sub([ca, cb]),
                Binary::Mul => MathLang::Mul([ca, cb]),
                Binary::Div => MathLang::Div([ca, cb]),
            })
        }
    }
}

fn from_rec(rec: &RecExpr<MathLang>, id: Id) -> Expr {
    let nodes = rec.as_ref();
    let node = &nodes[usize::from(id)];
    let unary = |op: Unary, a: &Id| Expr::Unary(op, Box::new(from_rec(rec, *a)));
    let binary = |op: Binary, a: &Id, b: &Id| {
        Expr::Binary(op, Box::new(from_rec(rec, *a)), Box::new(from_rec(rec, *b)))
    };
    match node {
        MathLang::Constant(c) => Expr::Const(c.into_inner()),
        MathLang::Symbol(s) => s
            .as_str()
            .strip_prefix('v')
            .and_then(|n| n.parse::<usize>().ok())
            .map(Expr::Var)
            .unwrap_or(Expr::Const(0.0)),
        MathLang::Add([a, b]) => binary(Binary::Add, a, b),
        MathLang::Sub([a, b]) => binary(Binary::Sub, a, b),
        MathLang::Mul([a, b]) => binary(Binary::Mul, a, b),
        MathLang::Div([a, b]) => binary(Binary::Div, a, b),
        MathLang::ENeg([a]) => unary(Unary::Neg, a),
        MathLang::Sin([a]) => unary(Unary::Sin, a),
        MathLang::Cos([a]) => unary(Unary::Cos, a),
        MathLang::Exp([a]) => unary(Unary::Exp, a),
        MathLang::Log([a]) => unary(Unary::Log, a),
        MathLang::Sqrt([a]) => unary(Unary::Sqrt, a),
        MathLang::Square([a]) => unary(Unary::Square, a),
        MathLang::Tanh([a]) => unary(Unary::Tanh, a),
        MathLang::Abs([a]) => unary(Unary::Abs, a),
    }
}

/// Canonicalise an expression via bounded equality saturation, returning a
/// value-equivalent but simpler tree. Total: returns the input unchanged on any
/// failure, an empty saturation, or a pathological size blow-up. `budget` bounds
/// the wall-clock time the e-graph may spend (in addition to fixed node and
/// iteration caps), so it always honours the surrounding fit deadline.
pub fn canonicalize(e: &Expr, budget: Duration) -> Expr {
    // A zero/elapsed budget means the caller is already over its deadline.
    if budget.is_zero() {
        return e.clone();
    }
    let mut rec = RecExpr::default();
    to_rec(e, &mut rec);

    let runner = Runner::default()
        .with_node_limit(20_000)
        .with_iter_limit(40)
        .with_time_limit(budget)
        .with_expr(&rec)
        .run(&rules());

    let Some(&root) = runner.roots.last() else {
        return e.clone();
    };
    let extractor = Extractor::new(&runner.egraph, OpCost);
    let (_cost, best) = extractor.find_best(root);
    if best.as_ref().is_empty() {
        return e.clone();
    }
    // `a * a` and `square(a)` tie on cost, so extraction may keep the product.
    // Prefer the squared form for display (and a smaller tree); it is exactly
    // equal under the protected ops (`square` evaluates as `v * v`).
    let out = fold_squares(&from_rec(&best, (best.as_ref().len() - 1).into()));

    // Defensive: extraction should only shrink, but never hand back something
    // wildly larger than we started with.
    if out.size() <= e.size().max(4) * 4 {
        out
    } else {
        e.clone()
    }
}

/// Structural equality on expression trees (constants compared by exact bits via
/// `to_bits`, so this is a pure syntactic match — good enough to spot `a * a`).
fn struct_eq(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Const(x), Expr::Const(y)) => x.to_bits() == y.to_bits(),
        (Expr::Var(i), Expr::Var(j)) => i == j,
        (Expr::Unary(oa, x), Expr::Unary(ob, y)) => oa == ob && struct_eq(x, y),
        (Expr::Binary(oa, x1, x2), Expr::Binary(ob, y1, y2)) => {
            oa == ob && struct_eq(x1, y1) && struct_eq(x2, y2)
        }
        _ => false,
    }
}

/// Rewrite every structural `a * a` into `square(a)` bottom-up. Value-identical
/// under the protected ops (`square` evaluates as `v * v`); purely a display /
/// size improvement applied after extraction.
fn fold_squares(e: &Expr) -> Expr {
    match e {
        Expr::Const(_) | Expr::Var(_) => e.clone(),
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(fold_squares(a))),
        Expr::Binary(Binary::Mul, a, b) => {
            let (fa, fb) = (fold_squares(a), fold_squares(b));
            if struct_eq(&fa, &fb) {
                Expr::Unary(Unary::Square, Box::new(fa))
            } else {
                Expr::Binary(Binary::Mul, Box::new(fa), Box::new(fb))
            }
        }
        Expr::Binary(op, a, b) => {
            Expr::Binary(*op, Box::new(fold_squares(a)), Box::new(fold_squares(b)))
        }
    }
}

/// Recognise a float as a clean analytic symbol — π, e, √2, φ, or a small
/// rational `p/q` — when it is within a relative tolerance. Display-only; returns
/// `None` when nothing matches (the caller then renders a plain decimal). Never
/// rewrites a plain integer (handled by the decimal formatter) and never matches
/// far from a known value, so it cannot quietly distort a reported equation.
pub fn recognize_constant(x: f64) -> Option<String> {
    if !x.is_finite() || x == 0.0 || x.fract() == 0.0 {
        return None;
    }
    let tol = 1e-6 * x.abs().max(1.0);
    let named: [(f64, &str); 11] = [
        (std::f64::consts::PI, "pi"),
        (std::f64::consts::E, "e"),
        (std::f64::consts::TAU, "2*pi"),
        (std::f64::consts::FRAC_PI_2, "pi/2"),
        (std::f64::consts::FRAC_1_PI, "1/pi"),
        (std::f64::consts::PI * std::f64::consts::PI, "pi^2"),
        (std::f64::consts::SQRT_2, "sqrt(2)"),
        (1.732_050_807_568_877_2, "sqrt(3)"),
        (2.236_067_977_499_79, "sqrt(5)"),
        (std::f64::consts::LN_2, "ln(2)"),
        (1.618_033_988_749_895, "phi"),
    ];
    for (val, name) in named {
        if (x - val).abs() <= tol {
            return Some(name.to_string());
        }
        if (x + val).abs() <= tol {
            return Some(format!("-{name}"));
        }
    }
    // Small rationals p/q with 2 ≤ q ≤ 12 (skip q that divides p — that's an
    // integer, already handled above).
    for q in 2..=12i64 {
        let p = (x * q as f64).round();
        if p.abs() < 1e6 && (x - p / q as f64).abs() <= tol {
            let p = p as i64;
            if p % q != 0 {
                return Some(format!("{p}/{q}"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> Duration {
        Duration::from_millis(200)
    }

    fn approx(a: f64, b: f64, t: f64) -> bool {
        (a - b).abs() <= t
    }

    fn mul(a: Expr, b: Expr) -> Expr {
        Expr::Binary(Binary::Mul, Box::new(a), Box::new(b))
    }
    fn add(a: Expr, b: Expr) -> Expr {
        Expr::Binary(Binary::Add, Box::new(a), Box::new(b))
    }

    #[test]
    fn folds_constants_and_shrinks() {
        // (2 + 3) * x  ->  5 * x  (value preserved, smaller tree)
        let e = mul(add(Expr::Const(2.0), Expr::Const(3.0)), Expr::Var(0));
        let c = canonicalize(&e, budget());
        assert!(approx(c.eval(&[4.0]), 20.0, 1e-9));
        assert!(c.size() < e.size());
    }

    #[test]
    fn factors_common_term() {
        // a*b + a*c  ->  a*(b + c): value-equivalent, no larger.
        let e = add(mul(Expr::Var(0), Expr::Var(1)), mul(Expr::Var(0), Expr::Var(2)));
        let c = canonicalize(&e, budget());
        let pt = [2.0, 3.0, 4.0];
        assert!(approx(c.eval(&pt), e.eval(&pt), 1e-9));
        assert!(c.size() <= e.size());
    }

    #[test]
    fn preserves_value_over_grid() {
        // sin(x) + 2*x^2 must stay value-equivalent everywhere after rewriting.
        let e = add(
            Expr::Unary(Unary::Sin, Box::new(Expr::Var(0))),
            mul(Expr::Const(2.0), Expr::Unary(Unary::Square, Box::new(Expr::Var(0)))),
        );
        let c = canonicalize(&e, budget());
        for i in 0..50 {
            let xv = -3.0 + i as f64 * 0.12;
            assert!(approx(c.eval(&[xv]), e.eval(&[xv]), 1e-6), "mismatch at x={xv}");
        }
    }

    #[test]
    fn sqrt_of_square_collapses_to_abs() {
        // sqrt(square(x)) == |x| exactly under the protected ops, and the tree
        // should shrink to abs(x).
        let e = Expr::Unary(
            Unary::Sqrt,
            Box::new(Expr::Unary(Unary::Square, Box::new(Expr::Var(0)))),
        );
        let c = canonicalize(&e, budget());
        for &xv in &[-3.0, -0.5, 0.5, 3.0] {
            assert!(approx(c.eval(&[xv]), xv.abs(), 1e-9), "mismatch at x={xv}");
        }
        assert!(c.size() <= 3, "did not collapse: size {}", c.size());
    }

    #[test]
    fn div_self_is_one_under_protected_semantics() {
        let e = Expr::Binary(Binary::Div, Box::new(Expr::Var(0)), Box::new(Expr::Var(0)));
        let c = canonicalize(&e, budget());
        assert!(approx(c.eval(&[7.0]), 1.0, 1e-9));
    }

    #[test]
    fn zero_budget_is_identity() {
        let e = mul(add(Expr::Const(2.0), Expr::Const(3.0)), Expr::Var(0));
        let c = canonicalize(&e, Duration::ZERO);
        assert_eq!(c.size(), e.size());
    }

    #[test]
    fn recognises_named_and_rational_constants() {
        assert_eq!(recognize_constant(3.14159265).as_deref(), Some("pi"));
        assert_eq!(recognize_constant(2.71828182).as_deref(), Some("e"));
        assert_eq!(recognize_constant(-3.14159265).as_deref(), Some("-pi"));
        assert_eq!(recognize_constant(0.333333333).as_deref(), Some("1/3"));
        // No false positives: a plain integer or an unremarkable value.
        assert_eq!(recognize_constant(3.0), None);
        assert_eq!(recognize_constant(4.137), None);
    }
}
