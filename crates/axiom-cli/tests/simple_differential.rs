//! Compares simple-syntax rules, compiled to WASM, with hand-written oracles
//! on random and boundary inputs.

use std::{fs, path::PathBuf, time::Duration};

use axiom_parser::parse_module;
use axiom_synth::{SynthesisOptions, check_requires, synthesize_wasm_with_metrics};

const CASES_PER_RULE: usize = 100_000;

fn source(path: &str) -> String {
    fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path),
    )
    .unwrap()
}

// These oracles belong to the test, never to synthesis or generated code.
fn oracle(name: &str, a: &[u32]) -> u32 {
    match name {
        "base_amount" => a[0] * a[1],
        "items_total" => a.iter().sum(),
        "capped" => a[0].min(a[1]),
        "floor_cap" | "fit_price" => {
            if a[0] < a[1] {
                a[1]
            } else if a[0] > a[2] {
                a[2]
            } else {
                a[0]
            }
        }
        "gap" => a[0].abs_diff(a[1]),
        "discount" => a[0] - a[0] * a[1] / 100,
        "share" => a[0] / a[1],
        "clamp_mm" => a[0].min(a[2]).max(a[1]),
        "is_check" => u32::from(a[0] == 3 && a[1] != 4),
        "leftover" => {
            let left = a[0] - a[1];
            if left > 10 { left * 2 } else { left }
        }
        "change" => (a[0] as i32 - a[1] as i32) as u32,
        "leftover_share" => a[0] % a[1],
        "fair_share" => (a[0] + a[1] / 2) / a[1],
        "cart" => (a[0] + a[1] + a[2]).min(200_000),
        "spread" => a.iter().max().unwrap() - a.iter().min().unwrap(),
        "first" => a[0],
        // `with N decimals`: values are stored times 10^N.
        "vat" => a[0] * 10 / 100,
        "per" => a[0] * 10 / a[1],
        "over" => {
            if a[0] > 999 {
                100
            } else {
                0
            }
        }
        // Signed rules: inputs and answers are i32 bits.
        "floor_at" => (a[0] as i32).max(-100) as u32,
        "split" => ((a[0] as i32) / (a[1] as i32)) as u32,
        "left" => ((a[0] as i32) % (a[1] as i32)) as u32,
        "drift" => ((a[0] as i32) * -150 / 100) as u32,
        // Text answers: the position of the text.
        "grade" => match a[0] {
            90.. => 0,
            80.. => 1,
            _ => 2,
        },
        _ => panic!("no oracle for {name}"),
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }

    /// Mostly small values and edges, so `need` bounds are usually met.
    fn value(&mut self) -> u32 {
        const EDGES: [u32; 8] = [0, 1, 2, 99, 100, 250_000, 1_000_000, u32::MAX];
        match self.next() % 5 {
            0 => EDGES[(self.next() % 8) as usize],
            1 => self.next() % 128,
            2 => self.next() % 1_000_001,
            3 => self.next() % 10_001,
            // Negative values (as i32 bits) for `can be negative` rules.
            _ => ((self.next() % 200_001) as i32 - 100_000) as u32,
        }
    }
}

fn check_file(path: &str, text: &str) {
    let module = parse_module(text).unwrap();
    let mut rng = Rng(0x5eed);
    for contract in &module.contracts {
        let options = SynthesisOptions {
            timeout: Some(Duration::from_secs(20)),
            ..SynthesisOptions::default()
        };
        let (_, wasm, _, _) = synthesize_wasm_with_metrics(&contract, options)
            .unwrap_or_else(|error| panic!("{path} {}: {error}", contract.name));
        let mut accepted = 0;
        let mut tries = 0;
        while accepted < CASES_PER_RULE {
            tries += 1;
            assert!(
                tries < CASES_PER_RULE * 50,
                "{}: too few valid inputs",
                contract.name
            );
            let args: Vec<u32> = contract.params.iter().map(|_| rng.value()).collect();
            if check_requires(contract, &args).is_err() {
                continue;
            }
            accepted += 1;
            let got = axiom_wasm::run_with_args(&wasm, &args).unwrap();
            assert_eq!(
                got,
                oracle(&contract.name, &args),
                "{path} {} {args:?}",
                contract.name
            );
        }
    }
}

#[test]
fn policy_calculator_rules_match_oracles() {
    let path = "examples/policy-calculator/policy.ax";
    check_file(path, &source(path));
}

#[test]
fn fit_price_matches_oracle() {
    let path = "examples/rules/fit_price.ax";
    check_file(path, &source(path));
}

#[test]
fn negatives_mod_round_and_lists_match_oracles() {
    let text = "\
change(now, before) can be negative
  need now <= 1000000 and now >= 0
  need before <= 1000000 and before >= 0
  -> now - before

leftover_share(total, people)
  need people >= 1
  -> total mod people

fair_share(total, people)
  need total <= 1000000
  need people >= 1
  -> round(total / people)

cart(prices[3])
  need prices <= 100000
  sum(prices) > 200000 -> 200000
  -> sum(prices)

spread(prices[3])
  -> max(prices) - min(prices)

first(prices[3])
  -> prices[1]

vat(price) with 2 decimals
  need price <= 100000
  -> price * 0.1

per(total, people) with 1 decimal
  need total <= 100000
  need people >= 1
  -> total / people

over(price) with 2 decimals
  price > 9.99 -> 1
  -> 0
";
    check_file("inline-new", text);
}

#[test]
fn negative_division_text_and_signed_decimals_match_oracles() {
    let text = "\
floor_at(x) can be negative
  need x >= -1000 and x <= 1000
  x < -100 -> -100
  -> x

split(total, people) can be negative
  need total >= -100000 and total <= 100000
  need people >= 1 and people <= 100
  -> total / people

left(total, people) can be negative
  need total >= -100000 and total <= 100000
  need people >= 1 and people <= 100
  -> total mod people

drift(price) can be negative with 2 decimals
  need price >= -1000 and price <= 1000
  -> price * -1.5

grade(score)
  score >= 90 -> \"A\"
  score >= 80 -> \"B\"
  -> \"C\"
";
    check_file("inline-signed", text);
}

#[test]
fn subtraction_and_is_match_oracles() {
    let text = "\
gap(a, b)
  a > b -> a - b
  -> b - a

discount(price, rate)
  need price <= 1000000
  need rate <= 100
  -> price - price * rate / 100

share(total, people)
  need people >= 1
  -> total / people

clamp_mm(t, lo, hi)
  need lo <= hi
  -> max(lo, min(t, hi))

is_check(a, b)
  a is 3 and b is not 4 -> 1
  -> 0

leftover(total, used)
  need used <= total
  need total <= 1000000
  left = total - used
  left > 10 -> left * 2
  -> left
";
    check_file("inline", text);
}
