use concordance::{Catalog, Language};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-test-{}-{}",
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
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
            &path,
        );
        Self(path)
    }
    fn load(&self) -> Catalog {
        Catalog::load(&self.0).expect("valid fixture")
    }
    fn load_collect_only(&self) -> Catalog {
        Catalog::load_collect(&self.0).expect("loadable fixture").0
    }
    fn mutate(&self, area: &str, edit: impl FnOnce(&mut Value)) {
        let path = self.0.join(format!("catalog/{area}.yaml"));
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read fixture"))
                .expect("JSON fixture");
        edit(&mut value);
        fs::write(
            path,
            serde_json::to_string_pretty(&value).expect("serialize"),
        )
        .expect("write fixture");
    }
    fn cli(&self, command: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .arg(command)
            .arg(&self.0)
            .args(args)
            .output()
            .expect("run CLI")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF8 output")
}

#[test]
fn real_fixture_all_cli_commands_and_report_staleness() {
    let fixture = Fixture::new();
    assert!(success(fixture.cli("validate", &[])).contains("4 entries in 3 areas"));
    let value: Value = serde_json::from_str(&success(fixture.cli("entry", &["reader.open-book"])))
        .expect("entry JSON");
    assert_eq!(value["id"], "reader.open-book");
    let yaml: Value = serde_yaml::from_str(&success(
        fixture.cli("entry", &["reader.open-book", "--format", "yaml"]),
    ))
    .expect("entry YAML");
    assert_eq!(yaml, value);
    let brief = success(fixture.cli("entry", &["reader.open-book", "--format", "brief"]));
    for expected in [
        "## Interactions",
        "## States",
        "## Copy",
        "## Accessibility",
        "## Acceptance",
        "library.open-library",
        "android missing",
        "Reader.swift",
    ] {
        assert!(brief.contains(expected), "missing {expected}");
    }
    assert!(!fixture.cli("entry", &["reader.unknown"]).status.success());
    assert!(!fixture.cli("report", &["--check"]).status.success());
    success(fixture.cli("report", &[]));
    success(fixture.cli("report", &["--check"]));
    let report = fs::read_to_string(fixture.0.join("parity.md")).expect("report");
    assert_eq!(report, include_str!("expected-parity.md"));
    fs::write(fixture.0.join("parity.md"), "edited").expect("edit report");
    assert!(!fixture.cli("report", &["--check"]).status.success());
    assert_eq!(
        fs::read_to_string(fixture.0.join("parity.md")).expect("preserved report"),
        "edited"
    );
    let queue = success(fixture.cli(
        "next",
        &["--platform", "android", "--area", "reader", "--limit", "2"],
    ));
    assert!(queue.starts_with("library.open-library\tlow\tmissing"));
    assert!(queue.contains("reader.open-book\tcritical\tpartial"));
    for (language, extension) in [("kotlin", "kt"), ("swift", "swift")] {
        let out = fixture.0.join(format!("Ids.{extension}"));
        success(fixture.cli(
            "ids",
            &["--platform", language, "--out", out.to_str().expect("path")],
        ));
        assert!(fs::read_to_string(out)
            .expect("IDs")
            .contains("reader.open"));
        let out = fixture.0.join(language);
        success(fixture.cli(
            "journeys",
            &["--platform", language, "--out", out.to_str().expect("path")],
        ));
        assert_eq!(fs::read_dir(out).expect("journeys").count(), 4);
    }
}

#[test]
fn rejects_schema_errors_and_unknown_config() {
    let mutations: Vec<(&str, Value)> = vec![
        ("id", json!("bad")),
        ("title", json!("")),
        ("surfaces", json!([])),
        ("states", json!(["ready", "ready"])),
        ("priority", json!(0)),
        ("priority", json!(1001)),
        ("priority", json!(1.5)),
        ("acceptance", json!([])),
        ("interactions", json!([{"trigger":"tap", "result":7}])),
        ("platforms", json!({"ios":{"status":"complete"}})),
        ("unknown", json!(true)),
        (
            "depends_on",
            json!(["library.open-library", "library.open-library"]),
        ),
    ];
    for (field, value) in mutations {
        let fixture = Fixture::new();
        fixture.mutate("reader", |v| v[0][field] = value);
        assert!(
            Catalog::load(&fixture.0).is_err(),
            "accepted invalid {field}"
        );
    }
    let fixture = Fixture::new();
    fixture.mutate("reader", |v| {
        v[0].as_object_mut().expect("entry").remove("acceptance");
    });
    assert!(Catalog::load(&fixture.0).is_err());
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("catalog.toml"),
        "typo = true\n[platforms]\nios = '.'",
    )
    .expect("write config");
    assert!(Catalog::load(&fixture.0).is_err());
}

#[test]
fn yaml_support_duplicate_keys_and_empty_catalog() {
    let fixture = Fixture::new();
    let path = fixture.0.join("catalog/reader.yaml");
    let value: Value =
        serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("JSON");
    fs::write(&path, serde_yaml::to_string(&value).expect("YAML")).expect("write");
    assert_eq!(fixture.load().entries.len(), 4);
    for text in [
        "[{\"id\":\"a.b\",\"id\":\"a.c\"}]",
        "- id: a.b\n  id: a.c",
        "[]",
        "[.nan]",
    ] {
        fs::write(&path, text).expect("write");
        assert!(Catalog::load(&fixture.0).is_err(), "accepted {text}");
    }
    fs::remove_dir_all(fixture.0.join("catalog")).expect("remove areas");
    assert!(Catalog::load(&fixture.0).is_err());
}

#[test]
fn dependencies_duplicates_cycles_and_retirements() {
    let fixture = Fixture::new();
    fixture.mutate("search", |v| v[0]["id"] = json!("reader.open-book"));
    assert!(Catalog::load(&fixture.0)
        .expect_err("duplicate")
        .to_string()
        .contains("duplicate ID"));
    for deps in [json!(["library.unknown"]), json!(["reader.open-book"])] {
        let fixture = Fixture::new();
        fixture.mutate("reader", |v| v[0]["depends_on"] = deps);
        assert!(Catalog::load(&fixture.0).is_err());
    }
    let fixture = Fixture::new();
    fixture.mutate("library", |v| {
        v[0]["depends_on"] = json!(["reader.open-book"])
    });
    assert!(Catalog::load(&fixture.0)
        .expect_err("cycle")
        .to_string()
        .contains("dependency cycle"));
    for record in [
        json!({"reader.open-book":{"reason":"removed","replaced_by":[]}}),
        json!({"reader.old-open":{"reason":"removed","replaced_by":["reader.unknown"]}}),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("retired-ids.json"), record.to_string()).expect("retirement");
        assert!(Catalog::load(&fixture.0).is_err());
    }
}

#[test]
fn paths_missing_unsafe_unqualified_tests_and_repo_override() {
    for evidence in [
        "../Reader.swift",
        "/etc/passwd",
        "C:\\Reader.swift",
        "missing.swift",
    ] {
        let fixture = Fixture::new();
        fixture.mutate("reader", |v| {
            v[0]["platforms"]["ios"]["source"] = json!([evidence])
        });
        assert!(
            fixture.load().validate_paths(None).is_err(),
            "accepted {evidence}"
        );
    }
    let fixture = Fixture::new();
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["ios"]["tests"] = json!(["MissingTests.swift"])
    });
    assert!(fixture.load().validate_paths(None).is_err());
    let fixture = Fixture::new();
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(
        fixture.0.join("catalog.toml"),
        format!("repo_root = 'unavailable'\n{config}"),
    )
    .expect("config");
    assert!(fixture.load().validate_paths(None).is_err());
    fixture
        .load()
        .validate_paths(Some(&fixture.0))
        .expect("override root");
    success(fixture.cli("validate", &["--repo", fixture.0.to_str().expect("path")]));
    // Entry and report inspection do not need a local source tree.
    success(fixture.cli("entry", &["reader.open-book"]));
    success(fixture.cli("report", &[]));
}

#[cfg(unix)]
#[test]
fn symlink_evidence_cannot_escape_platform_root() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink(
        fixture.0.join("catalog.toml"),
        fixture.0.join("native/outside.swift"),
    )
    .expect("symlink");
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["ios"]["source"] = json!(["outside.swift"])
    });
    assert!(fixture.load().validate_paths(None).is_err());
}

#[test]
fn legacy_prefixes_resolve_per_platform() {
    let fixture = Fixture::new();
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(
        fixture.0.join("catalog.toml"),
        format!("{config}\n[path_prefixes]\nios = 'reference/'\n"),
    )
    .expect("config");
    for area in ["library", "reader", "search"] {
        fixture.mutate(area, |entries| {
            for e in entries.as_array_mut().expect("entries") {
                for key in ["source", "tests"] {
                    for p in e["platforms"]["ios"][key].as_array_mut().expect("paths") {
                        *p = json!(format!("reference/{}", p.as_str().expect("path")));
                    }
                }
            }
        });
    }
    fixture.load().validate_paths(None).expect("prefix mapping");
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["ios"]["source"] = json!(["Reader.swift"])
    });
    assert!(fixture.load().validate_paths(None).is_err());
}

#[test]
fn ready_queue_impact_priority_area_limit_and_unknowns() {
    let fixture = Fixture::new();
    let catalog = fixture.load();
    let ids: Vec<_> = catalog
        .next("android", None, 10)
        .expect("queue")
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "search.find-book",
            "library.open-library",
            "reader.open-book"
        ]
    );
    assert!(catalog
        .next("android", None, 0)
        .expect("zero limit")
        .is_empty());
    assert!(catalog.next("unknown", None, 1).is_err());
    assert!(catalog.next("android", Some("unknown"), 1).is_err());
    fixture.mutate("library", |v| {
        v[0]["impact"] = json!("high");
        v[0]["priority"] = json!(1);
    });
    let catalog = fixture.load();
    let ids: Vec<_> = catalog
        .next("android", None, 10)
        .expect("queue")
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "library.open-library",
            "reader.open-book",
            "search.find-book"
        ]
    );
}

#[test]
fn report_order_baseline_and_markdown_escaping() {
    let fixture = Fixture::new();
    fixture.mutate("search", |v| {
        v[0]["title"] = json!("Find | books\nnow");
        v[0]["platforms"]["android"]["gap"] = json!("Missing | action\nnext");
    });
    let report = fixture.load().report().expect("report");
    assert!(report
        .contains("3 iOS capabilities: 0 Android implemented (0 verified), 1 partial, 2 missing"));
    assert!(report.contains("Find \\| books now | Missing \\| action next"));
    assert!(report.contains("## Android-specific adaptations"));
    fixture.mutate("search", |v| {
        v[0]["platforms"]["android"]["status"] = json!("not-applicable")
    });
    assert!(fixture
        .load()
        .report()
        .expect("report")
        .contains("1 not-applicable"));
}

#[test]
fn generators_are_deterministic_and_never_overwrite_edited_or_stale_files() {
    let fixture = Fixture::new();
    let catalog = fixture.load();
    let ids = catalog
        .identifiers(Language::Kotlin, Some("org.example.reader"))
        .expect("IDs");
    assert!(ids.contains("package `org`.`example`.`reader`"));
    assert!(ids.contains("const val entry_reader_d_open_h_book = \"reader.open\""));
    assert!(ids.contains("const val entry_library_d_open_h_library = \"library.open-library\""));
    assert_eq!(
        ids,
        catalog
            .identifiers(Language::Kotlin, Some("org.example.reader"))
            .expect("IDs")
    );
    for language in [Language::Kotlin, Language::Swift] {
        let out = fixture.0.join(format!("{language:?}"));
        let result = catalog.journeys(language, &out).expect("generate");
        assert_eq!(result.created.len(), 4);
        let file = &result.created[0];
        let text = fs::read_to_string(file).expect("stub");
        assert!(text.contains("// Given:"));
        assert!(text.contains("// When:"));
        assert!(text.contains("// Then:"));
        assert!(text.contains("TODO"));
        assert_eq!(
            catalog
                .journeys(language, &out)
                .expect("rerun")
                .unchanged
                .len(),
            4
        );
        fs::write(file, "// implemented by a human\n").expect("edit");
        assert_eq!(
            catalog
                .journeys(language, &out)
                .expect("preserve")
                .preserved,
            std::slice::from_ref(file)
        );
        assert_eq!(
            fs::read_to_string(file).expect("read"),
            "// implemented by a human\n"
        );
    }
}

#[test]
fn validation_collects_schema_dependencies_retirements_and_every_path() {
    let fixture = Fixture::new();
    fixture.mutate("reader", |v| {
        v[0]["title"] = json!("");
        v[0]["summary"] = json!("");
        v[0]["depends_on"] = json!(["reader.open-book", "missing.one", "missing.two"]);
        v[0]["platforms"]["ios"]["source"] = json!(["z.swift", "a.swift"]);
        v[0]["platforms"]["android"]["tests"] = json!(["Missing.kt", "runner"]);
    });
    fixture.mutate("search", |v| v[0]["title"] = json!(""));
    fs::write(
        fixture.0.join("retired-ids.json"),
        json!({
            "reader.open-book": {"reason":"retired", "replaced_by":["gone.one", "gone.two"]},
            "search.find-book": {"reason":"retired", "replaced_by":[]}
        })
        .to_string(),
    )
    .expect("retirements");
    let report =
        Catalog::validate(&fixture.0, &concordance::RootOptions::default()).expect("validation");
    for (group, count) in [
        ("schema", 3),
        ("dependencies", 3),
        ("retired", 4),
        ("evidence", 3),
    ] {
        assert_eq!(
            report.errors.iter().filter(|i| i.group == group).count(),
            count,
            "{report}"
        );
    }
    assert!(report.errors.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(report.clone().into_result().is_err());
    let output = fixture.cli("validate", &[]);
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("13 validation error(s)"), "{text}");
    assert!(text.contains("a.swift") && text.contains("z.swift") && text.contains("Missing.kt"));
}

#[test]
fn validation_continues_after_unparseable_documents() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("catalog/library.yaml"), "[bad").expect("bad YAML");
    fixture.mutate("reader", |v| {
        v[0]["title"] = json!("");
        v[0]["platforms"]["ios"]["source"] = json!(["Missing.swift"]);
    });
    let report =
        Catalog::validate(&fixture.0, &concordance::RootOptions::default()).expect("report");
    assert_eq!(
        report.errors.iter().filter(|i| i.group == "schema").count(),
        2
    );
    assert!(report.errors.iter().any(|i| i.reference == "Missing.swift"));
}

fn configure_external(fixture: &Fixture) {
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(fixture.0.join("catalog.toml"), format!("{config}\n[external_roots.ios]\ndescription = 'Read-only reference checkout'\ngit = 'https://example.com/reference.git'\ncommit = 'HEAD'\n")).expect("config");
}

#[test]
fn external_roots_are_optional_and_explicit_roots_override_env() {
    let fixture = Fixture::new();
    let external = Fixture::new();
    configure_external(&fixture);
    let output = success(fixture.cli("validate", &[]));
    assert!(output.contains("unchecked (root not supplied)"));
    assert!(output.contains("https://example.com/reference.git"));
    assert!(output.contains("CONCORDANCE_ROOT_IOS"));
    assert!(!fixture
        .cli("validate", &["--require-roots"])
        .status
        .success());
    let root = external.0.join("native");
    let root_arg = format!("ios={}", root.display());
    for command in ["validate", "entry", "next", "report"] {
        let mut args = vec!["--root", &root_arg];
        if command == "entry" {
            args.push("reader.open-book");
        }
        if command == "next" {
            args.extend(["--platform", "android"]);
        }
        // Do not request drift with this non-git fixture's pin.
        if command == "report" {
            continue;
        }
        success(fixture.cli(command, &args));
    }
    let env_run = |explicit: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_concordance"));
        command.arg("validate").arg(&fixture.0).env(
            "CONCORDANCE_ROOT_IOS",
            if explicit {
                fixture.0.join("does-not-exist")
            } else {
                root.clone()
            },
        );
        if explicit {
            command.args(["--root", &root_arg]);
        }
        success(command.output().expect("CLI"))
    };
    assert!(env_run(false).contains("0 unchecked"));
    assert!(env_run(true).contains("0 unchecked"));
    assert!(!fixture
        .cli("validate", &["--root", "unknown=."])
        .status
        .success());
    assert!(!fixture
        .cli("validate", &["--root", "ios="])
        .status
        .success());
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["ios"]["source"] = json!(["../catalog.toml"])
    });
    assert!(!fixture
        .cli("validate", &["--root", &root_arg])
        .status
        .success());
    assert!(!fixture.cli("validate", &[]).status.success()); // Unsafe paths fail even without a root.
}

fn git_test(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git UTF8")
        .trim()
        .to_owned()
}
fn commit_test(root: &Path) -> String {
    git_test(root, &["add", "native"]);
    git_test(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "fixture",
        ],
    );
    git_test(root, &["rev-parse", "HEAD"])
}

#[test]
fn git_baseline_drift_tracks_shared_evidence_deletions_globs_and_subdirectory_roots() {
    let fixture = Fixture::new();
    let external = Fixture::new();
    git_test(&external.0, &["init", "-q"]);
    let before = commit_test(&external.0);
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(
        fixture.0.join("catalog.toml"),
        format!(
            "{config}\n[baseline]\ncommit = '{before}'\n[source_globs]\nios = ['**/*.swift']\n"
        ),
    )
    .expect("config");
    fs::remove_file(external.0.join("native/Reader.swift")).expect("delete evidence");
    fs::write(external.0.join("native/Uncited file.swift"), "new").expect("uncited file");
    fs::write(external.0.join("native/ignore.txt"), "ignore").expect("non-source");
    let after = commit_test(&external.0);
    let options = concordance::RootOptions {
        roots: [("ios".into(), external.0.join("native"))].into(),
        ..Default::default()
    };
    let catalog = fixture.load();
    let drift = catalog
        .drift(&options, Some("ios"), None, "HEAD")
        .expect("drift");
    let ios = &drift.platforms[0];
    assert_eq!(ios.uncatalogued, ["Uncited file.swift"]);
    assert!(!ios.entries.is_empty());
    assert!(ios.entries.iter().all(|e| e.files == ["Reader.swift"]));
    let unchanged = catalog
        .drift(&options, Some("ios"), Some(&after), "HEAD")
        .expect("override");
    assert!(unchanged.platforms[0].entries.is_empty());
    assert!(unchanged.platforms[0].uncatalogued.is_empty());
    assert!(catalog
        .drift(&options, Some("ios"), Some("--bad"), "HEAD")
        .is_err());
    assert!(catalog
        .drift(&options, Some("unknown"), None, "HEAD")
        .is_err());
    let report = catalog.report_with_roots(&options).expect("parity report");
    assert!(report.contains("## Baseline drift") && report.contains("target status is stale"));
    let root_arg = format!("ios={}", external.0.join("native").display());
    let json: Value = serde_json::from_str(&success(fixture.cli(
        "drift",
        &["--root", &root_arg, "--platform", "ios", "--format", "json"],
    )))
    .expect("drift JSON");
    assert_eq!(
        json["platforms"][0]["uncatalogued"][0],
        "Uncited file.swift"
    );
    assert!(success(fixture.cli(
        "drift",
        &[
            "--root",
            &root_arg,
            "--platform",
            "ios",
            "--since",
            &after,
            "--to",
            &before
        ]
    ))
    .contains("Reader.swift"));
    success(fixture.cli("report", &["--root", &root_arg]));
    success(fixture.cli("report", &["--root", &root_arg, "--check"]));
}

#[test]
fn per_platform_pins_override_shared_baseline_and_prefixes_are_stripped_for_drift() {
    let fixture = Fixture::new();
    git_test(&fixture.0, &["init", "-q"]);
    let before = commit_test(&fixture.0);
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(fixture.0.join("catalog.toml"), format!("{config}\n[path_prefixes]\nios = 'reference/'\n[baseline]\ncommit = 'invalid'\n[baselines.ios]\ncommit = '{before}'\n")).expect("config");
    for area in ["library", "reader", "search"] {
        fixture.mutate(area, |entries| {
            for entry in entries.as_array_mut().expect("entries") {
                for key in ["source", "tests"] {
                    for reference in entry["platforms"]["ios"][key]
                        .as_array_mut()
                        .expect("paths")
                    {
                        *reference =
                            json!(format!("reference/{}", reference.as_str().expect("path")));
                    }
                }
            }
        });
    }
    fs::write(fixture.0.join("native/ReaderTests.swift"), "changed").expect("change");
    commit_test(&fixture.0);
    let drift = fixture
        .load()
        .drift(&Default::default(), Some("ios"), None, "HEAD")
        .expect("drift");
    assert_eq!(drift.platforms[0].since, before);
    assert!(drift.platforms[0]
        .entries
        .iter()
        .any(|e| e.files == ["ReaderTests.swift"]));
}

#[cfg(unix)]
#[test]
fn explicit_roots_preserve_symlink_containment() {
    let fixture = Fixture::new();
    let external = Fixture::new();
    std::os::unix::fs::symlink(
        fixture.0.join("catalog.toml"),
        external.0.join("native/escape.swift"),
    )
    .expect("symlink");
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["ios"]["source"] = json!(["escape.swift"])
    });
    let root = format!("ios={}", external.0.join("native").display());
    assert!(!fixture.cli("validate", &["--root", &root]).status.success());
}

#[test]
fn drift_marks_missing_pins_and_roots_unchecked() {
    let fixture = Fixture::new();
    configure_external(&fixture);
    let output = success(fixture.cli("drift", &[]));
    assert!(output.contains("ios: unchecked (root not supplied)"));
    assert!(output.contains("android: unchecked (baseline commit not supplied)"));
    assert!(!fixture
        .cli("drift", &["--platform", "ios", "--require-roots"])
        .status
        .success());
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(
        fixture.0.join("catalog.toml"),
        config.replace("ios = \"native\"", "ios = \"missing-checkout\""),
    )
    .expect("config");
    // Missing external roots do not prevent parity reports.
    success(fixture.cli("report", &[]));
    assert!(fs::read_to_string(fixture.0.join("parity.md"))
        .expect("report")
        .contains("unchecked (root not supplied)"));
}

#[test]
fn verification_is_derived_from_independent_evidence() {
    let fixture = Fixture::new();
    let set = |verification: Value, implementer: &str, status: &str| {
        fixture.mutate("reader", |v| {
            let android = &mut v[1]["platforms"]["android"];
            android["status"] = json!(status);
            android["implemented_by"] = json!(implementer);
            android["verification"] = verification;
        });
    };
    // A self-review is a validation error and never counts as verified.
    set(
        json!({"by": "sol", "at": "2026-11-20", "journeys": ["ReaderTests.swift#ReaderTests.testOpen"]}),
        "sol",
        "implemented",
    );
    let output = fixture.cli("validate", &[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("verifier must differ"));
    let catalog = fixture.load_collect_only();
    let record = &catalog.entries["reader.native-menu"].platforms["android"];
    assert_eq!(record.status, concordance::Status::Implemented);
    assert_eq!(
        record.unverified_reason(),
        Some("verifier is the implementer")
    );

    // A record with no captures or journeys is accepted but unverified.
    set(json!({"by": "agy", "at": "2026-11-20"}), "sol", "shipped");
    success(fixture.cli("validate", &[]));
    let report = fixture.load().report().expect("report");
    assert!(report.contains("1 of 1 Android implemented entries (all entries, including any outside the iOS baseline) have no independent verification"));
    assert!(report.contains("verification has no captures or journeys"));

    // An independent journey verifies it; the report counts it separately from implemented.
    set(
        json!({"by": "agy", "at": "2026-11-20", "commit": "abcdef1", "journeys": ["ReaderTests.swift#ReaderTests.testOpen"]}),
        "sol",
        "shipped",
    );
    success(fixture.cli("validate", &[]));
    // The entry writes state, so by default it needs a capture as well as a journey.
    let report = fixture.load().report().expect("report");
    assert!(report.contains("verification lacks a required capture (verify_requires)"));
    fixture.mutate("reader", |v| v[1]["verify_requires"] = json!(["journey"]));
    let catalog = fixture.load();
    assert!(catalog.entries["reader.native-menu"].platforms["android"].verified());
    let report = catalog.report().expect("report");
    assert!(report.contains("0 of 1 Android implemented entries"));
    // The tool writes `implemented`; `shipped` is only a read alias.
    let json: Value = serde_json::from_str(&success(fixture.cli("entry", &["reader.native-menu"])))
        .expect("entry JSON");
    assert_eq!(json["platforms"]["android"]["status"], "implemented");
    // A purely visual entry (no writes) verifies on a capture alone.
    fs::write(fixture.0.join("shot.png"), "png").expect("capture");
    fixture.mutate("reader", |v| {
        v[1].as_object_mut()
            .expect("entry")
            .remove("verify_requires");
        v[1]["data"]["writes"] = json!([]);
        v[1]["platforms"]["android"]["verification"] =
            json!({"by": "agy", "at": "2026-11-20", "captures": ["shot.png"]});
    });
    success(fixture.cli("validate", &[]));
    assert!(fixture
        .load()
        .report()
        .expect("report")
        .contains("0 of 1 Android implemented entries"));
    // verification requires implemented_by.
    fixture.mutate("reader", |v| {
        v[1]["platforms"]["android"]
            .as_object_mut()
            .expect("record")
            .remove("implemented_by");
    });
    let output = fixture.cli("validate", &[]);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("verification requires implemented_by")
    );
    set(
        json!({"by": "agy", "at": "2026-11-20", "commit": "abcdef1", "journeys": ["ReaderTests.swift#ReaderTests.testOpen"]}),
        "sol",
        "shipped",
    );
    fixture.mutate("reader", |v| v[1]["verify_requires"] = json!(["journey"]));
    let catalog = fixture.load();
    assert!(catalog
        .brief("reader.native-menu")
        .expect("brief")
        .contains("verified by agy on 2026-11-20 at abcdef1"));

    // Evidence and semantics: missing capture files, bad dates and commits, verified missing work.
    set(
        json!({"by": "agy", "at": "2026-13-01", "commit": "not-a-sha", "captures": ["build/missing.png"], "journeys": ["Gone.kt#GoneTest.run"]}),
        "sol",
        "missing",
    );
    let stderr = String::from_utf8(fixture.cli("validate", &[]).stderr).expect("UTF8");
    for expected in [
        "invalid date",
        "commit must be a hexadecimal commit SHA",
        "missing capture",
        "Gone.kt#GoneTest.run",
        "verification recorded for a missing status",
    ] {
        assert!(stderr.contains(expected), "missing {expected}: {stderr}");
    }
}

#[test]
fn detail_items_carry_numbers_status_and_baseline_evidence() {
    let fixture = Fixture::new();
    let details = |extra: Value| {
        fixture.mutate("reader", |v| {
            let mut items = vec![
                json!({
                    "id": "open-spring",
                    "kind": "motion",
                    "spec": {"text": "Menu opens with a settling spring", "motion": {"curve": "spring", "response": 0.4, "damping": 0.8}},
                    "baseline": ["Reader.swift:1"],
                    "platforms": {"android": {"status": "missing", "gap": "Uses a tween"}}
                }),
                json!({
                    "id": "tap-haptic",
                    "kind": "haptic",
                    "spec": {"text": "Light impact on open", "haptic": {"kind": "light-impact"}},
                    "baseline": ["Reader.swift#Reader"],
                    "platforms": {"android": {"status": "shipped", "implemented_by": "sol",
                        "verification": {"by": "agy", "at": "2026-11-20", "journeys": ["ReaderTests.swift#ReaderTests.testOpen"]}}}
                }),
            ];
            if !extra.is_null() {
                items.push(extra);
            }
            v[1]["details"] = json!(items);
        });
    };
    details(Value::Null);
    success(fixture.cli("validate", &[]));
    let catalog = fixture.load();
    let report = catalog.report().expect("report");
    assert!(report.contains("Details implemented / verified / total"));
    assert!(report.contains("## Detail gaps in implemented entries"));
    assert!(report.contains(
        "| `reader.native-menu#open-spring` | motion | missing | Menu opens with a settling spring (motion spring response 0.4 damping 0.8) |"
    ));
    assert!(!report.contains("reader.native-menu#tap-haptic"));

    // Briefs and the work queue expose the numbers to implementing agents.
    let brief = success(fixture.cli(
        "entry",
        &["reader.native-menu", "--format", "brief", "--details"],
    ));
    assert!(brief.contains("## Details"));
    assert!(brief.contains("Spec: motion spring response 0.4 damping 0.8"));
    assert!(brief.contains("Baseline (ios): Reader.swift:1"));
    assert!(brief.contains("android: implemented; verified by agy"));
    let plain = success(fixture.cli("entry", &["reader.native-menu", "--format", "brief"]));
    assert!(!plain.contains("## Details"));
    let queue = success(fixture.cli("next", &["--platform", "android", "--details"]));
    assert!(queue.contains("reader.native-menu\tmedium\timplemented"));
    assert!(queue.contains(
        "reader.native-menu#open-spring\tdetail:motion\tmissing\tMenu opens with a settling spring [motion spring response 0.4 damping 0.8]"
    ));
    let queue = success(fixture.cli("next", &["--platform", "android"]));
    assert!(!queue.contains("reader.native-menu"));

    // Invalid details are collected with the entry#detail label.
    details(
        json!({"id": "open-spring", "kind": "motion", "baseline": ["Gone.swift:3"],
        "spec": {"text": "Duplicate", "motion": {"curve": "ease", "response": 0.3}}}),
    );
    let stderr = String::from_utf8(fixture.cli("validate", &[]).stderr).expect("UTF8");
    for expected in [
        "duplicate detail ID",
        "ease and linear motion need a duration",
        "reader.native-menu#open-spring ios Gone.swift:3: missing evidence",
    ] {
        assert!(stderr.contains(expected), "missing {expected}: {stderr}");
    }
    details(json!({"id": "Bad_Id", "kind": "sound", "spec": {"text": "x"}}));
    let stderr = String::from_utf8(fixture.cli("validate", &[]).stderr).expect("UTF8");
    assert!(stderr.contains("invalid slug"), "{stderr}");
    assert!(stderr.contains("invalid value \"sound\""), "{stderr}");
}

#[test]
fn drift_matches_symbol_and_line_anchors_against_changed_hunks() {
    let fixture = Fixture::new();
    let player = "struct Player {\n    func play() {\n        start()\n    }\n\n    func pause() {\n        stop()\n    }\n}\n";
    fs::write(fixture.0.join("native/Player.swift"), player).expect("player");
    git_test(&fixture.0, &["init", "-q"]);
    let before = commit_test(&fixture.0);
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(
        fixture.0.join("catalog.toml"),
        format!("{config}\n[baseline]\ncommit = '{before}'\n"),
    )
    .expect("config");
    let cite = |area: &str, index: usize, source: &str| {
        fixture.mutate(area, |v| {
            v[index]["platforms"]["ios"]["source"] = json!([source]);
            v[index]["platforms"]["ios"]["tests"] = json!([]);
        });
    };
    cite("reader", 0, "Player.swift#Player.pause");
    cite("reader", 1, "Player.swift:L2-L4");
    cite("search", 0, "Player.swift");
    cite("library", 0, "Player.swift#missingSymbol");
    // Change only the body of pause().
    fs::write(
        fixture.0.join("native/Player.swift"),
        player.replace("stop()", "stop(fade: true)"),
    )
    .expect("edit");
    commit_test(&fixture.0);
    let drift = fixture
        .load()
        .drift(&Default::default(), Some("ios"), None, "HEAD")
        .expect("drift");
    let ios = &drift.platforms[0];
    let ids: Vec<&str> = ios.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "library.open-library",
            "reader.open-book",
            "search.find-book"
        ]
    );
    let pause = &ios.entries[1].hits[0];
    assert_eq!(pause.precision, concordance::Precision::Precise);
    assert_eq!(pause.lines, Some((6, 8)));
    assert_eq!(
        ios.entries[0].hits[0].note.as_deref(),
        Some("symbol not found at baseline")
    );
    assert_eq!(
        ios.entries[2].hits[0].precision,
        concordance::Precision::Coarse
    );
    assert_eq!(
        (ios.precise_hits, ios.coarse_hits, ios.anchored_unaffected),
        (1, 2, 1)
    );
    let markdown = drift.markdown();
    assert!(markdown.contains("`Player.swift#Player.pause` precise (L6-L8)"));
    assert!(markdown.contains("`Player.swift` file-level (coarse)"));
    assert!(markdown.contains("3 entries: 1 with precise hits, 2 file-level (coarse) only"));
    // Deleting the file is a precise hit for every anchored reference.
    fs::remove_file(fixture.0.join("native/Player.swift")).expect("delete");
    commit_test(&fixture.0);
    let drift = fixture
        .load()
        .drift(&Default::default(), Some("ios"), None, "HEAD")
        .expect("drift");
    let ios = &drift.platforms[0];
    assert_eq!(ios.precise_hits, 2);
    assert!(drift.markdown().contains("file deleted"));
}

#[test]
fn report_links_resolve_from_the_output_location() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("docs/out")).expect("mkdir");
    success(fixture.cli(
        "report",
        &[
            "--out",
            fixture.0.join("docs/out/parity.md").to_str().expect("path"),
        ],
    ));
    let report = fs::read_to_string(fixture.0.join("docs/out/parity.md")).expect("report");
    assert!(
        report.contains("[reader](../../catalog/reader.yaml)"),
        "{report}"
    );
    assert!(report.contains("[README](../../README.md)"));
    success(fixture.cli(
        "report",
        &[
            "--out",
            fixture.0.join("docs/out/parity.md").to_str().expect("path"),
            "--check",
        ],
    ));
}

fn append_config(fixture: &Fixture, extra: &str) {
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    let config = config.replacen("[platforms]\n", &format!("[platforms]\n{}", ""), 1);
    fs::write(
        fixture.0.join("catalog.toml"),
        format!("{config}\n{extra}\n"),
    )
    .expect("config");
}

#[test]
fn blockers_hold_back_work_transitively_and_rank_in_the_report() {
    let fixture = Fixture::new();
    append_config(
        &fixture,
        "[blockers.engine]\ntitle = 'On-device engine'\nkind = 'dependency'\nowner = 'Mike'\nlink = 'docs/decisions.md#1'\n\n[blockers.old]\ntitle = 'Settled question'\nstatus = 'resolved'\n",
    );
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["android"]["blocked_by"] = json!(["engine"])
    });
    fixture.mutate("search", |v| {
        v[0]["depends_on"] = json!(["reader.open-book"])
    });
    success(fixture.cli("validate", &[]));
    let queue = success(fixture.cli("next", &["--platform", "android"]));
    assert!(queue.starts_with("library.open-library"));
    assert!(!queue.contains("reader.open-book") && !queue.contains("search.find-book"));
    let all = success(fixture.cli("next", &["--platform", "android", "--include-blocked"]));
    assert!(all.contains("reader.open-book\tcritical\tpartial\tOpen a book [blocked: engine]"));
    assert!(all.contains("search.find-book\thigh\tmissing\tFind a book [blocked: engine]"));
    let report = fixture.load().report().expect("report");
    for expected in [
        "## Ten highest-impact actionable gaps",
        "## Ten highest-impact blocked gaps",
        "Blocked by engine. Implement the native journey.",
        "| `engine` | On-device engine | dependency | Mike | open | 2 | 0 | critical | docs/decisions.md#1 |",
        " Blocked |",
    ] {
        assert!(report.contains(expected), "missing {expected}:\n{report}");
    }
    let engine = report.find("`engine`").expect("engine row");
    assert!(engine < report.find("| `old`").expect("old row"));
    let brief = success(fixture.cli("entry", &["reader.open-book", "--format", "brief"]));
    assert!(brief.contains("Blocked by engine: On-device engine (docs/decisions.md#1) [open]."));
    // Unknown keys and blockers on implemented work are errors; resolved ones warn.
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["android"]["blocked_by"] = json!(["nope", "old"]);
        v[1]["platforms"]["android"]["blocked_by"] = json!(["engine"]);
    });
    let stderr = String::from_utf8(fixture.cli("validate", &[]).stderr).expect("UTF8");
    assert!(stderr.contains("unknown blocker"), "{stderr}");
    assert!(
        stderr.contains("blocked_by on a implemented record"),
        "{stderr}"
    );
    assert!(
        stderr.contains("blocker is resolved; revisit this record"),
        "{stderr}"
    );
}

#[test]
fn set_edits_records_in_place_and_refuses_invalid_results() {
    let fixture = Fixture::new();
    let path = fixture.0.join("catalog/reader.yaml");
    let before = fs::read_to_string(&path).expect("area");
    let dir = fixture.0.to_str().expect("path");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .arg("set")
            .arg(dir)
            .args(args)
            .output()
            .expect("CLI")
    };
    let output = run(&[
        "reader.open-book",
        "--platform",
        "android",
        "--status",
        "implemented",
        "--by",
        "sol",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // An implemented record that keeps its gap is a warning, and it is reported.
    assert!(String::from_utf8_lossy(&output.stderr).contains("implemented record still has a gap"));
    let after = fs::read_to_string(&path).expect("area");
    // Only the edited lines differ: status, the gap line's comma, and one added field.
    let old: std::collections::BTreeSet<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().filter(|l| !old.contains(l)).collect();
    assert_eq!(after.lines().count(), before.lines().count() + 1);
    assert_eq!(new_lines.len(), 3, "{new_lines:?}");
    assert!(after.contains("\"implemented_by\": \"sol\""));
    assert!(run(&[
        "reader.open-book",
        "--platform",
        "android",
        "--clear",
        "gap"
    ])
    .status
    .success());
    assert!(!fs::read_to_string(&path)
        .expect("area")
        .contains("\"gap\": \"Implement the native journey.\",\n        \"implemented_by\""));
    // Leaving verification on a missing record is refused, and the file is restored.
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["android"]["verification"] =
            json!({"by": "agy", "at": "2026-11-20", "captures": ["shot.png"]});
    });
    fs::write(fixture.0.join("shot.png"), "png").expect("capture");
    let snapshot = fs::read_to_string(&path).expect("area");
    let output = run(&[
        "reader.open-book",
        "--platform",
        "android",
        "--status",
        "missing",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--clear verification"));
    assert_eq!(fs::read_to_string(&path).expect("area"), snapshot);
    assert!(run(&[
        "reader.open-book",
        "--platform",
        "android",
        "--status",
        "missing",
        "--clear",
        "verification"
    ])
    .status
    .success());
    assert!(!run(&[
        "reader.open-book",
        "--platform",
        "android",
        "--clear",
        "title"
    ])
    .status
    .success());
    // Details are addressed as entry/detail or entry#detail; a missing record needs --status.
    fixture.mutate("reader", |v| {
        v[1]["details"] =
            json!([{"id": "tap-haptic", "kind": "haptic", "spec": {"text": "Light impact"}}]);
    });
    assert!(!run(&[
        "reader.native-menu/tap-haptic",
        "--platform",
        "android",
        "--gap",
        "x"
    ])
    .status
    .success());
    let output = run(&[
        "reader.native-menu/tap-haptic",
        "--platform",
        "android",
        "--status",
        "partial",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The implemented entry now has an open detail: validate warns.
    let stdout = success(fixture.cli("validate", &[]));
    assert!(
        stdout.contains("tap-haptic: implemented record has open details"),
        "{stdout}"
    );
    assert!(!fixture.cli("validate", &["--strict"]).status.success());
}

#[test]
fn geometry_lists_motion_tokens_and_detail_journey_requirements() {
    let fixture = Fixture::new();
    let details = |items: Value| fixture.mutate("reader", |v| v[1]["details"] = items);
    details(json!([
        {"id": "cover", "kind": "geometry", "spec": {"text": "Mini player cover", "geometry": [
            {"property": "width", "value": 38, "unit": "pt"},
            {"property": "corner-radius", "value": 8, "unit": "pt"}]}},
        {"id": "fold", "kind": "motion", "spec": {"text": "Fold open", "motion": {"token": "Motion.fold"}},
         "platforms": {"android": {"status": "implemented", "implemented_by": "sol",
            "verification": {"by": "agy", "at": "2026-11-20", "captures": ["shot.png"]}}}}
    ]));
    fs::write(fixture.0.join("shot.png"), "png").expect("capture");
    success(fixture.cli("validate", &[]));
    let catalog = fixture.load();
    let entry = &catalog.entries["reader.native-menu"];
    assert_eq!(
        entry.details[0].spec.numbers(),
        "width = 38pt; corner-radius = 8pt"
    );
    assert_eq!(entry.details[1].spec.numbers(), "motion token Motion.fold");
    // A still capture cannot verify motion: it needs a journey.
    let record = &entry.details[1].platforms["android"];
    assert_eq!(
        entry.details[1].unverified_reason(record).as_deref(),
        Some("verification lacks a required journey (verify_requires)")
    );
    details(json!([
        {"id": "a", "kind": "motion", "spec": {"text": "x", "motion": {}}},
        {"id": "b", "kind": "motion", "spec": {"text": "x", "motion": {"token": "t", "response": 0.3}}}
    ]));
    let stderr = String::from_utf8(fixture.cli("validate", &[]).stderr).expect("UTF8");
    for expected in [
        "motion needs a curve or a token",
        "motion numbers need a curve",
    ] {
        assert!(stderr.contains(expected), "missing {expected}: {stderr}");
    }
    details(
        json!([{"id": "c", "kind": "geometry", "spec": {"text": "x", "geometry": [{"property": "w"}]}}]),
    );
    let stderr = String::from_utf8(fixture.cli("validate", &[]).stderr).expect("UTF8");
    assert!(
        stderr.contains("matches none of the allowed forms"),
        "{stderr}"
    );
}

#[test]
fn rebase_anchors_moves_ranges_and_flags_edited_or_lost_anchors() {
    let fixture = Fixture::new();
    let player = "struct Player {\n    func play() {\n        start()\n    }\n\n    func pause() {\n        stop()\n    }\n}\n";
    fs::write(fixture.0.join("native/Player.swift"), player).expect("player");
    git_test(&fixture.0, &["init", "-q"]);
    let before = commit_test(&fixture.0);
    append_config(&fixture, &format!("[baselines.ios]\ncommit = '{before}'\n"));
    let cite = |area: &str, index: usize, source: Value| {
        fixture.mutate(area, |v| {
            v[index]["platforms"]["ios"]["source"] = source;
            v[index]["platforms"]["ios"]["tests"] = json!([]);
        });
    };
    cite("reader", 0, json!(["Player.swift:L2-L4"]));
    cite("reader", 1, json!(["Player.swift:L6-L8"]));
    cite(
        "search",
        0,
        json!(["Player.swift#Player.play", "Player.swift#Player.pause"]),
    );
    let edited = format!(
        "// header\n// more\n{}",
        player
            .replace("stop()", "stop(fade: true)")
            .replace("    func pause() {", "    func halt() {")
    );
    fs::write(fixture.0.join("native/Player.swift"), edited).expect("edit");
    commit_test(&fixture.0);
    let catalog = fixture.load();
    let report = catalog
        .rebase_anchors(&Default::default(), "ios", None, "HEAD", false, false)
        .expect("rebase");
    let fate = |r: &str| {
        report
            .moves
            .iter()
            .find(|m| m.reference == r)
            .expect(r)
            .clone()
    };
    assert_eq!(
        fate("Player.swift:L2-L4").new_reference.as_deref(),
        Some("Player.swift:L4-L6")
    );
    assert_eq!(
        fate("Player.swift:L6-L8").fate,
        concordance::AnchorFate::Changed
    );
    assert_eq!(
        fate("Player.swift#Player.play").fate,
        concordance::AnchorFate::Unchanged
    );
    assert_eq!(
        fate("Player.swift#Player.pause").fate,
        concordance::AnchorFate::Lost
    );
    assert!(report
        .markdown()
        .contains("1 moved, 1 changed (review), 1 lost"));
    let dir = fixture.0.to_str().expect("path");
    let output = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args(["rebase-anchors", dir, "--to", "HEAD", "--write"])
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::read_to_string(fixture.0.join("catalog/reader.yaml"))
        .expect("area")
        .contains("\"Player.swift:L4-L6\""));
}

#[test]
fn flavour_platforms_inherit_records_unless_overridden() {
    let fixture = Fixture::new();
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(
        fixture.0.join("catalog.toml"),
        config.replace(
            "android = \"native\"\n",
            "android = \"native\"\nhorizon = { root = \"native\", inherits = \"android\" }\n",
        ),
    )
    .expect("config");
    fixture.mutate("reader", |v| {
        v[0]["platforms"]["horizon"] = json!({"status": "missing", "gap": "No headset build"})
    });
    fixture.mutate("search", |v| {
        v[0]["platforms"]["android"]["source"] = json!(["Gone.kt"])
    });
    let catalog = fixture.load_collect_only();
    let open = &catalog.entries["reader.open-book"];
    assert_eq!(
        open.platforms["horizon"].status,
        concordance::Status::Missing
    );
    // Overrides keep inherited evidence fields.
    assert_eq!(
        open.platforms["horizon"].source,
        open.platforms["android"].source
    );
    let search = &catalog.entries["search.find-book"].platforms["horizon"];
    assert!(search.inherited);
    // An explicit null unsets an inherited key.
    fixture.mutate("search", |v| {
        v[0]["platforms"]["horizon"] = json!({"gap": null, "source": null})
    });
    let catalog = fixture.load_collect_only();
    let unset = &catalog.entries["search.find-book"].platforms["horizon"];
    assert!(unset.gap.is_none() && unset.source.is_empty() && !unset.inherited);
    assert_eq!(unset.status, concordance::Status::Missing);
    fixture.mutate("search", |v| {
        v[0]["platforms"]
            .as_object_mut()
            .expect("platforms")
            .remove("horizon");
    });
    // Inherited copies are validated once, through the parent.
    let stderr = String::from_utf8(fixture.cli("validate", &[]).stderr).expect("UTF8");
    assert_eq!(stderr.matches("Gone.kt").count(), 1, "{stderr}");
    let queue = success(fixture.cli("next", &["--platform", "horizon"]));
    assert!(queue.contains("reader.open-book\tcritical\tmissing"));
    // set --unset writes null on the inheriting platform; --unset elsewhere is refused.
    let dir = fixture.0.to_str().expect("path");
    let set = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .args(["set", dir])
            .args(args)
            .output()
            .expect("CLI")
    };
    let output = set(&[
        "search.find-book",
        "--platform",
        "horizon",
        "--status",
        "missing",
        "--unset",
        "gap",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fixture.load_collect_only().entries["search.find-book"].platforms["horizon"]
            .gap
            .is_none()
    );
    assert!(!set(&[
        "search.find-book",
        "--platform",
        "android",
        "--unset",
        "gap"
    ])
    .status
    .success());
    let config = fs::read_to_string(fixture.0.join("catalog.toml")).expect("config");
    fs::write(
        fixture.0.join("catalog.toml"),
        config.replace("inherits = \"android\"", "inherits = \"nowhere\""),
    )
    .expect("config");
    assert!(
        String::from_utf8_lossy(&fixture.cli("validate", &[]).stderr)
            .contains("inherits an unknown platform")
    );
}
