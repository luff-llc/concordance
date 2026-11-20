//! Product behavior catalogs shared by native implementations and their tests.
mod drift;
mod evidence;
mod generate;
mod model;
mod report;
mod validation;
pub use drift::*;
pub use evidence::{split_anchor, Anchor};
pub use report::{relative_path, CaptureState, ReportContext};
pub use validation::*;
pub mod schema;
pub mod ui;

pub use generate::{JourneyResult, Language};
pub use model::*;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(message.into().into())
}

#[derive(Debug)]
pub struct Catalog {
    pub config: Config,
    /// Sorted by stable ID, independent of YAML sequence or file enumeration order.
    pub entries: BTreeMap<String, Entry>,
    pub areas: BTreeMap<String, String>,
    pub directory: PathBuf,
    pub area_files: BTreeMap<String, String>,
}

impl Catalog {
    /// Load catalog.toml and catalog/*.yaml (or *.yaml beside catalog.toml).
    /// Enforces the bundled schema, IDs, retirement records and dependency DAG.
    /// Evidence existence is separate so inspection/generation works without source checkouts.
    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        let (catalog, report) = Self::load_collect(directory)?;
        report.into_result()?;
        Ok(catalog)
    }

    /// Load recoverable entries and collect independent structural problems.
    pub fn load_collect(directory: impl AsRef<Path>) -> Result<(Self, ValidationReport)> {
        let mut report = ValidationReport::default();
        let directory = directory.as_ref().to_path_buf();
        let config_path = directory.join("catalog.toml");
        let config: Config = toml::from_str(
            &fs::read_to_string(&config_path)
                .map_err(|e| format!("{}: {e}", config_path.display()))?,
        )?;
        if config.platforms.is_empty() {
            report.error(
                "config",
                "",
                "",
                "catalog.toml",
                "platforms must not be empty",
            );
        }
        for (name, root) in &config.platforms {
            if name.is_empty() || !safe_relative(root, true) {
                report.error("config", "", name, root, "unsafe platform root");
            }
        }
        for (name, prefix) in &config.path_prefixes {
            if !config.platforms.contains_key(name)
                || !prefix.ends_with('/')
                || !safe_relative(prefix, false)
            {
                report.error("config", "", name, prefix, "invalid path prefix");
            }
        }
        if Path::new(&config.repo_root).is_absolute() {
            report.error(
                "config",
                "",
                "",
                "repo_root",
                "repo_root must be relative to the catalog directory",
            );
        }
        for (group, names) in [
            (
                "external_roots",
                config.external_roots.keys().collect::<Vec<_>>(),
            ),
            ("baselines", config.baselines.keys().collect()),
            ("source_globs", config.source_globs.keys().collect()),
        ] {
            for name in names {
                if !config.platforms.contains_key(name) {
                    report.error("config", "", name, group, "unknown platform");
                }
            }
        }
        let nested = directory.join("catalog").is_dir();
        let data_dir = if nested {
            directory.join("catalog")
        } else {
            directory.clone()
        };
        let mut files = fs::read_dir(&data_dir)?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        files.retain(|p| {
            matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
                && (nested || p.file_name().and_then(|s| s.to_str()) != Some("tokens.yaml"))
        });
        files.sort();
        if files.is_empty() {
            report.error(
                "schema",
                "",
                "",
                &data_dir.display().to_string(),
                "no catalog YAML files",
            );
        }
        let mut catalog = Self {
            config,
            entries: BTreeMap::new(),
            areas: BTreeMap::new(),
            area_files: BTreeMap::new(),
            directory,
        };
        for file in files {
            let area = file
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("non-UTF8 area name")?
                .to_owned();
            if !safe_area(&area) {
                report.error(
                    "schema",
                    "",
                    "",
                    &file.display().to_string(),
                    "invalid area filename",
                );
                continue;
            }
            if catalog.area_files.contains_key(&area) {
                report.error("schema", "", "", &area, "duplicate area filename");
                continue;
            }
            let name = file
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("non-UTF8 filename")?;
            catalog.area_files.insert(
                area.clone(),
                format!("{}{name}", if nested { "catalog/" } else { "" }),
            );
            // Parsing failures affect this document only; other files still contribute issues.
            let values = (|| -> Result<serde_json::Value> {
                let value: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(&file)?)?;
                Ok(serde_json::to_value(value)?)
            })();
            let reference = file.display().to_string();
            let value = match values {
                Ok(value) => value,
                Err(error) => {
                    report.error("schema", "", "", &reference, &error.to_string());
                    continue;
                }
            };
            let Some(values) = value.as_array() else {
                report.error("schema", "", "", &reference, "expected array");
                continue;
            };
            if values.is_empty() {
                report.error("schema", "", "", &reference, "too few items");
            }
            for (index, value) in values.iter().enumerate() {
                let id = value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let errors = schema::validate_all(&serde_json::json!([value]));
                for error in &errors {
                    report.error(
                        "schema",
                        id,
                        "",
                        &reference,
                        &error.replace("$[0]", &format!("$[{index}]")),
                    );
                }
                let entry: Entry = match serde_json::from_value(value.clone()) {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                validation::check_entry(&entry, &mut report);
                let id = &entry.id;
                let detail_platforms = entry.details.iter().flat_map(|d| {
                    d.platforms
                        .keys()
                        .map(move |p| (format!("{id}#{}", d.id), p))
                });
                for (label, platform) in entry
                    .platforms
                    .keys()
                    .map(|p| (entry.id.clone(), p))
                    .chain(detail_platforms)
                {
                    if !catalog.config.platforms.contains_key(platform) {
                        report.error(
                            "identity",
                            &label,
                            platform,
                            "",
                            "platform has no configured repo root",
                        );
                    }
                }
                if catalog.entries.contains_key(&entry.id) {
                    report.error("identity", &entry.id, "", &reference, "duplicate ID");
                    continue;
                }
                catalog.areas.insert(entry.id.clone(), area.clone());
                catalog.entries.insert(entry.id.clone(), entry);
            }
        }
        for entry in catalog.entries.values() {
            for dependency in &entry.depends_on {
                if !catalog.entries.contains_key(dependency) {
                    report.error(
                        "dependencies",
                        &entry.id,
                        "",
                        dependency,
                        "unresolved depends_on",
                    );
                }
            }
        }
        // Find every node participating in a cycle without hiding independent missing dependencies.
        for id in catalog.entries.keys() {
            let mut pending = catalog.entries[id].depends_on.clone();
            let mut seen = BTreeSet::new();
            while let Some(next) = pending.pop() {
                if &next == id {
                    report.error("dependencies", id, "", "", "dependency cycle");
                    break;
                }
                if seen.insert(next.clone()) {
                    if let Some(entry) = catalog.entries.get(&next) {
                        pending.extend(entry.depends_on.iter().cloned());
                    }
                }
            }
        }
        catalog.validate_retired(&mut report)?;
        report.sort();
        Ok((catalog, report))
    }

    pub fn entry(&self, id: &str) -> Result<&Entry> {
        self.entries
            .get(id)
            .ok_or_else(|| format!("unknown entry: {id}").into())
    }

    fn validate_retired(&self, report: &mut ValidationReport) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Retirement {
            replaced_by: Vec<String>,
            reason: String,
        }
        let path = self.directory.join("retired-ids.json");
        if !path.exists() {
            return Ok(());
        }
        let retired = (|| -> Result<BTreeMap<String, serde_yaml::Value>> {
            let value: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(&path)?)?;
            Ok(serde_yaml::from_value(value)?)
        })();
        let retired = match retired {
            Ok(retired) => retired,
            Err(error) => {
                report.error("retired", "", "", "retired-ids.json", &error.to_string());
                return Ok(());
            }
        };
        for (id, value) in retired {
            let record: Retirement = match serde_yaml::from_value(value) {
                Ok(record) => record,
                Err(error) => {
                    report.error("retired", &id, "", "retired-ids.json", &error.to_string());
                    continue;
                }
            };
            if !schema::valid_id(&id) || record.reason.trim().is_empty() {
                report.error("retired", &id, "", "", "invalid retirement record");
            }
            if self.entries.contains_key(&id) {
                report.error("retired", &id, "", "", "retired ID cannot be reused");
            }
            for replacement in record.replaced_by {
                if !self.entries.contains_key(&replacement) {
                    report.error(
                        "retired",
                        &id,
                        "",
                        &replacement,
                        "unresolved retired-ID replacement",
                    );
                }
            }
        }
        Ok(())
    }

    /// Check referenced source/test files, including unqualified test file paths.
    /// `path#Suite.method` checks the file, not language declarations or test coverage.
    pub fn validate_paths(&self, repo: Option<&Path>) -> Result<()> {
        self.validate_evidence(&RootOptions {
            repo: repo.map(Path::to_path_buf),
            ..RootOptions::default()
        })
        .into_result()
    }

    pub(crate) fn gap_key<'a>(&'a self, entry: &'a Entry) -> (Impact, u16, usize, &'a str) {
        let area = &self.areas[&entry.id];
        (
            entry.impact,
            entry.priority.unwrap_or(1000),
            self.config
                .report
                .area_order
                .iter()
                .position(|a| a == area)
                .unwrap_or(self.config.report.area_order.len()),
            &entry.id,
        )
    }

    fn dependency_order(&self, candidates: Option<&BTreeSet<String>>) -> Result<Vec<&Entry>> {
        let mut remaining: BTreeMap<&str, usize> = self
            .entries
            .values()
            .map(|e| (e.id.as_str(), e.depends_on.len()))
            .collect();
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for entry in self.entries.values() {
            for dependency in &entry.depends_on {
                dependents.entry(dependency).or_default().push(&entry.id);
            }
        }
        let mut ready: BTreeSet<_> = remaining
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(id, _)| *id)
            .collect();
        let mut ordered = Vec::new();
        while !ready.is_empty() {
            let id = *ready
                .iter()
                .min_by_key(|id| {
                    let entry = &self.entries[**id];
                    // Resolve completed/nonselected dependencies before choosing among ready gaps.
                    (
                        candidates.is_some_and(|set| set.contains(**id)),
                        self.gap_key(entry),
                    )
                })
                .ok_or("empty ready queue")?;
            ready.remove(id);
            remaining.remove(id);
            ordered.push(&self.entries[id]);
            for next in dependents.get(id).into_iter().flatten() {
                let count = remaining.get_mut(next).ok_or("invalid dependency graph")?;
                *count -= 1;
                if *count == 0 {
                    ready.insert(next);
                }
            }
        }
        if !remaining.is_empty() {
            return fail(format!(
                "dependency cycle (or blocked by cycle): {}",
                remaining.keys().copied().collect::<Vec<_>>().join(", ")
            ));
        }
        Ok(ordered)
    }

    /// Missing/partial work in dependency order, ranked by impact among ready items.
    /// An area filter includes unfinished prerequisites from other areas.
    pub fn next(&self, platform: &str, area: Option<&str>, limit: usize) -> Result<Vec<&Entry>> {
        self.next_with(platform, area, limit, false)
    }

    /// Details of an entry that are missing, partial or unrecorded on a platform.
    pub fn open_details<'a>(&self, entry: &'a Entry, platform: &str) -> Vec<&'a Detail> {
        entry
            .details
            .iter()
            .filter(|d| d.platforms.get(platform).is_none_or(|p| p.status.is_gap()))
            .collect()
    }

    /// Like `next`; with `details`, implemented entries with open details are also selected.
    pub fn next_with(
        &self,
        platform: &str,
        area: Option<&str>,
        limit: usize,
        details: bool,
    ) -> Result<Vec<&Entry>> {
        if !self.config.platforms.contains_key(platform) {
            return fail(format!("unknown platform: {platform}"));
        }
        if area.is_some_and(|area| !self.area_files.contains_key(area)) {
            return fail(format!("unknown area: {}", area.unwrap_or_default()));
        }
        let mut selected = BTreeSet::new();
        let mut pending: Vec<_> = self
            .entries
            .values()
            .filter(|e| {
                area.is_none_or(|a| self.areas[&e.id] == a)
                    && e.platforms.get(platform).is_some_and(|p| {
                        p.status.is_gap()
                            || (details
                                && p.status == Status::Implemented
                                && !self.open_details(e, platform).is_empty())
                    })
            })
            .map(|e| e.id.clone())
            .collect();
        while let Some(id) = pending.pop() {
            if selected.insert(id.clone()) {
                pending.extend(self.entries[&id].depends_on.iter().cloned());
            }
        }
        // A missing platform record is unknown, never silently treated as completed.
        for id in &selected {
            if !self.entries[id].platforms.contains_key(platform) {
                return fail(format!("{id}: no status for dependency on {platform}"));
            }
        }
        selected.retain(|id| {
            let entry = &self.entries[id];
            let status = entry.platforms[platform].status;
            status.is_gap()
                || (details
                    && status == Status::Implemented
                    && !self.open_details(entry, platform).is_empty())
        });
        Ok(self
            .dependency_order(Some(&selected))?
            .into_iter()
            .filter(|e| selected.contains(&e.id))
            .take(limit)
            .collect())
    }

    pub fn brief(&self, id: &str) -> Result<String> {
        self.brief_with(id, false)
    }

    /// An implementation brief; with `details`, every detail item with its numbers and status.
    pub fn brief_with(&self, id: &str, details: bool) -> Result<String> {
        let entry = self.entry(id)?;
        let mut lines = vec![
            format!("# {} — `{}`", entry.title, entry.id),
            String::new(),
            entry.summary.clone(),
        ];
        fn section(lines: &mut Vec<String>, title: &str, values: &[String]) {
            lines.extend([String::new(), format!("## {title}")]);
            if values.is_empty() {
                lines.push("None recorded.".into());
            }
            for value in values {
                lines.push(format!("- {value}"));
            }
        }
        section(&mut lines, "Surfaces", &entry.surfaces);
        section(
            &mut lines,
            "Interactions",
            &entry
                .interactions
                .iter()
                .map(|i| format!("{} → {}", i.trigger, i.result))
                .collect::<Vec<_>>(),
        );
        section(&mut lines, "States", &entry.states);
        section(&mut lines, "Settings", &entry.settings);
        section(&mut lines, "Data reads", &entry.data.reads);
        section(&mut lines, "Data writes", &entry.data.writes);
        section(&mut lines, "Copy", &entry.copy);
        lines.extend([
            String::new(),
            "## Accessibility".into(),
            format!(
                "Label: {:?}\nIdentifier: {:?}",
                entry.accessibility.label, entry.accessibility.identifier
            ),
        ]);
        lines.extend(entry.accessibility.notes.iter().map(|n| format!("- {n}")));
        section(
            &mut lines,
            "Acceptance",
            &entry
                .acceptance
                .iter()
                .map(|a| format!("Given {}; when {}; then {}.", a.given, a.when, a.then))
                .collect::<Vec<_>>(),
        );
        section(
            &mut lines,
            "Dependencies and status",
            &entry
                .depends_on
                .iter()
                .map(|id| {
                    format!(
                        "`{id}`: {}",
                        self.entries[id]
                            .platforms
                            .iter()
                            .map(|(p, e)| format!("{p} {}", e.status.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .collect::<Vec<_>>(),
        );
        section(
            &mut lines,
            "Implementation status",
            &entry
                .platforms
                .iter()
                .map(|(p, e)| {
                    format!(
                        "{p}: {}; {}. {} {}",
                        e.status.as_str(),
                        verification_line(e),
                        e.gap.as_deref().unwrap_or(""),
                        e.notes.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>(),
        );
        section(
            &mut lines,
            "iOS source paths to read",
            &entry
                .platforms
                .get("ios")
                .map(|p| p.source.clone())
                .unwrap_or_default(),
        );
        section(
            &mut lines,
            "Test evidence",
            &entry
                .platforms
                .iter()
                .flat_map(|(p, e)| e.tests.iter().map(move |t| format!("{p}: {t}")))
                .collect::<Vec<_>>(),
        );
        section(&mut lines, "Design", &entry.design);
        section(&mut lines, "Notes", &entry.notes);
        if details {
            lines.extend([String::new(), "## Details".into()]);
            if entry.details.is_empty() {
                lines.push("None recorded.".into());
            }
            for detail in &entry.details {
                lines.push(format!(
                    "- `{}#{}` ({}): {}",
                    entry.id,
                    detail.id,
                    detail.kind.as_str(),
                    detail.spec.text
                ));
                let numbers = detail.spec.numbers();
                if !numbers.is_empty() {
                    lines.push(format!("  - Spec: {numbers}"));
                }
                if !detail.baseline.is_empty() {
                    lines.push(format!(
                        "  - Baseline ({}): {}",
                        self.config.report.baseline,
                        detail.baseline.join(", ")
                    ));
                }
                if detail.platforms.is_empty() {
                    lines.push("  - Status: none recorded".into());
                }
                for (platform, record) in &detail.platforms {
                    lines.push(format!(
                        "  - {platform}: {}; {}{}",
                        record.status.as_str(),
                        verification_line(record),
                        record
                            .gap
                            .as_ref()
                            .map(|g| format!(". Gap: {g}"))
                            .unwrap_or_default()
                    ));
                    for reference in record.source.iter().chain(&record.tests) {
                        lines.push(format!("    - {reference}"));
                    }
                }
            }
        }
        Ok(lines.join("\n") + "\n")
    }
}

/// One-line verification summary for briefs.
pub(crate) fn verification_line(record: &Platform) -> String {
    match (&record.verification, record.unverified_reason()) {
        (Some(v), None) => format!(
            "verified by {} on {}{}",
            v.by,
            v.at,
            v.commit
                .as_ref()
                .map(|c| format!(" at {c}"))
                .unwrap_or_default()
        ),
        (_, Some(reason)) => format!("unverified ({reason})"),
        (None, None) => "unverified".into(),
    }
}

fn safe_relative(path: &str, allow_dot: bool) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':', '\0', '#'])
        && !Path::new(path).is_absolute()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)) || (allow_dot && c == Component::CurDir))
}
fn safe_area(area: &str) -> bool {
    !area.is_empty()
        && area
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn is_test_path(reference: &str) -> bool {
    reference.contains(['/', '\\', '#'])
        || Path::new(evidence::split_anchor(reference).0)
            .extension()
            .is_some()
}
