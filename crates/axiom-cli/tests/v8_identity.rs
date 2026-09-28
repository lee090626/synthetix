use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use axiom_project::{Project, load};
use serde_json::{Value, json};

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
const FIXTURES: &str = include_str!("../../../benchmarks/v8/identity-fixtures.json");

struct TempProject(PathBuf);

impl TempProject {
    fn from_fixture(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "synthetix-v8-identity-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        let fixture: Value = serde_json::from_str(FIXTURES).unwrap();
        let files = fixture[name].as_object().unwrap();
        for (relative, contents) in files {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents.as_str().unwrap()).unwrap();
        }
        fs::write(
            root.join("synthetix.toml"),
            "[project]\nentry = \"entry.ax\"\ntarget = \"web\"\n",
        )
        .unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ids(project: &Project) -> Vec<String> {
    let mut values = project
        .contracts
        .iter()
        .map(contract_id)
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn contract_id(linked: &axiom_project::LinkedContract) -> String {
    format!("{}::{}", linked.source.display(), linked.contract.name)
}

fn ids_named(project: &Project, name: &str) -> Vec<String> {
    let mut values = project
        .contracts
        .iter()
        .filter(|linked| linked.contract.name == name)
        .map(contract_id)
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn file_paths(project: &Project) -> Vec<String> {
    project
        .files
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

fn order(project: &Project) -> Vec<String> {
    project
        .verification_order
        .iter()
        .map(|id| format!("{}::{}", id.source.display(), id.name))
        .collect()
}

fn emit(case: &str, expected: Value, observed: Value) {
    let row = json!({
        "case": case,
        "status": if expected == observed { "PASS" } else { "FAIL" },
        "expected": expected,
        "observed": observed,
    });
    println!("V8_IDENTITY_EVIDENCE={row}");
    assert_eq!(row["expected"], row["observed"], "identity case {case}");
}

fn import_error(root: &TempProject) -> String {
    load(root.path("synthetix.toml")).unwrap_err()
}

fn local_bindings(project: &Project, entry: &Path) -> Vec<(String, String)> {
    let mut bindings = project
        .verification_callee_ids
        .get(entry)
        .unwrap()
        .iter()
        .map(|(name, id)| {
            (
                name.clone(),
                format!("{}::{}", id.source.display(), id.name),
            )
        })
        .collect::<Vec<_>>();
    bindings.sort();
    bindings
}

#[test]
fn v8_1_project_identity_evidence() {
    let aliases = TempProject::from_fixture("alias_shared");
    let alias_project = load(aliases.path("synthetix.toml")).unwrap();
    let shared = fs::canonicalize(aliases.path("shared.ax")).unwrap();
    let entry = fs::canonicalize(aliases.path("entry.ax")).unwrap();
    let left = fs::canonicalize(aliases.path("left.ax")).unwrap();
    let right = fs::canonicalize(aliases.path("right.ax")).unwrap();
    let expected_alias_ids = vec![
        format!("{}::first", shared.display()),
        format!("{}::second", shared.display()),
        format!("{}::left", left.display()),
        format!("{}::right", right.display()),
        format!("{}::root", entry.display()),
    ];
    let mut expected_alias_ids = expected_alias_ids;
    expected_alias_ids.sort();
    let mut expected_alias_files = vec![
        entry.display().to_string(),
        shared.display().to_string(),
        left.display().to_string(),
        right.display().to_string(),
    ];
    expected_alias_files.sort();
    let mut observed_alias_files = file_paths(&alias_project);
    observed_alias_files.sort();
    let mut expected_entry_bindings = vec![
        ("first".to_string(), format!("{}::first", shared.display())),
        ("left".to_string(), format!("{}::left", left.display())),
        ("root".to_string(), format!("{}::root", entry.display())),
        ("right".to_string(), format!("{}::right", right.display())),
        (
            "second".to_string(),
            format!("{}::second", shared.display()),
        ),
    ];
    expected_entry_bindings.sort();
    emit(
        "canonical_alias_shared_dependency_and_disjoint_imports",
        json!({"contract_ids": expected_alias_ids, "files": expected_alias_files, "entry_bindings": expected_entry_bindings}),
        json!({"contract_ids": ids(&alias_project), "files": observed_alias_files, "entry_bindings": local_bindings(&alias_project, &entry)}),
    );

    let same_name = TempProject::from_fixture("same_name");
    let same_project = load(same_name.path("synthetix.toml")).unwrap();
    let mut expected_same_ids = vec![
        format!(
            "{}::same_name",
            fs::canonicalize(same_name.path("a.ax")).unwrap().display()
        ),
        format!(
            "{}::same_name",
            fs::canonicalize(same_name.path("b.ax")).unwrap().display()
        ),
    ];
    expected_same_ids.sort();
    let observed_same_ids = ids_named(&same_project, "same_name");
    emit(
        "same_name_contracts_have_distinct_project_ids",
        json!({"ids": expected_same_ids}),
        json!({"ids": observed_same_ids}),
    );
    let web_build = Command::new(env!("CARGO_BIN_EXE_axiom"))
        .arg("build")
        .current_dir(&same_name.0)
        .output()
        .unwrap();
    let web_error = String::from_utf8_lossy(&web_build.stderr)
        .lines()
        .next()
        .unwrap_or_default()
        .strip_prefix("error: ")
        .unwrap_or_default()
        .to_string();
    let expected_web_error = format!(
        "duplicate web function same_name: {} and {}",
        fs::canonicalize(same_name.path("a.ax")).unwrap().display(),
        fs::canonicalize(same_name.path("b.ax")).unwrap().display()
    );
    emit(
        "web_duplicate_name_rejection_preserved",
        json!({"exit_success": false, "diagnostic": expected_web_error}),
        json!({"exit_success": web_build.status.success(), "diagnostic": web_error}),
    );

    let renamed = TempProject::from_fixture("rename");
    let before = load(renamed.path("synthetix.toml")).unwrap();
    let old_id = ids_named(&before, "renamed").into_iter().next().unwrap();
    fs::rename(renamed.path("old.ax"), renamed.path("new.ax")).unwrap();
    let entry_source = fs::read_to_string(renamed.path("entry.ax")).unwrap();
    fs::write(
        renamed.path("entry.ax"),
        entry_source.replace("./old.ax", "./new.ax"),
    )
    .unwrap();
    let after = load(renamed.path("synthetix.toml")).unwrap();
    let new_id = format!(
        "{}::renamed",
        fs::canonicalize(renamed.path("new.ax")).unwrap().display()
    );
    let observed_after_ids = ids_named(&after, "renamed");
    emit(
        "fresh_graph_rename_replaces_path_identity",
        json!({"before_id": old_id, "after_id": new_id, "old_identity_absent": true}),
        json!({"before_id": old_id, "after_id": observed_after_ids.first(), "old_identity_absent": !after.files.contains(&renamed.path("old.ax")) && !observed_after_ids.contains(&old_id)}),
    );

    let duplicate = TempProject::from_fixture("duplicate_binding");
    let duplicate_expected = format!(
        "{}:2: duplicate imported name: first",
        fs::canonicalize(duplicate.path("entry.ax"))
            .unwrap()
            .display()
    );
    let duplicate_observed = import_error(&duplicate);
    let duplicate_repeated = import_error(&duplicate);
    emit(
        "duplicate_local_binding_fails_deterministically",
        json!({"diagnostic": duplicate_expected, "repeat_equal": true}),
        json!({"diagnostic": duplicate_observed, "repeat_equal": duplicate_repeated == duplicate_observed}),
    );

    let cycle = TempProject::from_fixture("import_cycle");
    let cycle_expected = format!(
        "circular import: {} -> {} -> {} -> {}",
        fs::canonicalize(cycle.path("entry.ax")).unwrap().display(),
        fs::canonicalize(cycle.path("a.ax")).unwrap().display(),
        fs::canonicalize(cycle.path("b.ax")).unwrap().display(),
        fs::canonicalize(cycle.path("a.ax")).unwrap().display()
    );
    let cycle_observed = import_error(&cycle);
    let cycle_repeated = import_error(&cycle);
    emit(
        "import_cycle_fails_deterministically",
        json!({"diagnostic": cycle_expected, "repeat_equal": true}),
        json!({"diagnostic": cycle_observed, "repeat_equal": cycle_repeated == cycle_observed}),
    );

    let deterministic = TempProject::from_fixture("deterministic");
    let first = load(deterministic.path("synthetix.toml")).unwrap();
    let second = load(deterministic.path("synthetix.toml")).unwrap();
    let a = fs::canonicalize(deterministic.path("a.ax")).unwrap();
    let z = fs::canonicalize(deterministic.path("z.ax")).unwrap();
    let entry = fs::canonicalize(deterministic.path("entry.ax")).unwrap();
    let expected_order = vec![
        format!("{}::alpha", a.display()),
        format!("{}::zeta", z.display()),
        format!("{}::root", entry.display()),
    ];
    let expected_files = vec![
        z.display().to_string(),
        a.display().to_string(),
        entry.display().to_string(),
    ];
    emit(
        "deterministic_order_source_mapping_and_repeat_diagnostics",
        json!({"order": expected_order, "files": expected_files, "repeat_order_equal": true, "repeat_files_equal": true}),
        json!({"order": order(&first), "files": file_paths(&first), "repeat_order_equal": order(&first) == order(&second), "repeat_files_equal": file_paths(&first) == file_paths(&second)}),
    );
}
