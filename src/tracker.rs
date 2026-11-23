//! Optional `tracker/releases.yaml` and `tracker/issues.yaml` beside catalog.toml: releases,
//! bug reports and feature requests that point at catalog entries. See docs/tracker.md.
use crate::{schema, Catalog, Impact, ValidationReport};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

pub const RELEASES_FILE: &str = "tracker/releases.yaml";
pub const ISSUES_FILE: &str = "tracker/issues.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseState {
    Idea,
    Planned,
    Candidate,
    InReview,
    Released,
    Pulled,
}
impl ReleaseState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idea => "idea",
            Self::Planned => "planned",
            Self::Candidate => "candidate",
            Self::InReview => "in-review",
            Self::Released => "released",
            Self::Pulled => "pulled",
        }
    }
    /// Not yet released or pulled.
    pub fn is_upcoming(self) -> bool {
        !matches!(self, Self::Released | Self::Pulled)
    }
    /// Section order for upcoming releases: in-review, candidate, planned, idea.
    pub(crate) fn upcoming_rank(self) -> u8 {
        match self {
            Self::InReview => 0,
            Self::Candidate => 1,
            Self::Planned => 2,
            Self::Idea => 3,
            Self::Released | Self::Pulled => 4,
        }
    }
}

/// A target window. Never a gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub id: String,
    pub state: ReleaseState,
    #[serde(default)]
    pub title: Option<String>,
    /// The reason for the timing.
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub window: Option<Window>,
    /// Required when state is released; forbidden before.
    #[serde(default)]
    pub released: Option<String>,
    /// Platform -> build label.
    #[serde(default)]
    pub builds: BTreeMap<String, String>,
    /// Catalog entry IDs and issue IDs.
    #[serde(default)]
    pub carries: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueKind {
    Bug,
    Request,
}
impl IssueKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Request => "request",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssueState {
    #[default]
    Open,
    Planned,
    InProgress,
    Fixed,
    Verified,
    Closed,
    WontFix,
    Duplicate,
}
impl IssueState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Planned => "planned",
            Self::InProgress => "in-progress",
            Self::Fixed => "fixed",
            Self::Verified => "verified",
            Self::Closed => "closed",
            Self::WontFix => "wont-fix",
            Self::Duplicate => "duplicate",
        }
    }
    /// Open means anything but verified, closed, wont-fix or duplicate.
    pub fn is_open(self) -> bool {
        !matches!(
            self,
            Self::Verified | Self::Closed | Self::WontFix | Self::Duplicate
        )
    }
    /// Done for release readiness: verified or closed.
    pub fn is_done(self) -> bool {
        matches!(self, Self::Verified | Self::Closed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackerIssue {
    pub id: String,
    pub kind: IssueKind,
    pub title: String,
    #[serde(default)]
    pub summary: Option<String>,
    /// Defaults to `open`.
    #[serde(default)]
    pub state: IssueState,
    #[serde(default)]
    pub impact: Option<Impact>,
    #[serde(default)]
    pub reported: Option<String>,
    #[serde(default)]
    pub source: Vec<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub entries: Vec<String>,
    #[serde(default)]
    pub release: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Another issue ID; required when state is duplicate.
    #[serde(default)]
    pub duplicate_of: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Releases and issues in file order. Both are empty when no tracker files exist.
/// With `tracker/log.jsonl`, issues and releases defined only by the log follow those from
/// the files, and states are derived from the log (see `log_fold`).
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    pub releases: Vec<Release>,
    pub issues: Vec<TrackerIssue>,
    /// Valid log events in file order.
    pub log: Vec<crate::log::Event>,
    /// Issue ID -> what the log says beyond its state.
    pub derived: BTreeMap<String, crate::log_fold::Derivation>,
    /// Declared and derived states that disagree, and facts that could not be checked.
    pub warnings: Vec<String>,
    pub(crate) log_only_issues: BTreeSet<String>,
    pub(crate) log_only_releases: BTreeSet<String>,
    /// Issue ID -> position in `issues`, kept by `index_issues` and `push_issue`.
    pub(crate) issue_index: BTreeMap<String, usize>,
}
impl Tracker {
    pub fn release(&self, id: &str) -> Option<&Release> {
        self.releases.iter().find(|r| r.id == id)
    }
    /// By index when it covers every issue; otherwise (duplicate ids, or issues pushed
    /// from outside) by scanning.
    pub fn issue(&self, id: &str) -> Option<&TrackerIssue> {
        if self.issue_index.len() == self.issues.len() {
            return self.issue_index.get(id).map(|&i| &self.issues[i]);
        }
        self.issues.iter().find(|i| i.id == id)
    }
    /// Rebuild the issue index (the first of any duplicate ids wins).
    pub(crate) fn index_issues(&mut self) {
        self.issue_index.clear();
        for (position, issue) in self.issues.iter().enumerate() {
            self.issue_index.entry(issue.id.clone()).or_insert(position);
        }
    }
    pub(crate) fn push_issue(&mut self, issue: TrackerIssue) {
        self.issue_index
            .entry(issue.id.clone())
            .or_insert(self.issues.len());
        self.issues.push(issue);
    }
}

/// A real calendar date in `YYYY-MM-DD` form, as days since 1970-01-01.
pub fn calendar_day(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<u32> {
        text.get(range)
            .filter(|p| p.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|p| p.parse().ok())
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > days {
        return None;
    }
    Some(crate::continuous::days_from_civil(
        i64::from(year),
        month,
        day,
    ))
}

/// `bug.` or `request.` matching the kind, then lowercase letters, numbers and hyphens.
pub(crate) fn valid_issue_id(id: &str, kind: IssueKind) -> bool {
    id.strip_prefix(kind.as_str())
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

/// `<app>:<entry-id>`.
pub fn valid_shared_ref(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(app, id)| schema::valid_slug(app) && schema::valid_id(id))
}

/// Parse one tracker file strictly: a nonempty array whose items match `T` exactly.
/// Problems are reported per item; valid items are returned.
fn load_items<T: serde::de::DeserializeOwned>(
    path: &Path,
    file: &str,
    collections: &[&str],
    report: &mut ValidationReport,
) -> Vec<T> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            report.error("tracker", "", "", file, &error.to_string());
            return vec![];
        }
    };
    let value = serde_yaml::from_str::<serde_yaml::Value>(&text);
    let value = match value {
        Ok(value) => value,
        Err(error) => {
            report.error("tracker", "", "", file, &error.to_string());
            return vec![];
        }
    };
    let Some(items) = value.as_sequence() else {
        report.error("tracker", "", "", file, "expected array");
        return vec![];
    };
    if items.is_empty() {
        report.error("tracker", "", "", file, "too few items");
    }
    // Whole-file parsing stops at the first bad item but names the field and line, so its
    // message replaces the first per-item one; later items still report their own errors.
    let mut first_error = serde_yaml::from_str::<Vec<T>>(&text)
        .err()
        .map(|e| e.to_string());
    let mut out = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("");
        // serde would read an explicit null list or map as its empty default; strict types
        // reject it instead.
        let nulls: Vec<&str> = collections
            .iter()
            .copied()
            .filter(|field| item.get(*field).is_some_and(serde_yaml::Value::is_null))
            .collect();
        for field in &nulls {
            report.error(
                "tracker",
                id,
                "",
                &format!("{file}[{index}]"),
                &format!("{field} must be a list or map, not null; omit it or write []/{{}}"),
            );
        }
        if !nulls.is_empty() {
            continue;
        }
        match serde_yaml::from_value::<T>(item.clone()) {
            Ok(parsed) => out.push(parsed),
            Err(error) => report.error(
                "tracker",
                id,
                "",
                &format!("{file}[{index}]"),
                &first_error.take().unwrap_or_else(|| error.to_string()),
            ),
        }
    }
    out
}

impl Catalog {
    /// Load and validate the optional tracker files. Called from `load_collect`.
    pub(crate) fn load_tracker(&mut self, report: &mut ValidationReport) {
        let releases_path = self.directory.join(RELEASES_FILE);
        let issues_path = self.directory.join(ISSUES_FILE);
        if releases_path.exists() {
            self.tracker.releases = load_items(
                &releases_path,
                RELEASES_FILE,
                &["window", "builds", "carries", "notes"],
                report,
            );
        }
        if issues_path.exists() {
            self.tracker.issues = load_items(
                &issues_path,
                ISSUES_FILE,
                &["source", "platforms", "entries", "evidence", "notes"],
                report,
            );
        }
        self.tracker.index_issues();
        if let Some(config) = &self.config.tracker {
            for pattern in config.tag_pattern.patterns() {
                if let Err(error) = crate::scan_git::tag_regex(pattern) {
                    report.error("config", "", "", "catalog.toml", &error.to_string());
                }
            }
        }
        self.load_log(report);
        self.check_tracker(report);
    }

    fn check_tracker(&self, report: &mut ValidationReport) {
        let tracker = &self.tracker;
        let platforms = &self.config.platforms;
        let date =
            |report: &mut ValidationReport, id: &str, file: &str, field: &str, value: &str| {
                let ok = calendar_day(value).is_some();
                if !ok {
                    report.error(
                        "tracker",
                        id,
                        "",
                        file,
                        &format!("{field} {value:?} is not a calendar date (YYYY-MM-DD)"),
                    );
                }
                ok
            };
        let issue_ids: BTreeSet<&str> = tracker.issues.iter().map(|i| i.id.as_str()).collect();

        let mut seen = BTreeSet::new();
        // Log-only items were checked line by line when the log was loaded.
        for release in &tracker.releases {
            if tracker.log_only_releases.contains(&release.id) {
                continue;
            }
            let id = release.id.as_str();
            let file = RELEASES_FILE;
            if id.trim().is_empty() {
                report.error("tracker", id, "", file, "release id must not be empty");
            }
            if !seen.insert(id) {
                report.error("tracker", id, "", file, "duplicate release id");
            }
            if let Some(window) = &release.window {
                let from = date(report, id, file, "window.from", &window.from);
                let to = date(report, id, file, "window.to", &window.to);
                if from && to && calendar_day(&window.from) > calendar_day(&window.to) {
                    report.error("tracker", id, "", file, "window.from is after window.to");
                }
            }
            match (&release.released, release.state) {
                (Some(value), _) => {
                    date(report, id, file, "released", value);
                    if !matches!(release.state, ReleaseState::Released | ReleaseState::Pulled) {
                        report.error(
                            "tracker",
                            id,
                            "",
                            file,
                            &format!(
                                "released date is forbidden while state is {}",
                                release.state.as_str()
                            ),
                        );
                    }
                }
                (None, ReleaseState::Released) => report.error(
                    "tracker",
                    id,
                    "",
                    file,
                    "released date is required when state is released",
                ),
                (None, _) => {}
            }
            for platform in release.builds.keys() {
                if !platforms.contains_key(platform) {
                    report.error(
                        "tracker",
                        id,
                        platform,
                        file,
                        "builds names an unconfigured platform",
                    );
                }
            }
            for item in &release.carries {
                if !self.entries.contains_key(item) && !issue_ids.contains(item.as_str()) {
                    report.error(
                        "tracker",
                        id,
                        "",
                        file,
                        &format!("carries {item:?}, which is neither an entry nor an issue"),
                    );
                }
            }
        }

        let mut seen = BTreeSet::new();
        for issue in &tracker.issues {
            if tracker.log_only_issues.contains(&issue.id) {
                continue;
            }
            let id = issue.id.as_str();
            let file = ISSUES_FILE;
            if !valid_issue_id(id, issue.kind) {
                report.error(
                    "tracker",
                    id,
                    "",
                    file,
                    &format!(
                        "issue id must be {}. then lowercase letters, numbers and hyphens",
                        issue.kind.as_str()
                    ),
                );
            }
            if !seen.insert(id) {
                report.error("tracker", id, "", file, "duplicate issue id");
            }
            if self.entries.contains_key(id) {
                report.error(
                    "tracker",
                    id,
                    "",
                    file,
                    "issue id collides with an entry id",
                );
            }
            if issue.title.trim().is_empty() {
                report.error("tracker", id, "", file, "title must not be empty");
            }
            if let Some(reported) = &issue.reported {
                date(report, id, file, "reported", reported);
            }
            for entry in &issue.entries {
                if !self.entries.contains_key(entry) {
                    report.error(
                        "tracker",
                        id,
                        "",
                        file,
                        &format!("entries names unknown entry {entry:?}"),
                    );
                }
            }
            for platform in &issue.platforms {
                if !platforms.contains_key(platform) {
                    report.error(
                        "tracker",
                        id,
                        platform,
                        file,
                        "platforms names an unconfigured platform",
                    );
                }
            }
            if let Some(release) = &issue.release {
                if tracker.release(release).is_none() {
                    report.error(
                        "tracker",
                        id,
                        "",
                        file,
                        &format!("release {release:?} is not in {RELEASES_FILE}"),
                    );
                }
            }
            if issue.state == IssueState::Verified && issue.evidence.is_empty() {
                report.error("tracker", id, "", file, "verified requires evidence");
            }
            match (&issue.duplicate_of, issue.state) {
                (None, IssueState::Duplicate) => {
                    report.error("tracker", id, "", file, "duplicate requires duplicate_of")
                }
                (Some(other), _) if other == id || !issue_ids.contains(other.as_str()) => report
                    .error(
                        "tracker",
                        id,
                        "",
                        file,
                        &format!("duplicate_of {other:?} must name another issue"),
                    ),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar_dates() {
        for good in ["2026-11-20", "2024-02-29", "2000-02-29", "1999-12-31"] {
            assert!(calendar_day(good).is_some(), "{good}");
        }
        for bad in [
            "2026-02-29",
            "1900-02-29",
            "2026-04-31",
            "2026-13-01",
            "2026-00-10",
            "2026-1-01",
            "26-01-01",
            "2026/01/01",
            "2026-01-0a",
        ] {
            assert!(calendar_day(bad).is_none(), "{bad}");
        }
        assert_eq!(calendar_day("1970-01-02"), Some(1));
    }
    #[test]
    fn issue_ids() {
        assert!(valid_issue_id("bug.home-blank-2", IssueKind::Bug));
        assert!(!valid_issue_id("bug.home-blank", IssueKind::Request));
        assert!(!valid_issue_id("bug.", IssueKind::Bug));
        assert!(!valid_issue_id("bug.Home", IssueKind::Bug));
        assert!(!valid_issue_id("bugs.home", IssueKind::Bug));
    }
}
