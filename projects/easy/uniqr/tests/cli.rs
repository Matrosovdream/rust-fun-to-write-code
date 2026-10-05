use assert_cmd::Command;
use predicates::prelude::*;

fn uniqr() -> Command {
    Command::cargo_bin("uniqr").unwrap()
}

#[test]
fn dedupes_stdin_to_stdout() {
    uniqr()
        .write_stdin("x\nx\ny\n")
        .assert()
        .success()
        .stdout("x\ny\n");
}

#[test]
fn counts_with_flag() {
    uniqr()
        .arg("-c")
        .write_stdin("x\nx\ny\n")
        .assert()
        .success()
        .stdout("      2 x\n      1 y\n");
}

#[test]
fn writes_to_output_file() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.txt");

    uniqr()
        .args(["-", out_path.to_str().unwrap()])
        .write_stdin("a\na\nb\n")
        .assert()
        .success()
        .stdout("");

    let written = std::fs::read_to_string(&out_path).unwrap();
    assert_eq!(written, "a\nb\n");
}

#[test]
fn missing_input_file_fails() {
    uniqr()
        .arg("no-such-file.txt")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no-such-file.txt"));
}
