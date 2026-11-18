//! `settle(owe)` rules: on random balances that add up to 0, the transfers
//! bring every balance to 0, only people who owe pay, nobody pays more than
//! they owe, and there are at most n - 1 transfers.

use axiom_parser::{SpecExpr, parse_module};
use axiom_verify::reference::{Value, eval_spec};

#[test]
fn settle_brings_every_balance_to_zero() {
    let n = 4;
    let module = parse_module(&format!(
        "page P\n  owe[{n}], sends[{}] = 0 can be negative\n  when \"Go\" is clicked\n    sends = settle(owe)\n",
        n * n
    ))
    .unwrap();
    let rule = |from: usize, to: usize| {
        module
            .contracts
            .iter()
            .find(|c| c.name == format!("settle_{n}_{from}_{to}"))
            .unwrap()
    };
    let mut seed = 7u64;
    let mut next = |limit: i64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as i64 % (2 * limit + 1)) - limit
    };
    for case in 0..2000 {
        let limit = if case % 2 == 0 { 5 } else { 100_000 };
        let mut owe: Vec<i64> = (0..n - 1).map(|_| next(limit)).collect();
        owe.push(-owe.iter().sum::<i64>());
        let args: Vec<u32> = owe.iter().map(|v| *v as i32 as u32).collect();
        let mut left = owe.clone();
        let mut transfers = 0;
        for from in 1..=n {
            for to in 1..=n {
                let contract = rule(from, to);
                let SpecExpr::Binary(_, _, value) = &contract.ensures[0] else {
                    panic!("ensures result == ...");
                };
                let amount = match eval_spec(value, contract, &args, 0).unwrap() {
                    Value::I32(amount) => i64::from(amount),
                    Value::U32(bits) => i64::from(bits as i32),
                    other => panic!("settle answers a number, got {other:?}"),
                };
                assert!(amount >= 0, "{owe:?}");
                if amount > 0 {
                    assert!(owe[from - 1] > 0 && owe[to - 1] < 0, "{owe:?} {from}->{to}");
                    transfers += 1;
                }
                left[from - 1] -= amount;
                left[to - 1] += amount;
            }
        }
        assert!(left.iter().all(|v| *v == 0), "{owe:?} left {left:?}");
        assert!(transfers < n, "{owe:?}: {transfers} transfers");
    }
}
