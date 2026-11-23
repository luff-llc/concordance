//! `concordance suite`: several app catalogs joined into one view of releases, beats and
//! shared features.
use crate::track::{gaps_text, release_text, row, window_text, Format, Summary};
use crate::tracker::calendar_day;
use crate::{Catalog, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Releases whose window overlaps this many days before a beat are listed with it.
const BEAT_LEAD_DAYS: i64 = 60;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuiteFile {
    title: String,
    #[serde(default)]
    apps: BTreeMap<String, SuiteApp>,
    #[serde(default)]
    beats: Vec<BeatFile>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuiteApp {
    /// Relative to the suite file.
    catalog: String,
    #[serde(default)]
    label: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BeatFile {
    date: toml::value::Datetime,
    title: String,
    #[serde(default)]
    why: Option<String>,
}

/// A loaded suite: its title, beats and every app's catalog.
#[derive(Debug)]
pub struct Suite {
    pub title: String,
    pub directory: PathBuf,
    /// App key -> (label, catalog).
    pub apps: BTreeMap<String, (String, Catalog)>,
    pub beats: Vec<Beat>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Beat {
    pub date: String,
    pub title: String,
    pub why: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SuiteView {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub today: Option<String>,
    pub apps: Vec<AppRow>,
    pub beats: Vec<BeatView>,
    pub shared: Vec<SharedFeature>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppRow {
    pub app: String,
    pub label: String,
    pub summary: Summary,
    /// The app's tracker warnings (declared and derived states that disagree). Omitted
    /// when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BeatView {
    pub date: String,
    pub title: String,
    pub why: Option<String>,
    pub releases: Vec<BeatRelease>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BeatRelease {
    pub app: String,
    pub release: String,
    pub state: String,
    pub from: String,
    pub to: String,
}

/// One shared feature: every member reference, resolved or not.
#[derive(Debug, Clone, Serialize)]
pub struct SharedFeature {
    /// The smallest resolved member reference (else the smallest reference), `<app>:<entry-id>`.
    pub feature: String,
    pub members: Vec<SharedMember>,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedMember {
    pub app: String,
    pub entry: String,
    pub title: String,
    /// Status on that catalog's report baseline platform.
    pub baseline: String,
    pub baseline_status: String,
}

impl Suite {
    /// Load the suite file and every catalog it names (structural validation only).
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file: SuiteFile =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // Output defaults beside the path as given; catalogs resolve from the suite file's
        // real location, so a symlinked suite.toml reads its target's neighbours.
        let directory = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let real = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = real.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut apps = BTreeMap::new();
        for (key, app) in file.apps {
            if !crate::schema::valid_slug(&key) {
                return Err(format!(
                    "{}: app key {key:?} must be a lowercase slug",
                    path.display()
                )
                .into());
            }
            // `..` and absolute paths are allowed: apps live in sibling repositories.
            let dir = base.join(&app.catalog);
            let catalog =
                Catalog::load(&dir).map_err(|e| format!("app {key} ({}): {e}", dir.display()))?;
            apps.insert(key.clone(), (app.label.unwrap_or(key), catalog));
        }
        let mut beats = Vec::new();
        for beat in file.beats {
            let date = match (&beat.date.date, &beat.date.time, &beat.date.offset) {
                (Some(_), None, None) => beat.date.to_string(),
                _ => {
                    return Err(format!(
                        "{}: beat {:?} date must be a plain YYYY-MM-DD date",
                        path.display(),
                        beat.title
                    )
                    .into())
                }
            };
            if calendar_day(&date).is_none() {
                return Err(format!(
                    "{}: beat {:?} date {date} is not a calendar date",
                    path.display(),
                    beat.title
                )
                .into());
            }
            beats.push(Beat {
                date,
                title: beat.title,
                why: beat.why,
            });
        }
        beats.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.title.cmp(&b.title)));
        Ok(Self {
            title: file.title,
            directory,
            apps,
            beats,
        })
    }

    pub fn view(&self, today: Option<&str>) -> Result<SuiteView> {
        let today_day =
            match today {
                Some(text) => Some(calendar_day(text).ok_or_else(|| {
                    format!("--today {text:?} is not a calendar date (YYYY-MM-DD)")
                })?),
                None => None,
            };
        let apps = self
            .apps
            .iter()
            .map(|(key, (label, catalog))| AppRow {
                app: key.clone(),
                label: label.clone(),
                summary: catalog.tracker_summary(),
                warnings: catalog.tracker.warnings.clone(),
            })
            .collect();
        let beats = self
            .beats
            .iter()
            .filter_map(|beat| {
                let day = calendar_day(&beat.date)?;
                if today_day.is_some_and(|today| day < today) {
                    return None;
                }
                let mut releases = Vec::new();
                for (key, (_, catalog)) in &self.apps {
                    for release in &catalog.tracker.releases {
                        let Some(w) = &release.window else { continue };
                        let (Some(from), Some(to)) = (calendar_day(&w.from), calendar_day(&w.to))
                        else {
                            continue;
                        };
                        // At least one window day falls in the lead-up, strictly before the beat.
                        if from < day && to >= day - BEAT_LEAD_DAYS {
                            releases.push(BeatRelease {
                                app: key.clone(),
                                release: release.id.clone(),
                                state: release.state.as_str().into(),
                                from: w.from.clone(),
                                to: w.to.clone(),
                            });
                        }
                    }
                }
                releases.sort_by(|a, b| {
                    (&a.from, &a.app, &a.release).cmp(&(&b.from, &b.app, &b.release))
                });
                Some(BeatView {
                    date: beat.date.clone(),
                    title: beat.title.clone(),
                    why: beat.why.clone(),
                    releases,
                })
            })
            .collect();
        Ok(SuiteView {
            title: self.title.clone(),
            today: today.map(str::to_owned),
            apps,
            beats,
            shared: self.shared(),
        })
    }

    /// Group entries connected by `shared_with` into features.
    fn shared(&self) -> Vec<SharedFeature> {
        // Union-find over `<app>:<entry-id>` references.
        let mut parent: BTreeMap<String, String> = BTreeMap::new();
        fn find(parent: &mut BTreeMap<String, String>, key: &str) -> String {
            let mut current = key.to_owned();
            while let Some(next) = parent.get(&current).filter(|n| **n != current).cloned() {
                current = next;
            }
            current
        }
        fn union(parent: &mut BTreeMap<String, String>, a: &str, b: &str) {
            parent.entry(a.to_owned()).or_insert_with(|| a.to_owned());
            parent.entry(b.to_owned()).or_insert_with(|| b.to_owned());
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                // The smaller root wins so the feature name is deterministic.
                let (keep, merge) = if ra < rb { (ra, rb) } else { (rb, ra) };
                parent.insert(merge, keep);
            }
        }
        for (key, (_, catalog)) in &self.apps {
            for entry in catalog.entries.values() {
                let own = format!("{key}:{}", entry.id);
                for other in &entry.shared_with {
                    union(&mut parent, &own, other);
                }
            }
        }
        let members: Vec<String> = parent.keys().cloned().collect();
        let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for member in members {
            let root = find(&mut parent, &member);
            groups.entry(root).or_default().insert(member);
        }
        groups
            .into_values()
            .map(|refs| {
                let mut resolved = Vec::new();
                let mut unresolved = Vec::new();
                for reference in &refs {
                    let found = reference.split_once(':').and_then(|(app, id)| {
                        let (_, catalog) = self.apps.get(app)?;
                        Some((app, catalog, catalog.entries.get(id)?))
                    });
                    match found {
                        Some((app, catalog, entry)) => {
                            let baseline = &catalog.config.report.baseline;
                            resolved.push(SharedMember {
                                app: app.to_owned(),
                                entry: entry.id.clone(),
                                title: entry.title.clone(),
                                baseline: baseline.clone(),
                                baseline_status: entry
                                    .platforms
                                    .get(baseline)
                                    .map(|r| r.status.as_str())
                                    .unwrap_or("no record")
                                    .into(),
                            });
                        }
                        None => unresolved.push(reference.clone()),
                    }
                }
                SharedFeature {
                    feature: resolved
                        .first()
                        .map(|m| format!("{}:{}", m.app, m.entry))
                        .or_else(|| refs.iter().next().cloned())
                        .unwrap_or_default(),
                    members: resolved,
                    unresolved,
                }
            })
            .collect()
    }
}

impl SuiteView {
    pub fn render(&self, format: Format) -> Result<String> {
        Ok(match format {
            Format::Markdown => self.markdown(),
            Format::Json => serde_json::to_string_pretty(self)? + "\n",
        })
    }

    pub fn markdown(&self) -> String {
        let mut out = vec![
            format!("# {}", self.title),
            String::new(),
            "Generated by `concordance suite`. Do not edit.".into(),
        ];
        if let Some(today) = &self.today {
            out.push(format!("As of {today}."));
        }
        out.extend(["".into(), "## Apps".into(), String::new()]);
        out.push(
            "| App | Last release | Next release | Window | Open bugs (critical/high) | Open requests | Unverified | Gaps |"
                .into(),
        );
        out.push("| --- | --- | --- | --- | --- | --- | --- | --- |".into());
        for app in &self.apps {
            let s = &app.summary;
            out.push(row(&[
                app.label.clone(),
                release_text(s.last_release.as_ref()),
                release_text(s.next_release.as_ref()),
                s.next_release
                    .as_ref()
                    .map(|r| window_text(r.window.as_ref()))
                    .unwrap_or_else(|| "—".into()),
                format!(
                    "{} ({}/{})",
                    s.open_bugs.total, s.open_bugs.critical, s.open_bugs.high
                ),
                s.open_requests.to_string(),
                s.unverified.to_string(),
                gaps_text(&s.gaps),
            ]));
        }
        if self.apps.iter().any(|a| !a.warnings.is_empty()) {
            out.extend(["".into(), "## Warnings".into(), String::new()]);
            for app in &self.apps {
                for warning in &app.warnings {
                    out.push(format!("- {}: {warning}", app.label));
                }
            }
        }
        out.extend(["".into(), "## Beats".into()]);
        if self.beats.is_empty() {
            out.extend([String::new(), "None.".into()]);
        }
        for beat in &self.beats {
            out.push(String::new());
            out.push(format!("### {} — {}", beat.date, beat.title));
            out.push(String::new());
            if let Some(why) = &beat.why {
                out.push(why.clone());
                out.push(String::new());
            }
            if beat.releases.is_empty() {
                out.push(format!(
                    "No release windows in the {BEAT_LEAD_DAYS} days before."
                ));
            }
            for r in &beat.releases {
                out.push(format!(
                    "- {} `{}` ({}): {} to {}",
                    r.app, r.release, r.state, r.from, r.to
                ));
            }
        }
        out.extend(["".into(), "## Shared features".into(), String::new()]);
        if self.shared.is_empty() {
            out.push("None.".into());
        } else {
            out.push("| Feature | Entries (baseline status) | Unresolved |".into());
            out.push("| --- | --- | --- |".into());
            for feature in &self.shared {
                let members = feature
                    .members
                    .iter()
                    .map(|m| {
                        format!(
                            "{}:`{}` {} {}",
                            m.app, m.entry, m.baseline, m.baseline_status
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                let unresolved = if feature.unresolved.is_empty() {
                    "—".to_owned()
                } else {
                    feature
                        .unresolved
                        .iter()
                        .map(|u| format!("**`{u}`**"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                out.push(row(&[
                    format!("`{}`", feature.feature),
                    members,
                    unresolved,
                ]));
            }
        }
        out.join("\n") + "\n"
    }
}
