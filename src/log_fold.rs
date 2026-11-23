//! Replay `tracker/log.jsonl` over the plan: issues and releases defined only by the log
//! join those in issues.yaml and releases.yaml, and states are derived from facts. When a
//! declared state disagrees with the derived one, the derived state wins and a warning is
//! kept for `track` and `suite`.
use crate::log::{issue_kind, Event, EventKind, LOG_FILE};
use crate::tracker::{IssueKind, IssueState, Release, ReleaseState, Tracker, TrackerIssue};
use crate::{Catalog, ValidationReport};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

/// What the log says about one issue beyond its state.
#[derive(Debug, Clone, Default)]
pub struct Derivation {
    /// Fix commits recorded since the last reopen.
    pub fixes: Vec<String>,
    /// Released releases whose commit contains a fix commit.
    pub shipped_in: Vec<String>,
    /// People who verified their own fix (those checks do not count).
    pub self_verified_by: Vec<String>,
    /// Others whose checks predate the latest fix and are not of its commit or a later one.
    pub stale_verified_by: Vec<String>,
    /// Released releases git says do not contain the latest fix.
    pub not_in: Vec<String>,
    /// Released releases whose containment of the latest fix git could not answer.
    pub containment_unknown: Vec<String>,
}

/// Two spellings of one commit: equal, or one an abbreviation (7+ characters) of the other.
fn same_commit(a: &str, b: &str) -> bool {
    a == b || (a.len().min(b.len()) >= 7 && (a.starts_with(b) || b.starts_with(a)))
}

/// Ancestry questions for the fold, turning git failures into warnings.
struct GitAsk {
    ancestry: crate::scan_git::Ancestry,
    unavailable: bool,
    warnings: Vec<String>,
}
impl GitAsk {
    /// `Some(answer)`, or `None` when git cannot say (a warning names why, once).
    fn contains(&mut self, ancestor: &str, descendant: &str, context: &str) -> Option<bool> {
        if self.unavailable {
            return None;
        }
        if same_commit(ancestor, descendant) {
            return Some(true);
        }
        match self.ancestry.contains(ancestor, descendant) {
            Ok(Some(answer)) => Some(answer),
            Ok(None) => {
                self.warnings.push(format!(
                    "{context}: shallow history cannot tell whether {} contains {}",
                    short(descendant),
                    short(ancestor)
                ));
                None
            }
            Err(error) => {
                let text = error.to_string();
                if text.starts_with("git unavailable") {
                    self.unavailable = true;
                    self.warnings
                        .push(format!("shipped and commit checks skipped: {text}"));
                } else {
                    self.warnings.push(format!("{context}: {text}"));
                }
                None
            }
        }
    }
}
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

fn issue_from_report(event: &Event) -> TrackerIssue {
    TrackerIssue {
        id: event.subject.clone(),
        kind: issue_kind(&event.subject).unwrap_or(IssueKind::Bug),
        title: event.title.clone().unwrap_or_default(),
        summary: None,
        state: IssueState::Open,
        impact: event.impact,
        reported: Some(event.at[..10].to_owned()),
        source: event.source.clone(),
        platforms: event.platforms.clone(),
        entries: event.entries.clone(),
        release: None,
        evidence: vec![],
        duplicate_of: None,
        notes: vec![],
    }
}

/// Progress level used to compare declared and derived issue states.
fn level(state: IssueState) -> u8 {
    match state {
        IssueState::Fixed => 1,
        IssueState::Verified => 2,
        _ => 0,
    }
}

impl Tracker {
    /// Remember an event appended during this run, so later events can refer to it.
    pub(crate) fn record(&mut self, event: &Event) {
        if event.kind == EventKind::Reported && self.issue(&event.subject).is_none() {
            self.issues.push(issue_from_report(event));
            self.log_only_issues.insert(event.subject.clone());
        }
        self.log.push(event.clone());
    }
}

impl Catalog {
    /// Problems with what an event refers to: unknown issues, entries or platforms.
    pub fn event_reference_errors(&self, event: &Event) -> Vec<String> {
        let mut errors = Vec::new();
        let platforms = &self.config.platforms;
        match event.kind {
            EventKind::Reported => {
                if self.entries.contains_key(&event.subject) {
                    errors.push(format!("{} is an entry id", event.subject));
                }
                for entry in &event.entries {
                    if !self.entries.contains_key(entry) {
                        errors.push(format!("entries names unknown entry {entry:?}"));
                    }
                }
                for platform in &event.platforms {
                    if !platforms.contains_key(platform) {
                        errors.push(format!(
                            "platforms names unconfigured platform {platform:?}"
                        ));
                    }
                }
            }
            EventKind::Fixed | EventKind::Verified | EventKind::Reopened => {
                if self.tracker.issue(&event.subject).is_none() {
                    errors.push(format!(
                        "{} is not an issue in issues.yaml or reported in the log",
                        event.subject
                    ));
                }
            }
            EventKind::Uploaded | EventKind::Released | EventKind::Pulled => {
                if let Some(platform) = &event.platform {
                    if !platforms.contains_key(platform) {
                        errors.push(format!("platform {platform:?} is not configured"));
                    }
                }
            }
        }
        errors
    }

    /// Load the optional log, add log-only issues, then derive states. Called from
    /// `load_tracker` before the tracker files are cross-checked.
    pub(crate) fn load_log(&mut self, report: &mut ValidationReport) {
        let path = self.directory.join(LOG_FILE);
        if !path.exists() {
            return;
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                report.error("log", "", "", LOG_FILE, &error.to_string());
                return;
            }
        };
        let parsed = crate::log::parse(&text);
        let events = parsed.events;
        self.tracker.warnings.extend(
            parsed
                .warnings
                .into_iter()
                .map(|(line, message)| format!("{LOG_FILE}:{line}: {message}")),
        );
        let mut problems: Vec<(usize, String, String)> = parsed
            .errors
            .into_iter()
            .map(|(line, message)| (line, String::new(), message))
            .collect();
        // Reports first, so later facts may refer to issues the log defines.
        let mut valid = Vec::new();
        for pass in [true, false] {
            for (line, event) in &events {
                if (event.kind == EventKind::Reported) != pass {
                    continue;
                }
                let errors = self.event_reference_errors(event);
                if errors.is_empty() {
                    if pass && self.tracker.issue(&event.subject).is_none() {
                        self.tracker.issues.push(issue_from_report(event));
                        self.tracker.log_only_issues.insert(event.subject.clone());
                    }
                    valid.push((*line, event.clone()));
                } else {
                    problems.extend(
                        errors
                            .into_iter()
                            .map(|m| (*line, event.subject.clone(), m)),
                    );
                }
            }
        }
        problems.sort();
        for (line, subject, message) in problems {
            report.error("log", &subject, "", &format!("{LOG_FILE}:{line}"), &message);
        }
        valid.sort_by_key(|(line, _)| *line);
        self.tracker.log = valid.into_iter().map(|(_, e)| e).collect();
        self.derive_from_log();
    }

    /// Issues in issues.yaml that leave `state` out (it defaults to open, but nothing was
    /// declared, so a derived state replaces it without a warning).
    fn issues_without_state(&self) -> BTreeSet<String> {
        let text = fs::read_to_string(self.directory.join(crate::tracker::ISSUES_FILE))
            .unwrap_or_default();
        let value: serde_yaml::Value = serde_yaml::from_str(&text).unwrap_or_default();
        value
            .as_sequence()
            .into_iter()
            .flatten()
            .filter(|item| item.get("state").is_none())
            .filter_map(|item| item.get("id").and_then(serde_yaml::Value::as_str))
            .map(str::to_owned)
            .collect()
    }

    fn derive_from_log(&mut self) {
        let repo = crate::log::repo_dir(self);
        let undeclared = self.issues_without_state();
        let tracker = &mut self.tracker;
        let mut warnings: Vec<String> = std::mem::take(&mut tracker.warnings);
        // Replay in time order; the file order breaks ties.
        let mut order: Vec<&Event> = tracker.log.iter().collect();
        order.sort_by(|a, b| a.at.cmp(&b.at));

        struct Fix {
            commit: String,
            by: String,
            at: String,
        }
        struct Check {
            by: String,
            evidence: Vec<String>,
            at: String,
            commit: Option<String>,
        }
        #[derive(Default)]
        struct IssueRun {
            stateful: bool,
            reported: Option<String>,
            fixes: Vec<Fix>,
            verifications: Vec<Check>,
        }
        #[derive(Default)]
        struct ReleaseRun {
            builds: BTreeMap<String, String>,
            dates: Vec<String>,
            commits: Vec<String>,
            last: Option<EventKind>,
        }
        let mut issues: BTreeMap<String, IssueRun> = BTreeMap::new();
        let mut releases: BTreeMap<String, ReleaseRun> = BTreeMap::new();
        for event in order {
            if event.kind.is_issue() {
                let run = issues.entry(event.subject.clone()).or_default();
                match event.kind {
                    EventKind::Reported => {
                        run.reported
                            .get_or_insert_with(|| event.at[..10].to_owned());
                    }
                    EventKind::Fixed => {
                        run.stateful = true;
                        run.fixes.push(Fix {
                            commit: event.commit.clone().unwrap_or_default(),
                            by: event.by.clone(),
                            at: event.at.clone(),
                        });
                    }
                    EventKind::Verified => {
                        run.stateful = true;
                        run.verifications.push(Check {
                            by: event.by.clone(),
                            evidence: event.evidence.clone(),
                            at: event.at.clone(),
                            commit: event.commit.clone(),
                        });
                    }
                    _ => {
                        // Reopened: back to open; earlier fixes and checks no longer count.
                        run.stateful = true;
                        run.fixes.clear();
                        run.verifications.clear();
                    }
                }
            } else {
                let run = releases.entry(event.subject.clone()).or_default();
                match event.kind {
                    EventKind::Uploaded => {
                        if let (Some(p), Some(b)) = (&event.platform, &event.build) {
                            run.builds.insert(p.clone(), b.clone());
                        }
                    }
                    EventKind::Released => {
                        run.last = Some(EventKind::Released);
                        run.dates.extend(event.date.clone());
                        run.commits.extend(event.commit.clone());
                    }
                    _ => run.last = Some(EventKind::Pulled),
                }
            }
        }

        // Releases.
        let mut release_commits: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, run) in releases {
            let date = run.dates.iter().min().cloned();
            let derived = match run.last {
                Some(EventKind::Released) => ReleaseState::Released,
                Some(_) => ReleaseState::Pulled,
                None => ReleaseState::Candidate,
            };
            if derived == ReleaseState::Released && !run.commits.is_empty() {
                release_commits.insert(id.clone(), run.commits.clone());
            }
            let Some(release) = tracker.releases.iter_mut().find(|r| r.id == id) else {
                tracker.releases.push(Release {
                    id: id.clone(),
                    state: derived,
                    title: None,
                    why: None,
                    window: None,
                    released: date,
                    builds: run.builds,
                    carries: vec![],
                    notes: vec![],
                });
                tracker.log_only_releases.insert(id);
                continue;
            };
            for (platform, build) in run.builds {
                if let Some(old) = release.builds.get(&platform).filter(|old| **old != build) {
                    warnings.push(format!(
                        "release `{id}`: releases.yaml lists {platform} build {old}; the log uploaded {build}"
                    ));
                }
                release.builds.insert(platform, build);
            }
            let declared = release.state;
            match run.last {
                Some(_) => {
                    if declared != derived {
                        warnings.push(format!(
                            "release `{id}`: releases.yaml says {}; the log says {}",
                            declared.as_str(),
                            derived.as_str()
                        ));
                    }
                    release.state = derived;
                    if let Some(date) = date {
                        if let Some(old) = release.released.as_ref().filter(|old| **old != date) {
                            warnings.push(format!(
                                "release `{id}`: releases.yaml says released {old}; the log says {date}"
                            ));
                        }
                        release.released = Some(date);
                    }
                }
                None => {
                    // Uploads alone: a declared released/pulled state stands.
                    if matches!(declared, ReleaseState::Idea | ReleaseState::Planned) {
                        warnings.push(format!(
                            "release `{id}`: releases.yaml says {}; the log has uploaded builds",
                            declared.as_str()
                        ));
                        release.state = ReleaseState::Candidate;
                    }
                }
            }
        }

        // Issues.
        let mut git = GitAsk {
            ancestry: crate::scan_git::Ancestry::new(&repo),
            unavailable: false,
            warnings: vec![],
        };
        for (id, run) in issues {
            let Some(issue) = tracker.issues.iter_mut().find(|i| i.id == id) else {
                continue;
            };
            if issue.reported.is_none() {
                issue.reported = run.reported.clone();
            }
            // A report alone does not override a declared state.
            if !run.stateful {
                continue;
            }
            let fixers: BTreeSet<&str> = run.fixes.iter().map(|f| f.by.as_str()).collect();
            // A check counts only for a fix it could have checked: recorded no earlier than
            // the latest fix, or of that fix's commit or a later one.
            let latest = run.fixes.last();
            let mut counted: Vec<&Check> = Vec::new();
            let mut selves: BTreeSet<String> = BTreeSet::new();
            let mut stale: BTreeSet<String> = BTreeSet::new();
            if let Some(fix) = latest {
                for check in &run.verifications {
                    let covers = check.at >= fix.at
                        || check.commit.as_deref().is_some_and(|commit| {
                            git.contains(
                                &fix.commit,
                                commit,
                                &format!("issue `{id}`: verification by {}", check.by),
                            ) == Some(true)
                        });
                    let own = fixers.contains(check.by.as_str());
                    match (covers, own) {
                        (true, false) => counted.push(check),
                        (true, true) => {
                            selves.insert(check.by.clone());
                        }
                        (false, false) => {
                            stale.insert(check.by.clone());
                        }
                        (false, true) => {}
                    }
                }
            }
            let derived = if run.fixes.is_empty() {
                if !run.verifications.is_empty() {
                    warnings.push(format!(
                        "issue `{id}`: verified in the log without a recorded fix"
                    ));
                }
                IssueState::Open
            } else if counted.is_empty() {
                IssueState::Fixed
            } else {
                IssueState::Verified
            };
            let declared = issue.state;
            let terminal = matches!(
                declared,
                IssueState::Closed | IssueState::WontFix | IssueState::Duplicate
            );
            if tracker.log_only_issues.contains(&id) || undeclared.contains(&id) {
                issue.state = derived;
            } else if terminal {
                // The log has no words for closed, wont-fix or duplicate; they stand.
            } else if level(declared) != level(derived) {
                warnings.push(format!(
                    "issue `{id}`: issues.yaml says {}; the log says {}",
                    declared.as_str(),
                    derived.as_str()
                ));
                issue.state = derived;
            } else if level(declared) > 0 {
                issue.state = derived;
            }
            for check in &counted {
                for item in &check.evidence {
                    if !issue.evidence.contains(item) {
                        issue.evidence.push(item.clone());
                    }
                }
            }
            let mut derivation = Derivation {
                fixes: run.fixes.iter().map(|f| f.commit.clone()).collect(),
                ..Derivation::default()
            };
            if derived == IssueState::Fixed && !terminal {
                derivation.self_verified_by = selves.into_iter().collect();
                derivation.stale_verified_by = stale.into_iter().collect();
            }
            if let (Some(fix), false) = (latest, derived == IssueState::Open || terminal) {
                for (release, commits) in &release_commits {
                    let answers: Vec<Option<bool>> = commits
                        .iter()
                        .map(|commit| {
                            git.contains(
                                &fix.commit,
                                commit,
                                &format!("issue `{id}`: fix {} in `{release}`", short(&fix.commit)),
                            )
                        })
                        .collect();
                    if answers.contains(&Some(true)) {
                        derivation.shipped_in.push(release.clone());
                    } else if answers.iter().all(|a| *a == Some(false)) {
                        derivation.not_in.push(release.clone());
                    } else {
                        derivation.containment_unknown.push(release.clone());
                    }
                }
            }
            tracker.derived.insert(id, derivation);
        }
        warnings.append(&mut git.warnings);
        warnings.sort();
        warnings.dedup();
        tracker.warnings = warnings;
    }
}
