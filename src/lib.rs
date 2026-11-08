//! Product behavior catalogs shared by native implementations and their tests.
mod generate;
mod model;
mod report;
pub mod schema;

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
        let directory = directory.as_ref().to_path_buf();
        let config_path = directory.join("catalog.toml");
        let config: Config = toml::from_str(
            &fs::read_to_string(&config_path)
                .map_err(|e| format!("{}: {e}", config_path.display()))?,
        )?;
        if config.platforms.is_empty() {
            return fail("catalog.toml: platforms must not be empty");
        }
        for (name, root) in &config.platforms {
            if name.is_empty() || !safe_relative(root, true) {
                return fail(format!(
                    "catalog.toml: unsafe platform root {name} = {root:?}"
                ));
            }
        }
        for (name, prefix) in &config.path_prefixes {
            if !config.platforms.contains_key(name)
                || !prefix.ends_with('/')
                || !safe_relative(prefix, false)
            {
                return fail(format!("catalog.toml: invalid path prefix for {name}"));
            }
        }
        if Path::new(&config.repo_root).is_absolute() {
            return fail("catalog.toml: repo_root must be relative to the catalog directory");
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
        files.retain(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml")));
        files.sort();
        if files.is_empty() {
            return fail(format!("no catalog YAML files in {}", data_dir.display()));
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
                return fail(format!("invalid area filename: {}", file.display()));
            }
            if catalog.area_files.contains_key(&area) {
                return fail(format!("duplicate area filename: {area}"));
            }
            let name = file
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("non-UTF8 filename")?;
            catalog.area_files.insert(
                area.clone(),
                format!("{}{name}", if nested { "catalog/" } else { "" }),
            );
            // Deserialize YAML first: unlike serde_json::Value, this rejects duplicate map keys.
            let value: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(&file)?)
                .map_err(|e| format!("{}: {e}", file.display()))?;
            let value = serde_json::to_value(value)?;
            schema::validate(&value).map_err(|e| format!("{}: {e}", file.display()))?;
            let entries: Vec<Entry> = serde_json::from_value(value)?;
            for entry in entries {
                for platform in entry.platforms.keys() {
                    if !catalog.config.platforms.contains_key(platform) {
                        return fail(format!(
                            "{}: platform {platform} has no configured repo root",
                            entry.id
                        ));
                    }
                }
                if catalog.entries.contains_key(&entry.id) {
                    return fail(format!("duplicate ID: {}", entry.id));
                }
                catalog.areas.insert(entry.id.clone(), area.clone());
                catalog.entries.insert(entry.id.clone(), entry);
            }
        }
        for entry in catalog.entries.values() {
            for dependency in &entry.depends_on {
                if !catalog.entries.contains_key(dependency) {
                    return fail(format!("{}: unresolved depends_on {dependency}", entry.id));
                }
            }
        }
        catalog.dependency_order(None)?;
        catalog.validate_retired()?;
        Ok(catalog)
    }

    pub fn entry(&self, id: &str) -> Result<&Entry> {
        self.entries
            .get(id)
            .ok_or_else(|| format!("unknown entry: {id}").into())
    }

    fn validate_retired(&self) -> Result<()> {
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
        let value: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(path)?)?;
        let retired: BTreeMap<String, Retirement> = serde_yaml::from_value(value)?;
        for (id, record) in retired {
            if !schema::valid_id(&id) || record.reason.trim().is_empty() {
                return fail(format!("invalid retirement record: {id}"));
            }
            if self.entries.contains_key(&id) {
                return fail(format!("retired ID cannot be reused: {id}"));
            }
            for replacement in record.replaced_by {
                if !self.entries.contains_key(&replacement) {
                    return fail(format!(
                        "{id}: unresolved retired-ID replacement {replacement}"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Check referenced source/test files, including unqualified test file paths.
    /// `path#Suite.method` checks the file, not language declarations or test coverage.
    pub fn validate_paths(&self, repo: Option<&Path>) -> Result<()> {
        let repo = repo
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.directory.join(&self.config.repo_root))
            .canonicalize()?;
        let mut checked = BTreeSet::new();
        for entry in self.entries.values() {
            for (platform, evidence) in &entry.platforms {
                for reference in evidence
                    .source
                    .iter()
                    .chain(evidence.tests.iter().filter(|s| is_test_path(s)))
                {
                    if !checked.insert((platform, reference)) {
                        continue;
                    }
                    let relative = reference
                        .split_once('#')
                        .map(|(p, _)| p)
                        .unwrap_or(reference);
                    let relative = if let Some(prefix) = self.config.path_prefixes.get(platform) {
                        relative.strip_prefix(prefix).ok_or_else(|| {
                            format!(
                                "{}: {platform} path must start with {prefix}: {reference}",
                                entry.id
                            )
                        })?
                    } else {
                        relative
                    };
                    if !safe_relative(relative, false) {
                        return fail(format!(
                            "{}: unsafe {platform} evidence path: {reference}",
                            entry.id
                        ));
                    }
                    let root = repo
                        .join(&self.config.platforms[platform])
                        .canonicalize()
                        .map_err(|e| format!("{platform} root: {e}"))?;
                    if !root.starts_with(&repo) {
                        return fail(format!("{platform} root escapes repo: {}", root.display()));
                    }
                    let target = root.join(relative).canonicalize().map_err(|e| {
                        format!("{}: missing {platform} evidence {reference}: {e}", entry.id)
                    })?;
                    if !target.starts_with(&root) || !target.is_file() {
                        return fail(format!(
                            "{}: evidence is not a file within {platform} root: {reference}",
                            entry.id
                        ));
                    }
                }
            }
        }
        Ok(())
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
                    && e.platforms.get(platform).is_some_and(|p| p.status.is_gap())
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
        selected.retain(|id| self.entries[id].platforms[platform].status.is_gap());
        Ok(self
            .dependency_order(Some(&selected))?
            .into_iter()
            .filter(|e| selected.contains(&e.id))
            .take(limit)
            .collect())
    }

    pub fn brief(&self, id: &str) -> Result<String> {
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
                        "{p}: {}. {} {}",
                        e.status.as_str(),
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
        Ok(lines.join("\n") + "\n")
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
    reference.contains(['/', '\\', '#']) || Path::new(reference).extension().is_some()
}
