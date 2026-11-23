use concordance::{Catalog, Issue};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A temporary copy of tests/tracker-fixtures (taupe, marginalia and suite.toml).
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-tracker-test-{}-{}",
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
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tracker-fixtures"),
            &path,
        );
        Self(path)
    }
    fn taupe(&self) -> PathBuf {
        self.0.join("taupe")
    }
    fn write(&self, relative: &str, content: &str) {
        let path = self.taupe().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, content).expect("write fixture");
    }
    fn edit(&self, relative: &str, from: &str, to: &str) {
        let path = self.taupe().join(relative);
        let text = fs::read_to_string(&path).expect("read fixture");
        assert!(text.contains(from), "{relative} lacks {from:?}");
        fs::write(path, text.replacen(from, to, 1)).expect("write fixture");
    }
    fn errors(&self) -> Vec<Issue> {
        Catalog::load_collect(self.taupe())
            .expect("loadable catalog")
            .1
            .errors
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_concordance"))
            .args(args)
            .current_dir(&self.0)
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

/// Expect exactly one tracker error naming `id`, `file` and a message containing `message`.
fn expect_error(fixture: &Fixture, id: &str, file: &str, message: &str) {
    let errors = fixture.errors();
    let found = errors
        .iter()
        .any(|e| e.entry == id && e.reference.starts_with(file) && e.message.contains(message));
    assert!(found, "no error {id} {file} {message:?} in {errors:#?}");
}

#[test]
fn valid_tracker_loads() {
    let fixture = Fixture::new();
    let catalog = Catalog::load(fixture.taupe()).expect("valid tracker fixture");
    assert_eq!(catalog.tracker.releases.len(), 7);
    assert_eq!(catalog.tracker.issues.len(), 10);
    let validate = success(fixture.run(&["validate", "taupe"]));
    assert!(validate.contains("Valid: 5 entries"));
    Catalog::load(fixture.0.join("marginalia")).expect("valid second app");
}

#[test]
fn catalog_without_tracker_is_valid() {
    let fixture = Fixture::new();
    fs::remove_dir_all(fixture.taupe().join("tracker")).expect("remove tracker");
    let catalog = Catalog::load(fixture.taupe()).expect("valid without tracker");
    let view = catalog.track(None).expect("track");
    assert!(view.releases.is_empty());
    assert_eq!(view.summary.gaps["android"].total(), 4);
    assert_eq!(view.shared.len(), 2);
}

#[test]
fn strict_parsing_errors() {
    let r = "tracker/releases.yaml";
    let i = "tracker/issues.yaml";
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            r,
            "- id: \"1.0\"\n  state: idea\n  colour: red\n",
            "1.0",
            "unknown field",
        ),
        (
            r,
            "- id: \"1.0\"\n  state: idea\n  state: planned\n",
            "",
            "duplicate",
        ),
        (r, "- id: \"1.0\"\n  state: 5\n", "1.0", "state"),
        (
            r,
            "- id: \"1.0\"\n  state: idea\n  carries: one\n",
            "1.0",
            "invalid type",
        ),
        (r, "[]\n", "", "too few items"),
        (r, "id: \"1.0\"\n", "", "expected array"),
        (
            i,
            "- id: bug.x\n  kind: bug\n",
            "bug.x",
            "missing field `title`",
        ),
        (
            i,
            "- id: bug.x\n  title: X\n",
            "bug.x",
            "missing field `kind`",
        ),
        (
            i,
            "- id: bug.x\n  kind: issue\n  title: X\n",
            "bug.x",
            "unknown variant",
        ),
    ];
    for (file, content, id, message) in cases {
        let fixture = Fixture::new();
        if *file == i {
            // Releases carry issue IDs; drop them so only the issues file is under test.
            fixture.write(r, "- id: \"1.0\"\n  state: idea\n");
        }
        fixture.write(file, content);
        expect_error(&fixture, id, file, message);
    }
}

#[test]
fn release_rules() {
    let r = "tracker/releases.yaml";
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "- id: \"1.2.1\"",
            "- id: \"1.2.0\"",
            "1.2.0",
            "duplicate release id",
        ),
        (
            "carries: [player.speed]",
            "carries: [player.speed, player.nothing]",
            "1.3.0",
            "carries \"player.nothing\", which is neither an entry nor an issue",
        ),
        (
            "builds: { android: \"127\" }",
            "builds: { web: \"1\" }",
            "1.2.1",
            "unconfigured platform",
        ),
        (
            "window: { from: 2027-01-10, to: 2027-02-01 }",
            "window: { from: 2027-03-10, to: 2027-02-01 }",
            "1.3.0",
            "window.from is after window.to",
        ),
        (
            "window: { from: 2027-01-10, to: 2027-02-01 }",
            "window: { from: 2027-01-10, to: 2027-02-30 }",
            "1.3.0",
            "window.to \"2027-02-30\" is not a calendar date",
        ),
        (
            "  released: 2026-08-01\n",
            "",
            "1.0.0",
            "released date is required",
        ),
        (
            "  title: Speed controls\n",
            "  title: Speed controls\n  released: 2027-01-01\n",
            "1.3.0",
            "released date is forbidden while state is planned",
        ),
        (
            "released: 2026-07-01",
            "released: 2026-7-01",
            "0.9.0",
            "not a calendar date",
        ),
    ];
    for (from, to, id, message) in cases {
        let fixture = Fixture::new();
        fixture.edit(r, from, to);
        expect_error(&fixture, id, r, message);
    }
}

#[test]
fn issue_rules() {
    let i = "tracker/issues.yaml";
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "- id: bug.small-glitch",
            "- id: request.small-glitch",
            "request.small-glitch",
            "issue id must be bug.",
        ),
        (
            "- id: bug.small-glitch",
            "- id: bug.Small",
            "bug.Small",
            "lowercase",
        ),
        (
            "- id: bug.small-glitch",
            "- id: bug.old-thing",
            "bug.old-thing",
            "duplicate issue id",
        ),
        (
            "entries: [home.show-library]",
            "entries: [home.nothing]",
            "bug.home-recovery-blank",
            "unknown entry \"home.nothing\"",
        ),
        (
            "release: \"1.2.1\"",
            "release: \"9.9\"",
            "bug.crash-on-open",
            "release \"9.9\" is not in tracker/releases.yaml",
        ),
        (
            "platforms: [android]",
            "platforms: [web]",
            "bug.crash-on-open",
            "unconfigured platform",
        ),
        (
            "evidence: [\"journey-7\"]",
            "evidence: []",
            "bug.verified-one",
            "verified requires evidence",
        ),
        (
            "  duplicate_of: bug.crash-on-open\n",
            "",
            "bug.dupe",
            "duplicate requires duplicate_of",
        ),
        (
            "duplicate_of: bug.crash-on-open",
            "duplicate_of: bug.gone",
            "bug.dupe",
            "must name another issue",
        ),
        (
            "duplicate_of: bug.crash-on-open",
            "duplicate_of: bug.dupe",
            "bug.dupe",
            "must name another issue",
        ),
        (
            "reported: 2026-11-21",
            "reported: 2026-11-31",
            "bug.home-recovery-blank",
            "not a calendar date",
        ),
        (
            "title: Small glitch",
            "title: \"  \"",
            "bug.small-glitch",
            "title must not be empty",
        ),
    ];
    for (from, to, id, message) in cases {
        let fixture = Fixture::new();
        fixture.edit(i, from, to);
        expect_error(&fixture, id, i, message);
    }
}

#[test]
fn issue_id_must_not_collide_with_entry() {
    let fixture = Fixture::new();
    let player = fs::read_to_string(fixture.taupe().join("catalog/player.yaml")).expect("read");
    let entry = player
        .split("\n- id: ")
        .next()
        .expect("first entry")
        .replace("player.skip-forward", "bug.small-glitch")
        .replace("  shared_with: [\"marginalia:bug.small-glitch\"]\n", "");
    fixture.write("catalog/bug.yaml", &entry);
    expect_error(
        &fixture,
        "bug.small-glitch",
        "tracker/issues.yaml",
        "collides with an entry id",
    );
}

#[test]
fn shared_with_syntax_is_checked() {
    for bad in [
        "marginalia",
        "Marginalia:player.skip-forward",
        "marginalia:player",
    ] {
        let fixture = Fixture::new();
        fixture.edit(
            "catalog/player.yaml",
            "\"marginalia:player.skip-forward\"",
            &format!("\"{bad}\""),
        );
        let errors = fixture.errors();
        assert!(
            errors.iter().any(|e| e.entry == "player.skip-forward"
                && e.message.contains("shared feature (<app>:<entry-id>)")),
            "{bad}: {errors:#?}"
        );
    }
}

#[test]
fn validate_cli_names_file_and_id() {
    let fixture = Fixture::new();
    fixture.edit(
        "tracker/issues.yaml",
        "release: \"1.2.1\"",
        "release: \"9.9\"",
    );
    let output = fixture.run(&["validate", "taupe"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tracker:"), "{stderr}");
    assert!(stderr.contains("bug.crash-on-open"), "{stderr}");
    assert!(stderr.contains("tracker/issues.yaml"), "{stderr}");
    // Every command that loads the catalog refuses it.
    assert!(!fixture.run(&["track", "taupe"]).status.success());
}

#[test]
fn track_markdown_matches_golden_and_check() {
    let fixture = Fixture::new();
    // The golden file ships in the fixture, so --check is fresh before anything is written.
    success(fixture.run(&["track", "taupe", "--check"]));
    let golden = include_str!("tracker-fixtures/taupe/tracker.md");
    fs::remove_file(fixture.taupe().join("tracker.md")).expect("remove golden copy");
    assert!(!fixture.run(&["track", "taupe", "--check"]).status.success());
    success(fixture.run(&["track", "taupe"]));
    let written = fs::read_to_string(fixture.taupe().join("tracker.md")).expect("written");
    assert_eq!(written, golden);
    // Deterministic: a second run produces the same bytes.
    success(fixture.run(&["track", "taupe"]));
    assert_eq!(
        fs::read_to_string(fixture.taupe().join("tracker.md")).expect("rewritten"),
        golden
    );
    // A tracker change makes the checked file stale; --check never writes.
    fixture.edit(
        "tracker/issues.yaml",
        "title: Small glitch",
        "title: Tiny glitch",
    );
    let stale = fixture.run(&["track", "taupe", "--check"]);
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("stale track output"));
    assert_eq!(
        fs::read_to_string(fixture.taupe().join("tracker.md")).expect("preserved"),
        golden
    );
    // Without --today there is no relative time.
    assert!(!golden.contains("overdue"));
    assert!(!golden.contains("## Upcoming"));
}

#[test]
fn track_json_shape() {
    let fixture = Fixture::new();
    let out = fixture.0.join("tracker.json");
    let out_arg = out.display().to_string();
    success(fixture.run(&["track", "taupe", "--format", "json", "--out", &out_arg]));
    let text = fs::read_to_string(&out).expect("json");
    let value: Value = serde_json::from_str(&text).expect("valid JSON");
    // Stable key order: top-level keys appear in struct order in the raw text.
    let positions: Vec<usize> = [
        "title",
        "summary",
        "releases",
        "bugs",
        "requests",
        "unverified",
        "gaps",
        "shared",
    ]
    .iter()
    .map(|k| text.find(&format!("\n  \"{k}\": ")).expect("top-level key"))
    .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    assert_eq!(value.as_object().map(serde_json::Map::len), Some(8));
    assert_eq!(value["summary"]["last_release"]["id"], "1.1.0");
    assert_eq!(value["summary"]["next_release"]["id"], "1.2.1");
    assert_eq!(value["summary"]["open_bugs"]["total"], 6);
    assert_eq!(value["summary"]["open_bugs"]["critical"], 1);
    assert_eq!(value["summary"]["open_requests"], 2);
    assert_eq!(value["summary"]["unverified"], 3);
    assert_eq!(value["summary"]["gaps"]["android"]["stale"], 1);
    let releases: Vec<&str> = value["releases"]
        .as_array()
        .expect("releases")
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(
        releases,
        ["1.2.1", "1.2.0", "1.3.0", "2.0.0", "1.1.0", "1.0.0"]
    );
    assert_eq!(value["releases"][1]["readiness"]["open"], 3);
    assert_eq!(value["releases"][0]["readiness"]["open"], 1);
    assert_eq!(
        value["releases"][0]["carries"][1]["statuses"]["android"],
        "implemented"
    );
    let bugs: Vec<&str> = value["bugs"]
        .as_array()
        .expect("bugs")
        .iter()
        .filter_map(|r| r["id"].as_str())
        .collect();
    assert_eq!(
        bugs,
        [
            "bug.crash-on-open",
            "bug.small-glitch",
            "bug.home-recovery-blank",
            "bug.old-thing",
            "bug.carried-fix",
            "bug.pulled-fix"
        ]
    );
    assert_eq!(value["gaps"][0]["platform"], "android");
    assert_eq!(value["gaps"][0]["next"][0]["id"], "home.show-library");
    assert_eq!(value["gaps"][0]["order"], "work-queue");
    assert!(value["gaps"][0]["order_note"].is_null());
    // A fixed issue carried by a released release is unverified even without its own
    // release field; one carried only by a pulled release is not. Without a log and git,
    // the plan alone never proves the fix is in the release.
    let unverified: Vec<(&str, &str)> = value["unverified"]
        .as_array()
        .expect("unverified")
        .iter()
        .filter_map(|u| Some((u["id"].as_str()?, u["reasons"][0].as_str()?)))
        .collect();
    assert_eq!(
        unverified,
        [
            (
                "bug.carried-fix",
                "release 1.1.0 is out; containment unknown"
            ),
            ("bug.home-recovery-blank", "fixed without evidence"),
            ("bug.old-thing", "release 1.1.0 is out; containment unknown"),
        ]
    );
    assert!(value.get("today").is_none());
    assert!(value["releases"][0].get("overdue").is_none());
    // JSON default output path sits beside catalog.toml.
    success(fixture.run(&["track", "taupe", "--format", "json"]));
    assert_eq!(
        fs::read_to_string(fixture.taupe().join("tracker.json")).expect("default json"),
        text
    );
}

#[test]
fn track_today_marks_overdue_and_upcoming() {
    let fixture = Fixture::new();
    let out = fixture.0.join("today.md").display().to_string();
    success(fixture.run(&["track", "taupe", "--today", "2026-11-22", "--out", &out]));
    let text = fs::read_to_string(&out).expect("today output");
    assert!(text.contains("As of 2026-11-22."));
    assert!(text.contains("### `1.2.1` — Crash fix (in-review) **overdue**"));
    assert!(!text.contains("(candidate) **overdue**"));
    assert!(text.contains("- `1.2.0`: 2026-11-20 to 2026-12-05, open, closes in 13 days"));
    assert!(text.contains("- `1.3.0`: 2027-01-10 to 2027-02-01, opens in 49 days"));
    let catalog = Catalog::load(fixture.taupe()).expect("catalog");
    let view = catalog.track(Some("2026-11-22")).expect("view");
    let overdue: Vec<&str> = view
        .releases
        .iter()
        .filter(|r| r.overdue == Some(true))
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(overdue, ["1.2.1"]);
    for bad in ["2026-02-29", "2026-13-01", "tomorrow"] {
        assert!(!fixture
            .run(&["track", "taupe", "--today", bad, "--out", &out])
            .status
            .success());
    }
}

#[test]
fn suite_joins_apps_and_resolves_shared_features() {
    let fixture = Fixture::new();
    success(fixture.run(&["suite", "suite.toml", "--check"]));
    let golden = include_str!("tracker-fixtures/suite.md");
    assert_eq!(
        fs::read_to_string(fixture.0.join("suite.md")).expect("suite golden"),
        golden
    );
    // Resolved: both apps' skip-forward entries in one feature with no unresolved refs.
    assert!(golden.contains(
        "| `marginalia:player.skip-forward` | marginalia:`player.skip-forward` ios implemented; taupe:`player.skip-forward` ios implemented | — |"
    ));
    // Unresolved: taupe's speed points at an entry marginalia does not have.
    assert!(golden.contains("**`marginalia:player.speed-menu`**"));
    // Without --today every beat is listed.
    assert!(golden.contains("Past beat"));

    let json = success(fixture.run(&[
        "suite",
        "suite.toml",
        "--format",
        "json",
        "--today",
        "2026-11-22",
        "--out",
        "/dev/stdout",
    ]));
    let json = json
        .rsplit_once("Wrote ")
        .map(|(body, _)| body)
        .expect("json then status line");
    let value: Value = serde_json::from_str(json).expect("suite JSON");
    let apps: Vec<&str> = value["apps"]
        .as_array()
        .expect("apps")
        .iter()
        .filter_map(|a| a["app"].as_str())
        .collect();
    assert_eq!(apps, ["marginalia", "taupe"]);
    assert_eq!(value["apps"][1]["summary"]["open_bugs"]["critical"], 1);
    let beats = value["beats"].as_array().expect("beats");
    assert_eq!(beats.len(), 1, "past beats are hidden with --today");
    assert_eq!(beats[0]["title"], "WWDC 2027");
    // A window counts when at least one day falls strictly before the beat and its end is
    // within the 60 days before it: 4.9 ends exactly 60 days before; 5.1 starts on the beat.
    let beat_releases: Vec<&str> = beats[0]["releases"]
        .as_array()
        .expect("beat releases")
        .iter()
        .filter_map(|r| r["release"].as_str())
        .collect();
    assert_eq!(beat_releases, ["4.9", "5.0"]);
    let shared = value["shared"].as_array().expect("shared");
    assert_eq!(shared.len(), 2);
    assert_eq!(shared[0]["unresolved"].as_array().map(Vec::len), Some(0));
    assert_eq!(shared[0]["members"].as_array().map(Vec::len), Some(2));
    assert_eq!(shared[1]["unresolved"][0], "marginalia:player.speed-menu");

    // Stale suite output fails --check without writing.
    fixture.edit(
        "tracker/releases.yaml",
        "title: Crash fix",
        "title: Crash fixes",
    );
    assert!(!fixture
        .run(&["suite", "suite.toml", "--check"])
        .status
        .success());
    assert_eq!(
        fs::read_to_string(fixture.0.join("suite.md")).expect("preserved"),
        golden
    );
}

#[test]
fn suite_rejects_bad_files() {
    let fixture = Fixture::new();
    let suite = fixture.0.join("suite.toml");
    let text = fs::read_to_string(&suite).expect("suite");
    for (from, to) in [
        ("date = 2027-06-07", "date = 2027-02-30"),
        ("date = 2027-06-07", "date = 2027-06-07T10:00:00"),
        ("label = \"Taupe\"", "label = \"Taupe\"\ncolour = \"red\""),
        ("catalog = \"marginalia\"", "catalog = \"missing\""),
    ] {
        fs::write(&suite, text.replacen(from, to, 1)).expect("edit suite");
        let output = fixture.run(&["suite", "suite.toml", "--out", "/dev/null"]);
        assert!(!output.status.success(), "{to} was accepted");
    }
}

#[test]
fn readiness_without_builds_uses_report_baseline_and_target() {
    let fixture = Fixture::new();
    // A configured platform with no records must not hold back a release without builds.
    fixture.edit(
        "catalog.toml",
        "android = \"android\"\n",
        "android = \"android\"\neink = \"eink\"\n",
    );
    fixture.edit(
        "tracker/releases.yaml",
        "carries: [player.speed]",
        "carries: [player.speed, player.skip-forward]",
    );
    let catalog = Catalog::load(fixture.taupe()).expect("catalog");
    let view = catalog.track(None).expect("view");
    let release = view
        .releases
        .iter()
        .find(|r| r.id == "1.3.0")
        .expect("1.3.0");
    let ready: Vec<(&str, bool)> = release
        .carries
        .iter()
        .map(|c| (c.id.as_str(), c.ready))
        .collect();
    assert_eq!(
        ready,
        [("player.speed", false), ("player.skip-forward", true)]
    );
    // Releases with builds still use exactly those platforms; no window means no window line.
    let markdown = view.markdown();
    assert!(!markdown.contains("Window: no window"));
    assert!(
        markdown.contains("- Gaps: android 2 missing · 1 partial · 1 stale; eink none; ios none")
    );
}

#[test]
fn explicit_null_collections_are_rejected() {
    let r = "tracker/releases.yaml";
    let i = "tracker/issues.yaml";
    for (file, from, to, id, field) in [
        (
            r,
            "builds: { android: \"127\" }",
            "builds: null",
            "1.2.1",
            "builds",
        ),
        (
            r,
            "carries: [player.speed]",
            "carries: null",
            "1.3.0",
            "carries",
        ),
        (r, "notes: []", "notes: ~", "1.2.0", "notes"),
        (
            r,
            "window: { from: 2027-01-10, to: 2027-02-01 }",
            "window: null",
            "1.3.0",
            "window",
        ),
        (
            i,
            "entries: [home.show-library]",
            "entries: null",
            "bug.home-recovery-blank",
            "entries",
        ),
        (
            i,
            "evidence: [\"journey-7\"]",
            "evidence: null",
            "bug.verified-one",
            "evidence",
        ),
        (
            i,
            "source: [device]",
            "source: null",
            "bug.home-recovery-blank",
            "source",
        ),
        (
            i,
            "platforms: [android]",
            "platforms: null",
            "bug.crash-on-open",
            "platforms",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.edit(file, from, to);
        expect_error(
            &fixture,
            id,
            file,
            &format!("{field} must be a list or map, not null"),
        );
    }
    // A null optional scalar stays allowed, as in the spec's example.
    let fixture = Fixture::new();
    let errors = fixture.errors();
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn gaps_fall_back_to_impact_order_visibly() {
    let fixture = Fixture::new();
    // library.search depends on home.show-library, which then has no android record, so
    // the dependency-ordered work queue cannot be built for android.
    fixture.edit(
        "catalog/home.yaml",
        "    android: { status: stale, previous_status: implemented, since_commit: abcdef1 }\n",
        "",
    );
    let catalog = Catalog::load(fixture.taupe()).expect("catalog");
    let view = catalog.track(None).expect("view");
    let android = view
        .gaps
        .iter()
        .find(|g| g.platform == "android")
        .expect("android gaps");
    assert_eq!(android.order, "impact");
    let note = android.order_note.as_deref().expect("order note");
    assert!(note.contains("home.show-library"), "{note}");
    let ids: Vec<&str> = android.next.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(ids, ["sync.join-by-qr", "player.speed", "library.search"]);
    let ios = view.gaps.iter().find(|g| g.platform == "ios").expect("ios");
    assert_eq!(ios.order, "work-queue");
    let markdown = view.markdown();
    assert!(markdown.contains("In impact order: the work queue could not be built ("));
    let json: Value =
        serde_json::from_str(&view.render(concordance::TrackFormat::Json).expect("json"))
            .expect("valid json");
    assert_eq!(json["gaps"][0]["order"], "impact");
    assert!(json["gaps"][0]["order_note"].is_string());
}

#[test]
fn table_cells_escape_pipes_and_line_breaks() {
    let fixture = Fixture::new();
    let golden = include_str!("tracker-fixtures/taupe/tracker.md");
    assert!(golden.contains("`request.no-impact` Unrated request \\| with pipe |"));
    fixture.edit(
        "tracker/issues.yaml",
        "title: Small glitch",
        "title: \"Small | glitch\\non two lines\"",
    );
    let catalog = Catalog::load(fixture.taupe()).expect("catalog");
    let markdown = catalog.track(None).expect("view").markdown();
    let line = markdown
        .lines()
        .find(|l| l.contains("bug.small-glitch"))
        .expect("bug row");
    assert_eq!(line.matches(" | ").count(), 6, "{line}");
    assert!(line.contains("Small \\| glitch on two lines"), "{line}");
    // Suite rows are escaped too: a label with a pipe stays in one cell.
    let suite = fixture.0.join("suite.toml");
    let text = fs::read_to_string(&suite).expect("suite");
    fs::write(
        &suite,
        text.replacen("label = \"Taupe\"", "label = \"Taupe | iOS\"", 1),
    )
    .expect("edit suite");
    let out = success(fixture.run(&["suite", "suite.toml", "--out", "/dev/stdout"]));
    let row = out
        .lines()
        .find(|l| l.starts_with("| Taupe"))
        .expect("taupe row");
    assert!(row.starts_with("| Taupe \\| iOS | "), "{row}");
}

#[test]
fn stale_hint_is_a_runnable_command() {
    let fixture = Fixture::new();
    success(fixture.run(&[
        "track",
        "taupe",
        "--format",
        "json",
        "--today",
        "2026-11-22",
    ]));
    fixture.edit(
        "tracker/issues.yaml",
        "title: Small glitch",
        "title: Tiny glitch",
    );
    let output = fixture.run(&[
        "track",
        "taupe",
        "--format",
        "json",
        "--today",
        "2026-11-22",
        "--check",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "regenerate with: concordance track taupe --format json --today 2026-11-22 --out taupe/tracker.json"
        ),
        "{stderr}"
    );
    fixture.edit(
        "tracker/releases.yaml",
        "title: Crash fix",
        "title: Crash fixes",
    );
    let output = fixture.run(&["suite", "suite.toml", "--check"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("regenerate with: concordance suite suite.toml --out suite.md"),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn suite_catalogs_resolve_from_the_real_suite_location() {
    let fixture = Fixture::new();
    // A symlinked suite.toml elsewhere still finds taupe/ and marginalia/ beside its target.
    let elsewhere = fixture.0.join("elsewhere");
    fs::create_dir(&elsewhere).expect("mkdir");
    std::os::unix::fs::symlink(fixture.0.join("suite.toml"), elsewhere.join("suite.toml"))
        .expect("symlink");
    let out = success(fixture.run(&["suite", "elsewhere/suite.toml"]));
    assert!(out.contains("elsewhere/suite.md"), "{out}");
    assert_eq!(
        fs::read_to_string(elsewhere.join("suite.md")).expect("written beside the link"),
        include_str!("tracker-fixtures/suite.md")
    );
    // `..` and absolute catalog paths are allowed.
    let nested = fixture.0.join("nested");
    fs::create_dir(&nested).expect("mkdir");
    let absolute = fixture.0.join("marginalia").display().to_string();
    fs::write(
        nested.join("suite.toml"),
        format!(
            "title = \"Nested\"\n[apps.taupe]\ncatalog = \"../taupe\"\n[apps.marginalia]\ncatalog = \"{absolute}\"\n"
        ),
    )
    .expect("nested suite");
    success(fixture.run(&["suite", "nested/suite.toml", "--out", "/dev/null"]));
}
