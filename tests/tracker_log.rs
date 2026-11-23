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
        warnings.iter().any(
            |w| w == "issue `bug.home-blank`: fix 1111111 in `1.1.0`: commit 1111111 is not known to git (or is ambiguous)"
        ),
        "{warnings:#?}"
    );
    let release = view["releases"]
        .as_array()
        .and_then(|r| r.iter().find(|r| r["id"] == "1.1.0"))
        .expect("1.1.0");
    assert_eq!(release["carries"][0]["state"], "verified");
}

// ---------------------------------------------------------------------------------------
// Review fixes: concurrency, path safety, verification and shipping rules, git edge cases.

fn fixed_line(subject: &str, commit: &str, by: &str, at: &str) -> String {
    let mut event =
        concordance::log::Event::new(concordance::log::EventKind::Fixed, subject, at, by);
    event.commit = Some(commit.into());
    event.with_id().line().expect("line")
}
fn lock_path(repo: &Repo) -> PathBuf {
    repo.0.join("spec/app/tracker/log.jsonl.lock")
}
/// Hold the log's OS advisory lock until dropped.
fn hold_lock(repo: &Repo) -> fs::File {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path(repo))
        .expect("lock file");
    file.lock().expect("lock");
    file
}
fn unverified_reasons(view: &Value, id: &str) -> Vec<String> {
    view["unverified"]
        .as_array()
        .and_then(|u| u.iter().find(|u| u["id"] == id))
        .map(|u| strings(&u["reasons"]))
        .unwrap_or_default()
}

#[test]
fn repeated_ids_load_and_blank_lines_are_ignored() {
    let repo = Repo::new();
    let first = fixed_line("bug.home-blank", "abc1234", "alice", "2026-11-10T00:00:00Z");
    // Same fact twice (two racing writers), with blank lines around: loads cleanly.
    fs::create_dir_all(repo.catalog().join("tracker")).expect("mkdir");
    fs::write(repo.0.join(LOG), format!("\n{first}\n\n  \n{first}\n\n")).expect("write");
    let catalog = Catalog::load(repo.catalog()).expect("repeat of the same line is fine");
    assert_eq!(catalog.tracker.log.len(), 1);
    assert!(
        catalog.tracker.warnings.is_empty(),
        "{:?}",
        catalog.tracker.warnings
    );
    // Appending works and never writes a blank line.
    repo.log(&["report", "bug", "After blanks"]);
    let text = fs::read_to_string(repo.0.join(LOG)).expect("log");
    assert!(text.ends_with("}\n") && !text.ends_with("\n\n"), "{text:?}");
    // Same id, different content: the first line is kept, with a warning.
    let other = fixed_line("bug.home-blank", "abc1234", "bob", "2026-11-11T00:00:00Z");
    fs::write(repo.0.join(LOG), format!("{first}\n{other}\n")).expect("write");
    let view = repo.track_json();
    let warnings = strings(&view["warnings"]);
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("tracker/log.jsonl:2: event ")
                && w.ends_with("repeats line 1 with different content; line 1 is kept")),
        "{warnings:#?}"
    );
    // A log of only newlines is an empty log.
    fs::write(repo.0.join(LOG), "\n\n").expect("write");
    Catalog::load(repo.catalog()).expect("blank log");
}

#[test]
fn concurrent_writers_never_duplicate() {
    let repo = Repo::new();
    let children: Vec<std::process::Child> = (0..8)
        .map(|n| {
            let title = if n % 2 == 0 {
                "Same thing".to_owned()
            } else {
                format!("Thing {n}")
            };
            Command::new(env!("CARGO_BIN_EXE_concordance"))
                .args(["log", "report", "bug", &title])
                .current_dir(&repo.0)
                .env("CONCORDANCE_ACTOR", "tester")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env_remove("CONCORDANCE_CATALOG")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn")
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().expect("wait");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = fs::read_to_string(repo.0.join(LOG)).expect("log");
    let parsed = concordance::log::parse(&text);
    assert!(
        parsed.errors.is_empty() && parsed.warnings.is_empty(),
        "{text}"
    );
    assert_eq!(parsed.events.len(), 5, "{text}");
    assert_eq!(text.lines().count(), 5, "{text}");
}

#[test]
fn lock_waits_for_the_holder() {
    let repo = Repo::new();
    fs::create_dir_all(repo.catalog().join("tracker")).expect("mkdir");
    // An OS advisory lock held elsewhere: the writer waits until it is released.
    let held = hold_lock(&repo);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        drop(held);
    });
    let started = std::time::Instant::now();
    repo.log(&["report", "bug", "Waited"]);
    assert!(started.elapsed() >= std::time::Duration::from_millis(300));
    releaser.join().expect("thread");
    // A lock file left behind (no holder) never blocks: the OS lock died with its process.
    assert!(lock_path(&repo).exists());
    repo.log(&["report", "bug", "Unblocked"]);
    assert_eq!(repo.lines().len(), 2);
}

#[test]
fn refuses_symlinked_log_paths() {
    let repo = Repo::new();
    let outside = repo.0.join("outside");
    fs::create_dir(&outside).expect("mkdir");
    // tracker/ itself a symlink out of the catalog.
    let tracker = repo.catalog().join("tracker");
    fs::rename(&tracker, outside.join("tracker")).expect("move tracker");
    std::os::unix::fs::symlink(outside.join("tracker"), &tracker).expect("symlink");
    let err = failure(repo.run(&["log", "report", "bug", "Nope"]));
    assert!(err.contains("symlink"), "{err}");
    assert!(!outside.join("tracker/log.jsonl").exists());
    // log.jsonl a symlink.
    fs::remove_file(&tracker).expect("unlink");
    fs::rename(outside.join("tracker"), &tracker).expect("restore");
    std::os::unix::fs::symlink(outside.join("log.jsonl"), tracker.join("log.jsonl"))
        .expect("symlink");
    let err = failure(repo.run(&["log", "report", "bug", "Nope"]));
    assert!(err.contains("symlink"), "{err}");
    assert!(!outside.join("log.jsonl").exists());
}

#[test]
fn verification_must_postdate_or_descend_from_the_fix() {
    let repo = Repo::new();
    // Bob checks before any fix exists, then Alice fixes: the check does not count.
    repo.log(&[
        "--at",
        "2026-11-09T00:00:00Z",
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "early",
    ]);
    repo.log(&[
        "--at",
        "2026-11-10T00:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "aaaa1111",
    ]);
    let view = repo.track_json();
    assert_eq!(
        issue(&view, "bugs", "bug.home-blank").expect("open")["state"],
        "fixed"
    );
    assert_eq!(
        unverified_reasons(&view, "bug.home-blank")[0],
        "checked before the latest fix by bob"
    );

    // A check recorded earlier but of a commit that contains the fix counts.
    let fix = repo.commit("Alice", "2026-11-12T00:00:00Z", "Fix plan");
    let later = repo.commit("Alice", "2026-11-13T00:00:00Z", "Later");
    repo.log(&[
        "--at",
        "2026-11-14T00:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.declared-planned",
        "--commit",
        &fix,
    ]);
    repo.log(&[
        "--at",
        "2026-11-01T00:00:00Z",
        "--by",
        "carol",
        "verified",
        "bug.declared-planned",
        "--evidence",
        "ci",
        "--commit",
        &later,
    ]);
    let view = repo.track_json();
    assert!(
        issue(&view, "bugs", "bug.declared-planned").is_none(),
        "{view:#}"
    );
}

#[test]
fn verified_binds_to_the_current_fix() {
    let repo = Repo::new();
    let verify = |at: &str| {
        repo.log(&[
            "--at",
            at,
            "--by",
            "bob",
            "verified",
            "bug.home-blank",
            "--evidence",
            "unit 3/3",
        ])
    };
    repo.log(&[
        "--at",
        "2026-11-10T00:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "c1c1c1c1",
    ]);
    assert!(verify("2026-11-11T00:00:00Z").starts_with("recorded"));
    assert_eq!(repo.lines()[1]["fix"], "c1c1c1c1");
    assert!(repo.lines()[1].get("commit").is_none());
    // Rerunning for the same fix is a no-op.
    assert!(verify("2026-11-11T05:00:00Z").starts_with("already recorded"));
    repo.log(&[
        "--at",
        "2026-11-12T00:00:00Z",
        "reopened",
        "bug.home-blank",
        "--reason",
        "back",
    ]);
    repo.log(&[
        "--at",
        "2026-11-13T00:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "c2c2c2c2",
    ]);
    // The same evidence after the new fix is a new fact.
    assert!(verify("2026-11-14T00:00:00Z").starts_with("recorded"));
    assert_eq!(repo.lines()[4]["fix"], "c2c2c2c2");
    // The id includes the bound fix, so the two checks are distinct facts.
    assert_ne!(repo.lines()[1]["id"], repo.lines()[4]["id"]);
    let view = repo.track_json();
    assert!(issue(&view, "bugs", "bug.home-blank").is_none());
}

#[test]
fn released_without_commit_does_not_block_one_with_commit() {
    let repo = Repo::new();
    let fix = repo.commit("Alice", "2026-11-10T00:00:00Z", "Fix");
    repo.log(&[
        "--at",
        "2026-11-10T01:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        &fix,
    ]);
    repo.log(&[
        "--at",
        "2026-11-10T02:00:00Z",
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
    ]);
    repo.log(&["--at", "2026-11-11T00:00:00Z", "released", "1.1.0"]);
    let first = repo.track_json();
    let r = |view: &Value| {
        view["releases"]
            .as_array()
            .and_then(|r| r.iter().find(|r| r["id"] == "1.1.0"))
            .cloned()
            .expect("1.1.0")
    };
    assert_eq!(r(&first)["carries"][0]["state"], "verified");
    let out = repo.log(&[
        "--at",
        "2026-11-12T00:00:00Z",
        "released",
        "1.1.0",
        "--commit",
        "HEAD",
    ]);
    assert!(out.starts_with("recorded"), "{out}");
    let second = repo.track_json();
    assert_eq!(r(&second)["carries"][0]["state"], "shipped");
    assert_eq!(r(&second)["released"], "2026-11-11", "earliest date wins");
}

#[test]
fn shipped_in_unverified_needs_git_proof() {
    let repo = Repo::new();
    let release = repo.commit("Rel", "2026-11-10T00:00:00Z", "Release");
    let fix = repo.commit("Alice", "2026-11-11T00:00:00Z", "Fix after the release");
    // 1.1.0 carries bug.home-blank in the plan; git says its commit lacks the fix.
    repo.log(&[
        "--at",
        "2026-11-10T01:00:00Z",
        "released",
        "1.1.0",
        "--commit",
        &release,
    ]);
    repo.log(&[
        "--at",
        "2026-11-11T01:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        &fix,
    ]);
    let view = repo.track_json();
    assert_eq!(
        unverified_reasons(&view, "bug.home-blank"),
        vec!["fixed without evidence"]
    );
    // A release whose commit contains the fix has shipped it.
    let later = repo.commit("Rel", "2026-11-12T00:00:00Z", "Release 2");
    repo.log(&[
        "--at",
        "2026-11-12T01:00:00Z",
        "released",
        "1.2.0",
        "--commit",
        &later,
    ]);
    let view = repo.track_json();
    assert_eq!(
        unverified_reasons(&view, "bug.home-blank"),
        vec!["fixed without evidence", "release 1.2.0 has shipped"]
    );
}

#[test]
fn scan_git_since_is_resolved_and_checked() {
    let repo = Repo::new();
    repo.commit("Alice", "2026-11-10T00:00:00Z", "First");
    let err = failure(repo.run(&["log", "scan-git", "--since=--output=/tmp/x"]));
    assert!(err.contains("is not a git ref"), "{err}");
    let err = failure(repo.run(&["log", "scan-git", "--since", "no-such-ref"]));
    assert!(err.contains("does not name exactly one commit"), "{err}");
    let out = success(repo.run(&["log", "scan-git", "--since", "HEAD"]));
    assert_eq!(out, "scan-git: 0 recorded, 0 already recorded\n");
}

#[test]
fn scan_git_reads_tags_by_full_refname() {
    let repo = Repo::new();
    let tagged = repo.commit("Alice", "2026-11-10T00:00:00Z", "Tagged");
    repo.git(&["tag", "ios/v1.4.0"], "Alice", "2026-11-10T00:00:00Z");
    repo.commit("Alice", "2026-11-11T00:00:00Z", "Elsewhere");
    // A branch with the same short name points somewhere else.
    repo.git(&["branch", "ios/v1.4.0"], "Alice", "2026-11-11T00:00:00Z");
    success(repo.run(&["log", "scan-git"]));
    let lines = repo.lines();
    let release = lines
        .iter()
        .find(|l| l["kind"] == "released")
        .expect("tag recorded");
    assert_eq!(release["subject"], "1.4.0");
    assert_eq!(release["commit"], tagged.as_str());
}

#[test]
fn scan_git_waits_for_writers_and_rereads_under_the_lock() {
    let repo = Repo::new();
    let a = repo.commit(
        "Alice",
        "2026-11-10T00:00:00Z",
        "Fix\n\nFixes: bug.home-blank, bug.declared-planned\n",
    );
    repo.commit(
        "Bob",
        "2026-11-11T00:00:00Z",
        "Check\n\nVerifies: bug.home-blank (ok)\n",
    );
    fs::create_dir_all(repo.catalog().join("tracker")).expect("mkdir");
    let held = hold_lock(&repo);
    let child = Command::new(env!("CARGO_BIN_EXE_concordance"))
        .args(["log", "scan-git"])
        .current_dir(&repo.0)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("CONCORDANCE_CATALOG")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    std::thread::sleep(std::time::Duration::from_millis(400));
    // Another writer records one of the same facts while the scan waits.
    let line = fixed_line("bug.home-blank", &a, "Alice", "2026-11-10T00:00:00Z");
    fs::write(repo.0.join(LOG), format!("{line}\n")).expect("write");
    drop(held);
    let output = child.wait_with_output().expect("wait");
    let out = success(output);
    assert!(
        out.ends_with("scan-git: 2 recorded, 1 already recorded\n"),
        "{out}"
    );
    let text = fs::read_to_string(repo.0.join(LOG)).expect("log");
    assert_eq!(text.lines().count(), 3, "{text}");
    assert!(concordance::log::parse(&text).warnings.is_empty());
}

#[test]
fn shallow_history_is_unknown_not_no() {
    let origin = Repo::new();
    origin.git(&["add", "-A"], "Alice", "2026-11-09T00:00:00Z");
    let fix = origin.commit("Alice", "2026-11-10T00:00:00Z", "Fix");
    origin.git(
        &["tag", "ios/v0.9.0", &fix],
        "Alice",
        "2026-11-10T00:00:00Z",
    );
    origin.commit("Alice", "2026-11-11T00:00:00Z", "Middle");
    let head = origin.commit("Alice", "2026-11-12T00:00:00Z", "Head");
    let clone = origin.0.join("clone");
    let url = format!("file://{}", origin.0.display());
    origin.git(
        &["clone", "--quiet", "--depth", "1", &url, "clone"],
        "x",
        "2026-11-13T00:00:00Z",
    );
    let in_clone = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&clone)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    in_clone(&[
        "fetch",
        "--quiet",
        "--depth",
        "1",
        "origin",
        "refs/tags/ios/v0.9.0:refs/tags/ios/v0.9.0",
    ]);
    let catalog = clone.join("spec/app");
    let catalog_arg = catalog.display().to_string();
    let run = |args: &[&str]| {
        let mut all = vec!["log", "--catalog", &catalog_arg];
        all.extend_from_slice(args);
        origin.run(&all)
    };
    success(run(&[
        "--at",
        "2026-11-12T01:00:00Z",
        "released",
        "1.1.0",
        "--commit",
        &head,
    ]));
    success(run(&[
        "--at",
        "2026-11-10T01:00:00Z",
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        &fix,
    ]));
    let out = clone.join("tracker.json");
    success(origin.run(&[
        "track",
        &catalog_arg,
        "--format",
        "json",
        "--out",
        &out.display().to_string(),
    ]));
    let view: Value = serde_json::from_str(&fs::read_to_string(out).expect("json")).expect("JSON");
    let warnings = strings(&view["warnings"]);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("shallow history cannot tell")),
        "{warnings:#?}"
    );
    assert_eq!(
        unverified_reasons(&view, "bug.home-blank"),
        vec![
            "fixed without evidence",
            "release 1.1.0 is out; containment unknown"
        ]
    );
    // --since in a shallow clone: a tag it cannot place is scanned, with a warning.
    let output = run(&["scan-git", "--since", &head]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let stdout = success(output);
    assert!(
        stderr.contains("shallow history cannot tell whether --since contains it"),
        "{stderr}"
    );
    assert!(stdout.contains("recorded: released 0.9.0"), "{stdout}");
}

// ---------------------------------------------------------------------------------------
// Second review: legacy ids, racing reports, torn writes, commit identity, ordering, scale.

fn spawn_log(repo: &Repo, args: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_concordance"))
        .arg("log")
        .args(args)
        .current_dir(&repo.0)
        .env("CONCORDANCE_ACTOR", "tester")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("CONCORDANCE_CATALOG")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn")
}
fn finish(child: std::process::Child) -> String {
    success(child.wait_with_output().expect("wait"))
}

#[test]
fn legacy_ids_still_load_and_dedupe() {
    let repo = Repo::new();
    // A released event as the previous release wrote it: the id omits the commit part.
    let mut released = concordance::log::Event::new(
        concordance::log::EventKind::Released,
        "1.1.0",
        "2026-11-11T00:00:00Z",
        "rel",
    );
    released.date = Some("2026-11-11".into());
    let legacy = released.legacy_id();
    assert_ne!(legacy, released.stable_id());
    released.id = legacy.clone();
    let line = released.line().expect("line");
    fs::create_dir_all(repo.catalog().join("tracker")).expect("mkdir");
    fs::write(repo.0.join(LOG), format!("{line}\n")).expect("write");
    let catalog = Catalog::load(repo.catalog()).expect("legacy id accepted");
    assert_eq!(catalog.tracker.log[0].id, legacy);
    // Recording the same fact again matches the legacy id: nothing written.
    let again = repo.log(&["--at", "2026-11-11T00:00:00Z", "released", "1.1.0"]);
    assert!(again.starts_with("already recorded"), "{again}");
    assert_eq!(repo.lines().len(), 1);
    // New lines use the current form.
    repo.log(&["--at", "2026-11-12T00:00:00Z", "released", "1.2.0"]);
    let lines = repo.lines();
    let mut fresh = concordance::log::Event::new(
        concordance::log::EventKind::Released,
        "1.2.0",
        "2026-11-12T00:00:00Z",
        "tester",
    );
    fresh.date = Some("2026-11-12".into());
    assert_eq!(lines[1]["id"], fresh.stable_id().as_str());
}

#[test]
fn racing_reports_get_distinct_ids() {
    for _ in 0..3 {
        let repo = Repo::new();
        let a = spawn_log(&repo, &["report", "bug", "Login fails!"]);
        let b = spawn_log(&repo, &["report", "bug", "Login fails?"]);
        let (a, b) = (finish(a), finish(b));
        let mut subjects: Vec<String> = repo
            .lines()
            .iter()
            .filter_map(|l| l["subject"].as_str().map(str::to_owned))
            .collect();
        subjects.sort();
        assert_eq!(
            subjects,
            vec!["bug.login-fails", "bug.login-fails-2"],
            "{a}{b}"
        );
        let catalog = Catalog::load(repo.catalog()).expect("valid");
        assert!(catalog.tracker.issue("bug.login-fails-2").is_some());
    }
}

#[test]
fn torn_last_line_is_ignored_then_cut() {
    let repo = Repo::new();
    repo.log(&["--at", "2026-11-20", "report", "bug", "Kept"]);
    let good = fs::read_to_string(repo.0.join(LOG)).expect("log");
    fs::write(
        repo.0.join(LOG),
        format!("{good}{{\"id\":\"12ab\",\"kind\":\"fix"),
    )
    .expect("torn");
    let view = repo.track_json();
    assert!(
        strings(&view["warnings"])
            .iter()
            .any(|w| w.starts_with("tracker/log.jsonl:2: unterminated last line")),
        "{view:#}"
    );
    let output = repo.run(&["log", "report", "bug", "Next"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    success(output);
    assert!(stderr.contains("cutting a torn last line"), "{stderr}");
    let text = fs::read_to_string(repo.0.join(LOG)).expect("log");
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(concordance::log::parse(&text).warnings.is_empty());
    // A malformed line elsewhere stays an error.
    fs::write(repo.0.join(LOG), format!("{{broken\n{good}")).expect("write");
    let (_, report) = Catalog::load_collect(repo.catalog()).expect("loadable");
    assert!(report
        .errors
        .iter()
        .any(|e| e.reference == "tracker/log.jsonl:1"));
}

#[test]
fn commits_are_compared_by_resolved_hash() {
    let repo = Repo::new();
    let fix = repo.commit("Alice", "2026-11-10T00:00:00Z", "Fix");
    repo.log(&["--by", "alice", "fixed", "bug.home-blank", "--commit", &fix]);
    // A check naming a short form of the fix commit counts once git resolves it.
    repo.log(&[
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
        "--commit",
        &fix[..9],
    ]);
    let view = repo.track_json();
    assert!(issue(&view, "bugs", "bug.home-blank").is_none(), "{view:#}");

    // A check naming a commit git does not know is unconfirmed, never proof, even when it
    // shares a prefix with the fix.
    let other = repo.commit("Alice", "2026-11-11T00:00:00Z", "Fix plan");
    repo.log(&[
        "--by",
        "alice",
        "fixed",
        "bug.declared-planned",
        "--commit",
        &other,
    ]);
    let mut check = concordance::log::Event::new(
        concordance::log::EventKind::Verified,
        "bug.declared-planned",
        "2026-11-12T00:00:00Z",
        "bob",
    );
    check.evidence = vec!["ci".into()];
    check.commit = Some(format!("{}0000", &other[..8]));
    let line = check.with_id().line().expect("line");
    let mut text = fs::read_to_string(repo.0.join(LOG)).expect("log");
    text.push_str(&line);
    text.push('\n');
    fs::write(repo.0.join(LOG), text).expect("write");
    let view = repo.track_json();
    assert_eq!(
        unverified_reasons(&view, "bug.declared-planned")[0],
        "check by bob unconfirmed: git cannot place its commit after the fix"
    );
    assert!(strings(&view["warnings"])
        .iter()
        .any(|w| w.contains("is not known to git (or is ambiguous)")));
}

#[test]
fn file_order_decides_whether_a_check_follows_a_fix() {
    let repo = Repo::new();
    // Same timestamp; the check is appended before the fix.
    let at = "2026-11-10T00:00:00Z";
    repo.log(&[
        "--at",
        at,
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
    ]);
    repo.log(&[
        "--at",
        at,
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "f00d",
    ]);
    let view = repo.track_json();
    assert_eq!(
        unverified_reasons(&view, "bug.home-blank"),
        vec!["checked before the latest fix by bob"]
    );
    // Appended after the fix, even with an earlier timestamp, it counts.
    repo.log(&[
        "--at",
        "2026-11-01T00:00:00Z",
        "--by",
        "carol",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
    ]);
    let view = repo.track_json();
    assert!(issue(&view, "bugs", "bug.home-blank").is_none());
}

#[test]
fn large_logs_load_quickly() {
    let repo = Repo::new();
    let mut text = String::new();
    for n in 0..4000 {
        let subject = format!("bug.generated-{n}");
        let at = "2026-11-10T00:00:00Z";
        let mut report =
            concordance::log::Event::new(concordance::log::EventKind::Reported, &subject, at, "a");
        report.title = Some(format!("Generated {n}"));
        text.push_str(&report.with_id().line().expect("line"));
        text.push('\n');
        text.push_str(&fixed_line(&subject, &format!("c{n:07}"), "a", at));
        text.push('\n');
    }
    fs::create_dir_all(repo.catalog().join("tracker")).expect("mkdir");
    fs::write(repo.0.join(LOG), text).expect("write");
    let started = std::time::Instant::now();
    let catalog = Catalog::load(repo.catalog()).expect("valid");
    assert_eq!(catalog.tracker.issues.len(), 4003);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    let out = repo.log(&["report", "bug", "Generated 3999"]);
    assert!(out.starts_with("already recorded"), "{out}");
}

// ---------------------------------------------------------------------------------------
// Third review: bytes, legacy richness, bound fixes, rescans, ambiguity, index safety.

fn append_line(repo: &Repo, line: &str) {
    let mut text = fs::read_to_string(repo.0.join(LOG)).unwrap_or_default();
    text.push_str(line);
    text.push('\n');
    fs::create_dir_all(repo.catalog().join("tracker")).expect("mkdir");
    fs::write(repo.0.join(LOG), text).expect("write");
}

#[test]
fn torn_line_inside_a_multibyte_character() {
    let repo = Repo::new();
    repo.log(&["--at", "2026-11-20", "report", "bug", "Kept"]);
    let mut bytes = fs::read(repo.0.join(LOG)).expect("log");
    // "{"title":"é" cut after the first byte of é.
    bytes.extend_from_slice(b"{\"title\":\"\xc3");
    fs::write(repo.0.join(LOG), &bytes).expect("torn");
    let view = repo.track_json();
    assert!(strings(&view["warnings"])
        .iter()
        .any(|w| w.starts_with("tracker/log.jsonl:2: unterminated last line")));
    repo.log(&["report", "bug", "Next"]);
    let text = fs::read_to_string(repo.0.join(LOG)).expect("valid UTF-8 again");
    assert_eq!(text.lines().count(), 2, "{text}");
    // Invalid UTF-8 on an earlier line is an error naming that line.
    let mut bad = b"\xff\xfe\n".to_vec();
    bad.extend_from_slice(text.as_bytes());
    fs::write(repo.0.join(LOG), bad).expect("write");
    let (_, report) = Catalog::load_collect(repo.catalog()).expect("loadable");
    assert!(report
        .errors
        .iter()
        .any(|e| e.reference == "tracker/log.jsonl:1" && e.message.contains("UTF-8")));
}

#[test]
fn legacy_lines_do_not_hide_richer_facts() {
    let repo = Repo::new();
    let commit = repo.commit("Rel", "2026-11-10T00:00:00Z", "Release");
    // A legacy released line without a commit.
    let mut legacy = concordance::log::Event::new(
        concordance::log::EventKind::Released,
        "1.1.0",
        "2026-11-11T00:00:00Z",
        "rel",
    );
    legacy.date = Some("2026-11-11".into());
    legacy.id = legacy.legacy_id();
    append_line(&repo, &legacy.line().expect("line"));
    // The same release with a commit adds something: appended.
    let out = repo.log(&[
        "--at",
        "2026-11-11T00:00:00Z",
        "released",
        "1.1.0",
        "--commit",
        &commit,
    ]);
    assert!(out.starts_with("recorded"), "{out}");
    // Without a commit it adds nothing: already recorded.
    let out = repo.log(&["--at", "2026-11-11T00:00:00Z", "released", "1.1.0"]);
    assert!(out.starts_with("already recorded"), "{out}");
    // A legacy verification without a bound fix; the same check bound to a fix is new.
    repo.log(&[
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "f1f1f1f1",
    ]);
    let mut check = concordance::log::Event::new(
        concordance::log::EventKind::Verified,
        "bug.home-blank",
        "2026-11-12T00:00:00Z",
        "bob",
    );
    check.evidence = vec!["ok".into()];
    check.id = check.legacy_id();
    append_line(&repo, &check.line().expect("line"));
    let out = repo.log(&[
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
    ]);
    assert!(out.starts_with("recorded"), "{out}");
    assert_eq!(repo.lines().len(), 5);
}

#[test]
fn a_check_bound_to_an_earlier_fix_does_not_verify_a_newer_one() {
    let repo = Repo::new();
    repo.log(&[
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "aaaa0001",
    ]);
    // Bob's check is bound to the first fix, then a second fix lands.
    let mut check = concordance::log::Event::new(
        concordance::log::EventKind::Verified,
        "bug.home-blank",
        "2026-11-12T00:00:00Z",
        "bob",
    );
    check.evidence = vec!["ok".into()];
    check.fix = Some("aaaa0001".into());
    repo.log(&[
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        "bbbb0002",
    ]);
    // Appended after the second fix in the file, but bound to the first.
    append_line(&repo, &check.with_id().line().expect("line"));
    let view = repo.track_json();
    assert_eq!(
        unverified_reasons(&view, "bug.home-blank"),
        vec!["checked an earlier fix by bob"]
    );
    // `log verified` binds under the lock to the current (second) fix.
    repo.log(&[
        "--by",
        "carol",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
    ]);
    assert_eq!(repo.lines().last().expect("line")["fix"], "bbbb0002");
    let view = repo.track_json();
    assert!(issue(&view, "bugs", "bug.home-blank").is_none());
}

#[test]
fn rescan_binds_the_same_fix() {
    let repo = Repo::new();
    repo.commit(
        "Alice",
        "2026-11-10T00:00:00Z",
        "Fix A\n\nFixes: bug.home-blank\n",
    );
    repo.commit(
        "Bob",
        "2026-11-11T00:00:00Z",
        "Check A\n\nVerifies: bug.home-blank (ok)\n",
    );
    repo.commit(
        "Alice",
        "2026-11-12T00:00:00Z",
        "Fix B\n\nFixes: bug.home-blank\n",
    );
    let first = success(repo.run(&["log", "scan-git"]));
    assert!(
        first.ends_with("scan-git: 3 recorded, 0 already recorded\n"),
        "{first}"
    );
    let lines = repo.lines();
    assert_eq!(lines[1]["fix"], lines[0]["commit"]);
    let second = success(repo.run(&["log", "scan-git"]));
    assert_eq!(second, "scan-git: 0 recorded, 3 already recorded\n");
    assert_eq!(repo.lines().len(), 3);
}

#[test]
fn ambiguous_names_are_unknown() {
    let repo = Repo::new();
    let first = repo.commit("Alice", "2026-11-10T00:00:00Z", "First");
    repo.git(&["tag", "dup"], "Alice", "2026-11-10T00:00:00Z");
    let second = repo.commit("Alice", "2026-11-11T00:00:00Z", "Second");
    repo.git(&["branch", "dup"], "Alice", "2026-11-11T00:00:00Z");
    assert_ne!(first, second);
    // `dup` names a tag and a branch: never resolved, always unknown.
    let output = repo.run(&["log", "fixed", "bug.home-blank", "--commit", "dup"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    success(output);
    assert!(
        stderr.contains("does not resolve to exactly one commit"),
        "{stderr}"
    );
    assert_eq!(repo.lines()[0]["commit"], "dup");
    let err = failure(repo.run(&["log", "scan-git", "--since", "dup"]));
    assert!(err.contains("does not name exactly one commit"), "{err}");
    // Full ref names and hashes are fine.
    let out = success(repo.run(&["log", "scan-git", "--since", "refs/tags/dup"]));
    assert!(out.starts_with("scan-git:"), "{out}");
    repo.log(&["released", "1.1.0", "--commit", "refs/tags/dup"]);
    assert_eq!(repo.lines()[1]["commit"], first.as_str());
    // The ambiguous fix commit makes shipping unknown, with a warning.
    let view = repo.track_json();
    assert!(strings(&view["warnings"])
        .iter()
        .any(|w| w.contains("commit dup is not known to git (or is ambiguous)")));
}

#[test]
fn issue_index_never_returns_the_wrong_issue() {
    let repo = Repo::new();
    let mut catalog = Catalog::load(repo.catalog()).expect("valid");
    let id = catalog.tracker.issues[0].id.clone();
    // Reorder the public vector behind the index's back.
    catalog.tracker.issues.reverse();
    let found = catalog.tracker.issue(&id).expect("still found");
    assert_eq!(found.id, id);
    let last = catalog.tracker.issues[0].id.clone();
    assert_eq!(
        catalog.tracker.issue(&last).map(|i| i.id.clone()),
        Some(last)
    );
    catalog.tracker.issues.clear();
    assert!(catalog.tracker.issue(&id).is_none());
}

// ---------------------------------------------------------------------------------------
// Fourth review: character-safe truncation, hex ambiguity, trailer order, cycles, paths.

#[test]
fn non_ascii_refs_do_not_panic() {
    let repo = Repo::new();
    repo.commit("Alice", "2026-11-10T00:00:00Z", "First");
    // 13 bytes, 12 characters: a 12-byte cut would land inside the é.
    let reference = "feature/café";
    repo.log(&[
        "--by",
        "alice",
        "fixed",
        "bug.home-blank",
        "--commit",
        reference,
    ]);
    repo.log(&[
        "--by",
        "bob",
        "verified",
        "bug.home-blank",
        "--evidence",
        "ok",
        "--commit",
        "feature/cafébabe",
    ]);
    repo.log(&["released", "1.1.0", "--commit", "HEAD"]);
    let view = repo.track_json();
    let warnings = strings(&view["warnings"]);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("commit feature/café is not known to git")),
        "{warnings:#?}"
    );
}

#[test]
fn hex_looking_names_get_the_ambiguity_check() {
    let repo = Repo::new();
    let first = repo.commit("Alice", "2026-11-10T00:00:00Z", "First");
    let second = repo.commit("Alice", "2026-11-11T00:00:00Z", "Second");
    // A branch named like an abbreviation of another commit.
    let abbreviation = &first[..8];
    repo.git(
        &["branch", abbreviation, &second],
        "Alice",
        "2026-11-11T00:00:00Z",
    );
    let output = repo.run(&["log", "fixed", "bug.home-blank", "--commit", abbreviation]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    success(output);
    assert!(
        stderr.contains("does not resolve to exactly one commit"),
        "{stderr}"
    );
    assert_eq!(repo.lines()[0]["commit"], abbreviation);
    // A hex name that is both a tag and a branch.
    repo.git(&["tag", "beef", &first], "Alice", "2026-11-11T00:00:00Z");
    repo.git(
        &["branch", "beef", &second],
        "Alice",
        "2026-11-11T00:00:00Z",
    );
    let err = failure(repo.run(&["log", "scan-git", "--since", "beef"]));
    assert!(err.contains("does not name exactly one commit"), "{err}");
    // A full hash is fine.
    let out = success(repo.run(&["log", "scan-git", "--since", &first]));
    assert!(out.starts_with("scan-git:"), "{out}");
}

#[test]
fn fixes_are_read_before_verifies_in_one_commit() {
    for message in [
        "Fix and check\n\nVerifies: bug.home-blank (ok)\nFixes: bug.home-blank\n",
        "Fix and check\n\nFixes: bug.home-blank\nVerifies: bug.home-blank (ok)\n",
    ] {
        let repo = Repo::new();
        let sha = repo.commit("Alice", "2026-11-10T00:00:00Z", message);
        let first = success(repo.run(&["log", "scan-git"]));
        assert!(
            first.ends_with("scan-git: 2 recorded, 0 already recorded\n"),
            "{first}"
        );
        let lines = repo.lines();
        assert_eq!(lines[0]["kind"], "fixed");
        assert_eq!(lines[1]["kind"], "verified");
        assert_eq!(lines[1]["fix"], sha.as_str(), "{message}");
        let second = success(repo.run(&["log", "scan-git"]));
        assert_eq!(
            second, "scan-git: 0 recorded, 2 already recorded\n",
            "{message}"
        );
    }
}

#[test]
fn release_after_a_pull_and_fix_after_a_reopen_are_new_facts() {
    let repo = Repo::new();
    let commit = repo.commit("Rel", "2026-11-10T00:00:00Z", "Release");
    let release = |at: &str| repo.log(&["--at", at, "released", "1.1.0", "--commit", &commit]);
    assert!(release("2026-11-10T01:00:00Z").starts_with("recorded"));
    assert!(release("2026-11-10T02:00:00Z").starts_with("already recorded"));
    repo.log(&[
        "--at",
        "2026-11-11T00:00:00Z",
        "pulled",
        "1.1.0",
        "--reason",
        "crash",
    ]);
    assert!(release("2026-11-12T00:00:00Z").starts_with("recorded"));
    assert!(release("2026-11-12T01:00:00Z").starts_with("already recorded"));
    assert_eq!(repo.lines()[2]["cycle"], 1);
    let catalog = Catalog::load(repo.catalog()).expect("valid");
    let state = catalog.tracker.release("1.1.0").map(|r| r.state.as_str());
    assert_eq!(state, Some("released"));

    let fix = |at: &str| {
        repo.log(&[
            "--at",
            at,
            "--by",
            "alice",
            "fixed",
            "bug.home-blank",
            "--commit",
            &commit,
        ])
    };
    assert!(fix("2026-11-13T00:00:00Z").starts_with("recorded"));
    repo.log(&[
        "--at",
        "2026-11-14T00:00:00Z",
        "reopened",
        "bug.home-blank",
        "--reason",
        "back",
    ]);
    assert!(fix("2026-11-15T00:00:00Z").starts_with("recorded"));
    assert!(fix("2026-11-15T01:00:00Z").starts_with("already recorded"));
    let view = repo.track_json();
    assert_eq!(
        issue(&view, "bugs", "bug.home-blank").expect("open")["state"],
        "fixed"
    );
}

#[test]
fn catalog_paths_through_symlinks_are_refused() {
    let repo = Repo::new();
    // spec/app is a symlink to a real catalog elsewhere in the repository.
    fs::rename(repo.catalog(), repo.0.join("real-app")).expect("move");
    std::os::unix::fs::symlink(repo.0.join("real-app"), repo.catalog()).expect("symlink");
    for args in [
        vec!["log", "report", "bug", "Nope"],
        vec!["log", "--catalog", "spec/app", "report", "bug", "Nope"],
    ] {
        let err = failure(repo.run(&args));
        assert!(err.contains("goes through symlink"), "{err}");
    }
    let absolute = repo.catalog().display().to_string();
    let err = failure(repo.run(&["log", "--catalog", &absolute, "report", "bug", "Nope"]));
    assert!(err.contains("goes through symlink"), "{err}");
    // The real path works.
    repo.log(&["--catalog", "real-app", "report", "bug", "Fine"]);
    // A relative path that leaves the current directory is refused.
    let other = Repo::new();
    let outside = format!(
        "../{}/spec/app",
        other.0.file_name().and_then(|n| n.to_str()).expect("name")
    );
    let err = failure(repo.run(&["log", "--catalog", &outside, "report", "bug", "Nope"]));
    assert!(err.contains("resolves outside"), "{err}");
}
