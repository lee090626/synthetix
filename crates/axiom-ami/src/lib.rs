//! Axiom Machine IR (AMI) v0.1
//!
//! Minimal expression tree targeting modulo 2^32 bit-vector semantics.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    /// Function argument by index (e.g. arg 0 = x)
    Arg(u8),
    /// 32-bit unsigned constant
    Const(u32),
    /// Addition with wrap-around (modulo 2^32)
    Add(Box<Expr>, Box<Expr>),
    /// Multiplication with wrap-around (modulo 2^32)
    Mul(Box<Expr>, Box<Expr>),
    /// Left shift (masked by 31 in standard bit-vector / WASM semantics)
    Shl(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Select {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
}

impl Expr {
    /// Evaluates the expression given input arguments.
    /// All arithmetic operations enforce strict wrapping semantics (modulo 2^32).
    pub fn eval(&self, args: &[u32]) -> u32 {
        match self {
            Expr::Arg(idx) => args[*idx as usize],
            Expr::Const(val) => *val,
            Expr::Add(lhs, rhs) => lhs.eval(args).wrapping_add(rhs.eval(args)),
            Expr::Mul(lhs, rhs) => lhs.eval(args).wrapping_mul(rhs.eval(args)),
            Expr::Shl(lhs, rhs) => {
                let shift = rhs.eval(args) & 31;
                lhs.eval(args).wrapping_shl(shift)
            }
            Expr::Eq(lhs, rhs) => u32::from(lhs.eval(args) == rhs.eval(args)),
            Expr::Ne(lhs, rhs) => u32::from(lhs.eval(args) != rhs.eval(args)),
            Expr::Lt(lhs, rhs) => u32::from(lhs.eval(args) < rhs.eval(args)),
            Expr::Le(lhs, rhs) => u32::from(lhs.eval(args) <= rhs.eval(args)),
            Expr::Gt(lhs, rhs) => u32::from(lhs.eval(args) > rhs.eval(args)),
            Expr::Ge(lhs, rhs) => u32::from(lhs.eval(args) >= rhs.eval(args)),
            Expr::And(lhs, rhs) => u32::from(lhs.eval(args) != 0 && rhs.eval(args) != 0),
            Expr::Or(lhs, rhs) => u32::from(lhs.eval(args) != 0 || rhs.eval(args) != 0),
            Expr::Select {
                condition,
                then_branch,
                else_branch,
            } => {
                if condition.eval(args) != 0 {
                    then_branch.eval(args)
                } else {
                    else_branch.eval(args)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul8_candidate() {
        // x << 3
        let expr = Expr::Shl(Box::new(Expr::Arg(0)), Box::new(Expr::Const(3)));

        assert_eq!(expr.eval(&[0]), 0);
        assert_eq!(expr.eval(&[1]), 8);
        assert_eq!(expr.eval(&[2]), 16);
        assert_eq!(expr.eval(&[42]), 336);

        // Test modulo 2^32 wrapping behavior
        assert_eq!(expr.eval(&[u32::MAX]), u32::MAX.wrapping_mul(8));
    }

    #[test]
    fn evaluates_conditional_expression() {
        let expr = Expr::Select {
            condition: Box::new(Expr::Lt(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
            then_branch: Box::new(Expr::Arg(0)),
            else_branch: Box::new(Expr::Arg(1)),
        };

        assert_eq!(expr.eval(&[2, 5]), 2);
        assert_eq!(expr.eval(&[8, 5]), 5);
    }
}
