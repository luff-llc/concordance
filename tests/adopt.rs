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

#[cfg(unix)]
#[test]
fn init_refuses_to_write_through_symbolic_links() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let repo = temp.repo();
    let outside = temp.0.join("outside");
    fs::create_dir(&outside).expect("mkdir");

    // An output directory reached through a link that leaves the repository.
    symlink(&outside, repo.join("spec")).expect("symlink");
    let refused = run_in(&repo, &["init", "--app", "fixture"]);
    assert!(!refused.status.success());
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("symbolic link"), "{message}");
    assert_eq!(fs::read_dir(&outside).expect("read").count(), 0);
    // A dry run refuses too.
    assert!(!run_in(&repo, &["init", "--app", "fixture", "--dry-run"])
        .status
        .success());
    fs::remove_file(repo.join("spec")).expect("unlink");

    // A link inside the catalog directory, even one that stays inside the repository.
    fs::create_dir_all(repo.join("spec/fixture")).expect("mkdir");
    symlink(repo.join("apple"), repo.join("spec/fixture/tracker")).expect("symlink");
    let inner = run_in(&repo, &["init", "--app", "fixture"]);
    assert!(!inner.status.success());
    assert!(!repo.join("spec/fixture/catalog.toml").exists());
    fs::remove_file(repo.join("spec/fixture/tracker")).expect("unlink");

    // A dangling link where AGENTS.md goes.
    symlink(
        outside.join("AGENTS.md"),
        repo.join("spec/fixture/AGENTS.md"),
    )
    .expect("symlink");
    assert!(!run_in(&repo, &["init", "--app", "fixture"])
        .status
        .success());
    assert!(!outside.join("AGENTS.md").exists());
}

#[cfg(unix)]
#[test]
fn inventory_skips_symbolic_links_that_leave_the_root() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let outside = temp.0.join("outside");
    fs::create_dir(&outside).expect("mkdir");
    fs::write(
        outside.join("Leak.swift"),
        "import SwiftUI\nlet x = Text(\"x\").accessibilityIdentifier(\"leak.id\")\n",
    )
    .expect("write");
    let fixture = temp.repo().join("apple/Fixture");
    symlink(outside.join("Leak.swift"), fixture.join("Leak.swift")).expect("symlink");
    symlink(&outside, fixture.join("Outside")).expect("symlink");
    // A fixed glob prefix (the tests glob) that resolves outside the root.
    let tests = temp.repo().join("apple/FixtureUITests");
    fs::rename(&tests, outside.join("FixtureUITests")).expect("move");
    symlink(outside.join("FixtureUITests"), &tests).expect("symlink");
    let out = stdout(&run_in(
        &temp.0,
        &["inventory", "spec/fixture", "--platform", "ios"],
    ));
    assert!(!out.contains("leak.id"), "{out}");
    assert!(!out.contains("HomeUITests"), "{out}");
    assert!(
        out.contains("Skipped 3 symbolic link(s)")
            && out.contains("`Fixture/Leak.swift`")
            && out.contains("`Fixture/Outside`")
            && out.contains("`FixtureUITests`"),
        "{out}"
    );
}

#[test]
fn inventory_keeps_same_key_strings_per_module() {
    let temp = Temp::new();
    let config = temp.0.join("spec/fixture/catalog.toml");
    let text = fs::read_to_string(&config).expect("read");
    let text = text
        .replace(
            "user_visible = [\"app/src/main/**/*.kt\"]",
            "user_visible = [\"app/src/main/**/*.kt\", \"feature/src/main/**/*.kt\"]",
        )
        .replace(
            "strings = [\"app/src/main/res/values*/strings.xml\"]",
            "strings = [\"**/src/main/res/values*/strings.xml\"]",
        );
    fs::write(&config, text).expect("write");
    let feature = temp.repo().join("android/feature/src/main");
    fs::create_dir_all(feature.join("res/values")).expect("mkdir");
    fs::create_dir_all(feature.join("kotlin/com/example/feature")).expect("mkdir");
    fs::write(
        feature.join("res/values/strings.xml"),
        "<resources>\n    <string name=\"home_empty\">No features yet</string>\n</resources>\n",
    )
    .expect("write");
    fs::write(
        feature.join("kotlin/com/example/feature/FeatureScreen.kt"),
        "package com.example.feature\nfun label() = stringResource(R.string.home_empty)\n",
    )
    .expect("write");
    let json = stdout(&run_in(
        &temp.0,
        &[
            "inventory",
            "spec/fixture",
            "--platform",
            "android",
            "--format",
            "json",
        ],
    ));
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    let strings: Vec<(String, String, String)> = value["platforms"][0]["strings"]
        .as_array()
        .expect("strings")
        .iter()
        .filter(|s| s["key"] == "home_empty")
        .map(|s| {
            (
                s["file"].as_str().unwrap_or_default().to_owned(),
                s["value"].as_str().unwrap_or_default().to_owned(),
                s["areas"].to_string(),
            )
        })
        .collect();
    assert_eq!(
        strings,
        vec![
            (
                "app/src/main/res/values/strings.xml".to_owned(),
                "Nothing here yet".to_owned(),
                "[\"home\"]".to_owned()
            ),
            (
                "feature/src/main/res/values/strings.xml".to_owned(),
                "No features yet".to_owned(),
                "[\"feature\"]".to_owned()
            ),
        ]
    );
}

#[test]
fn init_scope_limits_detection() {
    let out = stdout(&run_in(
        &fixtures(),
        &[
            "init",
            "--repo",
            "repo",
            "--app",
            "fixture",
            "--scope",
            "apple/Fixture",
            "--dry-run",
        ],
    ));
    assert!(out.contains("Scopes: apple/Fixture"), "{out}");
    assert!(out.contains("- ios: `apple`"), "{out}");
    assert!(!out.contains("- android:"), "{out}");
    assert!(out.contains("all = [\"Fixture/**/*.swift\"]"), "{out}");
    assert!(out.contains("tests = []"), "{out}");
    let outside = run_in(
        &fixtures(),
        &["init", "--repo", "repo", "--scope", "../spec", "--dry-run"],
    );
    assert!(!outside.status.success());
}

#[test]
fn init_lists_several_apps_with_scope_suggestions() {
    let temp = Temp::new();
    let repo = temp.repo();
    add_other_app(&repo);
    // A root build file that declares the Android plugin without applying it is no app.
    fs::write(
        repo.join("android/build.gradle.kts"),
        "plugins {\n    id(\"com.android.application\") version \"8.7.0\" apply  false\n}\n",
    )
    .expect("write");
    let out = stdout(&run_in(&repo, &["init", "--app", "all", "--dry-run"]));
    assert!(out.contains("Several apps found (2)"), "{out}");
    assert!(
        !out.contains("Android module in android/build.gradle.kts"),
        "{out}"
    );
    assert!(
        out.contains(
            "    concordance init --app fixture --scope android/app --scope apple/Fixture"
        ),
        "{out}"
    );
    assert!(
        out.contains(
            "    concordance init --app other --scope other/Other --scope other/OtherWidgets"
        ),
        "{out}"
    );
}

/// A second app: an Xcode project with an application and a widget target.
fn add_other_app(repo: &Path) {
    let project = repo.join("other/Other.xcodeproj");
    fs::create_dir_all(&project).expect("mkdir");
    fs::write(
        project.join("project.pbxproj"),
        "{ objects = {\n\t\tA1 = {\n\t\t\tisa = PBXNativeTarget;\n\t\t\tname = Other;\n\t\t\tproductType = \"com.apple.product-type.application\";\n\t\t};\n\t\tA2 = {\n\t\t\tisa = PBXNativeTarget;\n\t\t\tname = OtherWidgets;\n\t\t\tproductType = \"com.apple.product-type.app-extension\";\n\t\t};\n\t\tA3 = {\n\t\t\tisa = PBXNativeTarget;\n\t\t\tname = OtherTests;\n\t\t\tproductType = \"com.apple.product-type.bundle.unit-test\";\n\t\t};\n\t}; }\n",
    )
    .expect("write");
    for dir in ["Other", "OtherWidgets"] {
        fs::create_dir_all(repo.join("other").join(dir)).expect("mkdir");
        fs::write(
            repo.join("other").join(dir).join("Main.swift"),
            "import SwiftUI\n",
        )
        .expect("write");
    }
}

#[cfg(unix)]
#[test]
fn inventory_never_walks_a_linked_prefix_even_inside_the_root() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let apple = temp.repo().join("apple");
    fs::rename(apple.join("FixtureUITests"), apple.join("RealTests")).expect("move");
    symlink(apple.join("RealTests"), apple.join("FixtureUITests")).expect("symlink");
    let out = stdout(&run_in(
        &temp.0,
        &["inventory", "spec/fixture", "--platform", "ios"],
    ));
    assert!(!out.contains("HomeUITests"), "{out}");
    assert!(
        out.contains("Skipped 1 symbolic link(s)") && out.contains("`FixtureUITests`"),
        "{out}"
    );
}

/// `- <platform>: `<root>` (<kind>; N user-visible files; ...` lines of an init plan.
fn planned_counts(plan: &str) -> Vec<(String, usize)> {
    plan.lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("- ")?;
            let (name, rest) = rest.split_once(": `")?;
            let count = rest
                .split("; ")
                .nth(1)?
                .strip_suffix(" user-visible files")?;
            Some((name.to_owned(), count.parse().ok()?))
        })
        .collect()
}

#[test]
fn narrow_scopes_give_narrow_globs_and_matching_counts() {
    let temp = Temp::new();
    let repo = temp.repo();
    let plan = stdout(&run_in(
        &repo,
        &[
            "init",
            "--app",
            "narrow",
            "--scope",
            "apple/Fixture/Home",
            "--scope",
            "android/app/src/main/kotlin",
        ],
    ));
    let toml = fs::read_to_string(repo.join("spec/narrow/catalog.toml")).expect("catalog");
    assert!(
        toml.contains("all = [\"Fixture/Home/**/*.swift\"]"),
        "{toml}"
    );
    assert!(
        toml.contains("all = [\"app/src/main/kotlin/**/*.kt\"]"),
        "{toml}"
    );
    assert!(!toml.contains("Fixture/**/*.swift"), "{toml}");
    let planned = planned_counts(&plan);
    assert_eq!(
        planned,
        vec![("android".to_owned(), 3), ("ios".to_owned(), 2)],
        "{plan}"
    );
    let json = stdout(&run_in(
        &repo,
        &["inventory", "spec/narrow", "--format", "json"],
    ));
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    let counted: Vec<(String, usize)> = value["platforms"]
        .as_array()
        .expect("platforms")
        .iter()
        .map(|p| {
            (
                p["platform"].as_str().unwrap_or_default().to_owned(),
                p["summary"]["files"].as_u64().unwrap_or_default() as usize,
            )
        })
        .collect();
    assert_eq!(counted, planned);
}

#[test]
fn whole_repository_counts_match_inventory() {
    let temp = Temp::new();
    let repo = temp.repo();
    let plan = stdout(&run_in(&repo, &["init", "--app", "fixture"]));
    let json = stdout(&run_in(
        &repo,
        &["inventory", "spec/fixture", "--format", "json"],
    ));
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    let counted: Vec<(String, usize)> = value["platforms"]
        .as_array()
        .expect("platforms")
        .iter()
        .map(|p| {
            (
                p["platform"].as_str().unwrap_or_default().to_owned(),
                p["summary"]["files"].as_u64().unwrap_or_default() as usize,
            )
        })
        .collect();
    assert_eq!(counted, planned_counts(&plan));
}

#[test]
fn narrow_web_scopes_keep_string_catalogs_anchored() {
    let temp = Temp::new();
    let repo = temp.repo();
    let web = repo.join("web");
    fs::create_dir_all(web.join("src/components")).expect("mkdir");
    fs::create_dir_all(web.join("src/i18n")).expect("mkdir");
    fs::create_dir_all(web.join("src/pages")).expect("mkdir");
    fs::write(web.join("package.json"), "{\"name\": \"web\"}\n").expect("write");
    fs::write(
        web.join("src/components/Button.tsx"),
        "export const Button = () => <button data-testid=\"web.button\">{t('button.save')}</button>;\n",
    )
    .expect("write");
    fs::write(
        web.join("src/pages/Home.tsx"),
        "export const Home = () => null;\n",
    )
    .expect("write");
    fs::write(
        web.join("src/i18n/en.json"),
        "{\"button\": {\"save\": \"Save\"}}\n",
    )
    .expect("write");
    let plan = stdout(&run_in(
        &repo,
        &[
            "init",
            "--app",
            "web",
            "--scope",
            "web/src/components",
            "--scope",
            "web/src/i18n",
        ],
    ));
    let toml = fs::read_to_string(repo.join("spec/web/catalog.toml")).expect("catalog");
    assert!(
        toml.contains("all = [\"src/components/**/*.tsx\"]"),
        "{toml}"
    );
    assert!(
        toml.contains("strings = [\"src/i18n/**/*.json\"]"),
        "{toml}"
    );
    let json = stdout(&run_in(
        &repo,
        &["inventory", "spec/web", "--format", "json"],
    ));
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    let web = &value["platforms"][0];
    assert_eq!(web["platform"], "web");
    assert_eq!(web["summary"]["strings"], 1, "{json}");
    assert_eq!(web["summary"]["identifiers"], 1, "{json}");
    assert_eq!(planned_counts(&plan), vec![("web".to_owned(), 1)], "{plan}");
    assert_eq!(web["summary"]["files"], 1);
}

#[cfg(unix)]
#[test]
fn init_counts_skip_file_links_that_leave_the_platform_root() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let repo = temp.repo();
    // Inside the repository, outside the `apple` platform root.
    fs::create_dir_all(repo.join("shared")).expect("mkdir");
    fs::write(repo.join("shared/Linked.swift"), "import SwiftUI\n").expect("write");
    symlink(
        repo.join("shared/Linked.swift"),
        repo.join("apple/Fixture/Home/Linked.swift"),
    )
    .expect("symlink");
    let plan = stdout(&run_in(&repo, &["init", "--app", "fixture"]));
    let planned = planned_counts(&plan);
    assert!(planned.contains(&("ios".to_owned(), 4)), "{plan}");
    let json = stdout(&run_in(
        &repo,
        &["inventory", "spec/fixture", "--format", "json"],
    ));
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    let ios = value["platforms"]
        .as_array()
        .expect("platforms")
        .iter()
        .find(|p| p["platform"] == "ios")
        .expect("ios")
        .clone();
    assert_eq!(ios["summary"]["files"], 4);
    let skipped = ios["skipped_symlinks"].to_string();
    assert!(skipped.contains("Fixture/Home/Linked.swift"), "{skipped}");
}

#[cfg(unix)]
#[test]
fn plan_audit_refuses_symbolic_links_and_outside_paths() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let outside = temp.0.join("outside");
    fs::create_dir(&outside).expect("mkdir");
    symlink(&outside, temp.0.join("spec/fixture/tasks")).expect("symlink");
    let refused = run_in(&temp.0, &["plan-audit", "spec/fixture"]);
    assert!(!refused.status.success());
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("symbolic link"), "{message}");
    assert_eq!(fs::read_dir(&outside).expect("read").count(), 0);
    // An output directory outside the catalog's repository is refused too.
    let elsewhere = run_in(
        &temp.0,
        &["plan-audit", "spec/fixture", "--out", "outside/briefs"],
    );
    assert!(!elsewhere.status.success());
    assert!(!outside.join("briefs").exists());
}

#[test]
fn explicit_empty_strings_mean_none() {
    let temp = Temp::new();
    let repo = temp.repo();
    let plan = stdout(&run_in(
        &repo,
        &[
            "init",
            "--app",
            "kotlin",
            "--scope",
            "android/app/src/main/kotlin",
        ],
    ));
    let toml = fs::read_to_string(repo.join("spec/kotlin/catalog.toml")).expect("catalog");
    assert!(toml.contains("strings = []"), "{toml}");
    let json = stdout(&run_in(
        &repo,
        &["inventory", "spec/kotlin", "--format", "json"],
    ));
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    let android = &value["platforms"][0];
    assert_eq!(android["platform"], "android");
    assert_eq!(android["summary"]["strings"], 0, "{json}");
    assert_eq!(
        planned_counts(&plan),
        vec![(
            "android".to_owned(),
            android["summary"]["files"].as_u64().unwrap_or_default() as usize
        )]
    );
}

#[test]
fn parent_components_are_refused() {
    let temp = Temp::new();
    let repo = temp.repo();
    for args in [
        vec!["init", "--app", "x", "--out", "spec/../spec/x", "--dry-run"],
        vec![
            "init",
            "--app",
            "x",
            "--scope",
            "apple/../android",
            "--dry-run",
        ],
    ] {
        let out = run_in(&repo, &args);
        assert!(!out.status.success(), "{args:?}");
        let message = String::from_utf8_lossy(&out.stderr);
        assert!(message.contains("must not contain `..`"), "{message}");
    }
    let audit = run_in(
        &temp.0,
        &[
            "plan-audit",
            "spec/fixture",
            "--out",
            "spec/fixture/../fixture/tasks",
        ],
    );
    assert!(!audit.status.success());
    assert!(String::from_utf8_lossy(&audit.stderr).contains("must not contain `..`"));
    assert!(!temp.0.join("spec/fixture/tasks").exists());
    // The default output directory is fine even when DIR itself has `..` in it.
    let fine = run_in(
        &temp.0,
        &["plan-audit", "repo/../spec/fixture", "--area", "home"],
    );
    assert!(stdout(&fine).starts_with("1 created;"));
    assert!(temp
        .0
        .join("spec/fixture/tasks/audit-ios-home.md")
        .is_file());
}

/// A Cargo virtual workspace: no root `src/`, members under `crates/*`.
fn cargo_workspace(dir: &Path) {
    let write = |path: &str, text: &str| {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    };
    write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    );
    write("crates/tool/Cargo.toml", "[package]\nname = \"tool\"\n");
    write("crates/tool/src/main.rs", "fn main() {}\n");
    write("crates/core/Cargo.toml", "[package]\nname = \"core\"\n");
    write("crates/core/src/lib.rs", "pub fn f() {}\n");
    write("crates/core/tests/it.rs", "#[test]\nfn t() {}\n");
}

#[test]
fn cargo_virtual_workspaces_get_member_globs() {
    let temp = Temp::new();
    let ws = temp.0.join("ws");
    cargo_workspace(&ws);
    let plan = stdout(&run_in(&ws, &["init", "--app", "ws", "--dry-run"]));
    assert!(
        plan.contains("    all = [\"crates/core/src/**/*.rs\", \"crates/tool/src/**/*.rs\"]"),
        "{plan}"
    );
    assert!(
        plan.contains("    tests = [\"crates/core/tests/**/*.rs\"]"),
        "{plan}"
    );
    assert_eq!(planned_counts(&plan), vec![("cli".to_owned(), 2)], "{plan}");

    // A scope inside one member keeps the Rust platform, narrowed to it.
    let scoped = stdout(&run_in(
        &ws,
        &["init", "--app", "ws", "--scope", "crates/tool/src"],
    ));
    assert_eq!(
        planned_counts(&scoped),
        vec![("cli".to_owned(), 1)],
        "{scoped}"
    );
    let toml = fs::read_to_string(ws.join("spec/ws/catalog.toml")).expect("catalog");
    assert!(
        toml.contains("all = [\"crates/tool/src/**/*.rs\"]"),
        "{toml}"
    );
    let json = stdout(&run_in(&ws, &["inventory", "spec/ws", "--format", "json"]));
    let value: serde_json::Value = serde_json::from_str(&json).expect("json");
    assert_eq!(value["platforms"][0]["summary"]["files"], 1, "{json}");
}
