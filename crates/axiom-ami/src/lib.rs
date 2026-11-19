//! Axiom Machine IR (AMI) v0.1
//!
//! Minimal expression tree targeting modulo 2^32 bit-vector semantics, or
//! 2^64 for contracts with `wide` (64-bit) words.

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Expr {
    /// Function argument by index (e.g. arg 0 = x)
    Arg(u8),
    /// Unsigned constant (a 64-bit word in `wide` contracts)
    Const(u64),
    /// Addition with wrap-around (modulo 2^32)
    Add(Box<Expr>, Box<Expr>),
    /// Subtraction with wrap-around (modulo 2^32)
    Sub(Box<Expr>, Box<Expr>),
    /// Multiplication with wrap-around (modulo 2^32)
    Mul(Box<Expr>, Box<Expr>),
    /// Unsigned division rounding down; a zero divisor is an error
    Div(Box<Expr>, Box<Expr>),
    /// Unsigned remainder; a zero divisor is an error
    Rem(Box<Expr>, Box<Expr>),
    /// Signed division rounding toward 0; a zero divisor or i32::MIN / -1 is an error
    SDiv(Box<Expr>, Box<Expr>),
    /// Signed remainder (sign of the dividend); same errors as `SDiv`
    SRem(Box<Expr>, Box<Expr>),
    /// Left shift (masked by 31 in standard bit-vector / WASM semantics)
    Shl(Box<Expr>, Box<Expr>),
    /// Bitwise AND
    BitAnd(Box<Expr>, Box<Expr>),
    /// Bitwise OR
    BitOr(Box<Expr>, Box<Expr>),
    /// Bitwise XOR
    BitXor(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Le(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Ge(Box<Expr>, Box<Expr>),
    SLt(Box<Expr>, Box<Expr>),
    SLe(Box<Expr>, Box<Expr>),
    SGt(Box<Expr>, Box<Expr>),
    SGe(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Select {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    /// Reads one element from a flattened fixed-array value and traps if out of bounds.
    ArrayGet {
        elements: Vec<Expr>,
        index: Box<Expr>,
    },
    /// Evaluates `value` only when `condition` is true; otherwise traps.
    Guard {
        condition: Box<Expr>,
        value: Box<Expr>,
    },
}

impl Expr {
    /// Evaluates the expression given input arguments.
    /// All arithmetic operations enforce strict wrapping semantics (modulo 2^32).
    pub fn eval(&self, args: &[u32]) -> u32 {
        self.try_eval(args)
            .expect("invalid AMI expression evaluation")
    }

    /// 32-bit evaluation; see [`Expr::try_eval_bits`].
    pub fn try_eval(&self, args: &[u32]) -> Result<u32, String> {
        let args: Vec<u64> = args.iter().map(|arg| u64::from(*arg)).collect();
        self.try_eval_bits(&args, 32).map(|value| value as u32)
    }

    /// Evaluates with `bits`-wide words (32 or 64, `Contract::wide`): every
    /// result wraps modulo 2^bits, and signed operations read words as
    /// two's complement of that width.
    pub fn try_eval_bits(&self, args: &[u64], bits: u32) -> Result<u64, String> {
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let signed = |value: u64| -> i64 {
            let shift = 64 - bits;
            ((value << shift) as i64) >> shift
        };
        let eval = |expr: &Expr| expr.try_eval_bits(args, bits);
        let pair =
            |lhs: &Expr, rhs: &Expr| -> Result<(u64, u64), String> { Ok((eval(lhs)?, eval(rhs)?)) };
        let flag = |value: bool| u64::from(value);
        let signed_op = |lhs: &Expr, rhs: &Expr, op: fn(i64, i64) -> Option<i64>| {
            let (l, r) = pair(lhs, rhs)?;
            let (l, r) = (signed(l), signed(r));
            // i32::MIN / -1 (or i64::MIN / -1) does not fit and traps in WebAssembly.
            let min = signed(1u64 << (bits - 1));
            if r == 0 || (l == min && r == -1) {
                return Err("division by zero or overflow".to_string());
            }
            op(l, r)
                .map(|value| value as u64 & mask)
                .ok_or_else(|| "division by zero or overflow".to_string())
        };
        let value = match self {
            Expr::Arg(idx) => args
                .get(*idx as usize)
                .copied()
                .ok_or_else(|| format!("missing argument {idx}"))?,
            Expr::Const(val) => *val,
            Expr::Add(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l.wrapping_add(r)
            }
            Expr::Sub(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l.wrapping_sub(r)
            }
            Expr::Mul(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l.wrapping_mul(r)
            }
            Expr::Div(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l.checked_div(r)
                    .ok_or_else(|| "division by zero".to_string())?
            }
            Expr::Rem(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l.checked_rem(r)
                    .ok_or_else(|| "division by zero".to_string())?
            }
            Expr::SDiv(lhs, rhs) => signed_op(lhs, rhs, i64::checked_div)?,
            Expr::SRem(lhs, rhs) => signed_op(lhs, rhs, i64::checked_rem)?,
            Expr::Shl(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l.wrapping_shl((r & u64::from(bits - 1)) as u32)
            }
            Expr::BitAnd(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l & r
            }
            Expr::BitOr(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l | r
            }
            Expr::BitXor(lhs, rhs) => {
                let (l, r) = pair(lhs, rhs)?;
                l ^ r
            }
            Expr::Eq(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(l == r))?,
            Expr::Ne(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(l != r))?,
            Expr::Lt(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(l < r))?,
            Expr::Le(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(l <= r))?,
            Expr::Gt(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(l > r))?,
            Expr::Ge(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(l >= r))?,
            Expr::SLt(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(signed(l) < signed(r)))?,
            Expr::SLe(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(signed(l) <= signed(r)))?,
            Expr::SGt(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(signed(l) > signed(r)))?,
            Expr::SGe(lhs, rhs) => pair(lhs, rhs).map(|(l, r)| flag(signed(l) >= signed(r)))?,
            Expr::And(lhs, rhs) => flag(eval(lhs)? != 0 && eval(rhs)? != 0),
            Expr::Or(lhs, rhs) => flag(eval(lhs)? != 0 || eval(rhs)? != 0),
            Expr::Select {
                condition,
                then_branch,
                else_branch,
            } => {
                if eval(condition)? != 0 {
                    eval(then_branch)?
                } else {
                    eval(else_branch)?
                }
            }
            Expr::ArrayGet { elements, index } => {
                let index = eval(index)?;
                let element = usize::try_from(index)
                    .ok()
                    .and_then(|index| elements.get(index))
                    .ok_or_else(|| format!("fixed array index {index} is out of bounds"))?;
                eval(element)?
            }
            Expr::Guard { condition, value } => {
                if eval(condition)? == 0 {
                    return Err("verified runtime guard failed".to_string());
                }
                eval(value)?
            }
        };
        Ok(value & mask)
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
        assert_eq!(
            expr.try_eval_bits(&[u64::MAX], 64).unwrap(),
            u64::MAX.wrapping_mul(8)
        );
    }

    #[test]
    fn wide_words_wrap_and_divide_at_64_bits() {
        let add = Expr::Add(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1)));
        assert_eq!(
            add.try_eval_bits(&[u64::from(u32::MAX)], 64).unwrap(),
            1 << 32
        );
        assert_eq!(add.try_eval_bits(&[u64::MAX], 64).unwrap(), 0);
        let div = Expr::SDiv(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)));
        assert_eq!(
            div.try_eval_bits(&[-7i64 as u64, 2], 64).unwrap(),
            -3i64 as u64
        );
        assert!(div.try_eval_bits(&[i64::MIN as u64, u64::MAX], 64).is_err());
        assert!(div.try_eval(&[i32::MIN as u32, u32::MAX]).is_err());
        let lt = Expr::SLt(Box::new(Expr::Arg(0)), Box::new(Expr::Const(0)));
        assert_eq!(lt.try_eval_bits(&[u64::MAX], 64).unwrap(), 1);
        assert_eq!(lt.try_eval_bits(&[u64::from(u32::MAX)], 64).unwrap(), 0);
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

    #[test]
    fn evaluates_wrapping_subtraction_and_bitwise_ops() {
        let expr = Expr::BitXor(
            Box::new(Expr::BitAnd(Box::new(Expr::Arg(0)), Box::new(Expr::Arg(1)))),
            Box::new(Expr::Sub(Box::new(Expr::Arg(0)), Box::new(Expr::Const(1)))),
        );

        assert_eq!(expr.eval(&[0b1100, 0b1010]), 0b0011);
        assert_eq!(
            Expr::Sub(Box::new(Expr::Const(0)), Box::new(Expr::Const(1))).eval(&[]),
            u32::MAX
        );
    }
}
