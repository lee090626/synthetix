use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime},
};

use axiom_ami::Expr;
use axiom_parser::{Contract, parse};
use axiom_synth::{SynthesisOptions, check_requires, lower_requires, synthesize_wasm_with_metrics};
use axiom_verify::{PreconditionResult, ProofResult, check_preconditions, reference, verify};
use serde_json::{Value, json};

struct Product {
    contract: Contract,
    candidate: Expr,
    wasm: Vec<u8>,
    guard: Expr,
    guard_wasm: Vec<u8>,
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/first-product")
        .join(format!("{name}.ax"))
}

fn compile(name: &str) -> Product {
    let contract = parse(&fs::read_to_string(fixture(name)).unwrap()).unwrap();
    assert_eq!(
        check_preconditions(&contract).unwrap(),
        PreconditionResult::Satisfiable,
        "{name}: valid domain"
    );
    let options = SynthesisOptions {
        timeout: Some(Duration::from_secs(10)),
        ..SynthesisOptions::default()
    };
    let (candidate, wasm, _, _) = synthesize_wasm_with_metrics(&contract, options).unwrap();
    assert_eq!(
        verify(&contract, &candidate).unwrap(),
        ProofResult::Verified,
        "{name}: source proof"
    );
    let guard = lower_requires(&contract).unwrap().unwrap();
    let (_, arity) = axiom_parser::verified_abi_layout(&contract.params).unwrap();
    let guard_wasm = axiom_wasm::emit(&guard, arity as u32).unwrap();
    Product {
        contract,
        candidate,
        wasm,
        guard,
        guard_wasm,
    }
}

fn assert_pass(row: Value) {
    assert_eq!(row["status"], "PASS", "differential failure: {row}");
}

// This oracle belongs to the test, never to synthesis or generated app code.
fn expected(name: &str, args: &[u32]) -> u32 {
    match name {
        "total" => args[0] * args[1],
        "capped" => args[0].min(args[1]),
        "floor_cap" => args[0].clamp(args[1], args[2]),
        "aggregate" => args.iter().sum(),
        _ => panic!("unknown fixture {name}"),
    }
}

fn outputs(product: &Product, args: &[u32]) -> [u32; 3] {
    check_requires(&product.contract, args).expect("input admission before core execution");
    [
        reference::eval_ami(&product.candidate, args).expect("Reference execution"),
        product.candidate.try_eval(args).expect("AMI execution"),
        axiom_wasm::run_with_args(&product.wasm, args).expect("WASM execution"),
    ]
}

fn holds(product: &Product, args: &[u32], results: &[u32]) -> bool {
    results.iter().all(|result| {
        product
            .contract
            .requires
            .iter()
            .chain(&product.contract.ensures)
            .all(|clause| {
                reference::eval_spec(clause, &product.contract, args, *result)
                    == Ok(reference::Value::Bool(true))
            })
    })
}

fn valid_row(
    name: &str,
    label: &str,
    index: usize,
    product: &Product,
    args: &[u32],
    expected: u32,
) {
    let actual = outputs(product, args);
    let guards = [
        reference::eval_ami(&product.guard, args).unwrap(),
        product.guard.try_eval(args).unwrap(),
        axiom_wasm::run_with_args(&product.guard_wasm, args).unwrap(),
    ];
    let contract_holds = holds(product, args, &actual);
    let mut failures = Vec::new();
    for (stage, value) in ["reference", "ami", "wasm"].iter().zip(actual) {
        if value != expected {
            failures.push(format!(
                "{stage} output differs from independent expectation"
            ));
        }
    }
    if guards != [1; 3] {
        failures.push("precondition guard mismatch".into());
    }
    if !contract_holds {
        failures.push("original contract false or undefined".into());
    }
    assert_pass(
        json!({"class":"valid","case":format!("{name}-{label}-{index}"),"fixture":name,
        "input_class":label,"inputs":args,"expected":expected,"reference":actual[0],"ami":actual[1],"wasm":actual[2],
        "precondition_guards":guards,"contract_holds":contract_holds,"failures":failures,
        "status":if failures.is_empty() {"PASS"} else {"FAIL"}}),
    );
}

fn rejected_row(name: &str, index: usize, product: &Product, args: &[u32]) {
    let (_, arity) = axiom_parser::verified_abi_layout(&product.contract.params).unwrap();
    let admission = check_requires(&product.contract, args);
    let (guards, reference_rejected, wasm_arity_error) = if args.len() == arity {
        let allowed = product.contract.requires.iter().all(|clause| {
            reference::eval_spec(clause, &product.contract, args, 0)
                == Ok(reference::Value::Bool(true))
        });
        let guards = [
            reference::eval_ami(&product.guard, args).unwrap(),
            product.guard.try_eval(args).unwrap(),
            axiom_wasm::run_with_args(&product.guard_wasm, args).unwrap(),
        ];
        (Some(guards), !allowed, None)
    } else {
        let error = axiom_wasm::run_with_args(&product.guard_wasm, args).unwrap_err();
        let reference_rejected =
            reference::eval_spec(&product.contract.requires[0], &product.contract, args, 0)
                .is_err();
        (None, reference_rejected, Some(error))
    };
    let pass =
        admission.is_err() && reference_rejected && guards.is_none_or(|values| values == [0; 3]);
    assert_pass(
        json!({"class":"rejected","case":format!("{name}-reject-{index}"),"fixture":name,"inputs":args,
        "reason":if args.len()==arity {"PRECONDITION"} else {"ARITY"},"admission_error":admission.err(),
        "reference_rejected":reference_rejected,"precondition_guards":guards,"wasm_arity_error":wasm_arity_error,
        "core_executed":false,"status":if pass {"PASS"} else {"FAIL"}}),
    );
}

#[test]
fn product_differential() {
    let products: BTreeMap<_, _> = ["total", "capped", "floor_cap", "aggregate"]
        .into_iter()
        .map(|name| (name, compile(name)))
        .collect();
    let gold: BTreeMap<String, Vec<(Vec<u32>, u32)>> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/first-product/expected.json"
    ))
    .unwrap();
    for (name, product) in &products {
        for (index, (args, output)) in gold[*name].iter().enumerate() {
            assert_eq!(
                expected(name, args),
                *output,
                "test oracle disagrees with frozen gold {name}"
            );
            valid_row(name, "gold", index, product, args, *output);
        }
        let mut boundary = Vec::new();
        match *name {
            "total" => {
                for price in [0, 1, 9999, 10000] {
                    for quantity in [0, 1, 99, 100] {
                        boundary.push(vec![price, quantity]);
                    }
                }
            }
            "capped" => {
                for total in [0, 1, 999999, 1000000] {
                    for cap in [0, 1, 999999, 1000000] {
                        boundary.push(vec![total, cap]);
                    }
                }
            }
            "floor_cap" => {
                for total in [0, 1, 499999, 500000, 500001, 999999, 1000000] {
                    for (floor, cap) in [
                        (0, 0),
                        (0, 1000000),
                        (1, 1),
                        (499999, 500001),
                        (1000000, 1000000),
                    ] {
                        boundary.push(vec![total, floor, cap]);
                    }
                }
            }
            "aggregate" => {
                boundary.extend([vec![0; 4], vec![250000; 4]]);
                for index in 0..4 {
                    let mut args = vec![0; 4];
                    args[index] = 250000;
                    boundary.push(args);
                }
            }
            _ => unreachable!(),
        }
        for (index, args) in boundary.iter().enumerate() {
            valid_row(name, "boundary", index, product, args, expected(name, args));
        }
        for seed in 0..32u32 {
            let a = seed.wrapping_mul(0x9e3779b9);
            let b = a.rotate_left(13);
            let args = match *name {
                "total" => vec![a % 10001, b % 101],
                "capped" => vec![a % 1000001, b % 1000001],
                "floor_cap" => vec![
                    a % 1000001,
                    (b % 1000001).min(seed * 7919),
                    (b % 1000001).max(seed * 7919),
                ],
                "aggregate" => (0..4).map(|i| a.rotate_left(i * 7) % 250001).collect(),
                _ => unreachable!(),
            };
            valid_row(
                name,
                "deterministic",
                seed as usize,
                product,
                &args,
                expected(name, &args),
            );
        }
        let rejects = match *name {
            "total" => vec![
                vec![10001, 1],
                vec![1, 101],
                vec![u32::MAX, 1],
                vec![1, u32::MAX],
                vec![1],
                vec![1, 1, 1],
            ],
            "capped" => vec![
                vec![1000001, 0],
                vec![0, 1000001],
                vec![u32::MAX, 1],
                vec![1],
                vec![1, 1, 1],
            ],
            "floor_cap" => vec![
                vec![0, 81, 80],
                vec![1000001, 0, 1000000],
                vec![0, 0, 1000001],
                vec![u32::MAX, 0, 1000000],
                vec![1, 0],
                vec![1, 0, 1, 1],
            ],
            "aggregate" => vec![
                vec![250001, 0, 0, 0],
                vec![0, 250001, 0, 0],
                vec![0, 0, 250001, 0],
                vec![0, 0, 0, 250001],
                vec![u32::MAX, 0, 0, 0],
                vec![0; 3],
                vec![0; 5],
            ],
            _ => unreachable!(),
        };
        for (index, args) in rejects.iter().enumerate() {
            rejected_row(name, index, product, args);
        }
    }
    let aggregate = &products["aggregate"];
    let capped = &products["capped"];
    let changed = &products["floor_cap"];
    for (index, args) in [vec![0; 4], vec![10, 20, 30, 40], vec![250000; 4]]
        .iter()
        .enumerate()
    {
        let intermediate = outputs(aggregate, args);
        for cap in [0, 80, 1000000] {
            let admitted = intermediate
                .iter()
                .all(|value| check_requires(&capped.contract, &[*value, cap]).is_ok());
            assert!(
                admitted,
                "aggregate-cap-{index}-{cap}: intermediate input rejected"
            );
            let final_values = [
                reference::eval_ami(&capped.candidate, &[intermediate[0], cap]).unwrap(),
                capped.candidate.try_eval(&[intermediate[1], cap]).unwrap(),
                axiom_wasm::run_with_args(&capped.wasm, &[intermediate[2], cap]).unwrap(),
            ];
            let expected = args.iter().sum::<u32>().min(cap);
            let contract_holds = holds(aggregate, args, &intermediate)
                && intermediate
                    .iter()
                    .zip(final_values)
                    .all(|(sum, result)| holds(capped, &[*sum, cap], &[result]));
            assert_pass(
                json!({"class":"pipeline","case":format!("aggregate-cap-{index}-{cap}"),"inputs":args,"cap":cap,
                "aggregate_outputs":intermediate,"reference":final_values[0],"ami":final_values[1],"wasm":final_values[2],
                "expected":expected,"contract_holds":contract_holds,"status":if admitted&&contract_holds&&final_values==[expected;3] {"PASS"} else {"FAIL"}}),
            );
        }
    }
    for total in [0, 1, 19, 20, 21, 79, 80, 81, 100, 1000000] {
        let before = outputs(capped, &[total, 80]);
        let after = outputs(changed, &[total, 20, 80]);
        let pass = before == [total.min(80); 3]
            && after == [total.clamp(20, 80); 3]
            && holds(capped, &[total, 80], &before)
            && holds(changed, &[total, 20, 80], &after);
        assert_pass(
            json!({"class":"policy_change","case":format!("cap-to-floor-{total}"),
            "old_inputs":[total,80],"new_inputs":[total,20,80],"before":before,"after":after,
            "status":if pass {"PASS"} else {"FAIL"}}),
        );
    }
    let old_inputs = [100, 80];
    let admission_error = check_requires(&changed.contract, &old_inputs).unwrap_err();
    let wasm_error = axiom_wasm::run_with_args(&changed.wasm, &old_inputs).unwrap_err();
    assert_pass(
        json!({"class":"abi_change","case":"two-input-call-rejected-by-three-input-policy",
        "old_inputs":old_inputs,"new_arity":3,"admission_error":admission_error,"wasm_error":wasm_error,"status":"PASS"}),
    );

    let root = std::env::temp_dir().join(format!(
        "synthetix-product-types-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    for (index, value) in ["-1", "4294967296", "2.5", "oops"].iter().enumerate() {
        let wasm = root.join(format!("{index}.wasm"));
        let result = Command::new(env!("CARGO_BIN_EXE_axiom"))
            .args([
                "run",
                fixture("total").to_str().unwrap(),
                "-o",
                wasm.to_str().unwrap(),
                "--",
                value,
                "1",
            ])
            .output()
            .unwrap();
        let error = String::from_utf8(result.stderr).unwrap();
        assert_pass(
            json!({"class":"type_rejected","case":format!("cli-type-{index}"),"inputs":[value,"1"],
            "error":error,"wasm_created":wasm.exists(),"status":if !result.status.success()&&!wasm.exists() {"PASS"} else {"FAIL"}}),
        );
    }
    fs::remove_dir_all(root).unwrap();
}
