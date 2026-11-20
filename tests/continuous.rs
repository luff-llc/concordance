//! Continuous parity: gate, sync, gate-release and strings export.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-continuous-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).expect("mkdir");
            for e in fs::read_dir(from).expect("fixture") {
                let e = e.expect("entry");
                let dest = to.join(e.file_name());
                if e.path().is_dir() {
                    copy(&e.path(), &dest);
                } else {
                    fs::copy(e.path(), dest).expect("copy");
                }
            }
        }
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
            &path,
        );
        Self(path)
    }
    fn mutate(&self, area: &str, edit: impl FnOnce(&mut Value)) {
        let path = self.0.join(format!("catalog/{area}.yaml"));
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("JSON");
        edit(&mut value);
        fs::write(
            path,
            serde_json::to_string_pretty(&value).expect("serialize") + "\n",
        )
        .expect("write");
    }
    fn config(&self, extra: &str) {
        let config = fs::read_to_string(self.0.join("catalog.toml")).expect("config");
        fs::write(self.0.join("catalog.toml"), format!("{config}\n{extra}\n")).expect("config");
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF8")
            .trim()
            .to_owned()
    }
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
        ]);
        self.git(&["rev-parse", "HEAD"])
    }
    fn cli(&self, command: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .arg(command)
            .arg(&self.0)
            .args(args)
            .output()
            .expect("CLI")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn gate_requires_catalog_changes_for_user_visible_files_or_a_trailer() {
    let f = Fixture::new();
    f.config(
        "[source_globs.ios]\nall = ['**/*.swift']\nuser_visible = ['Reader.swift', 'New*.swift']\n",
    );
    f.git(&["init", "-q"]);
    let base = f.commit("base");
    fs::write(
        f.0.join("native/Reader.swift"),
        "struct Reader { let title = \"Open\" }\n",
    )
    .expect("edit");
    fs::write(f.0.join("native/Helper.swift"), "// not user-visible\n").expect("helper");
    let head = f.commit("Change the reader");
    let gate = |base: &str, head: &str| {
        f.cli(
            "gate",
            &["--platform", "ios", "--base", base, "--head", head],
        )
    };
    let output = gate(&base, &head);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let out = text(&output);
    assert!(out.contains("`Reader.swift`: cited by `library.open-library`, `reader.open-book`, `search.find-book`, whose catalog files did not change"), "{out}");
    assert!(!out.contains("Helper.swift"));
    // Updating one citing entry's area file covers it.
    f.mutate("reader", |v| v[0]["notes"] = json!(["Title copy changed."]));
    let covered = f.commit("Record the reader change");
    let output = gate(&base, &covered);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    // An uncited user-visible file needs some catalog change, or a trailer.
    fs::write(f.0.join("native/NewScreen.swift"), "struct NewScreen {}\n").expect("new");
    let uncited = f.commit("Add a screen");
    assert_eq!(gate(&covered, &uncited).status.code(), Some(1));
    let waived = f.commit("Tidy\n\nCatalog: none — internal preview only");
    let output = gate(&covered, &waived);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        text(&output).contains("Waived by commit trailer: `Catalog: none — internal preview only`")
    );
    // Patch mode, as an integration script would call it.
    let patch = f.0.join("change.patch");
    fs::write(&patch, f.git(&["diff", &covered, &uncited]) + "\n").expect("patch");
    let message = f.0.join("message.txt");
    fs::write(&message, "Add a screen\n").expect("message");
    let args = [
        "--platform",
        "ios",
        "--patch",
        patch.to_str().expect("p"),
        "--message-file",
        message.to_str().expect("m"),
    ];
    assert_eq!(f.cli("gate", &args).status.code(), Some(1));
    fs::write(&message, "Add a screen\n\nCatalog: none - preview\n").expect("message");
    assert_eq!(f.cli("gate", &args).status.code(), Some(0));
    assert_eq!(
        f.cli(
            "gate",
            &["--platform", "ios", "--base", "nope", "--head", "HEAD"]
        )
        .status
        .code(),
        Some(2)
    );
    let names = concordance::patch_paths("diff --git a/x/A.swift b/x/B.swift\nrename from x/A.swift\nrename to x/B.swift\n--- a/x/A.swift\n+++ b/x/B.swift\n");
    assert_eq!(names, ["x/A.swift", "x/B.swift"]);
}

#[test]
fn sync_marks_precise_hits_stale_and_queues_file_level_hits_for_review() {
    let f = Fixture::new();
    f.mutate("reader", |v| {
        v[0]["platforms"]["android"] = json!({"status": "implemented", "implemented_by": "sol", "source": ["Reader.kt"]});
        v[0]["details"] = json!([{"id": "title-copy", "kind": "copy", "spec": {"text": "Reader title", "copy": {"text": "Open"}},
            "platforms": {"android": {"status": "implemented"}}}]);
    });
    fs::write(
        f.0.join("native/Reader.swift"),
        "struct Reader { let title = \"Open\" }\n",
    )
    .expect("base");
    f.git(&["init", "-q"]);
    let base = f.commit("base");
    f.config(&format!("[baselines.ios]\ncommit = '{base}'\n"));
    f.commit("pin");
    fs::write(
        f.0.join("native/Reader.swift"),
        "struct Reader { let title = \"Open book\" }\n",
    )
    .expect("edit");
    let head = f.commit("Retitle the reader");
    let tasks = f.0.join("tasks");
    let tasks_arg = tasks.to_str().expect("path");
    let args = [
        "--platform-from",
        "ios",
        "--platform-to",
        "android",
        "--tasks-dir",
        tasks_arg,
    ];
    let dry = f.cli("sync", &args);
    assert!(dry.status.success(), "{}", text(&dry));
    let out = text(&dry);
    // The file-level hit goes to review; the changed copy string makes its detail stale.
    assert!(
        out.contains("dry run") && out.contains("| `reader.open-book` | implemented | review |"),
        "{out}"
    );
    assert!(
        out.contains(
            "| `reader.open-book#title-copy` | implemented | stale | copy \"Open\" changed |"
        ),
        "{out}"
    );
    assert!(!tasks.exists());
    let all_stale = text(&f.cli("sync", &[&args[..], &["--coarse-as", "stale"]].concat()));
    assert!(
        all_stale.contains("| `reader.open-book` | implemented | stale |"),
        "{all_stale}"
    );
    // Issues are a dry run here: tests never call gh.
    let issues = f.cli(
        "sync",
        &[&args[..], &["--issues", "gh", "--label", "parity"]].concat(),
    );
    assert!(text(&issues).contains("would run: gh issue create --title \"[parity] reader.open-book: Open a book\" --label parity"), "{}", text(&issues));
    let apply = f.cli("sync", &[&args[..], &["--apply"]].concat());
    assert!(apply.status.success(), "{}", text(&apply));
    let catalog = concordance::Catalog::load(&f.0).expect("catalog");
    let entry = &catalog.entries["reader.open-book"];
    assert_eq!(
        entry.platforms["android"].status,
        concordance::Status::Implemented
    );
    let detail = &entry.details[0].platforms["android"];
    assert_eq!(detail.status, concordance::Status::Stale);
    assert_eq!(
        detail.previous_status,
        Some(concordance::Status::Implemented)
    );
    assert_eq!(detail.since_commit.as_deref(), Some(head.as_str()));
    assert_eq!(
        catalog.entries["search.find-book"].platforms["android"].status,
        concordance::Status::Missing
    );
    assert_eq!(catalog.baseline_commit("ios"), Some(head.as_str()));
    let task = fs::read_to_string(tasks.join("reader.open-book.md")).expect("task");
    assert!(
        task.contains("Kind: stale")
            && task.contains("+struct Reader { let title = \"Open book\" }"),
        "{task}"
    );
    assert!(task.contains("Retitle the reader"));
    let review = fs::read_to_string(tasks.join("search.find-book.md")).expect("review task");
    assert!(
        review.contains("Kind: review") || review.contains("android missing → missing"),
        "{review}"
    );
    // A second run is a no-op.
    let catalog_before = fs::read(f.0.join("catalog/reader.yaml")).expect("area");
    let again = f.cli("sync", &[&args[..], &["--apply"]].concat());
    assert!(text(&again).contains("already current"), "{}", text(&again));
    assert_eq!(
        fs::read(f.0.join("catalog/reader.yaml")).expect("area"),
        catalog_before
    );
    // Triage the review item: reviewed unchanged, or stale against the current pin.
    let dir = f.0.to_str().expect("path");
    let set = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .args(["set", dir, "reader.open-book", "--platform", "android"])
            .args(args)
            .output()
            .expect("CLI")
    };
    assert!(set(&["--reviewed-unchanged"]).status.success());
    let catalog = concordance::Catalog::load(&f.0).expect("catalog");
    assert_eq!(
        catalog.entries["reader.open-book"].platforms["android"]
            .reviewed_commit
            .as_deref(),
        Some(head.as_str())
    );
    let output = set(&["--stale"]);
    assert!(output.status.success(), "{}", text(&output));
    let catalog = concordance::Catalog::load(&f.0).expect("catalog");
    let record = &catalog.entries["reader.open-book"].platforms["android"];
    assert_eq!(
        (
            record.status,
            record.since_commit.as_deref(),
            record.reviewed_commit.as_deref()
        ),
        (concordance::Status::Stale, Some(head.as_str()), None)
    );
    assert!(catalog
        .report()
        .expect("report")
        .contains("## Stale after baseline changes"));
}

#[test]
fn anchors_suggest_proposes_symbols_containing_identifiers_or_copy() {
    let f = Fixture::new();
    fs::write(
        f.0.join("native/Reader.swift"),
        "struct Reader {\n    var body: some View {\n        openButton()\n    }\n\n    func openButton() -> some View {\n        Button(\"Open\") {}\n            .accessibilityIdentifier(\"reader.open\")\n    }\n}\n",
    )
    .expect("source");
    let out = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args(["anchors", "suggest"])
        .arg(&f.0)
        .args(["reader.open-book"])
        .output()
        .expect("CLI");
    assert!(out.status.success(), "{}", text(&out));
    let out = text(&out);
    assert!(out.contains("- ios `Reader.swift` → `Reader.swift#openButton` (\"Open\" at L7, \"reader.open\" at L8)"), "{out}");
    // Anchored test references are left alone; files without a match say so.
    assert!(!out.contains("ReaderTests.swift"), "{out}");
    assert!(
        out.contains("- android `Reader.kt`: no type or function contains"),
        "{out}"
    );
}

#[test]
fn gate_release_applies_default_and_per_kind_limits_and_intended_differences_clear_stale() {
    let f = Fixture::new();
    f.mutate("reader", |v| {
        v[0]["platforms"]["android"] = json!({"status": "stale", "previous_status": "implemented",
            "since_commit": "abcdef1", "since_date": "2026-11-10"});
        v[1]["details"] = json!([{"id": "title-copy", "kind": "copy", "spec": {"text": "Title", "copy": {"text": "Open book"}},
            "platforms": {"android": {"status": "stale", "previous_status": "implemented", "since_commit": "abcdef1", "since_date": "2026-11-20"}}}]);
    });
    let release = |args: &[&str]| {
        f.cli(
            "gate-release",
            &[&["--platform", "android"][..], args].concat(),
        )
    };
    let out = release(&["--max-age-days", "7", "--today", "2026-11-20"]);
    assert_eq!(out.status.code(), Some(1));
    let out = text(&out);
    assert!(
        out.contains("| `reader.open-book` | 2026-11-10 | 10 | 7 |"),
        "{out}"
    );
    assert!(!out.contains("title-copy"), "{out}");
    let out = release(&[
        "--max-age-days",
        "14",
        "--max-age-days-for",
        "copy=0",
        "--today",
        "2026-11-20",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let out = text(&out);
    assert!(
        out.contains("| `reader.native-menu#title-copy` | 2026-11-20 | 0 | 0 |")
            && !out.contains("reader.open-book"),
        "{out}"
    );
    assert_eq!(
        release(&["--max-age-days", "14", "--today", "2026-11-20"])
            .status
            .code(),
        Some(0)
    );
    // An intended difference restores the previous status and records the reason.
    let dir = f.0.to_str().expect("path");
    let set = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args([
            "set",
            dir,
            "reader.native-menu/title-copy",
            "--platform",
            "android",
            "--intended",
            "Android uses sentence case",
        ])
        .output()
        .expect("CLI");
    assert!(set.status.success(), "{}", text(&set));
    let catalog = concordance::Catalog::load(&f.0).expect("catalog");
    let record = &catalog.entries["reader.native-menu"].details[0].platforms["android"];
    assert_eq!(record.status, concordance::Status::Implemented);
    assert!(record.previous_status.is_none() && record.intended_difference.is_some());
    assert_eq!(
        release(&[
            "--max-age-days",
            "14",
            "--max-age-days-for",
            "copy=0",
            "--today",
            "2026-11-20"
        ])
        .status
        .code(),
        Some(0)
    );
    assert_eq!(
        release(&["--max-age-days-for", "sound=1", "--max-age-days", "1"])
            .status
            .code(),
        Some(2)
    );
    // Stale fields on a non-stale record are invalid.
    f.mutate("reader", |v| {
        v[0]["platforms"]["android"]["status"] = json!("partial")
    });
    assert!(text(&f.cli("validate", &[])).contains("belong only to stale records"));
}

#[test]
fn strings_export_writes_android_resources_for_catalog_copy_keys() {
    let f = Fixture::new();
    f.mutate("reader", |v| {
        v[1]["details"] = json!([
            {"id": "play", "kind": "copy", "spec": {"text": "Play label", "copy": {"text": "Play %@", "key": "Play %@"}}},
            {"id": "open", "kind": "copy", "spec": {"text": "Open label", "copy": {"text": "Open"}}},
            {"id": "gone", "kind": "copy", "spec": {"text": "Gone", "copy": {"text": "Gone", "key": "Removed key"}}}
        ]);
    });
    let xcstrings = f.0.join("Localizable.xcstrings");
    fs::write(
        &xcstrings,
        json!({
            "sourceLanguage": "en",
            "strings": {
                "Play %@": {"comment": "PlayerView play button", "localizations": {
                    "en": {"stringUnit": {"state": "translated", "value": "Play %@"}},
                    "zh-Hans": {"stringUnit": {"state": "translated", "value": "播放“%@”"}}}},
                "Open": {},
                "Unused": {}
            },
            "version": "1.0"
        })
        .to_string(),
    )
    .expect("xcstrings");
    let res = f.0.join("res");
    let args = [
        "--from",
        xcstrings.to_str().expect("p"),
        "--keys-from-catalog",
        "--match-text",
        "--platform",
        "android",
        "--app",
        "reader",
        "--out",
        res.to_str().expect("p"),
    ];
    let out = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args(["strings", "export"])
        .arg(&f.0)
        .args(args)
        .output()
        .expect("CLI");
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("2 keys in 2 file(s); 1 missing"),
        "{}",
        text(&out)
    );
    let play = concordance::strings::resource_name("reader", "Play %@");
    let english = fs::read_to_string(res.join("values/strings_reader_shared.xml")).expect("values");
    assert!(
        english.contains(&format!("<string name=\"{play}\">Play %s</string>")),
        "{english}"
    );
    assert!(english.contains("<!-- Play %@ (PlayerView play button) -->"));
    assert!(english.contains(">Open</string>") && !english.contains("Unused"));
    let chinese =
        fs::read_to_string(res.join("values-zh-rCN/strings_reader_shared.xml")).expect("zh");
    assert!(chinese.contains(&format!("<string name=\"{play}\">播放“%s”</string>")));
    let check = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .args(["strings", "export"])
            .arg(&f.0)
            .args(args)
            .arg("--check")
            .output()
            .expect("CLI")
    };
    assert!(check(&args).status.success());
    fs::write(res.join("values/strings_reader_shared.xml"), "edited").expect("edit");
    assert!(!check(&args).status.success());
}

#[test]
fn version_check_compares_files_patterns_commands_and_source_wiring() {
    let f = Fixture::new();
    fs::write(f.0.join("version.txt"), "1.2.0\n").expect("source");
    fs::write(f.0.join("native/Info.txt"), "MARKETING_VERSION = 1.2.0\n").expect("ios");
    fs::write(
        f.0.join("native/build.gradle.kts"),
        "android {\n    versionName = \"1.1.0\"\n}\n",
    )
    .expect("android");
    f.config("[version]\nsource = \"version.txt\"\n\n[version.platforms.ios]\nfile = \"Info.txt\"\npattern = 'MARKETING_VERSION = (\\S+)'\n\n[[version.platforms.android]]\nfile = \"build.gradle.kts\"\npattern = 'versionName = \"([^\"]+)\"'\n\n[[version.platforms.android]]\ncommand = \"echo 1.2.0\"\n");
    let check = |args: &[&str]| f.cli("version-check", args);
    let out = check(&[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let out = text(&out);
    assert!(out.contains("| android | file `build.gradle.kts` /versionName = \"([^\"]+)\"/ | 1.1.0 | expected 1.2.0 |"), "{out}");
    assert!(
        out.contains("| ios | file `Info.txt` /MARKETING_VERSION = (\\S+)/ | 1.2.0 | matches |"),
        "{out}"
    );
    assert!(
        out.contains("| android | command `echo 1.2.0` | 1.2.0 | matches |"),
        "{out}"
    );
    assert_eq!(check(&["--platform", "ios"]).status.code(), Some(0));
    // gate-release runs the version check for its platform unless skipped.
    let release = |extra: &[&str]| {
        f.cli(
            "gate-release",
            &[&["--platform", "android", "--max-age-days", "7"][..], extra].concat(),
        )
    };
    assert_eq!(release(&[]).status.code(), Some(1));
    assert_eq!(release(&["--skip-version"]).status.code(), Some(0));
    // A build file that reads the source passes a reads_source check without building.
    fs::write(
        f.0.join("native/build.gradle.kts"),
        "android {\n    versionName = rootProject.file(\"../version.txt\").readText().trim()\n}\n",
    )
    .expect("android");
    let config = fs::read_to_string(f.0.join("catalog.toml")).expect("config");
    let config = config.replace(
        "pattern = 'versionName = \"([^\"]+)\"'",
        "pattern = 'versionName = .*version\\.txt'\nreads_source = true",
    );
    fs::write(f.0.join("catalog.toml"), config).expect("config");
    let out = check(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(text(&out).contains("reads the source"));
    assert_eq!(release(&[]).status.code(), Some(0));
    // Commands time out.
    let config = fs::read_to_string(f.0.join("catalog.toml")).expect("config");
    fs::write(
        f.0.join("catalog.toml"),
        config.replace(
            "command = \"echo 1.2.0\"",
            "command = \"sleep 5\"\ntimeout_seconds = 1",
        ),
    )
    .expect("config");
    let out = check(&["--platform", "android"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("timed out after 1s"));
}
