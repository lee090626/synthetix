use std::{fs, process::Command};

fn axiom(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_axiom"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run axiom")
}

#[test]
fn new_project_checks_and_builds() {
    let base = std::env::temp_dir().join(format!("axiom-new-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();

    let created = axiom(&base, &["new", "app"]);
    assert!(created.status.success(), "{created:?}");
    let app = base.join("app");
    for args in [&["check"][..], &["build"][..]] {
        let out = axiom(&app, args);
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
    assert!(app.join("dist/index.html").exists());

    let again = axiom(&base, &["new", "app"]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("not empty"));
    fs::remove_dir_all(&base).unwrap();
}
