//! A fast first try for unsigned bit-vector queries: the same question in
//! integer arithmetic, where Z3 handles `*` and `/` together far better.
//!
//! Sound: each top-level conjunct `A` becomes `exact(A) -> A_int`, where
//! `exact(A)` says no operation in `A` wraps or divides by 0. A bit-vector
//! model makes every such implication true (if its operations are exact, the
//! integer values equal the bit-vector values), so integer UNSAT means
//! bit-vector UNSAT. Any other answer falls back to the bit-vector solver.

use std::collections::HashMap;

use z3::{
    DeclKind, Params, SatResult, Solver,
    ast::{Ast, Bool, Dynamic, Int},
};

/// Bounds the integer try; nonlinear integer problems can run without end.
const TIMEOUT_MS: u32 = 1000;

pub(crate) fn proves_unsat(solver: &Solver) -> bool {
    let mut conjuncts = Vec::new();
    for assertion in solver.get_assertions() {
        flatten_and(Dynamic::from_ast(&assertion), &mut conjuncts);
    }
    let mut lowering = Lowering::default();
    let mut implications = Vec::new();
    for conjunct in &conjuncts {
        lowering.exact.clear();
        lowering.memo.clear();
        let Some(Lowered::Bool(int)) = lowering.lower(conjunct) else {
            return false;
        };
        let exact = Bool::and(&lowering.exact);
        implications.push(exact.implies(&int));
    }
    let int_solver = Solver::new();
    let mut params = Params::new();
    params.set_u32("timeout", TIMEOUT_MS);
    int_solver.set_params(&params);
    for range in &lowering.ranges {
        int_solver.assert(range);
    }
    for implication in &implications {
        int_solver.assert(implication);
    }
    int_solver.check() == SatResult::Unsat
}

fn flatten_and(expr: Dynamic, out: &mut Vec<Dynamic>) {
    if expr.is_app() && expr.decl().kind() == DeclKind::And {
        for child in expr.children() {
            flatten_and(child, out);
        }
    } else {
        out.push(expr);
    }
}

#[derive(Clone)]
enum Lowered {
    Bool(Bool),
    /// An unsigned bit-vector value as an integer.
    Int(Int),
}

#[derive(Default)]
struct Lowering {
    /// Bit-vector constants as integers, by name.
    vars: HashMap<String, Int>,
    /// `0 <= var < 2^width` for each variable.
    ranges: Vec<Bool>,
    /// Exactness conditions of the conjunct being lowered.
    exact: Vec<Bool>,
    memo: HashMap<Dynamic, Lowered>,
}

impl Lowering {
    fn lower(&mut self, expr: &Dynamic) -> Option<Lowered> {
        if let Some(done) = self.memo.get(expr) {
            return Some(done.clone());
        }
        let lowered = self.lower_new(expr)?;
        self.memo.insert(expr.clone(), lowered.clone());
        Some(lowered)
    }

    fn lower_new(&mut self, expr: &Dynamic) -> Option<Lowered> {
        if !expr.is_app() {
            return None;
        }
        let kind = expr.decl().kind();
        if let Some(bv) = expr.as_bv() {
            let width = bv.get_size();
            if width > 63 {
                return None;
            }
            if kind == DeclKind::Bnum {
                return Some(Lowered::Int(Int::from_u64(bv.as_u64()?)));
            }
            if kind == DeclKind::Uninterpreted && expr.num_children() == 0 {
                let name = expr.decl().name();
                let var = match self.vars.get(&name) {
                    Some(var) => var.clone(),
                    None => {
                        let var = Int::new_const(format!("int!{name}"));
                        self.ranges.push(var.ge(&Int::from_u64(0)));
                        self.ranges.push(var.lt(&Int::from_u64(1 << width)));
                        self.vars.insert(name, var.clone());
                        var
                    }
                };
                return Some(Lowered::Int(var));
            }
            let max = Int::from_u64((1 << width) - 1);
            let args = self.ints(expr)?;
            let value = match kind {
                DeclKind::Badd => Int::add(&args),
                DeclKind::Bmul => Int::mul(&args),
                DeclKind::Bsub if args.len() == 2 => {
                    self.exact.push(args[0].ge(&args[1]));
                    return Some(Lowered::Int(Int::sub(&args)));
                }
                DeclKind::Budiv | DeclKind::BudivI if args.len() == 2 => {
                    self.exact.push(args[1].gt(&Int::from_u64(0)));
                    return Some(Lowered::Int(args[0].div(&args[1])));
                }
                DeclKind::Burem | DeclKind::BuremI if args.len() == 2 => {
                    self.exact.push(args[1].gt(&Int::from_u64(0)));
                    return Some(Lowered::Int(args[0].modulo(&args[1])));
                }
                DeclKind::Ite => {
                    let condition = self.boolean(&expr.nth_child(0)?)?;
                    let (Lowered::Int(then), Lowered::Int(other)) = (
                        self.lower(&expr.nth_child(1)?)?,
                        self.lower(&expr.nth_child(2)?)?,
                    ) else {
                        return None;
                    };
                    return Some(Lowered::Int(condition.ite(&then, &other)));
                }
                _ => return None,
            };
            self.exact.push(value.le(&max));
            return Some(Lowered::Int(value));
        }
        let expr_bool = expr.as_bool()?;
        let lowered = match kind {
            DeclKind::True | DeclKind::False => expr_bool,
            DeclKind::Uninterpreted if expr.num_children() == 0 => expr_bool,
            DeclKind::Not => self.boolean(&expr.nth_child(0)?)?.not(),
            DeclKind::And | DeclKind::Or => {
                let parts = expr
                    .children()
                    .iter()
                    .map(|child| self.boolean(child))
                    .collect::<Option<Vec<_>>>()?;
                if kind == DeclKind::And {
                    Bool::and(&parts)
                } else {
                    Bool::or(&parts)
                }
            }
            DeclKind::Implies | DeclKind::Iff | DeclKind::Xor => {
                let left = self.boolean(&expr.nth_child(0)?)?;
                let right = self.boolean(&expr.nth_child(1)?)?;
                match kind {
                    DeclKind::Implies => left.implies(&right),
                    DeclKind::Iff => left.iff(&right),
                    _ => left.xor(&right),
                }
            }
            DeclKind::Ite => {
                let condition = self.boolean(&expr.nth_child(0)?)?;
                let then = self.boolean(&expr.nth_child(1)?)?;
                let other = self.boolean(&expr.nth_child(2)?)?;
                condition.ite(&then, &other)
            }
            DeclKind::Eq | DeclKind::Distinct if expr.num_children() == 2 => {
                let equal = match (
                    self.lower(&expr.nth_child(0)?)?,
                    self.lower(&expr.nth_child(1)?)?,
                ) {
                    (Lowered::Int(left), Lowered::Int(right)) => left.eq(&right),
                    (Lowered::Bool(left), Lowered::Bool(right)) => left.iff(&right),
                    _ => return None,
                };
                if kind == DeclKind::Eq {
                    equal
                } else {
                    equal.not()
                }
            }
            DeclKind::Uleq | DeclKind::Ugeq | DeclKind::Ult | DeclKind::Ugt => {
                let args = self.ints(expr)?;
                let [left, right] = args.as_slice() else {
                    return None;
                };
                match kind {
                    DeclKind::Uleq => left.le(right),
                    DeclKind::Ugeq => left.ge(right),
                    DeclKind::Ult => left.lt(right),
                    _ => left.gt(right),
                }
            }
            _ => return None,
        };
        Some(Lowered::Bool(lowered))
    }

    fn boolean(&mut self, expr: &Dynamic) -> Option<Bool> {
        match self.lower(expr)? {
            Lowered::Bool(value) => Some(value),
            Lowered::Int(_) => None,
        }
    }

    fn ints(&mut self, expr: &Dynamic) -> Option<Vec<Int>> {
        expr.children()
            .iter()
            .map(|child| match self.lower(child)? {
                Lowered::Int(value) => Some(value),
                Lowered::Bool(_) => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3::ast::BV;

    fn bv(name: &str) -> BV {
        BV::new_const(name, 32)
    }

    #[test]
    fn proves_bounded_nonlinear_facts() {
        // up(up(t / p) / 100) * 100 * p >= t, the query that took 40 seconds as bit-vectors.
        let (t, p) = (bv("t"), bv("p"));
        let one = BV::from_u64(1, 32);
        let hundred = BV::from_u64(100, 32);
        let each = t.bvadd(&p).bvsub(&one).bvudiv(&p);
        let rounded = each
            .bvadd(&hundred)
            .bvsub(&one)
            .bvudiv(&hundred)
            .bvmul(&hundred);
        let solver = Solver::new();
        solver.assert(&t.bvule(&BV::from_u64(10_000_000, 32)));
        solver.assert(&p.bvuge(&one));
        solver.assert(&p.bvule(&BV::from_u64(50, 32)));
        solver.assert(&rounded.bvmul(&p).bvult(&t));
        assert!(proves_unsat(&solver));
    }

    #[test]
    fn never_hides_a_wrapping_counterexample() {
        // x * 2 < x holds only when the product wraps; integers alone would call it impossible.
        let x = bv("x");
        let solver = Solver::new();
        solver.assert(&x.bvmul(&BV::from_u64(2, 32)).bvult(&x));
        assert!(!proves_unsat(&solver));
        assert_eq!(solver.check(), SatResult::Sat);

        // 0 - 1 wraps to the largest value.
        let solver = Solver::new();
        solver.assert(&x.bvsub(&BV::from_u64(1, 32)).bvugt(&x));
        assert!(!proves_unsat(&solver));

        // Division by 0 is all ones for bit-vectors.
        let y = bv("y");
        let solver = Solver::new();
        solver.assert(&x.bvudiv(&y).bvugt(&x));
        assert!(!proves_unsat(&solver));
    }

    #[test]
    fn skips_operations_it_cannot_map() {
        let x = bv("x");
        let solver = Solver::new();
        solver.assert(&x.bvslt(&BV::from_u64(0, 32)));
        solver.assert(&x.bvsgt(&BV::from_u64(0, 32)));
        assert!(!proves_unsat(&solver));
    }
}
