//! tracker/log.jsonl: the `log` commands, `log scan-git`, and states derived by `track`
//! and `suite`. Each test builds a temporary git repository holding tests/log-fixtures/app
//! at spec/app, with fixed author and committer dates.
use concordance::Catalog;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const LOG: &str = "spec/app/tracker/log.jsonl";

struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "concordance-log-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("unique temp dir");
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).expect("mkdir");
            for entry in fs::read_dir(from).expect("fixture directory") {
                let entry = entry.expect("fixture entry");
                let target = to.join(entry.file_name());
                if entry.path().is_dir() {
                    copy(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).expect("copy fixture");
                }
            }
        }
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/log-fixtures/app"),
            &path.join("spec/app"),
        );
        let repo = Self(path);
        repo.git(&["init", "--quiet"], "Nobody", "2026-11-01T00:00:00Z");
        repo
    }
    fn catalog(&self) -> PathBuf {
        self.0.join("spec/app")
    }
    /// Run git with a fixed identity and date, isolated from the user's configuration.
    fn git(&self, args: &[&str], who: &str, date: &str) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", who)
            .env(
                "GIT_AUTHOR_EMAIL",
                format!("{}@example.com", who.to_lowercase()),
            )
            .env("GIT_COMMITTER_NAME", who)
            .env(
                "GIT_COMMITTER_EMAIL",
                format!("{}@example.com", who.to_lowercase()),
            )
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF8")
            .trim()
            .to_owned()
    }
    /// An empty commit; returns its hash.
    fn commit(&self, who: &str, date: &str, message: &str) -> String {
        self.git(
            &["commit", "--quiet", "--allow-empty", "-m", message],
            who,
            date,
        );
        self.git(&["rev-parse", "HEAD"], who, date)
    }
    fn run_as(&self, actor: Option<&str>, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_concordance"));
        command
            .args(args)
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("CONCORDANCE_CATALOG");
        match actor {
            Some(actor) => command.env("CONCORDANCE_ACTOR", actor),
            None => command.env_remove("CONCORDANCE_ACTOR"),
        };
        command.output().expect("run CLI")
    }
    fn run(&self, args: &[&str]) -> Output {
        self.run_as(Some("tester"), args)
    }
    fn log(&self, args: &[&str]) -> String {
        let mut all = vec!["log"];
        all.extend_from_slice(args);
        success(self.run(&all))
    }
    fn lines(&self) -> Vec<Value> {
        fs::read_to_string(self.0.join(LOG))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("JSON line"))
            .collect()
    }
    fn track_json(&self) -> Value {
        let out = self.0.join("tracker.json");
        success(self.run(&[
            "track",
            "spec/app",
            "--format",
            "json",
            "--out",
            &out.display().to_string(),
        ]));
        serde_json::from_str(&fs::read_to_string(out).expect("tracker.json")).expect("JSON")
    }
    fn track_markdown(&self) -> String {
        let out = self.0.join("tracker.md");
        success(self.run(&["track", "spec/app", "--out", &out.display().to_string()]));
        fs::read_to_string(out).expect("tracker.md")
    }
}
impl Drop for Repo {
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
fn failure(output: Output) -> String {
    assert!(!output.status.success(), "expected failure");
    String::from_utf8(output.stderr).expect("UTF8 output")
}
fn issue<'a>(view: &'a Value, section: &str, id: &str) -> Option<&'a Value> {
    view[section]
        .as_array()
        .and_then(|items| items.iter().find(|i| i["id"] == id))
}
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn report_writes_a_stable_line_and_unique_ids() {
    let repo = Repo::new();
    let out = repo.log(&[
        "--catalog",
        "spec/app",
        "--at",
        "2026-11-22T10:00:00Z",
        "report",
        "bug",
        "App crashes!",
        "--platform",
        "ios",
        "--impact",
        "high",
        "--entry",
        "home.show-library",
    ]);
    assert!(
        out.starts_with("recorded: reported bug.app-crashes ("),
        "{out}"
    );
    let text = fs::read_to_string(repo.0.join(LOG)).expect("log written");
    let line = text.lines().next().expect("one line");
    let id = line
        .strip_prefix("{\"id\":\"")
        .and_then(|rest| rest.get(..16))
        .expect("id first");
    assert!(id.bytes().all(|c| c.is_ascii_hexdigit()), "{id}");
    assert_eq!(
        &line[23..],
        "\",\"kind\":\"reported\",\"subject\":\"bug.app-crashes\",\"at\":\"2026-11-22T10:00:00Z\",\"by\":\"tester\",\"title\":\"App crashes!\",\"entries\":[\"home.show-library\"],\"platforms\":[\"ios\"],\"impact\":\"high\"}"
    );

    // The same title again is the same issue: nothing appended.
    let again = repo.log(&["--catalog", "spec/app", "report", "bug", "App crashes!"]);
    assert!(
        again.starts_with("already recorded: reported bug.app-crashes"),
        "{again}"
    );
    // Another title with the same slug gets -2; a slug taken by issues.yaml too.
    let other = repo.log(&["--catalog", "spec/app", "report", "bug", "App crashes"]);
    assert!(other.contains("bug.app-crashes-2"), "{other}");
    let taken = repo.log(&["--catalog", "spec/app", "report", "bug", "Home: blank"]);
    assert!(taken.contains("bug.home-blank-2"), "{taken}");
    // A request, found without --catalog via spec/*/catalog.toml.
    let request = repo.log(&["report", "request", "Dark mode"]);
    assert!(request.contains("request.dark-mode"), "{request}");
    // Reporting a title that issues.yaml already has records a report for that issue.
    let yaml = repo.log(&["report", "bug", "Home is blank"]);
    assert!(yaml.contains("reported bug.home-blank ("), "{yaml}");
    assert_eq!(repo.lines().len(), 5);

    let view = repo.track_json();
    let crash = issue(&view, "bugs", "bug.app-crashes").expect("log-only issue listed");
    assert_eq!(crash["impact"], "high");
    assert_eq!(crash["reported"], "2026-11-22");
    assert_eq!(crash["state"], "open");
    assert!(issue(&view, "requests", "request.dark-mode").is_some());
    // A report alone never changes a declared state, so no warnings.
    assert!(view.get("warnings").is_none(), "{view:#}");
}

#[test]
fn report_rejects_unknown_references() {
    let repo = Repo::new();
    let err = failure(repo.run(&["log", "report", "bug", "X", "--entry", "nope.entry"]));
    assert!(err.contains("unknown entry"), "{err}");
    let err = failure(repo.run(&["log", "report", "bug", "X", "--platform", "web"]));
    assert!(err.contains("unconfigured platform"), "{err}");
    let err = failure(repo.run(&["log", "fixed", "bug.unknown", "--commit", "abc"]));
    assert!(err.contains("not an issue"), "{err}");
    let err = failure(repo.run(&["log", "verified", "bug.home-blank", "--evidence", "x"]));
    assert!(err.contains("--by"), "{err}");
    let err = failure(repo.run(&[
        "log",
        "--at",
        "yesterday",
        "reopened",
        "bug.home-blank",
        "--reason",
        "r",
    ]));
    assert!(err.contains("--at"), "{err}");
    assert!(repo.lines().is_empty());
}

#[test]
fn actor_defaults() {
    let repo = Repo::new();
    repo.git(
        &["config", "user.name", "Repo Person"],
        "x",
        "2026-11-01T00:00:00Z",
    );
    success(repo.run_as(None, &["log", "report", "bug", "One"]));
    success(repo.run_as(Some("Env Person"), &["log", "report", "bug", "Two"]));
    success(repo.run_as(
        None,
        &["log", "--by", "Flag Person", "report", "bug", "Three"],
    ));
    let by: Vec<Value> = repo.lines().iter().map(|l| l["by"].clone()).collect();
    assert_eq!(by, vec!["Repo Person", "Env Person", "Flag Person"]);
}

#[test]
fn strict_log_parsing_names_lines() {
    let repo = Repo::new();
    repo.log(&["--at", "2026-11-20", "report", "bug", "Good"]);
    let good = fs::read_to_string(repo.0.join(LOG)).expect("log");
    let cases: &[(&str, &str)] = &[
        ("{\"id\":\"x\",\"kind\":\"fixed\",\"colour\":1}", "unknown field"),
        ("", "blank line"),
        ("not json", "expected"),
        (
            "{\"id\":\"0\",\"kind\":\"fixed\",\"subject\":\"bug.good\",\"at\":\"2026-11-20T00:00:00Z\",\"by\":\"a\"}",
            "fixed requires commit",
        ),
        (
            "{\"id\":\"0\",\"kind\":\"pulled\",\"subject\":\"1.0\",\"at\":\"2026-11-20T00:00:00Z\",\"by\":\"a\",\"reason\":\"r\",\"build\":\"4\"}",
            "build is not allowed on pulled",
        ),
        (
            "{\"id\":\"0\",\"kind\":\"pulled\",\"subject\":\"1.0\",\"at\":\"2026-11-20 00:00\",\"by\":\"a\",\"reason\":\"r\"}",
            "must be a UTC time",
        ),
        (
            "{\"id\":\"0\",\"kind\":\"pulled\",\"subject\":\"1.0\",\"at\":\"2026-11-20T00:00:00Z\",\"by\":\"a\",\"reason\":\"r\"}",
            "does not match its content",
        ),
        (good.trim_end(), "duplicate event id"),
        (
            "{\"id\":\"0\",\"kind\":\"fixed\",\"subject\":\"crash\",\"at\":\"2026-11-20T00:00:00Z\",\"by\":\"a\",\"commit\":\"abc\"}",
            "must be an issue id",
        ),
    ];
    for (line, message) in cases {
        fs::write(repo.0.join(LOG), format!("{good}{line}\n")).expect("write log");
        let (_, report) = Catalog::load_collect(repo.catalog()).expect("loadable");
        let found = report
            .errors
            .iter()
            .any(|e| e.reference == "tracker/log.jsonl:2" && e.message.contains(message));
        assert!(found, "{line:?}: no {message:?} in {:#?}", report.errors);
        // Appending refuses while the log is broken.
        let err = failure(repo.run(&["log", "report", "bug", "Another"]));
        assert!(err.contains("fix them before appending"), "{err}");
    }
    // A fact about an unknown issue is a reference error on its line.
    let mut event = concordance::log::Event::new(
        concordance::log::EventKind::Fixed,
        "bug.ghost",
        "2026-11-20T00:00:00Z",
        "a",
    );
    event.commit = Some("abc".into());
    let line = event.with_id().line().expect("line");
    fs::write(repo.0.join(LOG), format!("{good}{line}\n")).expect("write log");
    let (_, report) = Catalog::load_collect(repo.catalog()).expect("loadable");
    assert!(report
        .errors
        .iter()
        .any(|e| e.reference == "tracker/log.jsonl:2" && e.entry == "bug.ghost"));
}

#[test]
fn derived_issue_states_and_warnings() {
    let repo = Repo::new();
    let at = |t: &str| format!("2026-11-{t}");
    repo.log(&[
        "--at",
        &at("12T09:00:00Z"),
        "report",
        "bug",
        "Log only",
        "--impact",
        "low",
    ]);
    // Self-verified: the fixer's own check does not count.
    repo.log(&[
        "--at",
        &at("13T09:00:00Z"),
        "--by",
        "alice",
        "fixed",
        "bug.log-only",
        "--commit",
        "aaaa111",
    ]);
    repo.log(&[
        "--at",
        &at("13T10:00:00Z"),
        "--by",
        "alice",
        "verified",
        "bug.log-only",
        "--evidence",
        "unit 1/1",
    ]);
    // Verified by someone else.
    repo.log(&[
        "--at",
        &at("13T09:00:00Z"),
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "bbbb222",
    ]);
    repo.log(&[
        "--at",
        &at("14T09:00:00Z"),
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "journey 13/13",
        "--evidence",
        "unit 4/4",
    ]);
    // Declared fixed, reopened by the log: derived open wins with a warning.
    repo.log(&[
        "--at",
        &at("14T09:00:00Z"),
        "reopened",
        "bug.declared-fixed",
        "--reason",
        "still blank",
    ]);
    // Recording the same fact twice is a no-op.
    let again = repo.log(&[
        "--at",
        &at("15T09:00:00Z"),
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "bbbb222",
    ]);
    assert!(again.starts_with("already recorded"), "{again}");

    let view = repo.track_json();
    let log_only = issue(&view, "bugs", "bug.log-only").expect("self-verified stays open");
    assert_eq!(log_only["state"], "fixed");
    assert!(
        issue(&view, "bugs", "bug.home-blank").is_none(),
        "verified is not open"
    );
    let reopened = issue(&view, "bugs", "bug.declared-fixed").expect("reopened is open");
    assert_eq!(reopened["state"], "open");
    let unverified = view["unverified"].as_array().expect("unverified");
    let self_verified = unverified
        .iter()
        .find(|u| u["id"] == "bug.log-only")
        .expect("self-verified listed");
    assert_eq!(
        strings(&self_verified["reasons"]),
        vec!["self-verified by alice"]
    );
    assert_eq!(
        strings(&view["warnings"]),
        vec!["issue `bug.declared-fixed`: issues.yaml says fixed; the log says open"]
    );
    // Release readiness counts the verified issue as done.
    let release = view["releases"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["id"] == "1.1.0"))
        .expect("1.1.0");
    assert_eq!(release["readiness"]["open"], 0);
    assert_eq!(release["carries"][0]["state"], "verified");

    let markdown = repo.track_markdown();
    assert!(markdown.contains("## Warnings\n\n- issue `bug.declared-fixed`: issues.yaml says fixed; the log says open\n"), "{markdown}");
    assert!(
        markdown.contains("- `bug.log-only` Log only: self-verified by alice"),
        "{markdown}"
    );

    // A fix after the reopen brings it back; a check by someone else verifies it.
    repo.log(&[
        "--at",
        &at("16T09:00:00Z"),
        "--by",
        "carol",
        "fixed",
        "bug.declared-fixed",
        "--commit",
        "cccc333",
    ]);
    repo.log(&[
        "--at",
        &at("16T10:00:00Z"),
        "--by",
        "dan",
        "verified",
        "bug.declared-fixed",
        "--evidence",
        "unit 2/2",
    ]);
    let view = repo.track_json();
    assert!(issue(&view, "bugs", "bug.declared-fixed").is_none());
    assert_eq!(
        strings(&view["warnings"]),
        vec!["issue `bug.declared-fixed`: issues.yaml says fixed; the log says verified"]
    );
}

#[test]
fn release_facts_merge_with_the_plan() {
    let repo = Repo::new();
    repo.log(&[
        "--at",
        "2026-11-20T09:00:00Z",
        "uploaded",
        "1.1.0",
        "--platform",
        "ios",
        "--build",
        "41",
    ]);
    repo.log(&[
        "--at",
        "2026-11-20T09:00:00Z",
        "uploaded",
        "1.1.0",
        "--platform",
        "android",
        "--build",
        "130",
    ]);
    repo.log(&[
        "--at",
        "2026-11-21T09:00:00Z",
        "uploaded",
        "1.2.0",
        "--platform",
        "ios",
        "--build",
        "45",
    ]);
    let view = repo.track_json();
    let releases = view["releases"].as_array().expect("releases");
    let find = |id: &str| releases.iter().find(|r| r["id"] == id).expect("release");
    let r110 = find("1.1.0");
    assert_eq!(r110["state"], "candidate");
    assert_eq!(r110["builds"]["ios"], "41");
    assert_eq!(r110["builds"]["android"], "130");
    // A log-only release with only uploads is a candidate.
    assert_eq!(find("1.2.0")["state"], "candidate");
    // releases.yaml `state: released` without a log stays accepted.
    assert_eq!(view["summary"]["last_release"]["id"], "1.0.0");
    assert_eq!(
        strings(&view["warnings"]),
        vec!["release `1.1.0`: releases.yaml lists ios build 40; the log uploaded 41"]
    );

    repo.log(&["--at", "2026-11-25T18:30:00Z", "released", "1.1.0"]);
    repo.log(&[
        "--at",
        "2026-11-26T09:00:00Z",
        "released",
        "1.0.0",
        "--date",
        "2026-10-02",
    ]);
    repo.log(&[
        "--at",
        "2026-11-27T09:00:00Z",
        "pulled",
        "1.2.0",
        "--reason",
        "crash at launch",
    ]);
    let view = repo.track_json();
    let releases = view["releases"].as_array().expect("releases");
    let find = |id: &str| releases.iter().find(|r| r["id"] == id).expect("release");
    assert_eq!(find("1.1.0")["state"], "released");
    assert_eq!(find("1.1.0")["released"], "2026-11-25");
    // Past releases show the last two; the pulled one, undated, sorts after them.
    let catalog = Catalog::load(repo.catalog()).expect("valid");
    let pulled = catalog.tracker.release("1.2.0").expect("log-only release");
    assert_eq!(pulled.state.as_str(), "pulled");
    assert_eq!(pulled.builds["ios"], "45");
    assert_eq!(view["summary"]["last_release"]["id"], "1.1.0");
    assert_eq!(
        strings(&view["warnings"]),
        vec![
            "release `1.0.0`: releases.yaml says released 2026-10-01; the log says 2026-10-02",
            "release `1.1.0`: releases.yaml lists ios build 40; the log uploaded 41",
            "release `1.1.0`: releases.yaml says candidate; the log says released",
        ]
    );
}

#[test]
fn suite_shows_log_warnings() {
    let repo = Repo::new();
    repo.log(&[
        "--at",
        "2026-11-14T09:00:00Z",
        "reopened",
        "bug.declared-fixed",
        "--reason",
        "again",
    ]);
    fs::write(
        repo.0.join("suite.toml"),
        "title = \"Suite\"\n\n[apps.app]\ncatalog = \"spec/app\"\nlabel = \"Logged\"\n",
    )
    .expect("suite");
    success(repo.run(&["suite", "suite.toml"]));
    let markdown = fs::read_to_string(repo.0.join("suite.md")).expect("suite.md");
    assert!(markdown.contains("## Warnings\n\n- Logged: issue `bug.declared-fixed`: issues.yaml says fixed; the log says open\n"), "{markdown}");
    success(repo.run(&["suite", "suite.toml", "--format", "json"]));
    let json: Value =
        serde_json::from_str(&fs::read_to_string(repo.0.join("suite.json")).expect("json"))
            .expect("JSON");
    assert_eq!(strings(&json["apps"][0]["warnings"]).len(), 1);
}

#[test]
fn scan_git_reads_trailers_and_tags() {
    let repo = Repo::new();
    let a = repo.commit(
        "Alice",
        "2026-11-10T10:00:00Z",
        "Fix home and plan\n\nFixes: bug.home-blank, bug.declared-planned\nReports: bug.nope\n",
    );
    repo.commit(
        "Bob",
        "2026-11-11T09:00:00Z",
        "Check home\n\nVerifies: bug.home-blank (journey 13/13, unit 4/4)\n",
    );
    let c = repo.commit(
        "Alice",
        "2026-11-11T12:00:00Z",
        "Check my own fix\n\nVerifies: bug.declared-planned (unit 1/1)\n",
    );
    repo.git(&["tag", "ios/v1.1.0", &c], "Alice", "2026-11-11T12:00:00Z");
    let d = repo.commit(
        "Alice",
        "2026-11-12T10:00:00Z",
        "Late fix\n\nFixes: bug.declared-fixed\n",
    );
    repo.commit(
        "Bob",
        "2026-11-13T10:00:00Z",
        "Check late fix\n\nVerifies: bug.declared-fixed\nReports: bug.home-blank\n",
    );
    repo.git(
        &["tag", "-a", "app-1.2.0", "-m", "Release 1.2.0"],
        "Releaser",
        "2026-11-20T08:00:00Z",
    );
    repo.git(&["tag", "unrelated-tag"], "Alice", "2026-11-20T08:00:00Z");

    let output = repo.run(&["log", "scan-git"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let out = success(output);
    assert!(
        stderr.contains("Reports: bug.nope is not a known issue"),
        "{stderr}"
    );
    assert!(
        out.ends_with("scan-git: 9 recorded, 0 already recorded\n"),
        "{out}"
    );
    let lines = repo.lines();
    let kinds: Vec<(String, String, String)> = lines
        .iter()
        .map(|l| {
            (
                l["kind"].as_str().unwrap_or_default().to_owned(),
                l["subject"].as_str().unwrap_or_default().to_owned(),
                l["by"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String, String)> = [
        ("fixed", "bug.home-blank", "Alice"),
        ("fixed", "bug.declared-planned", "Alice"),
        ("verified", "bug.home-blank", "Bob"),
        ("verified", "bug.declared-planned", "Alice"),
        ("fixed", "bug.declared-fixed", "Alice"),
        ("verified", "bug.declared-fixed", "Bob"),
        ("reported", "bug.home-blank", "Bob"),
        ("released", "1.1.0", "Alice"),
        ("released", "1.2.0", "Releaser"),
    ]
    .iter()
    .map(|(k, s, b)| (k.to_string(), s.to_string(), b.to_string()))
    .collect();
    assert_eq!(kinds, expected);
    assert_eq!(lines[0]["commit"], a.as_str());
    assert_eq!(lines[0]["at"], "2026-11-10T10:00:00Z");
    assert_eq!(
        strings(&lines[2]["evidence"]),
        vec!["journey 13/13, unit 4/4"]
    );
    assert_eq!(
        strings(&lines[5]["evidence"]),
        vec![format!(
            "commit {}",
            &lines[5]["commit"].as_str().unwrap_or_default()[..12]
        )]
    );
    assert_eq!(lines[6]["title"], "Home is blank");
    assert_eq!(lines[7]["commit"], c.as_str());
    assert_eq!(lines[7]["date"], "2026-11-11");
    assert_eq!(lines[8]["at"], "2026-11-20T08:00:00Z");

    // Idempotent.
    let again = success(repo.run(&["log", "scan-git"]));
    assert_eq!(again, "scan-git: 0 recorded, 9 already recorded\n");

    let view = repo.track_json();
    // Fixed by Alice, verified by Bob, contained in both released tags: shipped.
    let releases = view["releases"].as_array().expect("releases");
    let r110 = releases.iter().find(|r| r["id"] == "1.1.0").expect("1.1.0");
    assert_eq!(r110["state"], "released");
    assert_eq!(r110["released"], "2026-11-11");
    assert_eq!(r110["carries"][0]["state"], "shipped");
    assert!(releases
        .iter()
        .any(|r| r["id"] == "1.2.0" && r["state"] == "released"));
    // Self-verified, and its fix shipped in both releases.
    let planned = view["unverified"]
        .as_array()
        .and_then(|u| u.iter().find(|u| u["id"] == "bug.declared-planned"))
        .expect("self-verified");
    assert_eq!(
        strings(&planned["reasons"]),
        vec![
            "self-verified by Alice",
            "release 1.1.0 has shipped",
            "release 1.2.0 has shipped"
        ]
    );
    assert_eq!(
        strings(&view["warnings"]),
        vec![
            "issue `bug.declared-fixed`: issues.yaml says fixed; the log says verified",
            "issue `bug.declared-planned`: issues.yaml says planned; the log says fixed",
            "release `1.1.0`: releases.yaml says candidate; the log says released",
        ]
    );
    // The late fix is in 1.2.0 only; shipped there.
    let catalog = Catalog::load(repo.catalog()).expect("valid");
    assert_eq!(
        catalog.tracker.derived["bug.declared-fixed"].shipped_in,
        vec!["1.2.0"]
    );
    assert_eq!(catalog.tracker.derived["bug.declared-fixed"].fixes, vec![d]);
    assert_eq!(
        catalog.tracker.derived["bug.home-blank"].shipped_in,
        vec!["1.1.0", "1.2.0"]
    );

    // --since: only later commits and tags outside the ref.
    let head = repo.git(&["rev-parse", "HEAD"], "x", "2026-11-21T00:00:00Z");
    repo.log(&["report", "bug", "New thing"]);
    repo.commit(
        "Carol",
        "2026-11-21T10:00:00Z",
        "Fix new thing\n\nFixes: bug.new-thing\n",
    );
    repo.git(&["tag", "ios/v1.3.0"], "Carol", "2026-11-21T10:00:00Z");
    let since = success(repo.run(&["log", "scan-git", "--since", &head]));
    assert!(
        since.ends_with("scan-git: 2 recorded, 0 already recorded\n"),
        "{since}"
    );
}

#[test]
fn shipped_needs_git() {
    let repo = Repo::new();
    // Commits git does not know: the check is skipped with a warning, never guessed.
    repo.log(&[
        "--at",
        "2026-11-10T09:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "1111111",
    ]);
    repo.log(&[
        "--at",
        "2026-11-11T09:00:00Z",
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
    ]);
    repo.log(&[
        "--at",
        "2026-11-12T09:00:00Z",
        "released",
        "1.1.0",
        "--commit",
        "2222222",
    ]);
    let view = repo.track_json();
    let warnings = strings(&view["warnings"]);
    assert!(
        warnings.iter().any(|w| w.starts_with(
            "issue `bug.home-blank`: could not check whether fix 1111111 shipped in `1.1.0`"
        )),
        "{warnings:#?}"
    );
    let release = view["releases"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["id"] == "1.1.0"))
        .expect("1.1.0");
    assert_eq!(release["carries"][0]["state"], "verified");
}
