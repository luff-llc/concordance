//! Product behavior catalogs shared by native implementations and their tests.
mod continuous;
mod drift;
pub mod edit;
mod rebase;
pub mod strings;
mod version;
pub use continuous::{
    parse_date, patch_paths, today, waiver_reason, waivers, waivers_markdown, GateFile, GateInput,
    GateReport, ReleaseFinding, SyncChange, SyncOptions, SyncReport, Waiver,
};
pub use version::{VersionReport, VersionResult};
pub mod log;
mod log_fold;
pub use log_fold::Derivation;
mod scan_git;
mod surface_scan;
mod surfaces;
pub use surface_scan::{
    parse_manifest, parse_plist, parse_swift, scan_platform, Declaration, Files, Finding,
    FindingKind, Mapped, ScanReport,
};
pub use surfaces::{default_signals, SignalPattern, SurfaceCell, SurfaceReleaseFinding};
mod set;
mod suite;
mod track;
pub mod tracker;
pub use rebase::{AnchorFate, AnchorMove, AnchorSuggestion, RebaseReport};
pub use set::{SetOutcome, SetRequest, CLEARABLE};
pub use suite::{Suite, SuiteView};
pub use track::{Format as TrackFormat, TrackView};
pub use tracker::{calendar_day, Tracker};
pub mod adopt;
mod audit_plan;
pub use audit_plan::{AuditOptions, AuditResult};
mod evidence;
mod generate;
pub mod inventory;
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

/// Which surface gaps `next_queue` adds to the work queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceFilter {
    None,
    All,
    Only(String),
}

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
    /// Optional releases and issues from `tracker/`; empty when absent.
    pub tracker: Tracker,
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
        for (name, platform) in &config.platforms {
            if let Some(parent) = platform.inherits() {
                let mut seen = BTreeSet::from([name.as_str()]);
                let mut next = Some(parent);
                while let Some(current) = next {
                    if !config.platforms.contains_key(current) {
                        report.error("config", "", name, current, "inherits an unknown platform");
                        break;
                    }
                    if !seen.insert(current) {
                        report.error("config", "", name, current, "platform inheritance cycle");
                        break;
                    }
                    next = config.platforms[current].inherits();
                }
            }
            let root = platform.root();
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
            ("gate", config.gate.keys().collect()),
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
        // A fresh `init` catalog has an empty `catalog/` directory and is valid.
        if files.is_empty() && !nested {
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
            tracker: Tracker::default(),
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
            let order = inheritance_order(&catalog.config);
            for (index, value) in values.iter().enumerate() {
                let mut value = value.clone();
                let copied = inherit(&mut value, &order, &catalog.config.surfaces);
                let value = &value;
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
                let mut entry: Entry = match serde_json::from_value(value.clone()) {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                for (detail, platform) in copied {
                    let record = match detail {
                        None => entry.platforms.get_mut(&platform),
                        Some(i) => entry
                            .details
                            .get_mut(i)
                            .and_then(|d| d.platforms.get_mut(&platform)),
                    };
                    if let Some(record) = record {
                        record.inherited = true;
                    }
                }
                validation::check_entry(&entry, &mut report);
                validation::apply_scope(&mut entry, &catalog.config, &mut report);
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
        catalog.check_work_state(&mut report);
        catalog.check_surfaces(&mut report);
        catalog.load_tracker(&mut report);
        report.sort();
        Ok((catalog, report))
    }

    /// The platform an entry follows: its `lead`, or the report baseline.
    pub fn lead<'a>(&'a self, entry: &'a Entry) -> &'a str {
        entry
            .lead
            .as_deref()
            .unwrap_or(&self.config.report.baseline)
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
        self.next_filtered(platform, area, limit, details, false)
    }

    /// Open blockers (by key) on a record.
    pub fn open_blockers<'a>(&self, record: &'a Platform) -> Vec<&'a str> {
        record
            .blocked_by
            .iter()
            .filter(|k| self.config.blockers.get(*k).is_none_or(|b| b.blocks()))
            .map(String::as_str)
            .collect()
    }

    /// Unfinished entries on a platform that are blocked, with the open blocker keys that
    /// hold them back, directly or through an unfinished dependency.
    pub fn blocked_entries(&self, platform: &str) -> BTreeMap<String, BTreeSet<String>> {
        fn visit(
            catalog: &Catalog,
            platform: &str,
            id: &str,
            memo: &mut BTreeMap<String, BTreeSet<String>>,
            stack: &mut BTreeSet<String>,
        ) -> BTreeSet<String> {
            if let Some(known) = memo.get(id) {
                return known.clone();
            }
            let Some(entry) = catalog.entries.get(id) else {
                return BTreeSet::new();
            };
            let Some(record) = entry.platforms.get(platform) else {
                return BTreeSet::new();
            };
            if !record.status.is_gap() || !stack.insert(id.to_owned()) {
                return BTreeSet::new();
            }
            let mut keys: BTreeSet<String> = catalog
                .open_blockers(record)
                .into_iter()
                .map(str::to_owned)
                .collect();
            for dependency in &entry.depends_on {
                keys.extend(visit(catalog, platform, dependency, memo, stack));
            }
            stack.remove(id);
            memo.insert(id.to_owned(), keys.clone());
            keys
        }
        let mut memo = BTreeMap::new();
        for id in self.entries.keys() {
            visit(self, platform, id, &mut memo, &mut BTreeSet::new());
        }
        memo.retain(|_, keys| !keys.is_empty());
        memo
    }

    /// Like `next_with`; blocked work is skipped unless `include_blocked`. Surface gaps are
    /// not considered (see `next_queue`).
    pub fn next_filtered(
        &self,
        platform: &str,
        area: Option<&str>,
        limit: usize,
        details: bool,
        include_blocked: bool,
    ) -> Result<Vec<&Entry>> {
        self.next_queue(
            platform,
            area,
            limit,
            details,
            include_blocked,
            &SurfaceFilter::None,
        )
    }

    /// Open surface records an entry contributes to the queue under `filter`.
    pub fn queued_surface_gaps<'a>(
        &'a self,
        entry: &'a Entry,
        platform: &'a str,
        filter: &SurfaceFilter,
        include_blocked: bool,
    ) -> Vec<SurfaceCell<'a>> {
        if matches!(filter, SurfaceFilter::None) {
            return Vec::new();
        }
        self.surface_gaps(entry, platform)
            .into_iter()
            .filter(|c| match filter {
                SurfaceFilter::Only(s) => c.surface == s,
                _ => true,
            })
            .filter(|c| {
                include_blocked || c.record.is_none_or(|r| self.open_blockers(r).is_empty())
            })
            .collect()
    }

    /// The work queue: entry gaps, open details with `details`, and surface gaps per
    /// `surfaces`. With `SurfaceFilter::Only`, only entries with a gap on that surface.
    pub fn next_queue(
        &self,
        platform: &str,
        area: Option<&str>,
        limit: usize,
        details: bool,
        include_blocked: bool,
        surfaces: &SurfaceFilter,
    ) -> Result<Vec<&Entry>> {
        if !self.config.platforms.contains_key(platform) {
            return fail(format!("unknown platform: {platform}"));
        }
        if area.is_some_and(|area| !self.area_files.contains_key(area)) {
            return fail(format!("unknown area: {}", area.unwrap_or_default()));
        }
        if let SurfaceFilter::Only(s) = surfaces {
            if !self.config.surfaces.contains_key(s) {
                return fail(format!("unknown surface: {s}"));
            }
        }
        let only = matches!(surfaces, SurfaceFilter::Only(_));
        let surface_open = |e: &Entry, blocked_ok: bool| {
            !self
                .queued_surface_gaps(e, platform, surfaces, blocked_ok)
                .is_empty()
        };
        let mut selected = BTreeSet::new();
        let mut pending: Vec<_> = self
            .entries
            .values()
            .filter(|e| {
                area.is_none_or(|a| self.areas[&e.id] == a)
                    && e.platforms.get(platform).is_some_and(|p| {
                        if only {
                            return surface_open(e, true);
                        }
                        p.status.is_gap()
                            || (details
                                && p.status == Status::Implemented
                                && !self.open_details(e, platform).is_empty())
                            || surface_open(e, true)
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
        let blocked = if include_blocked {
            BTreeMap::new()
        } else {
            self.blocked_entries(platform)
        };
        selected.retain(|id| {
            let entry = &self.entries[id];
            let status = entry.platforms[platform].status;
            if only {
                // Prerequisites stay only if they have their own gap on the surface.
                return surface_open(entry, include_blocked);
            }
            let own = !blocked.contains_key(id)
                && (status.is_gap()
                    || (details
                        && status == Status::Implemented
                        && self.open_details(entry, platform).iter().any(|d| {
                            include_blocked
                                || d.platforms
                                    .get(platform)
                                    .is_none_or(|p| self.open_blockers(p).is_empty())
                        })));
            // An entry that is done in the app but open on a surface: the surface record's
            // own blockers decide, not the entry's.
            own || (status == Status::Implemented && surface_open(entry, include_blocked))
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
                    let blockers: String = e
                        .blocked_by
                        .iter()
                        .map(|k| match self.config.blockers.get(k) {
                            Some(b) => format!(
                                "Blocked by {k}: {}{} [{}]. ",
                                b.title,
                                b.link
                                    .as_ref()
                                    .map(|l| format!(" ({l})"))
                                    .unwrap_or_default(),
                                match b.status {
                                    BlockerStatus::Open => "open",
                                    BlockerStatus::Decided => "decided",
                                    BlockerStatus::Resolved => "resolved",
                                }
                            ),
                            None => format!("Blocked by {k}. "),
                        })
                        .collect();
                    format!(
                        "{p}: {}; {}. {blockers}{} {}",
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

/// Inheriting platforms ordered so parents resolve before children; cycles are skipped.
fn inheritance_order(config: &Config) -> Vec<(String, String)> {
    let mut pending: Vec<(String, String)> = config
        .platforms
        .iter()
        .filter_map(|(name, p)| p.inherits().map(|parent| (name.clone(), parent.to_owned())))
        .filter(|(_, parent)| config.platforms.contains_key(parent))
        .collect();
    let mut order = Vec::new();
    while !pending.is_empty() {
        let before = pending.len();
        let children: BTreeSet<String> = pending.iter().map(|(c, _)| c.clone()).collect();
        pending.retain(|(child, parent)| {
            if children.contains(parent) {
                return true; // the parent still needs resolving
            }
            order.push((child.clone(), parent.clone()));
            false
        });
        if pending.len() == before {
            break; // cycle; reported by configuration validation
        }
    }
    order
}

/// Fill inherited platform records in a raw entry. A missing child record is copied whole
/// from its parent; a present one is merged over the parent's (child keys win, and a key
/// set to `null` removes the parent's value). When the child's status differs, the parent's
/// verification, gap, blockers and stale bookkeeping are not inherited. Surface records are
/// inherited only for surfaces that list the child platform, and merge per surface. A child
/// outside the entry's `applies_to` inherits nothing.
/// Returns the whole copies as (detail index, platform).
fn inherit(
    entry: &mut serde_json::Value,
    order: &[(String, String)],
    surfaces: &BTreeMap<String, SurfaceConfig>,
) -> Vec<(Option<usize>, String)> {
    use serde_json::{Map, Value};
    /// Fields that describe the parent's own status, not the child's.
    const STATUS_BOUND: &[&str] = &[
        "verification",
        "gap",
        "blocked_by",
        "previous_status",
        "since_commit",
        "since_date",
    ];
    fn merge(own: &mut Map<String, Value>, base: &Map<String, Value>) {
        let own_status = own.get("status").cloned();
        let differs = own_status.is_some() && own_status.as_ref() != base.get("status");
        for (key, value) in base {
            if differs && STATUS_BOUND.contains(&key.as_str()) {
                continue;
            }
            if key == "surfaces" {
                continue; // merged per surface by the caller
            }
            own.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    /// The parent's surface records the child may have.
    fn surface_base(
        base: &Map<String, Value>,
        child: &str,
        surfaces: &BTreeMap<String, SurfaceConfig>,
    ) -> Option<Map<String, Value>> {
        let records = base.get("surfaces")?.as_object()?;
        let kept: Map<String, Value> = records
            .iter()
            .filter(|(id, _)| {
                // Undeclared IDs pass through so validation can report them on the parent.
                surfaces
                    .get(*id)
                    .is_none_or(|s| s.platforms.iter().any(|p| p == child))
            })
            .map(|(id, v)| (id.clone(), v.clone()))
            .collect();
        (!kept.is_empty()).then_some(kept)
    }
    fn apply(
        platforms: Option<&mut Value>,
        order: &[(String, String)],
        scope: Option<&BTreeSet<String>>,
        surfaces: &BTreeMap<String, SurfaceConfig>,
        copied: &mut Vec<String>,
    ) {
        let Some(map) = platforms.and_then(Value::as_object_mut) else {
            return;
        };
        for (child, parent) in order {
            if scope.is_some_and(|s| !s.contains(child)) {
                continue;
            }
            let Some(base) = map.get(parent).and_then(Value::as_object).cloned() else {
                continue;
            };
            let base_surfaces = surface_base(&base, child, surfaces);
            if !map.contains_key(child) {
                let mut record = base.clone();
                record.remove("surfaces");
                if let Some(kept) = base_surfaces {
                    record.insert("surfaces".into(), Value::Object(kept));
                }
                map.insert(child.clone(), Value::Object(record));
                copied.push(child.clone());
                continue;
            }
            let Some(own) = map.get_mut(child).and_then(Value::as_object_mut) else {
                continue;
            };
            merge(own, &base);
            // `surfaces: null` drops every inherited surface record.
            let explicit_null = own.get("surfaces").is_some_and(Value::is_null);
            if let (Some(kept), false) = (base_surfaces, explicit_null) {
                let target = own
                    .entry("surfaces")
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(target) = target.as_object_mut() {
                    for (id, parent_record) in kept {
                        match target.get_mut(&id) {
                            None => {
                                target.insert(id, parent_record);
                            }
                            Some(Value::Object(own_record)) => {
                                if let Value::Object(parent_record) = &parent_record {
                                    merge(own_record, parent_record);
                                }
                                own_record.retain(|_, v| !v.is_null());
                            }
                            Some(_) => {} // null: this surface is not inherited
                        }
                    }
                    target.retain(|_, v| !v.is_null());
                }
            }
            // `surfaces: {id: null}` with nothing to inherit for that surface.
            if let Some(Value::Object(target)) = own.get_mut("surfaces") {
                target.retain(|_, v| !v.is_null());
            }
            // An explicit null unsets a parent key.
            own.retain(|_, value| !value.is_null());
        }
    }
    let mut out = Vec::new();
    if order.is_empty() {
        return out;
    }
    let scope: Option<BTreeSet<String>> =
        entry.get("applies_to").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        });
    let mut copied = Vec::new();
    apply(
        entry.get_mut("platforms"),
        order,
        scope.as_ref(),
        surfaces,
        &mut copied,
    );
    out.extend(copied.drain(..).map(|p| (None, p)));
    if let Some(details) = entry.get_mut("details").and_then(Value::as_array_mut) {
        for (index, detail) in details.iter_mut().enumerate() {
            apply(
                detail.get_mut("platforms"),
                order,
                scope.as_ref(),
                surfaces,
                &mut copied,
            );
            out.extend(copied.drain(..).map(|p| (Some(index), p)));
        }
    }
    out
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
