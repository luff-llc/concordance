//! Capture coverage: which required screen/state captures exist, were skipped, or are missing.
use super::{capture_file, UiCatalog};
use crate::{is_capture_key, CaptureState, Issue, ReportContext, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct CaptureRecord {
    pub screen: String,
    pub state: String,
    pub platform: String,
    pub capture: CaptureState,
}
impl CaptureRecord {
    pub fn key(&self) -> String {
        format!("{}.{}", self.screen, self.state)
    }
}

/// Required captures and their state for a set of platforms.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Coverage {
    pub platforms: Vec<String>,
    pub records: Vec<CaptureRecord>,
    /// Screen ID to the behavior areas of its linked entries.
    pub screen_areas: BTreeMap<String, BTreeSet<String>>,
}

/// [required, produced, skipped, missing]
fn tally<'a>(records: impl Iterator<Item = &'a CaptureRecord>) -> [usize; 4] {
    let mut c = [0; 4];
    for record in records {
        c[0] += 1;
        match record.capture {
            CaptureState::Produced => c[1] += 1,
            CaptureState::Skipped(_) => c[2] += 1,
            CaptureState::Missing(_) => c[3] += 1,
        }
    }
    c
}

impl Coverage {
    /// Capture states keyed for verification checks.
    pub fn context(&self) -> ReportContext {
        ReportContext {
            captures: self
                .records
                .iter()
                .map(|r| ((r.key(), r.platform.clone()), r.capture.clone()))
                .collect(),
        }
    }

    pub fn counts(&self, platform: &str) -> [usize; 4] {
        tally(self.records.iter().filter(|r| r.platform == platform))
    }

    pub fn markdown(&self, ui: &UiCatalog) -> String {
        let mut out = String::from("## Capture coverage\n\nRequired captures are declared screen states. Produced means the PNG and JSON pair exists; skipped means a `<screen>.<state>.<platform>.skip` marker records why; missing means neither. Skipped and missing captures cannot support verification.\n\n| Platform | Required | Produced | Skipped | Missing |\n| --- | ---: | ---: | ---: | ---: |\n");
        for platform in &self.platforms {
            let c = self.counts(platform);
            out.push_str(&format!(
                "| {platform} | {} | {} | {} | {} |\n",
                c[0], c[1], c[2], c[3]
            ));
        }
        let mut areas: BTreeSet<&str> = BTreeSet::new();
        for set in self.screen_areas.values() {
            areas.extend(set.iter().map(String::as_str));
        }
        out.push_str("\n### By area\n\nA screen counts toward every area of its linked entries.\n\n| Area | Platform | Required | Produced | Skipped | Missing |\n| --- | --- | ---: | ---: | ---: | ---: |\n");
        for area in areas {
            for platform in &self.platforms {
                let c = tally(self.records.iter().filter(|r| {
                    &r.platform == platform
                        && self
                            .screen_areas
                            .get(&r.screen)
                            .is_some_and(|a| a.contains(area))
                }));
                out.push_str(&format!(
                    "| {area} | {platform} | {} | {} | {} | {} |\n",
                    c[0], c[1], c[2], c[3]
                ));
            }
        }
        out.push_str("\n### By screen\n\n| Screen | Platform | Required | Produced | Skipped | Missing |\n| --- | --- | ---: | ---: | ---: | ---: |\n");
        for screen in ui.screens.keys() {
            for platform in &self.platforms {
                let c = tally(
                    self.records
                        .iter()
                        .filter(|r| &r.screen == screen && &r.platform == platform),
                );
                out.push_str(&format!(
                    "| {screen} | {platform} | {} | {} | {} | {} |\n",
                    c[0], c[1], c[2], c[3]
                ));
            }
        }
        for (title, skipped) in [("Skipped captures", true), ("Missing captures", false)] {
            out.push_str(&format!("\n### {title}\n\n"));
            let mut any = false;
            for record in &self.records {
                let reason = match (&record.capture, skipped) {
                    (CaptureState::Skipped(r), true) | (CaptureState::Missing(r), false) => r,
                    _ => continue,
                };
                any = true;
                out.push_str(&format!(
                    "- `{}.{}`: {}\n",
                    record.key(),
                    record.platform,
                    reason
                ));
            }
            if !any {
                out.push_str("None.\n");
            }
        }
        let context = self.context();
        out.push_str("\n### Verification blocked by captures\n\n");
        let mut blocked = Vec::new();
        for entry in ui.catalog.entries.values() {
            for platform in &self.platforms {
                let Some(record) = entry.platforms.get(platform) else {
                    continue;
                };
                if record.unverified_reason().is_some() {
                    continue;
                }
                if let Some(reason) = ui.catalog.verification_gap(entry, platform, &context) {
                    blocked.push(format!("- `{}` ({platform}): {reason}\n", entry.id));
                }
            }
        }
        if blocked.is_empty() {
            out.push_str("None.\n");
        }
        for line in blocked {
            out.push_str(&line);
        }
        out
    }
}

impl UiCatalog {
    /// Coverage of every declared screen state for the given platforms.
    pub fn coverage(&self, captures: &Path, platforms: &[&str]) -> Result<Coverage> {
        let mut coverage = Coverage {
            platforms: platforms.iter().map(|p| (*p).to_owned()).collect(),
            ..Coverage::default()
        };
        for platform in platforms {
            self.require_platform(platform)?;
        }
        for screen in self.screens.values() {
            let areas = screen
                .entries
                .iter()
                .filter_map(|id| self.catalog.areas.get(id).cloned())
                .collect::<BTreeSet<_>>();
            coverage.screen_areas.insert(
                screen.id.clone(),
                if areas.is_empty() {
                    BTreeSet::from(["(unlinked)".to_owned()])
                } else {
                    areas
                },
            );
            for state in screen.states.keys() {
                for platform in platforms {
                    let stem = format!("{}.{state}.{platform}", screen.id);
                    let png = capture_file(captures, platform, &stem, "png")?.is_file();
                    let json = capture_file(captures, platform, &stem, "json")?.is_file();
                    let skip = capture_file(captures, platform, &stem, "skip")?;
                    let capture = match (png, json) {
                        (true, true) => CaptureState::Produced,
                        _ if skip.is_file() => {
                            let reason = fs::read_to_string(&skip)?
                                .lines()
                                .map(str::trim)
                                .find(|l| !l.is_empty())
                                .unwrap_or("no reason recorded")
                                .to_owned();
                            CaptureState::Skipped(reason)
                        }
                        (true, false) => CaptureState::Missing("PNG without JSON".into()),
                        (false, true) => CaptureState::Missing("JSON without PNG".into()),
                        (false, false) => CaptureState::Missing("no PNG/JSON pair".into()),
                    };
                    coverage.records.push(CaptureRecord {
                        screen: screen.id.clone(),
                        state: state.clone(),
                        platform: (*platform).to_owned(),
                        capture,
                    });
                }
            }
        }
        Ok(coverage)
    }

    /// Verification capture keys must name declared screen states.
    pub fn verification_issues(&self) -> Vec<Issue> {
        let declared: BTreeSet<String> = self
            .screens
            .values()
            .flat_map(|s| {
                s.states
                    .keys()
                    .map(move |state| format!("{}.{state}", s.id))
            })
            .collect();
        let mut issues = Vec::new();
        for (label, platform, record) in self.catalog.records() {
            for capture in record
                .verification
                .iter()
                .flat_map(|v| v.captures.iter())
                .filter(|c| is_capture_key(c))
            {
                if !declared.contains(capture) {
                    issues.push(Issue {
                        group: "verification".into(),
                        entry: label.clone(),
                        platform: platform.to_owned(),
                        reference: capture.clone(),
                        message: "capture key is not a declared screen.state".into(),
                    });
                }
            }
        }
        issues
    }
}
