use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn cli_exposes_foundation_commands() {
    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("jux 0.1.0"));

    Command::cargo_bin("jux")
        .expect("jux binary exists")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Jux agent command line interface.",
        ));
}
