//! Platform scope (`applies_to`), inheritance of stale bookkeeping and surface records,
//! and per-platform gate modes.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-scope-{}-{}",
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
        let _ = fs::remove_dir_all(&path);
        copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join(name), &path);
        Self(path)
    }
    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
    fn config(&self, edit: impl FnOnce(String) -> String) {
        let path = self.path("catalog.toml");
        let text = fs::read_to_string(&path).expect("config");
        fs::write(path, edit(text)).expect("config");
    }
    fn mutate(&self, area: &str, edit: impl FnOnce(&mut Value)) {
        let path = self.path(&format!("catalog/{area}.yaml"));
        let mut value: Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("JSON");
        edit(&mut value);
        fs::write(
            path,
            serde_json::to_string_pretty(&value).expect("serialize") + "\n",
        )
        .expect("write");
    }
    fn cli(&self, command: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .arg(command)
            .arg(&self.0)
            .args(args)
            .output()
            .expect("CLI")
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
            "-m",
            message,
        ]);
        self.git(&["rev-parse", "HEAD"])
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
fn entry_json(f: &Fixture, id: &str) -> Value {
    let output = f.cli("entry", &[id, "--format", "json"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    serde_json::from_slice(&output.stdout).expect("JSON")
}

#[test]
fn a_differing_status_drops_the_parents_stale_bookkeeping() {
    let f = Fixture::new("tests/fixtures");
    f.config(|c| {
        c.replace(
            "android = \"native\"",
            "android = \"native\"\nhorizon = { root = \"native\", inherits = \"android\" }",
        )
    });
    f.mutate("reader", |v| {
        v[0]["platforms"]["android"] = json!({
            "status": "stale", "previous_status": "implemented",
            "since_commit": "0123456789abcdef0123456789abcdef01234567", "since_date": "2026-11-01",
            "gap": "The baseline changed."
        });
        v[0]["platforms"]["horizon"] = json!({"status": "implemented"});
    });
    let output = f.cli("validate", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let horizon = &entry_json(&f, "reader.open-book")["platforms"]["horizon"];
    assert_eq!(horizon, &json!({"status": "implemented"}));
    // The same status still inherits everything.
    f.mutate("reader", |v| {
        v[0]["platforms"]["horizon"] = json!({"notes": "Same as Android."});
    });
    let horizon = &entry_json(&f, "reader.open-book")["platforms"]["horizon"];
    assert_eq!(horizon["status"], "stale");
    assert_eq!(horizon["since_date"], "2026-11-01");
}

#[test]
fn inherited_surface_records_follow_the_surfaces_platforms() {
    let f = Fixture::new("tests/surface-fixtures");
    f.config(|c| {
        c.replace(
            "android = \"android\"",
            "android = \"android\"\nhorizon = { root = \"android\", inherits = \"android\" }",
        )
        .replace(
            "[surfaces.\"widget.medium\"]\ntitle = \"Medium widget\"\nplatforms = [\"ios\", \"android\"]",
            "[surfaces.\"widget.medium\"]\ntitle = \"Medium widget\"\nplatforms = [\"ios\", \"android\", \"horizon\"]",
        )
    });
    // A whole copy keeps only the surfaces horizon can have (widget.medium, not widget.small).
    let output = f.cli("validate", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let entry = entry_json(&f, "widgets.now-playing");
    let surfaces = entry["platforms"]["horizon"]["surfaces"]
        .as_object()
        .expect("surfaces")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(surfaces, ["widget.medium"]);
    // An own record merges per surface; a differing status drops the parent's gap.
    f.mutate("widgets", |v| {
        v[0]["platforms"]["horizon"] = json!({
            "status": "partial",
            "surfaces": {"widget.medium": {"status": "missing"}}
        });
        v[0]["platforms"]["android"]["surfaces"]["widget.medium"]["gap"] = json!("Resize.");
    });
    let output = f.cli("validate", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let entry = entry_json(&f, "widgets.now-playing");
    assert_eq!(
        entry["platforms"]["horizon"]["surfaces"]["widget.medium"],
        json!({"status": "missing", "declared": [".widget.NowPlayingWidgetReceiver"]})
    );
    // `surfaces: null` drops them all.
    f.mutate("widgets", |v| {
        v[0]["platforms"]["horizon"] = json!({"status": "partial", "surfaces": null});
    });
    let entry = entry_json(&f, "widgets.now-playing");
    assert!(entry["platforms"]["horizon"].get("surfaces").is_none());
}

#[test]
fn applies_to_scopes_records_reports_and_the_queue() {
    let f = Fixture::new("tests/fixtures");
    // reader.native-menu exists only on Android; library.open-library only on iOS.
    f.mutate("reader", |v| {
        v[1]["applies_to"] = json!(["android"]);
        v[1]["platforms"].as_object_mut().expect("p").remove("ios");
    });
    f.mutate("library", |v| {
        v[0]["applies_to"] = json!(["ios"]);
        v[0]["platforms"]
            .as_object_mut()
            .expect("p")
            .remove("android");
    });
    let output = f.cli("validate", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    // The stand-in records are not printed.
    let menu = entry_json(&f, "reader.native-menu");
    assert_eq!(menu["applies_to"], json!(["android"]));
    assert!(menu["platforms"].get("ios").is_none());
    // Android's queue no longer holds the iOS-only entry, nor does it need it as a dependency.
    let next = text(&f.cli("next", &["--platform", "android"]));
    assert!(!next.contains("library.open-library"), "{next}");
    assert!(next.contains("reader.open-book"), "{next}");
    let out = f.path("parity.md");
    let output = f.cli("report", &["--out", out.to_str().expect("path")]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = fs::read_to_string(&out).expect("report");
    // The iOS-only entry has no row; the Android-only one is an Android adaptation.
    assert!(!report.contains("library.open-library"), "{report}");
    assert!(report.contains("`reader.native-menu`"), "{report}");
    assert!(report.contains("**2 iOS capabilities"), "{report}");

    // Records outside the scope, unknown and empty scopes are errors.
    f.mutate("reader", |v| {
        v[1]["platforms"]["ios"] = json!({"status": "not-applicable"});
    });
    let out = text(&f.cli("validate", &[]));
    assert!(
        out.contains("record for a platform outside applies_to"),
        "{out}"
    );
    f.mutate("reader", |v| {
        v[1]["platforms"].as_object_mut().expect("p").remove("ios");
        v[1]["applies_to"] = json!(["android", "tv"]);
    });
    assert!(text(&f.cli("validate", &[])).contains("unknown platform"));
    f.mutate("reader", |v| v[1]["applies_to"] = json!([]));
    assert_ne!(f.cli("validate", &[]).status.code(), Some(0));

    // `set` refuses to add a record outside the scope.
    f.mutate("reader", |v| v[1]["applies_to"] = json!(["android"]));
    let output = f.cli(
        "set",
        &[
            "reader.native-menu",
            "--platform",
            "ios",
            "--status",
            "missing",
        ],
    );
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    let after = fs::read_to_string(f.path("catalog/reader.yaml")).expect("read");
    let value: Value = serde_json::from_str(&after).expect("JSON");
    assert!(value[1]["platforms"].get("ios").is_none());
}

#[test]
fn a_scoped_flavour_inherits_nothing() {
    let f = Fixture::new("tests/fixtures");
    f.config(|c| {
        c.replace(
            "android = \"native\"",
            "android = \"native\"\neink = { root = \"native\", inherits = \"android\" }",
        )
    });
    // Without a scope, eink copies android's record; with one, it gets none.
    assert!(entry_json(&f, "search.find-book")["platforms"]
        .get("eink")
        .is_some());
    f.mutate("search", |v| v[0]["applies_to"] = json!(["ios", "android"]));
    let output = f.cli("validate", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(entry_json(&f, "search.find-book")["platforms"]
        .get("eink")
        .is_none());
    assert!(!text(&f.cli("next", &["--platform", "eink"])).contains("search.find-book"));
}

#[test]
fn screens_take_a_scope_for_captures_and_plans() {
    let f = Fixture::new("tests/ui-fixtures");
    let screen = f.path("screens/demo.yaml");
    let text_before = fs::read_to_string(&screen).expect("screen");
    // iOS only: android captures are no longer required (and are refused).
    let scoped = text_before
        .replace(
            "entries: [demo.open-screen]",
            "entries: [demo.open-screen]\napplies_to: [ios]",
        )
        .lines()
        .filter(|l| !l.contains("android: captures/"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&screen, &scoped).expect("screen");
    let output = f.cli("validate", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let plan = text(&f.cli("capture-plan", &["--platform", "android"]));
    assert!(!plan.contains("demo"), "{plan}");
    let plan = text(&f.cli("capture-plan", &["--platform", "ios"]));
    assert!(plan.contains("demo\tpopulated"), "{plan}");
    let with_android = scoped.replace(
        "      ios: captures/demo.dark.ios.png",
        "      ios: captures/demo.dark.ios.png\n      android: captures/demo.dark.android.png",
    );
    fs::write(&screen, with_android).expect("screen");
    let out = text(&f.cli("validate", &[]));
    assert!(out.contains("outside applies_to"), "{out}");
}

#[test]
fn gate_modes_separate_files_from_surfaces() {
    let f = Fixture::new("tests/surface-fixtures");
    f.config(|c| {
        c + "\n[source_globs.android]\nall = [\"**/*.kt\"]\nuser_visible = [\"**/*.kt\"]\n"
    });
    f.git(&["init", "-q"]);
    let base = f.commit("base");
    fs::write(f.path("android/app/Screen.kt"), "class Screen\n").expect("write");
    let head = f.commit("Change a screen");
    let gate = |extra: &[&str]| {
        let mut args = vec!["--platform", "android", "--base", &base, "--head", &head];
        args.extend_from_slice(extra);
        f.cli("gate", &args)
    };
    let output = gate(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let output = gate(&["--files", "off"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("0 user-visible"),
        "{}",
        text(&output)
    );
    // Source globs kept only for inventory: an `all`-only table no longer gates files.
    f.config(|c| c.replace("user_visible = [\"**/*.kt\"]\n", ""));
    let output = gate(&[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("Modes: files off, surfaces enforce."),
        "{}",
        text(&output)
    );
    // [gate.<platform>] sets the modes.
    f.config(|c| c + "\n[gate.android]\nfiles = \"enforce\"\nsurfaces = \"report\"\n");
    let output = gate(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("Modes: files enforce, surfaces report."),
        "{}",
        text(&output)
    );
    // Checking surfaces needs a [surface_scan] section.
    f.config(|c| {
        c.replace(
            "android = \"android\"",
            "android = \"android\"\ntv = \"apple\"",
        )
    });
    let output = f.cli(
        "gate",
        &[
            "--platform",
            "tv",
            "--base",
            &base,
            "--head",
            &head,
            "--surfaces",
            "enforce",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(
        text(&output).contains("needs [surface_scan.tv]"),
        "{}",
        text(&output)
    );
}

#[test]
fn review_regressions_flavours_patches_sync_and_nulls() {
    let f = Fixture::new("tests/fixtures");
    f.config(|c| {
        c.replace(
            "android = \"native\"",
            "android = \"native\"\neink = { root = \"native\", inherits = \"android\" }",
        ) + "\n[source_globs.android]\nuser_visible = [\"*.kt\"]\n[source_globs.eink]\nuser_visible = [\"*.kt\"]\n"
    });
    f.mutate("search", |v| {
        v[0]["platforms"]["android"] = json!({"status": "partial", "source": ["One.kt"]});
    });
    fs::write(f.path("native/One.kt"), "class One\n").expect("write");
    f.git(&["init", "-q"]);
    let base = f.commit("base");
    // A flavour's file is cited through its parent's record.
    fs::write(f.path("native/One.kt"), "class One(val x: Int)\n").expect("write");
    f.mutate("search", |v| {
        v[0]["platforms"]["android"]["notes"] = json!("Changed.")
    });
    let head = f.commit("Change One");
    for platform in ["android", "eink"] {
        let output = f.cli(
            "gate",
            &["--platform", platform, "--base", &base, "--head", &head],
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "{platform}: {}",
            text(&output)
        );
    }
    // --patch keeps the area rule under enforce: an uncited file is covered by an area change.
    fs::write(f.path("native/Free.kt"), "class Free\n").expect("write");
    f.mutate("library", |v| v[0]["notes"] = json!(["Area changed."]));
    let patch = f.git(&["diff"]) + "\n";
    let untracked = "diff --git a/native/Free.kt b/native/Free.kt\nnew file mode 100644\n--- /dev/null\n+++ b/native/Free.kt\n@@ -0,0 +1 @@\n+class Free\n";
    fs::write(f.path("change.patch"), patch + untracked).expect("patch");
    let output = f.cli(
        "gate",
        &[
            "--platform",
            "android",
            "--patch",
            f.path("change.patch").to_str().expect("p"),
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(text(&output).contains("not cited; catalog entries changed in range"));

    // `lead` must be a configured platform inside applies_to.
    f.mutate("reader", |v| {
        v[1]["applies_to"] = json!(["android"]);
        v[1]["platforms"].as_object_mut().expect("p").remove("ios");
        v[1]["lead"] = json!("ios");
    });
    assert!(text(&f.cli("validate", &[])).contains("lead is outside applies_to"));
    f.mutate("reader", |v| v[1]["lead"] = json!("nosuch"));
    assert!(text(&f.cli("validate", &[])).contains("unknown platform"));
    f.mutate("reader", |v| {
        v[1].as_object_mut().expect("e").remove("lead");
    });

    // `surfaces: {id: null}` with nothing to inherit is accepted.
    let s = Fixture::new("tests/surface-fixtures");
    s.config(|c| {
        c.replace(
            "android = \"android\"",
            "android = \"android\"\neink = { root = \"android\", inherits = \"android\" }",
        )
    });
    s.mutate("widgets", |v| {
        v[0]["platforms"]["eink"] =
            json!({"status": "partial", "surfaces": {"widget.small": null}});
    });
    let output = s.cli("validate", &[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
}

#[test]
fn sync_skips_targets_outside_scope_and_follows_unled_entries_from_any_platform() {
    let f = Fixture::new("tests/fixtures");
    fs::write(
        f.path("native/Reader.swift"),
        "struct Reader { let title = \"Open\" }\n",
    )
    .expect("write");
    // library.open-library exists only on iOS; reader.open-book has an android source.
    f.mutate("library", |v| {
        v[0]["applies_to"] = json!(["ios"]);
        v[0]["platforms"]
            .as_object_mut()
            .expect("p")
            .remove("android");
    });
    f.mutate("reader", |v| {
        v[0]["platforms"]["android"] = json!({"status": "implemented", "source": ["Reader.kt"]});
    });
    fs::write(f.path("native/Reader.kt"), "class Reader\n").expect("write");
    f.git(&["init", "-q"]);
    let base = f.commit("base");
    f.config(|c| {
        format!("{c}\n[baselines.ios]\ncommit = '{base}'\n[baselines.android]\ncommit = '{base}'\n")
    });
    f.commit("pin");
    fs::write(
        f.path("native/Reader.swift"),
        "struct Reader { let title = \"Book\" }\n",
    )
    .expect("edit");
    fs::write(f.path("native/Reader.kt"), "class Reader(val t: String)\n").expect("edit");
    f.commit("Change both");
    let ios = text(&f.cli(
        "sync",
        &["--platform-from", "ios", "--platform-to", "android"],
    ));
    assert!(!ios.contains("library.open-library"), "{ios}");
    assert!(ios.contains("reader.open-book"), "{ios}");
    // No entry has a lead, so syncing from Android still follows Android's changes.
    let android = text(&f.cli(
        "sync",
        &["--platform-from", "android", "--platform-to", "ios"],
    ));
    assert!(android.contains("reader.open-book"), "{android}");
}
