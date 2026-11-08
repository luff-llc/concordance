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
    assert!(report.contains("3 iOS capabilities: 0 Android shipped, 1 partial, 2 missing"));
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
