//! End-to-end tests: run the real binary against fixture files.

use assert_cmd::Command;
use predicates::prelude::*;

fn catr() -> Command {
    Command::cargo_bin("catr").unwrap()
}

#[test]
fn prints_a_file_verbatim() {
    catr()
        .arg("tests/fixtures/two.txt")
        .assert()
        .success()
        .stdout("alpha\nbeta\n");
}

#[test]
fn concatenates_multiple_files_with_shared_numbering() {
    catr()
        .args(["-n", "tests/fixtures/two.txt", "tests/fixtures/two.txt"])
        .assert()
        .success()
        .stdout("     1\talpha\n     2\tbeta\n     3\talpha\n     4\tbeta\n");
}

#[test]
fn number_nonblank_skips_blanks() {
    catr()
        .args(["-b", "tests/fixtures/one.txt"])
        .assert()
        .success()
        .stdout("     1\tfirst\n     2\tsecond\n\n     3\tfourth\n");
}

#[test]
fn reads_stdin_by_default() {
    catr().write_stdin("hello\n").assert().success().stdout("hello\n");
}

#[test]
fn missing_file_fails_but_prints_the_rest() {
    catr()
        .args(["nope.txt", "tests/fixtures/two.txt"])
        .assert()
        .failure()
        .stdout("alpha\nbeta\n")
        .stderr(predicate::str::contains("nope.txt"));
}

#[test]
fn n_and_b_conflict() {
    catr()
        .args(["-n", "-b", "tests/fixtures/one.txt"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}
