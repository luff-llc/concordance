//! Regression suite: a seven-platform product (flavours that inherit, features on only
//! some platforms, surfaces and declared-surface scans) run through the read-only
//! commands, compared with the expected outputs in `tests/regression-fixtures/expected`.
//!
//! After an intended change, regenerate them with `CONCORDANCE_BLESS=1 cargo test --test
//! regression` and review the diff like any other change.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn suite() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/regression-fixtures/suite")
}

/// Exit code and stdout of one command run on the suite.
fn run(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args(args)
        .output()
        .expect("CLI");
    format!(
        "exit {}\n{}{}",
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn check(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/regression-fixtures/expected")
        .join(format!("{name}.txt"));
    if std::env::var_os("CONCORDANCE_BLESS").is_some() {
        fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        fs::write(&path, actual).expect("bless");
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{}: missing; run with CONCORDANCE_BLESS=1", path.display()));
    assert!(
        expected == actual,
        "{name} differs from {}.\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

#[test]
fn suite_outputs_match_the_expected_results() {
    let dir = suite();
    let dir = dir.to_str().expect("path");
    check("validate", &run(&["validate", dir]));
    check(
        "surfaces-scan",
        &run(&["surfaces", "scan", dir, "--format", "json"]),
    );
    for platform in ["ios", "android", "eink", "macos", "watchos", "wearos"] {
        check(
            &format!("next-{platform}"),
            &run(&["next", dir, "--platform", platform]),
        );
    }
    for id in ["widgets.now-playing", "player.resume", "watch.remote"] {
        check(
            &format!("entry-{id}"),
            &run(&["entry", dir, id, "--format", "json"]),
        );
    }
    for platform in ["android", "wearos"] {
        check(
            &format!("gate-release-{platform}"),
            &run(&[
                "gate-release",
                dir,
                "--platform",
                platform,
                "--max-age-days",
                "30",
                "--today",
                "2026-11-25",
            ]),
        );
    }
    // Inside the suite, so the report's relative links are the same on every machine.
    let out = suite().join(format!("parity-{}.md", std::process::id()));
    let status = run(&["report", dir, "--out", out.to_str().expect("path")]);
    assert!(status.starts_with("exit 0"), "{status}");
    let report = fs::read_to_string(&out).expect("report");
    let _ = fs::remove_file(&out);
    check("report", &report);
}
