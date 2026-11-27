//! Files in folders (issue #225, M4): a `from` line resolves from the file
//! that writes it, also through a file it names, and a rule from another file
//! bounds its callers' arithmetic as it does inside one file.

use std::{fs, path::Path, process::Command};

fn project(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axiom-modules-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for (path, text) in files {
        let file = dir.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    }
    dir
}

fn check(dir: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("check")
        .current_dir(dir)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

const CONFIG: &str = "[project]\nentry = \"rules.ax\"\n";

#[test]
fn from_lines_resolve_from_each_folder_through_a_chain() {
    let dir = project(
        "chain",
        &[
            ("synthetix.toml", CONFIG),
            (
                "lib/money.ax",
                "type Order\n  price\n  quantity\n\nfee(price)\n  need price <= 10000\n  always -> price / 100\n",
            ),
            (
                "lib/tax.ax",
                "from money: fee, Order\n\nvat(price)\n  need price <= 10000\n  always -> price / 10\n\ncharge(order: Order)\n  need order.price <= 10000\n  always -> fee(order.price) + vat(order.price)\n",
            ),
            (
                "rules.ax",
                "from lib/tax: charge, Order\n\nbill(order: Order)\n  need order.price <= 10000\n  need order.quantity <= 100\n  always -> order.price * order.quantity + charge(order)\n",
            ),
        ],
    );
    let (ok, text) = check(&dir);
    assert!(ok, "{text}");
    assert!(text.contains("verified: bill"), "{text}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_imported_rule_bounds_the_callers_arithmetic_as_in_one_file() {
    let caller = (
        "rules.ax",
        "from money: fee\n\nvat(p)\n  need p <= 10000\n  always -> fee(p) + p\n",
    );
    // `fee` answers at most 10000 here, so `fee(p) + p` stays in range.
    let bounded = project(
        "bounded",
        &[
            ("synthetix.toml", CONFIG),
            (
                "money.ax",
                "fee(price)\n  need price <= 10000\n  always -> price\n",
            ),
            caller,
        ],
    );
    let (ok, text) = check(&bounded);
    assert!(ok, "{text}");
    fs::remove_dir_all(bounded).unwrap();

    // `fee` answers any number here, so `fee(p) + p` may leave the range.
    let unbounded = project(
        "unbounded",
        &[
            ("synthetix.toml", CONFIG),
            ("money.ax", "fee(price)\n  always -> price\n"),
            caller,
        ],
    );
    let (ok, text) = check(&unbounded);
    assert!(!ok, "{text}");
    assert!(text.contains("may go outside"), "{text}");
    fs::remove_dir_all(unbounded).unwrap();
}

#[test]
fn a_rule_answering_a_record_is_imported_field_by_field() {
    // `quote` is `quote__goods` and `quote__tax` in `lib.ax`; `pay` reads both
    // fields from another file, as it would in one file.
    let quote = "type Bill\n  goods\n  tax\n\nquote(price, qty) -> Bill\n  need price <= 10000\n  need qty <= 100\n  goods:\n    always -> price * qty\n    promise answer <= 1000000\n  tax:\n    always -> price * qty / 10\n    promise answer <= 100000\n";
    let pay = |bound: u32| {
        format!(
            "from lib: quote, Bill\n\npay(price, qty)\n  need price <= 10000\n  need qty <= 100\n  always -> quote(price, qty).goods + quote(price, qty).tax\n  promise answer <= {bound}\n"
        )
    };
    let proven = project(
        "record-ok",
        &[
            ("synthetix.toml", CONFIG),
            ("lib.ax", quote),
            ("rules.ax", &pay(1100000)),
        ],
    );
    let (ok, text) = check(&proven);
    assert!(ok, "{text}");
    assert!(text.contains("verified: pay"), "{text}");
    fs::remove_dir_all(proven).unwrap();

    // 10000 * 100 + 10000 * 100 / 10 = 1100000 is the most `pay` can give.
    let too_small = project(
        "record-bad",
        &[
            ("synthetix.toml", CONFIG),
            ("lib.ax", quote),
            ("rules.ax", &pay(1000)),
        ],
    );
    let (ok, text) = check(&too_small);
    assert!(!ok, "{text}");
    assert!(text.contains("does not hold"), "{text}");
    fs::remove_dir_all(too_small).unwrap();
}

#[test]
fn a_text_choice_from_another_folder_reaches_the_page() {
    // The page's text box holds the values of `Grade`, which `lib/grades.ax`
    // declares; the page passes each chosen text to `bonus`.
    let dir = project(
        "text-page",
        &[
            ("synthetix.toml", CONFIG),
            (
                "lib/grades.ax",
                "type Grade = \"A\" | \"B\" | \"C\"\n\nbonus(grade: Grade)\n  grade is \"A\" -> 10\n  grade is \"B\" -> 5\n  else -> 0\n  promise answer <= 10\n",
            ),
            (
                "rules.ax",
                "from lib/grades: bonus, Grade\n\npage Score\n  save grade = \"\"\n  result = 0\n\n  ask \"Grade\" for grade\n  show result\n  when \"Calc\" is clicked\n    result = bonus(grade)\n",
            ),
        ],
    );
    let out = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("build")
        .current_dir(&dir)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}");
    let js = fs::read_to_string(dir.join("dist/app.js")).unwrap();
    assert!(js.contains("\"B\""), "the page has the texts of Grade");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_import_can_take_another_name_for_rules_types_and_record_fields() {
    // `fee as price_fee`, `Order as Purchase` and `quote as q` rename what
    // this file uses; the other file keeps its names.
    let money = "type Order\n  price\n\nfee(order: Order)\n  need order.price <= 10000\n  always -> order.price / 10\n\nquote(price, qty) -> Bill\n  need price <= 10000\n  need qty <= 100\n  goods:\n    always -> price * qty\n    promise answer <= 1000000\n  tax:\n    always -> price * qty / 10\n    promise answer <= 100000\n\ntype Bill\n  goods\n  tax\n";
    let dir = project(
        "alias",
        &[
            ("synthetix.toml", CONFIG),
            ("money.ax", money),
            (
                "rules.ax",
                "from money: Order as Purchase, fee as price_fee, quote as q, Bill as Invoice\n\ntotal(o: Purchase)\n  need o.price <= 10000\n  always -> price_fee(o) + o.price\n  promise answer <= 11000\n\npay(price, qty)\n  need price <= 10000\n  need qty <= 100\n  always -> q(price, qty).goods + q(price, qty).tax\n  promise answer <= 1100000\n",
            ),
        ],
    );
    let (ok, text) = check(&dir);
    assert!(ok, "{text}");
    assert!(text.contains("verified: total"), "{text}");
    assert!(text.contains("verified: pay"), "{text}");
    fs::remove_dir_all(dir).unwrap();
}
