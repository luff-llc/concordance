use crate::{fail, is_capture_key, Catalog, Entry, Result, Status};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

fn table(value: &str) -> String {
    value.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// Optional knowledge that refines derived verification, such as capture coverage.
#[derive(Debug, Default, Clone)]
pub struct ReportContext {
    /// Capture state keyed by (`screen.state`, platform). Empty means coverage is unknown.
    pub captures: BTreeMap<(String, String), CaptureState>,
    /// Directory the report will be written to; links are made relative to it.
    /// `None` keeps links relative to the catalog directory.
    pub link_base: Option<PathBuf>,
}

/// The directory containing a report output path.
pub(crate) fn link_base(out: &Path) -> PathBuf {
    match out.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// `target` expressed relative to directory `base`, with `/` separators.
pub fn relative_path(base: &Path, target: &Path) -> Result<String> {
    let base = base.canonicalize()?;
    let target = match target.canonicalize() {
        Ok(path) => path,
        // A not-yet-existing file: resolve its parent.
        Err(_) => target
            .parent()
            .unwrap_or(Path::new("."))
            .canonicalize()?
            .join(target.file_name().unwrap_or_default()),
    };
    let b: Vec<Component> = base.components().collect();
    let t: Vec<Component> = target.components().collect();
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); b.len() - common];
    parts.extend(
        t[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    Ok(if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    })
}

/// Whether a required screen/state capture exists for a platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "lowercase")]
pub enum CaptureState {
    Produced,
    Skipped(String),
    Missing(String),
}

/// Detail tallies for one platform.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DetailCounts {
    pub total: usize,
    pub implemented: usize,
    pub verified: usize,
}
pub(crate) fn detail_counts<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    platform: &str,
) -> DetailCounts {
    let mut c = DetailCounts::default();
    for detail in entries.into_iter().flat_map(|e| &e.details) {
        let record = detail.platforms.get(platform);
        if record.is_some_and(|p| p.status == Status::NotApplicable) {
            continue;
        }
        c.total += 1;
        if record.is_some_and(|p| p.status == Status::Implemented) {
            c.implemented += 1;
        }
        if record.is_some_and(|p| p.verified()) {
            c.verified += 1;
        }
    }
    c
}
/// (open, total) details on a platform, or None for entries without details.
pub(crate) fn open_details(entry: &Entry, platform: &str) -> Option<(usize, usize)> {
    let c = detail_counts([entry], platform);
    (c.total > 0).then_some((c.total - c.implemented, c.total))
}
pub(crate) fn detail_summary(detail: &crate::Detail) -> String {
    let numbers = detail.spec.numbers();
    if numbers.is_empty() {
        detail.spec.text.clone()
    } else {
        format!("{} ({numbers})", detail.spec.text)
    }
}

impl Catalog {
    /// Why an entry's platform record is not verified (including capture coverage), if it is not.
    pub fn verification_gap(
        &self,
        entry: &Entry,
        platform: &str,
        context: &ReportContext,
    ) -> Option<String> {
        let record = entry.platforms.get(platform)?;
        if let Some(reason) = record.unverified_reason() {
            return Some(reason.into());
        }
        let verification = record.verification.as_ref()?;
        let lacking = entry.missing_verification_evidence(verification);
        if !lacking.is_empty() {
            return Some(format!(
                "verification lacks a required {} (verify_requires)",
                lacking.join(" and a ")
            ));
        }
        if context.captures.is_empty() {
            return None;
        }
        for capture in verification.captures.iter().filter(|c| is_capture_key(c)) {
            match context
                .captures
                .get(&(capture.clone(), platform.to_owned()))
            {
                Some(CaptureState::Produced) => {}
                Some(CaptureState::Skipped(reason)) => {
                    return Some(format!("capture {capture} skipped: {reason}"))
                }
                Some(CaptureState::Missing(reason)) => {
                    return Some(format!("capture {capture} missing: {reason}"))
                }
                None => return Some(format!("capture {capture} is not a declared screen state")),
            }
        }
        None
    }

    pub fn report(&self) -> Result<String> {
        self.report_with(&ReportContext::default())
    }

    pub fn report_with(&self, context: &ReportContext) -> Result<String> {
        let config = &self.config.report;
        let (base, target) = (&config.baseline, &config.target);
        let (bl, tl) = (config.label(base), config.label(target));
        for entry in self.entries.values() {
            if !entry.platforms.contains_key(base) || !entry.platforms.contains_key(target) {
                return fail(format!(
                    "{}: report requires {base} and {target} statuses",
                    entry.id
                ));
            }
        }
        let baseline: Vec<_> = self
            .entries
            .values()
            .filter(|e| e.platforms[base].status == Status::Implemented)
            .collect();
        // [implemented, verified, partial, missing, not-applicable]
        let counts = |entries: &[&Entry]| -> [usize; 5] {
            let mut c = [0; 5];
            for entry in entries {
                let p = &entry.platforms[target];
                match p.status {
                    Status::Implemented => c[0] += 1,
                    Status::Partial => c[2] += 1,
                    Status::Missing => c[3] += 1,
                    Status::NotApplicable => c[4] += 1,
                }
                if self.verification_gap(entry, target, context).is_none() {
                    c[1] += 1;
                }
            }
            c
        };
        let totals = counts(&baseline);
        let has_na = totals[4] != 0;
        // Never a single "shipped" total: implemented and verified are separate claims.
        let mut summary = format!(
            "**{} {bl} capabilities: {} {tl} implemented ({} verified), {} partial, {} missing",
            baseline.len(),
            totals[0],
            totals[1],
            totals[2],
            totals[3]
        );
        if has_na {
            summary.push_str(&format!(", {} not-applicable", totals[4]));
        }
        summary.push_str(".**");
        let has_details = self.entries.values().any(|e| !e.details.is_empty());
        let detail_cell = |entries: &[&Entry]| -> String {
            if !has_details {
                return String::new();
            }
            let c = detail_counts(entries.iter().copied(), target);
            format!(" {} / {} / {} |", c.implemented, c.verified, c.total)
        };
        let mut lines = vec![
            format!("# {}", config.title),
            String::new(),
            "Generated by `concordance report`. Edit the catalog, then regenerate.".into(),
            String::new(),
            config.intro.trim_end().into(),
            String::new(),
            summary,
            String::new(),
            format!(
                "| Area | Entries | {bl} baseline | {tl} implemented | Verified | Partial | Missing |{}",
                if has_na { " Not applicable |" } else { "" }
            ) + if has_details { " Details implemented / verified / total |" } else { "" },
            format!(
                "| --- | ---: | ---: | ---: | ---: | ---: | ---: |{}",
                if has_na { " ---: |" } else { "" }
            ) + if has_details { " ---: |" } else { "" },
        ];
        let link = |relative: &str| -> Result<String> {
            match &context.link_base {
                Some(base) => relative_path(base, &self.directory.join(relative)),
                None => Ok(relative.to_owned()),
            }
        };
        for (area, file) in &self.area_files {
            let file = &link(file)?;
            let group: Vec<_> = self
                .entries
                .values()
                .filter(|e| self.areas[&e.id] == *area)
                .collect();
            let included: Vec<_> = group
                .iter()
                .copied()
                .filter(|e| e.platforms[base].status == Status::Implemented)
                .collect();
            let c = counts(&included);
            lines.push(
                format!(
                    "| [{area}]({file}) | {} | {} | {} | {} | {} | {} |{}",
                    group.len(),
                    included.len(),
                    c[0],
                    c[1],
                    c[2],
                    c[3],
                    if has_na {
                        format!(" {} |", c[4])
                    } else {
                        String::new()
                    }
                ) + &detail_cell(&included),
            );
        }
        lines.extend([
            format!(
                "| **Total** | **{}** | **{}** | **{}** | **{}** | **{}** | **{}** |{}",
                self.entries.len(),
                baseline.len(),
                totals[0],
                totals[1],
                totals[2],
                totals[3],
                if has_na {
                    format!(" **{}** |", totals[4])
                } else {
                    String::new()
                }
            ) + &detail_cell(&baseline),
            String::new(),
            "## Ten highest-impact gaps".into(),
            String::new(),
            config.ordering_note.trim_end().into(),
            String::new(),
        ]);
        let mut gaps: Vec<_> = baseline
            .iter()
            .copied()
            .filter(|e| e.platforms[target].status.is_gap())
            .collect();
        gaps.sort_by_key(|e| self.gap_key(e));
        for (index, entry) in gaps.iter().take(10).enumerate() {
            let p = &entry.platforms[target];
            lines.push(format!(
                "{}. **{}** — `{}` ({}, {}): {}{}",
                index + 1,
                entry.title,
                entry.id,
                entry.impact.as_str(),
                p.status.as_str(),
                p.gap.as_deref().unwrap_or("No gap recorded."),
                open_details(entry, target)
                    .map(|(open, total)| format!(" Details: {open} of {total} open."))
                    .unwrap_or_default()
            ));
        }
        for (status, heading) in [
            (Status::Missing, "Missing capabilities, by user impact"),
            (Status::Partial, "Partial capabilities, by user impact"),
        ] {
            lines.extend([
                String::new(),
                format!("## {heading}"),
                String::new(),
                format!(
                    "| Impact | ID | Capability | Gap |{}",
                    if has_details { " Open details |" } else { "" }
                ),
                format!(
                    "| --- | --- | --- | --- |{}",
                    if has_details { " ---: |" } else { "" }
                ),
            ]);
            for entry in &gaps {
                let p = &entry.platforms[target];
                if p.status == status {
                    lines.push(format!(
                        "| {} | `{}` | {} | {} |{}",
                        entry.impact.as_str(),
                        entry.id,
                        table(&entry.title),
                        table(p.gap.as_deref().unwrap_or("No gap recorded.")),
                        if has_details {
                            let (open, total) = open_details(entry, target).unwrap_or((0, 0));
                            format!(" {open} / {total} |")
                        } else {
                            String::new()
                        }
                    ));
                }
            }
        }
        if has_details {
            // An implemented entry can still hide unimplemented motion, haptics or geometry.
            lines.extend([
                String::new(),
                "## Detail gaps in implemented entries".into(),
                String::new(),
            ]);
            let mut rows = Vec::new();
            let mut implemented: Vec<_> = self
                .entries
                .values()
                .filter(|e| {
                    e.platforms
                        .get(target)
                        .is_some_and(|p| p.status == Status::Implemented)
                })
                .collect();
            implemented.sort_by_key(|e| self.gap_key(e));
            for entry in implemented {
                for detail in &entry.details {
                    let status = detail
                        .platforms
                        .get(target)
                        .map(|p| p.status.as_str())
                        .unwrap_or("unrecorded");
                    if matches!(status, "implemented" | "not-applicable") {
                        continue;
                    }
                    rows.push(format!(
                        "| `{}#{}` | {} | {status} | {} |",
                        entry.id,
                        detail.id,
                        detail.kind.as_str(),
                        table(&detail_summary(detail))
                    ));
                }
            }
            if rows.is_empty() {
                lines.push("None.".into());
            } else {
                lines.extend([
                    "| Detail | Kind | Status | Spec |".into(),
                    "| --- | --- | --- | --- |".into(),
                ]);
                lines.extend(rows);
            }
        }
        let claimed: Vec<_> = self
            .entries
            .values()
            .filter(|e| e.platforms[target].status == Status::Implemented)
            .filter_map(|e| self.verification_gap(e, target, context).map(|r| (e, r)))
            .collect();
        let implemented = self
            .entries
            .values()
            .filter(|e| e.platforms[target].status == Status::Implemented)
            .count();
        lines.extend([
            String::new(),
            "## Claimed shipped but unverified".into(),
            String::new(),
            format!(
                "{} of {implemented} {tl} implemented entries (all entries, including any outside the {bl} baseline) have no independent verification (a reviewer other than the implementer, with captures or journeys).",
                claimed.len()
            ),
        ]);
        if !claimed.is_empty() {
            lines.extend([
                String::new(),
                "| Impact | ID | Area | Reason |".into(),
                "| --- | --- | --- | --- |".into(),
            ]);
            let mut claimed = claimed;
            claimed.sort_by_key(|(e, _)| self.gap_key(e));
            for (entry, reason) in claimed {
                lines.push(format!(
                    "| {} | `{}` | {} | {} |",
                    entry.impact.as_str(),
                    entry.id,
                    self.areas[&entry.id],
                    table(&reason)
                ));
            }
        }
        let extras: Vec<_> = self
            .entries
            .values()
            .filter(|e| e.platforms[base].status != Status::Implemented)
            .collect();
        if !extras.is_empty() {
            // A partial baseline is not an adaptation; keep that distinction visible.
            let adaptations_only = extras
                .iter()
                .all(|e| e.platforms[base].status == Status::NotApplicable);
            lines.extend([String::new(), if adaptations_only { format!("## {tl}-specific adaptations") } else { "## Outside the shipped baseline".into() }, String::new(), if adaptations_only { format!("These entries have no {bl} counterpart and are excluded from the {bl} parity totals.") } else { format!("These entries are not shipped on {bl} and are excluded from its parity totals.") }, String::new()]);
            for entry in extras {
                let extra = config
                    .adaptation_platform
                    .as_ref()
                    .map(|p| {
                        format!(
                            "; {} {}",
                            config.label(p),
                            entry
                                .platforms
                                .get(p)
                                .map(|p| p.status.as_str())
                                .unwrap_or("not recorded")
                        )
                    })
                    .unwrap_or_default();
                lines.push(format!(
                    "- `{}`: {tl} {}{extra}. {}",
                    entry.id,
                    entry.platforms[target].status.as_str(),
                    entry.summary
                ));
            }
        }
        lines.extend([String::new(), "## Evidence limits".into(), String::new(), format!("- {bl} entries without linked tests: {}.", baseline.iter().filter(|e| e.platforms[base].tests.is_empty()).count()), format!("- {tl} implemented/partial entries without linked tests: {}.", baseline.iter().filter(|e| matches!(e.platforms[target].status, Status::Implemented | Status::Partial) && e.platforms[target].tests.is_empty()).count()), format!("- {}", config.evidence_note), format!("- See [README]({}) for status rules, exclusions, source roots, and audit limitations.", link("README.md")?), String::new()]);
        Ok(lines.join("\n"))
    }
}
