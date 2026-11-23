//! `init`, `inventory`, `plan-audit` and `guide` against tests/adopt-fixtures: a tiny
//! SwiftUI + Compose repository (`repo/`) and its catalog (`spec/fixture`, one entry).
//! Goldens live in tests/adopt-fixtures/expected; `CONCORDANCE_BLESS=1` rewrites them.
use concordance::Catalog;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/adopt-fixtures")
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_concordance"))
        .current_dir(dir)
        .args(args)
        .env_remove("CONCORDANCE_ROOT_IOS")
        .env_remove("CONCORDANCE_ROOT_ANDROID")
        .output()
        .expect("run concordance")
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stdout.clone()).expect("UTF-8 output")
}

fn golden(name: &str, actual: &str) {
    let path = fixtures().join("expected").join(name);
    if std::env::var_os("CONCORDANCE_BLESS").is_some() {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(actual, expected, "golden {name} differs");
}

/// A temporary copy of tests/adopt-fixtures.
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-adopt-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("unique temp dir");
        fn copy(from: &Path, to: &Path) {
            for entry in fs::read_dir(from).expect("fixture directory") {
                let entry = entry.expect("fixture entry");
                let target = to.join(entry.file_name());
                if entry.path().is_dir() {
                    fs::create_dir(&target).expect("mkdir");
                    copy(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).expect("copy fixture");
                }
            }
        }
        copy(&fixtures(), &path);
        Self(path)
    }
    fn repo(&self) -> PathBuf {
        self.0.join("repo")
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn init_dry_run_plans_without_writing() {
    let output = run_in(
        &fixtures(),
        &["init", "--repo", "repo", "--app", "fixture", "--dry-run"],
    );
    golden("init-dry-run.txt", &stdout(&output));
    assert!(!fixtures().join("repo/spec").exists());
}

#[test]
fn init_writes_a_valid_empty_catalog_and_never_overwrites() {
    let temp = Temp::new();
    let repo = temp.repo();
    // A file the person already has is kept.
    fs::create_dir_all(repo.join("spec/fixture")).expect("mkdir");
    fs::write(repo.join("spec/fixture/AGENTS.md"), "mine\n").expect("write");
    let first = stdout(&run_in(&repo, &["init", "--app", "fixture"]));
    assert!(
        first.contains("- create spec/fixture/catalog.toml"),
        "{first}"
    );
    assert!(
        first.contains("- keep spec/fixture/AGENTS.md (exists)"),
        "{first}"
    );
    assert!(
        first.contains("run `concordance guide spec/fixture`"),
        "{first}"
    );
    assert_eq!(
        fs::read_to_string(repo.join("spec/fixture/AGENTS.md")).expect("read"),
        "mine\n"
    );
    assert!(repo.join("spec/fixture/tracker").is_dir());
    assert!(repo.join("spec/fixture/catalog/README.md").is_file());

    // The empty catalog loads, validates and tracks.
    let catalog = Catalog::load(repo.join("spec/fixture")).expect("empty catalog loads");
    assert!(catalog.entries.is_empty());
    assert_eq!(catalog.config.report.baseline, "ios");
    assert_eq!(catalog.config.report.target, "android");
    let valid = stdout(&run_in(&repo, &["validate", "spec/fixture"]));
    assert!(valid.contains("Valid: 0 entries in 0 areas."), "{valid}");
    stdout(&run_in(
        &repo,
        &["track", "spec/fixture", "--out", "track.md"],
    ));
    // Its inventory lists everything as uncatalogued.
    let inventory = stdout(&run_in(
        &repo,
        &["inventory", "spec/fixture", "--uncatalogued"],
    ));
    assert!(
        inventory.contains("| ios | `apple` | 0/4 | 0/3 | 0/5 | 0/1 |"),
        "{inventory}"
    );

    // Without the kept file, AGENTS.md is exactly the guide.
    fs::remove_file(repo.join("spec/fixture/AGENTS.md")).expect("remove");
    fs::remove_file(repo.join("spec/fixture/catalog.toml")).expect("remove");
    stdout(&run_in(&repo, &["init", "--app", "fixture"]));
    let kit = fs::read_to_string(repo.join("spec/fixture/AGENTS.md")).expect("kit");
    assert_eq!(kit, stdout(&run_in(&repo, &["guide", "spec/fixture"])));

    // Rerunning on an adopted repository writes nothing.
    let before = fs::read_to_string(repo.join("spec/fixture/catalog.toml")).expect("read");
    let again = stdout(&run_in(&repo, &["init", "--app", "fixture"]));
    assert!(again.contains("Already adopted"), "{again}");
    assert!(again.contains("Nothing to add."), "{again}");
    assert_eq!(
        fs::read_to_string(repo.join("spec/fixture/catalog.toml")).expect("read"),
        before
    );
    // ...and reports what it would add.
    fs::remove_dir(repo.join("spec/fixture/tracker")).expect("rmdir");
    let missing = stdout(&run_in(&repo, &["init", "--app", "fixture"]));
    assert!(missing.contains("- spec/fixture/tracker/"), "{missing}");
    assert!(!repo.join("spec/fixture/tracker").exists());
}

#[test]
fn init_skips_build_and_dependency_folders() {
    let output = stdout(&run_in(
        &fixtures(),
        &["init", "--repo", "repo", "--app", "fixture", "--dry-run"],
    ));
    assert!(
        !output.contains("web"),
        "node_modules must not make a web platform"
    );
    let inventory = stdout(&run_in(&fixtures(), &["inventory", "spec/fixture"]));
    assert!(!inventory.contains("build.ignored"));
}

#[test]
fn inventory_markdown_and_json() {
    let markdown = stdout(&run_in(&fixtures(), &["inventory", "spec/fixture"]));
    golden("inventory.md", &markdown);
    let json = stdout(&run_in(
        &fixtures(),
        &[
            "inventory",
            "spec/fixture",
            "--platform",
            "ios",
            "--uncatalogued",
            "--format",
            "json",
        ],
    ));
    golden("inventory-ios-uncatalogued.json", &json);
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    let files = value["platforms"][0]["files"].as_array().expect("files");
    assert!(files
        .iter()
        .all(|f| f["cited_by"].as_array().is_some_and(Vec::is_empty)));
    // Summary counts cover everything, not only the listed items.
    assert_eq!(value["platforms"][0]["summary"]["files_cited"], 1);
}

#[test]
fn inventory_reports_unsupplied_external_roots() {
    let temp = Temp::new();
    let config = temp.0.join("spec/fixture/catalog.toml");
    let text = fs::read_to_string(&config).expect("read");
    fs::write(
        &config,
        format!("{text}\n[external_roots]\nandroid = \"Android checkout\"\n"),
    )
    .expect("write");
    let out = stdout(&run_in(&temp.0, &["inventory", "spec/fixture"]));
    assert!(
        out.contains("| android | `android` | unchecked: root not supplied; pass --root android=<path> or CONCORDANCE_ROOT_ANDROID | | | |"),
        "{out}"
    );
    let supplied = stdout(&run_in(
        &temp.0,
        &[
            "inventory",
            "spec/fixture",
            "--platform",
            "android",
            "--root",
            "android=repo/android",
        ],
    ));
    assert!(
        supplied.contains("| android | `android` | 1/3 |"),
        "{supplied}"
    );
}

#[test]
fn plan_audit_writes_briefs_once() {
    let temp = Temp::new();
    let created = stdout(&run_in(&temp.0, &["plan-audit", "spec/fixture"]));
    assert!(
        created.starts_with("3 created; 0 unchanged; 0 existing briefs preserved."),
        "{created}"
    );
    let tasks = temp.0.join("spec/fixture/tasks");
    golden(
        "audit-ios-home.md",
        &fs::read_to_string(tasks.join("audit-ios-home.md")).expect("brief"),
    );
    let parity = stdout(&run_in(
        &temp.0,
        &[
            "plan-audit",
            "spec/fixture",
            "--target",
            "android",
            "--area",
            "home",
        ],
    ));
    assert!(parity.starts_with("1 created;"), "{parity}");
    golden(
        "parity-ios-android-home.md",
        &fs::read_to_string(tasks.join("parity-ios-android-home.md")).expect("brief"),
    );
    // Never overwritten: identical briefs are unchanged, edited ones preserved.
    fs::write(tasks.join("audit-ios-player.md"), "my notes\n").expect("edit");
    let again = run_in(&temp.0, &["plan-audit", "spec/fixture"]);
    assert!(stdout(&again).starts_with("0 created; 2 unchanged; 1 existing briefs preserved."));
    assert_eq!(
        fs::read_to_string(tasks.join("audit-ios-player.md")).expect("read"),
        "my notes\n"
    );
    let unknown = run_in(&temp.0, &["plan-audit", "spec/fixture", "--area", "nope"]);
    assert!(!unknown.status.success());
    let message = String::from_utf8_lossy(&unknown.stderr);
    assert!(
        message.contains("candidates: app, home, player"),
        "{message}"
    );
}

#[test]
fn guide_prints_the_kit() {
    let out = stdout(&run_in(&fixtures(), &["guide", "spec/fixture"]));
    golden("guide.md", &out);
}

#[test]
fn guide_reads_tag_patterns_as_string_or_list() {
    let text = "[platforms]\nios = \".\"\n[tracker]\ntag_pattern = ['^v(.*)$', '^ios/v(.*)$']\n";
    let (config, patterns) = concordance::adopt::parse_config(text).expect("parse");
    assert_eq!(patterns, vec!["^v(.*)$", "^ios/v(.*)$"]);
    let kit = concordance::adopt::guide_text("app", "spec/app", &config, &patterns);
    assert!(
        kit.contains("Release tags match `^v(.*)$` or `^ios/v(.*)$`"),
        "{kit}"
    );
}
