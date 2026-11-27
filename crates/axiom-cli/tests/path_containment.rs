//! A project reads only files inside its folder: a `from` line or the entry
//! that leaves it (`..`, an absolute path) is rejected before the file is read,
//! so its text never reaches an error message.

use std::{fs, path::Path, process::Command};

const SECRET: &str = "API_TOKEN=secret-value-123";

/// `<temp>/<name>/outside/notes.txt` holds the secret; the project is `<temp>/<name>/proj`.
fn setup(name: &str, rules: &str, entry: &str) -> std::path::PathBuf {
    let root =
        std::env::temp_dir().join(format!("axiom-containment-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let project = root.join("proj");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(root.join("outside")).unwrap();
    fs::write(root.join("outside/notes.txt"), format!("{SECRET}\n")).unwrap();
    fs::write(
        project.join("synthetix.toml"),
        format!("[project]\nentry = \"{entry}\"\n"),
    )
    .unwrap();
    fs::write(project.join(entry), rules).unwrap();
    project
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

const USE_X: &str = "use_x(x)\n  need x <= 10\n  always -> x\n  promise answer <= 10\n";

#[test]
fn a_from_line_cannot_read_a_file_above_the_project() {
    let dir = setup(
        "parent",
        &format!("from ../outside/notes.txt: x\n\n{USE_X}"),
        "rules.ax",
    );
    let (ok, output) = check(&dir);
    assert!(!ok, "{output}");
    assert!(output.contains("inside its folder"), "{output}");
    assert!(!output.contains(SECRET), "{output}");
}

#[test]
fn a_from_line_cannot_read_an_absolute_path() {
    let dir = setup(
        "absolute",
        &format!("from /etc/hosts: x\n\n{USE_X}"),
        "rules.ax",
    );
    let (ok, output) = check(&dir);
    assert!(!ok, "{output}");
    assert!(output.contains("inside its folder"), "{output}");
}

#[test]
fn the_entry_cannot_leave_the_project() {
    let dir = setup("entry", USE_X, "rules.ax");
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"../outside/notes.txt\"\n",
    )
    .unwrap();
    let (ok, output) = check(&dir);
    assert!(!ok, "{output}");
    assert!(output.contains("inside its folder"), "{output}");
    assert!(!output.contains(SECRET), "{output}");
}

#[test]
fn a_from_line_inside_the_project_still_works() {
    let dir = setup(
        "inside",
        "from lib/money: money_rule\n\nuse_money(x)\n  need x <= 10\n  always -> money_rule(x)\n  promise answer <= 10\n",
        "rules.ax",
    );
    fs::create_dir_all(dir.join("lib")).unwrap();
    fs::write(
        dir.join("lib/money.ax"),
        "money_rule(x)\n  need x <= 10\n  always -> x\n  promise answer <= 10\n",
    )
    .unwrap();
    let (ok, output) = check(&dir);
    assert!(ok, "{output}");
}

#[test]
fn a_symlink_from_file_cannot_read_outside_the_project() {
    let dir = setup(
        "symlink-from",
        &format!("from lib/link: x\n\n{USE_X}"),
        "rules.ax",
    );
    fs::create_dir_all(dir.join("lib")).unwrap();
    std::os::unix::fs::symlink("../../outside/notes.txt", dir.join("lib/link.ax")).unwrap();
    let (ok, output) = check(&dir);
    assert!(!ok, "{output}");
    assert!(output.contains("outside the project folder"), "{output}");
    assert!(!output.contains(SECRET), "{output}");
}

#[test]
fn a_symlink_entry_cannot_read_outside_the_project() {
    let dir = setup("symlink-entry", USE_X, "rules.ax");
    std::os::unix::fs::symlink("../outside/notes.txt", dir.join("linked.ax")).unwrap();
    fs::write(
        dir.join("synthetix.toml"),
        "[project]\nentry = \"linked.ax\"\n",
    )
    .unwrap();
    let (ok, output) = check(&dir);
    assert!(!ok, "{output}");
    assert!(output.contains("outside the project folder"), "{output}");
    assert!(!output.contains(SECRET), "{output}");
}
