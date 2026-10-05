use assert_cmd::Command;
use predicates::prelude::*;

fn headr() -> Command {
    Command::cargo_bin("headr").unwrap()
}

#[test]
fn default_is_ten_lines() {
    let input: String = (1..=20).map(|i| format!("{i}\n")).collect();
    let expected: String = (1..=10).map(|i| format!("{i}\n")).collect();
    headr().write_stdin(input).assert().success().stdout(expected);
}

#[test]
fn lines_flag() {
    headr()
        .args(["-n", "2"])
        .write_stdin("a\nb\nc\n")
        .assert()
        .success()
        .stdout("a\nb\n");
}

#[test]
fn bytes_flag() {
    headr()
        .args(["-c", "4"])
        .write_stdin("abcdef")
        .assert()
        .success()
        .stdout("abcd");
}

#[test]
fn multiple_files_get_headers() {
    headr()
        .args(["-n", "1", "Cargo.toml", "Cargo.toml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("==> Cargo.toml <==").count(2));
}

#[test]
fn zero_lines_is_invalid() {
    headr().args(["-n", "0"]).assert().failure();
}

#[test]
fn missing_file_reports_and_fails() {
    headr()
        .arg("definitely-not-here.txt")
        .assert()
        .failure()
        .stderr(predicate::str::contains("definitely-not-here.txt"));
}
