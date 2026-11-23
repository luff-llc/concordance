//! `concordance track`: releases, open issues, unverified fixes, gaps and shared features
//! for one catalog, as Markdown or JSON built from one view.
use crate::tracker::{calendar_day, IssueKind, IssueState, Release, ReleaseState, TrackerIssue};
use crate::{Catalog, Entry, Impact, Result, Status};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Json,
}

/// How many released or pulled releases the Releases section keeps.
const PAST_RELEASES: usize = 2;
/// How many gap entries each platform lists.
const GAP_PREVIEW: usize = 5;

#[derive(Debug, Clone, Serialize)]
pub struct TrackView {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub today: Option<String>,
    pub summary: Summary,
    pub releases: Vec<ReleaseView>,
    /// Upcoming release windows relative to `today`; only with `--today`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upcoming: Option<Vec<Upcoming>>,
    pub bugs: Vec<IssueView>,
    pub requests: Vec<IssueView>,
    pub unverified: Vec<Unverified>,
    pub gaps: Vec<PlatformGaps>,
    pub shared: Vec<SharedView>,
    /// Declared states that disagree with tracker/log.jsonl, and facts that could not be
    /// checked. Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub last_release: Option<ReleaseRef>,
    pub next_release: Option<ReleaseRef>,
    pub open_bugs: ImpactCounts,
    pub open_requests: usize,
    pub unverified: usize,
    /// Platform -> gap counts.
    pub gaps: BTreeMap<String, GapCounts>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseRef {
    pub id: String,
    pub state: String,
    pub title: Option<String>,
    pub window: Option<WindowView>,
    pub released: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowView {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ImpactCounts {
    pub total: usize,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub unrated: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct GapCounts {
    pub missing: usize,
    pub partial: usize,
    pub stale: usize,
}
impl GapCounts {
    pub fn total(&self) -> usize {
        self.missing + self.partial + self.stale
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseView {
    pub id: String,
    pub state: String,
    pub title: Option<String>,
    pub why: Option<String>,
    pub window: Option<WindowView>,
    pub released: Option<String>,
    pub builds: BTreeMap<String, String>,
    /// Only with `--today`: the window ended before today without a release.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overdue: Option<bool>,
    pub readiness: Readiness,
    pub carries: Vec<CarriedItem>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Readiness {
    /// Carried items still open: entries not implemented on every built platform, and
    /// issues that are not verified or closed.
    pub open: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarriedItem {
    pub id: String,
    /// `entry`, `bug` or `request`.
    pub kind: String,
    pub title: String,
    /// Entry: platform -> status. Empty for issues.
    pub statuses: BTreeMap<String, String>,
    /// Issue state. `None` for entries.
    pub state: Option<String>,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Upcoming {
    pub release: String,
    pub from: String,
    pub to: String,
    /// Days from today to `from` (negative once the window has opened).
    pub starts_in_days: i64,
    /// Days from today to `to`.
    pub ends_in_days: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueView {
    pub id: String,
    pub title: String,
    pub state: String,
    pub impact: Option<String>,
    pub reported: Option<String>,
    pub platforms: Vec<String>,
    pub entries: Vec<String>,
    pub release: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unverified {
    pub id: String,
    pub title: String,
    pub release: Option<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlatformGaps {
    pub platform: String,
    pub label: String,
    pub missing: usize,
    pub partial: usize,
    pub stale: usize,
    /// `work-queue` (dependency order) or `impact` when the work queue could not be built.
    pub order: String,
    /// Why the impact-order fallback was used.
    pub order_note: Option<String>,
    /// The first entries in `order`.
    pub next: Vec<GapItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GapItem {
    pub id: String,
    pub title: String,
    pub status: String,
    pub impact: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedView {
    pub id: String,
    pub title: String,
    pub shared_with: Vec<String>,
    pub statuses: BTreeMap<String, String>,
}

fn window(release: &Release) -> Option<WindowView> {
    release.window.as_ref().map(|w| WindowView {
        from: w.from.clone(),
        to: w.to.clone(),
    })
}
fn release_ref(release: &Release) -> ReleaseRef {
    ReleaseRef {
        id: release.id.clone(),
        state: release.state.as_str().into(),
        title: release.title.clone(),
        window: window(release),
        released: release.released.clone(),
    }
}
fn statuses(entry: &Entry) -> BTreeMap<String, String> {
    entry
        .platforms
        .iter()
        .map(|(p, r)| (p.clone(), r.status.as_str().to_owned()))
        .collect()
}
/// Missing values sort after present ones.
fn last_if_none<T: Ord + Clone>(value: Option<&T>) -> (bool, Option<T>) {
    (value.is_none(), value.cloned())
}

impl Catalog {
    /// Upcoming releases (in-review, candidate, planned, idea; then window start and ID).
    pub(crate) fn upcoming_releases(&self) -> Vec<&Release> {
        let mut upcoming: Vec<&Release> = self
            .tracker
            .releases
            .iter()
            .filter(|r| r.state.is_upcoming())
            .collect();
        upcoming.sort_by_key(|r| {
            (
                r.state.upcoming_rank(),
                last_if_none(r.window.as_ref().map(|w| &w.from)),
                r.id.clone(),
            )
        });
        upcoming
    }
    /// Released or pulled releases, newest release date first; undated last.
    pub(crate) fn past_releases(&self) -> Vec<&Release> {
        let mut past: Vec<&Release> = self
            .tracker
            .releases
            .iter()
            .filter(|r| !r.state.is_upcoming())
            .collect();
        past.sort_by(|a, b| {
            b.released
                .is_some()
                .cmp(&a.released.is_some())
                .then_with(|| b.released.cmp(&a.released))
                .then_with(|| a.id.cmp(&b.id))
        });
        past
    }

    /// Whether a carried item counts as done for a release.
    fn carried_item(&self, release: &Release, id: &str) -> CarriedItem {
        if let Some(entry) = self.entries.get(id) {
            // Without builds, readiness covers the report's baseline and target platforms.
            let report = &self.config.report;
            let mut built: Vec<&String> = if release.builds.is_empty() {
                vec![&report.baseline, &report.target]
            } else {
                release.builds.keys().collect()
            };
            built.dedup();
            let ready = built.iter().all(|p| {
                entry.platforms.get(*p).is_some_and(|r| {
                    matches!(r.status, Status::Implemented | Status::NotApplicable)
                })
            });
            return CarriedItem {
                id: id.into(),
                kind: "entry".into(),
                title: entry.title.clone(),
                statuses: statuses(entry),
                state: None,
                ready,
            };
        }
        match self.tracker.issue(id) {
            Some(issue) => CarriedItem {
                id: id.into(),
                kind: issue.kind.as_str().into(),
                title: issue.title.clone(),
                statuses: BTreeMap::new(),
                state: Some(self.issue_state_label(issue)),
                ready: issue.state.is_done(),
            },
            None => CarriedItem {
                id: id.into(),
                kind: "unknown".into(),
                title: String::new(),
                statuses: BTreeMap::new(),
                state: None,
                ready: false,
            },
        }
    }

    fn release_view(&self, release: &Release, today: Option<i64>) -> ReleaseView {
        let carries: Vec<CarriedItem> = release
            .carries
            .iter()
            .map(|id| self.carried_item(release, id))
            .collect();
        let overdue = today.map(|today| {
            release.state.is_upcoming()
                && release
                    .window
                    .as_ref()
                    .and_then(|w| calendar_day(&w.to))
                    .is_some_and(|to| to < today)
        });
        ReleaseView {
            id: release.id.clone(),
            state: release.state.as_str().into(),
            title: release.title.clone(),
            why: release.why.clone(),
            window: window(release),
            released: release.released.clone(),
            builds: release.builds.clone(),
            overdue,
            readiness: Readiness {
                open: carries.iter().filter(|c| !c.ready).count(),
                total: carries.len(),
            },
            carries,
            notes: release.notes.clone(),
        }
    }

    /// An issue's state for display: `shipped` once verified and a released release
    /// contains its fix (from tracker/log.jsonl), else its state.
    pub(crate) fn issue_state_label(&self, issue: &TrackerIssue) -> String {
        let shipped = self
            .tracker
            .derived
            .get(&issue.id)
            .is_some_and(|d| !d.shipped_in.is_empty());
        if issue.state == IssueState::Verified && shipped {
            "shipped".into()
        } else {
            issue.state.as_str().into()
        }
    }

    /// Open issues of a kind, by impact, reported date, then ID.
    fn open_issues(&self, kind: IssueKind) -> Vec<&TrackerIssue> {
        let mut open: Vec<&TrackerIssue> = self
            .tracker
            .issues
            .iter()
            .filter(|i| i.kind == kind && i.state.is_open())
            .collect();
        open.sort_by_key(|i| {
            (
                last_if_none(i.impact.as_ref()),
                last_if_none(i.reported.as_ref()),
                i.id.clone(),
            )
        });
        open
    }

    fn unverified(&self) -> Vec<Unverified> {
        let mut out = Vec::new();
        for issue in &self.tracker.issues {
            if issue.state != IssueState::Fixed {
                continue;
            }
            let mut reasons = Vec::new();
            let derived = self.tracker.derived.get(&issue.id);
            let selves = derived
                .map(|d| d.self_verified_by.as_slice())
                .unwrap_or(&[]);
            if !selves.is_empty() {
                reasons.push(format!("self-verified by {}", selves.join(", ")));
            } else if issue.evidence.is_empty() {
                reasons.push("fixed without evidence".to_owned());
            }
            // The issue's own release and every released release that carries it; pulled
            // releases do not count.
            let mut shipped: std::collections::BTreeSet<&str> = self
                .tracker
                .releases
                .iter()
                .filter(|r| r.state == ReleaseState::Released)
                .filter(|r| {
                    issue.release.as_deref() == Some(r.id.as_str()) || r.carries.contains(&issue.id)
                })
                .map(|r| r.id.as_str())
                .collect();
            // Releases whose commit contains a fix (tracker/log.jsonl).
            shipped.extend(
                derived
                    .into_iter()
                    .flat_map(|d| d.shipped_in.iter().map(String::as_str)),
            );
            for release in shipped {
                reasons.push(format!("release {release} has shipped"));
            }
            if !reasons.is_empty() {
                out.push(Unverified {
                    id: issue.id.clone(),
                    title: issue.title.clone(),
                    release: issue.release.clone(),
                    reasons,
                });
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub(crate) fn gap_counts(&self, platform: &str) -> GapCounts {
        let mut counts = GapCounts::default();
        for entry in self.entries.values() {
            match entry.platforms.get(platform).map(|r| r.status) {
                Some(Status::Missing) => counts.missing += 1,
                Some(Status::Partial) => counts.partial += 1,
                Some(Status::Stale) => counts.stale += 1,
                _ => {}
            }
        }
        counts
    }

    /// Gaps on a platform in work-queue order (blocked work included). When the queue cannot
    /// be built, impact order with the queue error.
    fn gap_queue(&self, platform: &str) -> (Vec<&Entry>, Option<String>) {
        match self.next_filtered(platform, None, usize::MAX, false, true) {
            Ok(queue) => (queue, None),
            Err(error) => {
                let mut gaps: Vec<&Entry> = self
                    .entries
                    .values()
                    .filter(|e| e.platforms.get(platform).is_some_and(|r| r.status.is_gap()))
                    .collect();
                gaps.sort_by_key(|e| self.gap_key(e));
                (gaps, Some(error.to_string()))
            }
        }
    }

    fn open_bug_counts(&self) -> ImpactCounts {
        let mut counts = ImpactCounts::default();
        for issue in self.open_issues(IssueKind::Bug) {
            counts.total += 1;
            match issue.impact {
                Some(Impact::Critical) => counts.critical += 1,
                Some(Impact::High) => counts.high += 1,
                Some(Impact::Medium) => counts.medium += 1,
                Some(Impact::Low) => counts.low += 1,
                None => counts.unrated += 1,
            }
        }
        counts
    }

    pub(crate) fn tracker_summary(&self) -> Summary {
        let last = self
            .past_releases()
            .into_iter()
            .find(|r| r.state == ReleaseState::Released);
        let next = self
            .upcoming_releases()
            .into_iter()
            .find(|r| r.state != ReleaseState::Idea);
        Summary {
            last_release: last.map(release_ref),
            next_release: next.map(release_ref),
            open_bugs: self.open_bug_counts(),
            open_requests: self.open_issues(IssueKind::Request).len(),
            unverified: self.unverified().len(),
            gaps: self
                .config
                .platforms
                .keys()
                .map(|p| (p.clone(), self.gap_counts(p)))
                .collect(),
        }
    }

    /// Build the tracker view. `today` (YYYY-MM-DD) adds overdue flags and upcoming windows.
    pub fn track(&self, today: Option<&str>) -> Result<TrackView> {
        let today_day =
            match today {
                Some(text) => Some(calendar_day(text).ok_or_else(|| {
                    format!("--today {text:?} is not a calendar date (YYYY-MM-DD)")
                })?),
                None => None,
            };
        let upcoming_releases = self.upcoming_releases();
        let releases = upcoming_releases
            .iter()
            .copied()
            .chain(self.past_releases().into_iter().take(PAST_RELEASES))
            .map(|r| self.release_view(r, today_day))
            .collect();
        let upcoming = today_day.map(|today| {
            let mut windows: Vec<Upcoming> = upcoming_releases
                .iter()
                .filter_map(|r| {
                    let w = r.window.as_ref()?;
                    let (from, to) = (calendar_day(&w.from)?, calendar_day(&w.to)?);
                    (to >= today).then(|| Upcoming {
                        release: r.id.clone(),
                        from: w.from.clone(),
                        to: w.to.clone(),
                        starts_in_days: from - today,
                        ends_in_days: to - today,
                    })
                })
                .collect();
            windows.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.release.cmp(&b.release)));
            windows
        });
        let issue_view = |i: &TrackerIssue| IssueView {
            id: i.id.clone(),
            title: i.title.clone(),
            state: self.issue_state_label(i),
            impact: i.impact.map(|x| x.as_str().into()),
            reported: i.reported.clone(),
            platforms: i.platforms.clone(),
            entries: i.entries.clone(),
            release: i.release.clone(),
            summary: i.summary.clone(),
        };
        let gaps = self
            .config
            .platforms
            .keys()
            .map(|platform| {
                let counts = self.gap_counts(platform);
                let (queue, order_note) = self.gap_queue(platform);
                PlatformGaps {
                    order: if order_note.is_some() {
                        "impact"
                    } else {
                        "work-queue"
                    }
                    .into(),
                    order_note,
                    platform: platform.clone(),
                    label: self.config.report.label(platform).to_owned(),
                    missing: counts.missing,
                    partial: counts.partial,
                    stale: counts.stale,
                    next: queue
                        .into_iter()
                        .take(GAP_PREVIEW)
                        .map(|e| GapItem {
                            id: e.id.clone(),
                            title: e.title.clone(),
                            status: e
                                .platforms
                                .get(platform)
                                .map(|r| r.status.as_str())
                                .unwrap_or("no record")
                                .into(),
                            impact: e.impact.as_str().into(),
                        })
                        .collect(),
                }
            })
            .collect();
        let shared = self
            .entries
            .values()
            .filter(|e| !e.shared_with.is_empty())
            .map(|e| SharedView {
                id: e.id.clone(),
                title: e.title.clone(),
                shared_with: e.shared_with.clone(),
                statuses: statuses(e),
            })
            .collect();
        Ok(TrackView {
            title: format!("{} tracker", self.config.report.title),
            today: today.map(str::to_owned),
            summary: self.tracker_summary(),
            releases,
            upcoming,
            bugs: self
                .open_issues(IssueKind::Bug)
                .into_iter()
                .map(issue_view)
                .collect(),
            requests: self
                .open_issues(IssueKind::Request)
                .into_iter()
                .map(issue_view)
                .collect(),
            unverified: self.unverified(),
            gaps,
            shared,
            warnings: self.tracker.warnings.clone(),
        })
    }
}

pub(crate) fn window_text(window: Option<&WindowView>) -> String {
    match window {
        Some(w) => format!("{} to {}", w.from, w.to),
        None => "no window".into(),
    }
}
pub(crate) fn release_text(release: Option<&ReleaseRef>) -> String {
    match release {
        None => "none".into(),
        Some(r) => {
            let mut text = format!("`{}`", r.id);
            if let Some(title) = &r.title {
                text.push_str(&format!(" {title}"));
            }
            if let Some(released) = &r.released {
                text.push_str(&format!(" (released {released})"));
            } else {
                text.push_str(&format!(" ({})", r.state));
            }
            text
        }
    }
}
/// Table cell text: pipes escaped, line breaks flattened.
pub(crate) fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}
/// One Markdown table row; every cell is escaped.
pub(crate) fn row<S: AsRef<str>>(cells: &[S]) -> String {
    let cells: Vec<String> = cells.iter().map(|c| cell(c.as_ref())).collect();
    format!("| {} |", cells.join(" | "))
}
fn statuses_text(statuses: &BTreeMap<String, String>) -> String {
    statuses
        .iter()
        .map(|(p, s)| format!("{p} {s}"))
        .collect::<Vec<_>>()
        .join(", ")
}
/// `android 34 missing · 135 partial · 14 stale; ios none`, dropping zero parts; `none`
/// when no platform has gaps.
pub(crate) fn gaps_text(gaps: &BTreeMap<String, GapCounts>) -> String {
    if gaps.values().all(|c| c.total() == 0) {
        return "none".into();
    }
    gaps.iter()
        .map(|(p, c)| {
            let parts: Vec<String> = [
                (c.missing, "missing"),
                (c.partial, "partial"),
                (c.stale, "stale"),
            ]
            .into_iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect();
            if parts.is_empty() {
                format!("{p} none")
            } else {
                format!("{p} {}", parts.join(" · "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

impl TrackView {
    pub fn render(&self, format: Format) -> Result<String> {
        Ok(match format {
            Format::Markdown => self.markdown(),
            Format::Json => serde_json::to_string_pretty(self)? + "\n",
        })
    }

    pub fn markdown(&self) -> String {
        let mut out = Vec::new();
        out.push(format!("# {}", self.title));
        out.push(String::new());
        out.push(
            "Generated by `concordance track` from catalog.toml, the catalog and tracker/. Do not edit."
                .into(),
        );
        if let Some(today) = &self.today {
            out.push(format!("As of {today}."));
        }

        let s = &self.summary;
        out.extend(["".into(), "## Summary".into(), String::new()]);
        out.push(format!(
            "- Last release: {}",
            release_text(s.last_release.as_ref())
        ));
        out.push(format!(
            "- Next release: {}{}",
            release_text(s.next_release.as_ref()),
            s.next_release
                .as_ref()
                .map(|r| format!(", window {}", window_text(r.window.as_ref())))
                .unwrap_or_default()
        ));
        let b = &s.open_bugs;
        out.push(format!(
            "- Open bugs: {} (critical {}, high {}, medium {}, low {}, unrated {})",
            b.total, b.critical, b.high, b.medium, b.low, b.unrated
        ));
        out.push(format!("- Open requests: {}", s.open_requests));
        out.push(format!("- Unverified: {}", s.unverified));
        out.push(format!("- Gaps: {}", gaps_text(&s.gaps)));

        if !self.warnings.is_empty() {
            out.extend(["".into(), "## Warnings".into(), String::new()]);
            for warning in &self.warnings {
                out.push(format!("- {warning}"));
            }
        }

        out.extend(["".into(), "## Releases".into()]);
        if self.releases.is_empty() {
            out.extend([String::new(), "None recorded.".into()]);
        }
        for r in &self.releases {
            out.push(String::new());
            let title = r
                .title
                .as_deref()
                .map(|t| format!(" — {t}"))
                .unwrap_or_default();
            let overdue = if r.overdue == Some(true) {
                " **overdue**"
            } else {
                ""
            };
            out.push(format!("### `{}`{title} ({}){overdue}", r.id, r.state));
            out.push(String::new());
            if let Some(window) = &r.window {
                out.push(format!("- Window: {} to {}", window.from, window.to));
            }
            if let Some(released) = &r.released {
                out.push(format!("- Released: {released}"));
            }
            if let Some(why) = &r.why {
                out.push(format!("- Why: {why}"));
            }
            if !r.builds.is_empty() {
                out.push(format!(
                    "- Builds: {}",
                    r.builds
                        .iter()
                        .map(|(p, b)| format!("{p} {b}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            out.push(format!(
                "- Readiness: {} of {} carried items open",
                r.readiness.open, r.readiness.total
            ));
            for note in &r.notes {
                out.push(format!("- Note: {note}"));
            }
            if !r.carries.is_empty() {
                out.push(String::new());
                out.push("| Item | Kind | Status | Ready |".into());
                out.push("| --- | --- | --- | --- |".into());
                for c in &r.carries {
                    let status = match &c.state {
                        Some(state) => state.clone(),
                        None => statuses_text(&c.statuses),
                    };
                    out.push(row(&[
                        format!("`{}` {}", c.id, c.title),
                        c.kind.clone(),
                        status,
                        if c.ready { "yes" } else { "no" }.into(),
                    ]));
                }
            }
        }

        if let Some(upcoming) = &self.upcoming {
            out.extend(["".into(), "## Upcoming".into(), String::new()]);
            if upcoming.is_empty() {
                out.push("No upcoming release windows.".into());
            }
            for u in upcoming {
                let when = if u.starts_in_days > 0 {
                    format!("opens in {} days", u.starts_in_days)
                } else {
                    format!("open, closes in {} days", u.ends_in_days)
                };
                out.push(format!("- `{}`: {} to {}, {when}", u.release, u.from, u.to));
            }
        }

        for (heading, issues) in [("Bugs", &self.bugs), ("Requests", &self.requests)] {
            out.extend(["".into(), format!("## {heading}"), String::new()]);
            if issues.is_empty() {
                out.push("None open.".into());
                continue;
            }
            out.push(
                "| Issue | Impact | State | Reported | Platforms | Entries | Release |".into(),
            );
            out.push("| --- | --- | --- | --- | --- | --- | --- |".into());
            for i in issues {
                out.push(row(&[
                    format!("`{}` {}", i.id, i.title),
                    i.impact.as_deref().unwrap_or("—").into(),
                    i.state.clone(),
                    i.reported.as_deref().unwrap_or("—").into(),
                    if i.platforms.is_empty() {
                        "—".into()
                    } else {
                        i.platforms.join(", ")
                    },
                    if i.entries.is_empty() {
                        "—".into()
                    } else {
                        i.entries
                            .iter()
                            .map(|e| format!("`{e}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                    i.release
                        .as_deref()
                        .map(|r| format!("`{r}`"))
                        .unwrap_or_else(|| "—".into()),
                ]));
            }
        }

        out.extend(["".into(), "## Unverified".into(), String::new()]);
        if self.unverified.is_empty() {
            out.push("None.".into());
        }
        for u in &self.unverified {
            out.push(format!(
                "- `{}` {}: {}",
                u.id,
                u.title,
                u.reasons.join("; ")
            ));
        }

        out.extend(["".into(), "## Gaps".into()]);
        for g in &self.gaps {
            out.push(String::new());
            out.push(format!(
                "### {}: {} missing, {} partial, {} stale",
                g.label, g.missing, g.partial, g.stale
            ));
            if let Some(note) = &g.order_note {
                out.push(String::new());
                out.push(format!(
                    "In impact order: the work queue could not be built ({note})."
                ));
            }
            if !g.next.is_empty() {
                out.push(String::new());
            }
            for (n, item) in g.next.iter().enumerate() {
                out.push(format!(
                    "{}. `{}` {} ({}, {})",
                    n + 1,
                    item.id,
                    item.title,
                    item.status,
                    item.impact
                ));
            }
        }

        out.extend(["".into(), "## Shared".into(), String::new()]);
        if self.shared.is_empty() {
            out.push("None.".into());
        } else {
            out.push("| Entry | Shared with | Status |".into());
            out.push("| --- | --- | --- |".into());
            for s in &self.shared {
                out.push(row(&[
                    format!("`{}` {}", s.id, s.title),
                    s.shared_with
                        .iter()
                        .map(|x| format!("`{x}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    statuses_text(&s.statuses),
                ]));
            }
        }
        out.join("\n") + "\n"
    }
}
