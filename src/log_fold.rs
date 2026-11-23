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
        let (events, errors) = crate::log::parse(&text);
        let mut problems: Vec<(usize, String, String)> = errors
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
        let mut warnings: Vec<String> = Vec::new();
        // Replay in time order; the file order breaks ties.
        let mut order: Vec<&Event> = tracker.log.iter().collect();
        order.sort_by(|a, b| a.at.cmp(&b.at));

        #[derive(Default)]
        struct IssueRun {
            stateful: bool,
            reported: Option<String>,
            fixes: Vec<(String, String)>,
            verifications: Vec<(String, Vec<String>)>,
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
                        let commit = event.commit.clone().unwrap_or_default();
                        run.fixes.push((commit, event.by.clone()));
                    }
                    EventKind::Verified => {
                        run.stateful = true;
                        run.verifications
                            .push((event.by.clone(), event.evidence.clone()));
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
        let mut ancestry: BTreeMap<(String, String), bool> = BTreeMap::new();
        let mut git_unavailable = false;
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
            let fixers: BTreeSet<&str> = run.fixes.iter().map(|(_, by)| by.as_str()).collect();
            let counted: Vec<&(String, Vec<String>)> = run
                .verifications
                .iter()
                .filter(|(by, _)| !fixers.contains(by.as_str()))
                .collect();
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
            for (_, evidence) in &counted {
                for item in evidence {
                    if !issue.evidence.contains(item) {
                        issue.evidence.push(item.clone());
                    }
                }
            }
            let mut derivation = Derivation {
                fixes: run.fixes.iter().map(|(c, _)| c.clone()).collect(),
                ..Derivation::default()
            };
            if derived == IssueState::Fixed && !terminal {
                let mut selves: Vec<String> =
                    run.verifications.iter().map(|(by, _)| by.clone()).collect();
                selves.sort();
                selves.dedup();
                derivation.self_verified_by = selves;
            }
            if derived != IssueState::Open && !terminal {
                for (release, commits) in &release_commits {
                    let mut shipped = false;
                    for fix in &derivation.fixes {
                        for commit in commits {
                            if git_unavailable || shipped {
                                continue;
                            }
                            let key = (fix.clone(), commit.clone());
                            let answer = match ancestry.get(&key) {
                                Some(answer) => Some(*answer),
                                None => match crate::scan_git::is_ancestor(&repo, fix, commit) {
                                    Ok(answer) => {
                                        ancestry.insert(key, answer);
                                        Some(answer)
                                    }
                                    Err(error) => {
                                        let text = error.to_string();
                                        if text.starts_with("git unavailable") {
                                            git_unavailable = true;
                                            warnings.push(format!("shipped not checked: {text}"));
                                        } else {
                                            warnings.push(format!(
                                                "issue `{id}`: could not check whether fix {} shipped in `{release}`: {text}",
                                                &fix[..fix.len().min(12)]
                                            ));
                                        }
                                        None
                                    }
                                },
                            };
                            shipped |= answer == Some(true);
                        }
                    }
                    if shipped {
                        derivation.shipped_in.push(release.clone());
                    }
                }
            }
            tracker.derived.insert(id, derivation);
        }
        warnings.sort();
        warnings.dedup();
        tracker.warnings = warnings;
    }
}
