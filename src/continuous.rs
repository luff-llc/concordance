//! Continuous parity: `gate` on every merge, `sync` after baseline merges, and `gate-release`.
use crate::drift::{git, glob, Precision};
use crate::edit::edit_file;
use crate::set::{key, record_mut, split_target, surface_of};
use crate::{fail, Catalog, DetailKind, GateMode, GateModes, Result, RootOptions, Status};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What `gate` inspects: a commit range, or a patch with its commit message.
#[derive(Debug, Clone)]
pub enum GateInput {
    Range { base: String, head: String },
    Patch { patch: String, message: String },
}

/// Repository-relative paths touched by a unified diff (both sides of renames).
pub fn patch_paths(patch: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for line in patch.lines() {
        let path = line
            .strip_prefix("+++ ")
            .or_else(|| line.strip_prefix("--- "))
            .map(|p| p.split('\t').next().unwrap_or(p).trim());
        if let Some(path) = path {
            if path == "/dev/null" {
                continue;
            }
            let path = path.trim_matches('"');
            let path = path
                .strip_prefix("a/")
                .or_else(|| path.strip_prefix("b/"))
                .unwrap_or(path);
            out.insert(path.to_owned());
        } else if let Some(rest) = line
            .strip_prefix("rename from ")
            .or_else(|| line.strip_prefix("rename to "))
        {
            out.insert(rest.trim().to_owned());
        }
    }
    out.into_iter().collect()
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
pub fn parse_date(text: &str) -> Option<i64> {
    if !crate::schema::valid_date(text) {
        return None;
    }
    Some(days_from_civil(
        text[0..4].parse().ok()?,
        text[5..7].parse().ok()?,
        text[8..10].parse().ok()?,
    ))
}
/// Today's UTC date as YYYY-MM-DD.
pub fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86_400)
        .unwrap_or(0);
    // Inverse of days_from_civil.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn resolve(root: &Path, revision: &str) -> Result<String> {
    let out = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
    )?;
    Ok(String::from_utf8(out)?.trim().to_owned())
}
fn lines(bytes: Vec<u8>) -> Vec<String> {
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct GateFile {
    pub file: String,
    pub entries: Vec<String>,
    pub covered: bool,
    pub reason: String,
}
#[derive(Debug, Serialize)]
pub struct GateReport {
    pub platform: String,
    pub base: String,
    pub head: String,
    pub changed: usize,
    pub user_visible: Vec<GateFile>,
    pub catalog_changed: Vec<String>,
    pub waiver: Option<String>,
    /// Declared-surface findings (new uncatalogued or unmapped declarations, removed
    /// declarations the catalog still claims); empty without `[surface_scan]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<crate::Finding>,
    pub modes: GateModes,
    /// A `report`-mode check found something that `enforce` would fail.
    pub would_fail: bool,
    pub passed: bool,
}
impl GateReport {
    pub fn markdown(&self) -> String {
        let unmatched: Vec<&GateFile> = self.user_visible.iter().filter(|f| !f.covered).collect();
        let mut out = format!(
            "## Catalog gate: {} `{}`..`{}`\n\n**{}.** {} changed files, {} user-visible, {} unmatched; {} catalog files changed.\n",
            self.platform,
            short(&self.base),
            short(&self.head),
            if self.passed { "Passed" } else { "Failed" },
            self.changed,
            self.user_visible.len(),
            unmatched.len(),
            self.catalog_changed.len()
        );
        out.push_str(&format!(
            "\nModes: files {}, surfaces {}.\n",
            self.modes.files.as_str(),
            self.modes.surfaces.as_str()
        ));
        if self.would_fail {
            out.push_str("\n**Report only:** an enforced gate would fail on the findings below.\n");
        }
        if let Some(reason) = &self.waiver {
            out.push_str(&format!(
                "\nWaived by commit trailer: `Catalog: none — {reason}`.\n"
            ));
        }
        if !unmatched.is_empty() {
            out.push_str("\n### Unmatched user-visible files\n\nUpdate the catalog entries below in the same change, or add a `Catalog: none — <reason>` commit trailer.\n\n");
            for f in &unmatched {
                out.push_str(&format!("- `{}`: {}\n", f.file, f.reason));
            }
        }
        if !self.surfaces.is_empty() {
            out.push_str("\n### Declared surfaces\n\nCatalog each new surface declaration (an entry that names the surface, with a surface record whose `declared` lists the ID), map it to a `[surfaces]` ID, or update the records that claim a removed one.\n\n");
            for f in &self.surfaces {
                out.push_str(&format!(
                    "- {}: `{}`{}{}: {}\n",
                    match f.kind {
                        crate::FindingKind::Uncatalogued => "new, uncatalogued",
                        crate::FindingKind::Unmapped => "new, unmapped",
                        crate::FindingKind::StatusDisagrees => "new, but recorded as not there",
                        _ => "removed",
                    },
                    f.subject,
                    f.surface
                        .as_ref()
                        .map(|s| format!(" (`{s}`)"))
                        .unwrap_or_default(),
                    f.location
                        .as_ref()
                        .map(|l| format!(" at `{l}`"))
                        .unwrap_or_default(),
                    f.message
                ));
            }
        }
        let covered: Vec<&GateFile> = self.user_visible.iter().filter(|f| f.covered).collect();
        if !covered.is_empty() {
            out.push_str("\n### Covered\n\n");
            for f in covered {
                out.push_str(&format!("- `{}`: {}\n", f.file, f.reason));
            }
        }
        out
    }
}
/// The reason in the first `Catalog: none — <reason>` trailer of a commit message.
pub fn waiver_reason(message: &str) -> Option<String> {
    message.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Catalog:")?.trim_start();
        let rest = rest.strip_prefix("none")?.trim_start();
        let rest = rest.trim_start_matches(['—', '–', '-', ':']).trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    })
}

/// One commit that waived the catalog gate.
#[derive(Debug, Clone, Serialize)]
pub struct Waiver {
    pub commit: String,
    pub date: String,
    pub subject: String,
    pub reason: String,
}

/// Every `Catalog: none` trailer in `base..head` of the repository at `root`, oldest first.
pub fn waivers(root: &Path, base: &str, head: &str) -> Result<Vec<Waiver>> {
    let (base, head) = (resolve(root, base)?, resolve(root, head)?);
    let log = git(
        root,
        &[
            "log",
            "--reverse",
            "--format=%H%x1f%cs%x1f%s%x1f%B%x1e",
            &format!("{base}..{head}"),
        ],
    )?;
    Ok(String::from_utf8_lossy(&log)
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').splitn(4, '\x1f');
            let commit = fields.next()?.to_owned();
            let date = fields.next()?.to_owned();
            let subject = fields.next()?.to_owned();
            let reason = waiver_reason(fields.next()?)?;
            Some(Waiver {
                commit,
                date,
                subject,
                reason,
            })
        })
        .collect())
}

/// Markdown: the waivers, then how often each reason (case-insensitive) recurs.
pub fn waivers_markdown(list: &[Waiver]) -> String {
    let mut out = format!(
        "## Catalog gate waivers\n\n{} commit(s) carry a `Catalog: none` trailer.\n",
        list.len()
    );
    if list.is_empty() {
        return out;
    }
    out.push_str("\n| Commit | Date | Subject | Reason |\n| --- | --- | --- | --- |\n");
    for w in list {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            short(&w.commit),
            w.date,
            w.subject.replace('|', "\\|"),
            w.reason.replace('|', "\\|")
        ));
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for w in list {
        *counts.entry(w.reason.to_lowercase()).or_default() += 1;
    }
    let mut recurring: Vec<(String, usize)> = counts.into_iter().filter(|(_, n)| *n > 1).collect();
    recurring.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if !recurring.is_empty() {
        out.push_str("\n### Recurring reasons\n\n");
        for (reason, n) in recurring {
            out.push_str(&format!("- {n}× {reason}\n"));
        }
    }
    out
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// One record `sync` changed or would change.
#[derive(Debug, Clone, Serialize)]
pub struct SyncChange {
    /// `entry` or `entry#detail`.
    pub label: String,
    pub before: String,
    pub after: String,
    pub hits: Vec<String>,
}
#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub from: String,
    pub to_platform: String,
    pub since: String,
    pub to: String,
    pub date: String,
    pub applied: bool,
    pub changes: Vec<SyncChange>,
    pub tasks: Vec<String>,
    pub issues: Vec<String>,
    pub anchors: String,
    pub baseline_advanced: bool,
}
impl SyncReport {
    pub fn markdown(&self) -> String {
        let count = |s: &str| {
            self.changes
                .iter()
                .filter(|c| c.after == s && c.before != s)
                .count()
        };
        let mut out = format!(
            "## Sync {} → {}: `{}` → `{}`{}\n\n{} records with changed baseline evidence: {} newly stale, {} already stale, {} queued for review (file-level hits only), {} unchanged (not implemented).\n",
            self.from,
            self.to_platform,
            short(&self.since),
            short(&self.to),
            if self.applied { "" } else { " (dry run; pass --apply to write)" },
            self.changes.len(),
            count("stale"),
            self.changes.iter().filter(|c| c.before == "stale").count(),
            self.changes.iter().filter(|c| c.after == "review").count(),
            self.changes.iter().filter(|c| c.after != "stale" && c.after != "review").count()
        );
        if !self.changes.is_empty() {
            out.push_str(&format!(
                "\n| Record | {} before | After | Evidence |\n| --- | --- | --- | --- |\n",
                self.to_platform
            ));
            for c in &self.changes {
                out.push_str(&format!(
                    "| `{}` | {} | {} | {} |\n",
                    c.label,
                    c.before,
                    c.after,
                    c.hits.join("; ").replace('|', "\\|")
                ));
            }
        }
        for (title, list) in [("Tasks", &self.tasks), ("Issues", &self.issues)] {
            if !list.is_empty() {
                out.push_str(&format!("\n### {title}\n\n"));
                for item in list {
                    out.push_str(&format!("- {item}\n"));
                }
            }
        }
        if !self.anchors.is_empty() {
            out.push_str(&format!("\n{}", self.anchors));
        }
        out.push_str(&format!(
            "\nBaseline pin {}.\n",
            if self.baseline_advanced {
                format!("advanced to `{}`", self.to)
            } else if self.since == self.to {
                "already current".into()
            } else {
                format!("would advance to `{}`", self.to)
            }
        ));
        out
    }
}

#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    pub from: String,
    pub to_platform: String,
    pub to: String,
    pub tasks_dir: Option<PathBuf>,
    pub captures: Option<PathBuf>,
    /// Open or update GitHub issues with `gh`.
    pub issues: bool,
    pub label: String,
    pub apply: bool,
    /// Mark file-level (coarse) hits stale too, instead of queueing them for review.
    pub coarse_as_stale: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseFinding {
    pub label: String,
    pub since_date: Option<String>,
    pub age_days: Option<i64>,
    pub limit_days: u32,
}

fn commit_url(catalog: &Catalog, platform: &str, sha: &str) -> Option<String> {
    match catalog.config.external_roots.get(platform) {
        Some(crate::ExternalRoot::Details { git: Some(url), .. })
            if url.starts_with("https://github.com/") =>
        {
            Some(format!("{}/commit/{sha}", url.trim_end_matches(".git")))
        }
        _ => None,
    }
}

impl Catalog {
    /// Fail when user-visible files changed without a covering catalog change.
    pub fn gate(
        &self,
        options: &RootOptions,
        platform: &str,
        base: &str,
        head: &str,
    ) -> Result<GateReport> {
        self.gate_input(
            options,
            platform,
            &GateInput::Range {
                base: base.into(),
                head: head.into(),
            },
        )
    }

    /// `gate` over a commit range or a patch file (paths relative to the repository top).
    pub fn gate_input(
        &self,
        options: &RootOptions,
        platform: &str,
        input: &GateInput,
    ) -> Result<GateReport> {
        self.gate_scoped(options, platform, input, None)
    }

    /// The modes `gate` applies on a platform: `[gate.<platform>]` over the defaults.
    pub fn gate_modes(&self, platform: &str) -> GateModes {
        let configured = self.config.gate.get(platform).copied().unwrap_or_default();
        let files_default = match self.config.source_globs.get(platform) {
            Some(crate::SourceGlobs::All(all)) if !all.is_empty() => GateMode::Enforce,
            Some(g) if !g.user_visible().is_empty() || !g.strings().is_empty() => GateMode::Enforce,
            _ => GateMode::Off,
        };
        let surfaces_default = if self.config.surface_scan.contains_key(platform) {
            GateMode::Enforce
        } else {
            GateMode::Off
        };
        GateModes {
            files: configured.files.unwrap_or(files_default),
            surfaces: configured.surfaces.unwrap_or(surfaces_default),
        }
    }

    /// `gate_input` with explicit modes (default: `gate_modes`).
    pub fn gate_scoped(
        &self,
        options: &RootOptions,
        platform: &str,
        input: &GateInput,
        modes: Option<GateModes>,
    ) -> Result<GateReport> {
        let modes = modes.unwrap_or_else(|| self.gate_modes(platform));
        if modes.surfaces != GateMode::Off && !self.config.surface_scan.contains_key(platform) {
            return fail(format!(
                "{platform}: checking surfaces needs [surface_scan.{platform}] in catalog.toml"
            ));
        }
        let root = self.platform_root(platform, options)?.ok_or_else(|| {
            format!("{platform}: root not supplied; use --root {platform}=<path>")
        })?;
        let top = |dir: &Path| -> Result<String> {
            Ok(
                String::from_utf8(git(dir, &["rev-parse", "--show-toplevel"])?)?
                    .trim()
                    .to_owned(),
            )
        };
        let prefix = |dir: &Path| -> Result<String> {
            Ok(
                String::from_utf8(git(dir, &["rev-parse", "--show-prefix"])?)?
                    .trim_end_matches('\n')
                    .to_owned(),
            )
        };
        let root_prefix = prefix(&root)?;
        let same_repo = top(&root)? == top(&self.directory).unwrap_or_default();
        let catalog_prefix = if same_repo {
            prefix(&self.directory)?
        } else {
            String::new()
        };
        // Repository-relative changed paths and the commit messages to search for a waiver.
        let (base, head, repo_changed, messages, catalog_repo_changed) = match input {
            GateInput::Range { base, head } => {
                let (base, head) = (resolve(&root, base)?, resolve(&root, head)?);
                let names = lines(git(
                    &root,
                    &[
                        "diff",
                        "--name-only",
                        "--no-renames",
                        &base,
                        &head,
                        "--",
                        ":/",
                    ],
                )?);
                let messages = String::from_utf8_lossy(&git(
                    &root,
                    &["log", "--format=%B", &format!("{base}..{head}")],
                )?)
                .into_owned();
                let catalog = if same_repo {
                    names.clone()
                } else {
                    let catalog_top = prefix(&self.directory)?;
                    lines(
                        git(
                            &self.directory,
                            &["diff", "--name-only", &base, &head, "--", ":/"],
                        )
                        .map_err(|e| {
                            format!("catalog repository: {e} (the range must exist there too)")
                        })?,
                    )
                    .into_iter()
                    .filter(|p| p.starts_with(&catalog_top))
                    .collect()
                };
                (base, head, names, messages, catalog)
            }
            GateInput::Patch { patch, message } => {
                let names = patch_paths(patch);
                let catalog = if same_repo { names.clone() } else { Vec::new() };
                (
                    "patch".to_owned(),
                    "working tree".to_owned(),
                    names,
                    message.clone(),
                    catalog,
                )
            }
        };
        let changed: Vec<String> = repo_changed
            .iter()
            .filter_map(|p| p.strip_prefix(root_prefix.as_str()).map(str::to_owned))
            .collect();
        let catalog_prefix = if same_repo {
            catalog_prefix
        } else {
            prefix(&self.directory)?
        };
        let catalog_changed: Vec<String> = catalog_repo_changed
            .iter()
            .filter_map(|p| p.strip_prefix(catalog_prefix.as_str()).map(str::to_owned))
            .collect();
        let globs = self.config.source_globs.get(platform);
        let mut patterns: Vec<&String> = globs
            .map(|g| g.user_visible().iter().chain(g.strings()).collect())
            .unwrap_or_default();
        if patterns.is_empty() {
            patterns = globs.map(|g| g.all().iter().collect()).unwrap_or_default();
        }
        let mut cited: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        // Inherited copies count: a flavour's file is cited through its parent's record.
        for (label, p, record) in self.records() {
            if p != platform || record.out_of_scope {
                continue;
            }
            for reference in crate::validation::evidence_refs(record) {
                let path = self.evidence_relative(platform, reference)?.to_owned();
                cited
                    .entry(path)
                    .or_default()
                    .insert(split_target(&label).0.to_owned());
            }
        }
        if platform == self.config.report.baseline {
            for entry in self.entries.values() {
                for detail in &entry.details {
                    for reference in &detail.baseline {
                        let path = self.evidence_relative(platform, reference)?.to_owned();
                        cited.entry(path).or_default().insert(entry.id.clone());
                    }
                }
            }
        }
        let changed_areas: BTreeSet<&String> = self
            .area_files
            .iter()
            .filter(|(_, file)| catalog_changed.contains(file))
            .map(|(area, _)| area)
            .collect();
        let any_area = !changed_areas.is_empty();
        if modes.files == GateMode::Off {
            patterns.clear();
        }
        // Enforced file checks are judged per entry (range mode): a cited file needs one of
        // its entries to change, and an uncited one needs a trailer.
        let strict = modes.files == GateMode::Enforce;
        let changed_entries = match input {
            GateInput::Range { .. } if strict => {
                Some(self.changed_entries(&catalog_changed, &base, &head))
            }
            _ => None,
        };
        let mut user_visible = Vec::new();
        for file in &changed {
            if !patterns.iter().any(|g| glob(g.as_bytes(), file.as_bytes())) {
                continue;
            }
            let entries: Vec<String> = cited
                .get(file)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            let touched: Vec<&String> = entries
                .iter()
                .filter(|e| match &changed_entries {
                    Some(changed) => changed.contains(*e),
                    None => self
                        .areas
                        .get(*e)
                        .is_some_and(|a| changed_areas.contains(a)),
                })
                .collect();
            let (covered, reason) = match (entries.is_empty(), touched.is_empty()) {
                (true, _) if changed_entries.is_some() => (
                    false,
                    "not cited by any entry; cite it from an entry or add a `Catalog: none` trailer"
                        .to_owned(),
                ),
                (true, _) if any_area => (
                    true,
                    "not cited; catalog entries changed in range".to_owned(),
                ),
                (true, _) => (
                    false,
                    "not cited by any entry; no catalog entry changed".to_owned(),
                ),
                (false, false) => (
                    true,
                    format!(
                        "catalog updated for {}",
                        touched
                            .iter()
                            .map(|e| format!("`{e}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
                (false, true) if changed_entries.is_some() => (
                    false,
                    format!(
                        "cited by {}, none of which changed",
                        entries
                            .iter()
                            .map(|e| format!("`{e}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
                (false, true) => (
                    false,
                    format!(
                        "cited by {}, whose catalog files did not change",
                        entries
                            .iter()
                            .map(|e| format!("`{e}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
            };
            user_visible.push(GateFile {
                file: file.clone(),
                entries,
                covered,
                reason,
            });
        }
        let waiver = waiver_reason(&messages);
        let surfaces = if modes.surfaces != GateMode::Off {
            let (head_rev, base_rev, touched) = match input {
                GateInput::Range { .. } => (Some(head.as_str()), base.as_str(), None),
                GateInput::Patch { .. } => (None, "HEAD", Some(changed.as_slice())),
            };
            self.gate_surfaces(options, platform, head_rev, base_rev, touched)?
        } else {
            Vec::new()
        };
        let files_ok = waiver.is_some() || user_visible.iter().all(|f| f.covered);
        let surfaces_ok = waiver.is_some() || surfaces.is_empty();
        let fails = |mode: GateMode, ok: bool| !ok && mode == GateMode::Enforce;
        let reports = |mode: GateMode, ok: bool| !ok && mode == GateMode::Report;
        let passed = !fails(modes.files, files_ok) && !fails(modes.surfaces, surfaces_ok);
        let would_fail = reports(modes.files, files_ok) || reports(modes.surfaces, surfaces_ok);
        Ok(GateReport {
            platform: platform.into(),
            base,
            head,
            changed: changed.len(),
            user_visible,
            catalog_changed,
            waiver,
            surfaces,
            modes,
            would_fail,
            passed,
        })
    }

    /// IDs of entries whose content differs between two revisions of the changed area files
    /// (catalog-relative paths), including added and removed entries.
    fn changed_entries(&self, changed: &[String], base: &str, head: &str) -> BTreeSet<String> {
        let read = |rev: &str, file: &str| -> BTreeMap<String, serde_json::Value> {
            git(&self.directory, &["show", &format!("{rev}:./{file}")])
                .ok()
                .and_then(|bytes| serde_yaml::from_slice::<serde_yaml::Value>(&bytes).ok())
                .and_then(|v| serde_json::to_value(v).ok())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| Some((e.get("id")?.as_str()?.to_owned(), e)))
                .collect()
        };
        let mut out = BTreeSet::new();
        for file in changed
            .iter()
            .filter(|f| self.area_files.values().any(|a| a == *f))
        {
            let (before, after) = (read(base, file), read(head, file));
            for id in before.keys().chain(after.keys()) {
                if before.get(id) != after.get(id) {
                    out.insert(id.clone());
                }
            }
        }
        out
    }

    /// Surface findings that a change introduces: declarations new at head that are
    /// uncatalogued or unmapped, and base declarations gone at head that a record claims.
    fn gate_surfaces(
        &self,
        options: &RootOptions,
        platform: &str,
        head: Option<&str>,
        base: &str,
        touched: Option<&[String]>,
    ) -> Result<Vec<crate::Finding>> {
        use crate::FindingKind;
        let config = &self.config.surface_scan[platform];
        let head_decls =
            crate::scan_platform(config, &self.surface_files(options, platform, head)?)?;
        // With a patch the working tree may hold other work: only declarations from files the
        // patch touches count as new.
        let in_patch: BTreeSet<String> = head_decls
            .iter()
            .filter(|d| touched.is_none_or(|t| t.iter().any(|p| *p == d.file || *p == d.origin)))
            .map(crate::Declaration::key)
            .collect();
        let base_decls =
            crate::scan_platform(config, &self.surface_files(options, platform, Some(base))?)?;
        let base_keys: BTreeSet<String> = base_decls.iter().map(crate::Declaration::key).collect();
        let head_keys: BTreeSet<String> = head_decls.iter().map(crate::Declaration::key).collect();
        let head_report = self.compare_surfaces(platform, head_decls);
        let mut out: Vec<crate::Finding> = head_report
            .findings
            .iter()
            .filter(|f| {
                matches!(
                    f.kind,
                    FindingKind::Uncatalogued
                        | FindingKind::Unmapped
                        | FindingKind::StatusDisagrees
                )
            })
            .filter(|f| {
                f.declaration
                    .as_ref()
                    .is_some_and(|d| !base_keys.contains(d) && in_patch.contains(d))
            })
            .cloned()
            .collect();
        // Removed declarations: claims that the base satisfied and the head does not.
        let base_report = self.compare_surfaces(
            platform,
            base_decls
                .into_iter()
                .filter(|d| !head_keys.contains(&d.key()))
                .collect(),
        );
        let removed_ids: BTreeSet<(String, String)> = base_report
            .declarations
            .iter()
            .flat_map(|m| {
                m.surfaces
                    .iter()
                    .map(|s| (s.clone(), m.declaration.id.clone()))
            })
            .collect();
        for f in head_report
            .findings
            .iter()
            .filter(|f| f.kind == FindingKind::NoLongerDeclared)
        {
            let surface = f.surface.clone().unwrap_or_default();
            // An exact record: was this ID declared at base? A coarse one: did the change
            // remove the surface's last declaration?
            let was_there = match &f.declared_id {
                Some(id) => removed_ids.contains(&(surface.clone(), id.clone())),
                None => removed_ids.iter().any(|(s, _)| *s == surface),
            };
            if was_there {
                out.push(f.clone());
            }
        }
        Ok(out)
    }

    /// Mark target records stale where the source platform's baseline evidence changed,
    /// write tasks and issues, rebase anchors, and advance the source pin. Dry run unless
    /// `apply`.
    pub fn sync(&self, options: &RootOptions, sync: &SyncOptions) -> Result<SyncReport> {
        let (from, target) = (sync.from.as_str(), sync.to_platform.as_str());
        if !self.config.platforms.contains_key(target) {
            return fail(format!("unknown platform: {target}"));
        }
        let since = self
            .baseline_commit(from)
            .ok_or_else(|| format!("{from}: no [baselines.{from}] commit to sync from"))?
            .to_owned();
        let root = self
            .platform_root(from, options)?
            .ok_or_else(|| format!("{from}: root not supplied; use --root {from}=<path>"))?;
        let since_sha = resolve(&root, &since)?;
        let to = resolve(&root, &sync.to)?;
        let date = String::from_utf8(git(&root, &["log", "-1", "--format=%cs", &to])?)?
            .trim()
            .to_owned();
        let mut report = SyncReport {
            from: from.into(),
            to_platform: target.into(),
            since: since_sha.clone(),
            to: to.clone(),
            date: date.clone(),
            applied: sync.apply,
            ..Default::default()
        };
        if since_sha == to {
            return Ok(report);
        }
        let drift = self.drift(options, Some(from), Some(&since_sha), &to)?;
        let Some(platform_drift) = drift.platforms.first() else {
            return fail(drift.unchecked.join("; "));
        };
        // Planned edits, grouped by area file.
        #[allow(clippy::type_complexity)]
        let mut edits: BTreeMap<
            String,
            Vec<(String, Option<String>, Option<String>, Status)>,
        > = BTreeMap::new();
        // Interface copy whose old text disappeared in the range (code or string catalogs).
        let full = String::from_utf8_lossy(&git(
            &root,
            &["diff", "-U0", "--no-color", &since_sha, &to, "--", "."],
        )?)
        .into_owned();
        let (mut removed, mut added) = (BTreeSet::new(), BTreeSet::new());
        for line in full.lines() {
            let set = if line.starts_with('-') && !line.starts_with("---") {
                &mut removed
            } else if line.starts_with('+') && !line.starts_with("+++") {
                &mut added
            } else {
                continue;
            };
            for literal in line.split('"').skip(1).step_by(2) {
                set.insert(literal.to_owned());
            }
        }
        let copy_changed: BTreeSet<&String> = removed.difference(&added).collect();
        // label -> (precise, hit descriptions)
        let mut labels: BTreeMap<String, (bool, Vec<String>)> = BTreeMap::new();
        for hit in &platform_drift.entries {
            let item = labels.entry(hit.id.clone()).or_default();
            for h in &hit.hits {
                item.0 |= h.precision == Precision::Precise;
                item.1.push(match (h.precision, h.lines) {
                    (Precision::Precise, Some((a, b))) => format!("{} (L{a}-L{b})", h.reference),
                    (Precision::Precise, None) => h.reference.clone(),
                    (Precision::Coarse, _) => format!("{} (file-level)", h.reference),
                });
            }
        }
        for entry in self.entries.values() {
            for detail in &entry.details {
                let Some(copy) = &detail.spec.copy else {
                    continue;
                };
                let changed = copy
                    .key
                    .iter()
                    .chain([&copy.text])
                    .find(|t| copy_changed.contains(t));
                if let Some(text) = changed {
                    let item = labels
                        .entry(format!("{}#{}", entry.id, detail.id))
                        .or_default();
                    item.0 = true;
                    item.1.push(format!("copy {text:?} changed"));
                }
            }
        }
        for (label, (precise, hits)) in labels {
            let (entry_id, detail_id) = split_target(&label);
            let surface = surface_of(&label);
            let Some(entry) = self.entries.get(entry_id) else {
                continue;
            };
            // An entry with an explicit lead is followed only from that platform.
            if entry.lead.as_deref().is_some_and(|lead| lead != from) {
                continue;
            }
            let record = match detail_id {
                None if surface.is_some() => entry
                    .platforms
                    .get(target)
                    .and_then(|p| p.surfaces.get(surface.unwrap_or_default())),
                None => entry.platforms.get(target),
                Some(d) => entry
                    .details
                    .iter()
                    .find(|x| x.id == d)
                    .and_then(|x| x.platforms.get(target)),
            };
            if record.is_some_and(|r| r.out_of_scope) {
                continue; // the feature does not exist on the target
            }
            let before = record.map(|r| r.status);
            let stale = precise || sync.coarse_as_stale;
            let after = match before {
                Some(Status::Stale) => "stale",
                Some(Status::Implemented | Status::Partial) if stale => "stale",
                Some(Status::Implemented | Status::Partial) => "review",
                Some(other) => other.as_str(),
                None => continue,
            };
            report.changes.push(SyncChange {
                label: label.clone(),
                before: before.map(Status::as_str).unwrap_or("unrecorded").into(),
                after: after.into(),
                hits,
            });
            if let Some(previous) =
                before.filter(|s| stale && matches!(s, Status::Implemented | Status::Partial))
            {
                let file = self.area_files[&self.areas[entry_id]].clone();
                edits.entry(file).or_default().push((
                    entry_id.to_owned(),
                    detail_id.map(str::to_owned),
                    surface.map(str::to_owned),
                    previous,
                ));
            }
        }
        // Tasks per entry, regenerated from this range; text after the end marker is kept.
        let mut by_entry: BTreeMap<&str, Vec<&SyncChange>> = BTreeMap::new();
        for change in &report.changes {
            by_entry
                .entry(split_target(&change.label).0)
                .or_default()
                .push(change);
        }
        let ui = match &sync.captures {
            Some(_) => crate::ui::UiCatalog::load(&self.directory).ok(),
            None => None,
        };
        let mut task_paths = Vec::new();
        if let Some(dir) = &sync.tasks_dir {
            for (entry_id, changes) in &by_entry {
                let body = self.task_markdown(
                    entry_id,
                    changes,
                    &root,
                    &since_sha,
                    &to,
                    &date,
                    sync,
                    ui.as_ref(),
                )?;
                let path = dir.join(format!("{entry_id}.md"));
                let begin = format!("<!-- concordance:task {entry_id} -->");
                let end = "<!-- concordance:task end -->";
                let kept = fs::read_to_string(&path)
                    .ok()
                    .and_then(|old| old.split_once(end).map(|(_, rest)| rest.to_owned()))
                    .unwrap_or_else(|| "\n\n## Notes\n\nAdd implementation notes below the generated block; sync keeps them.\n".into());
                let text = format!("{begin}\n{body}{end}{kept}");
                if sync.apply {
                    fs::create_dir_all(dir)?;
                    if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                        fs::write(&path, &text)?;
                    }
                }
                report.tasks.push(path.display().to_string());
                task_paths.push((entry_id.to_string(), path));
            }
        }
        if sync.issues {
            if task_paths.is_empty() && !by_entry.is_empty() {
                return fail("--issues gh needs --tasks-dir for issue bodies");
            }
            for (entry_id, path) in &task_paths {
                let title = format!("[parity] {entry_id}: {}", self.entries[entry_id].title);
                if sync.apply {
                    report
                        .issues
                        .push(gh_upsert(&title, entry_id, &sync.label, path)?);
                } else {
                    report.issues.push(format!(
                        "would run: gh issue create --title {title:?} --label {} --body-file {} (or edit the open issue whose title contains {entry_id})",
                        sync.label,
                        path.display()
                    ));
                }
            }
        }
        if sync.apply {
            for (file, list) in &edits {
                edit_file(&self.directory.join(file), false, |value| {
                    for (entry_id, detail_id, surface, previous) in list {
                        if let Some(record) = record_mut(
                            value,
                            entry_id,
                            detail_id.as_deref(),
                            surface.as_deref(),
                            target,
                            None,
                        )? {
                            record.insert(key("status"), key("stale"));
                            record.insert(key("previous_status"), key(previous.as_str()));
                            record.insert(key("since_commit"), key(&to));
                            record.insert(key("since_date"), key(&date));
                        }
                    }
                    Ok(())
                })?;
            }
        }
        let rebase =
            self.rebase_anchors(options, from, Some(&since_sha), &to, sync.apply, false)?;
        report.anchors = rebase.markdown();
        if sync.apply {
            advance_pin(&self.directory.join("catalog.toml"), from, &to)?;
            report.baseline_advanced = true;
            let (_, check) = Catalog::load_collect(&self.directory)?;
            if !check.errors.is_empty() {
                return fail(format!("sync left validation errors:\n{check}"));
            }
        }
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    fn task_markdown(
        &self,
        entry_id: &str,
        changes: &[&SyncChange],
        root: &Path,
        since: &str,
        to: &str,
        date: &str,
        sync: &SyncOptions,
        ui: Option<&crate::ui::UiCatalog>,
    ) -> Result<String> {
        let entry = self.entry(entry_id)?;
        let review = changes.iter().all(|c| c.after != "stale");
        let action = if review {
            format!(
                "Only file-level evidence changed, so nothing was marked stale. Triage: `concordance set <dir> <record> --platform {} --stale` if {} must change, or `--reviewed-unchanged` if not.",
                sync.to_platform, sync.to_platform
            )
        } else {
            format!(
                "Bring {} back in line, then clear the stale status with `concordance set` (or record `--intended`). Records marked review only need triage.",
                sync.to_platform
            )
        };
        let mut out = format!(
            "# Parity task: {} (`{entry_id}`)\n\nKind: {}\n\nThe {} baseline changed between `{}` and `{}` ({date}). {action}\n\n## Records\n\n",
            entry.title,
            if review { "review" } else { "stale" },
            sync.from,
            short(since),
            short(to),
        );
        let mut files = BTreeSet::new();
        for change in changes {
            let detail = split_target(&change.label)
                .1
                .and_then(|d| entry.details.iter().find(|x| x.id == d));
            out.push_str(&format!(
                "- `{}`: {} {} → {}{}\n",
                change.label,
                sync.to_platform,
                change.before,
                change.after,
                detail
                    .map(|d| {
                        let numbers = d.spec.numbers();
                        if numbers.is_empty() {
                            format!(". {}: {}", d.kind.as_str(), d.spec.text)
                        } else {
                            format!(". {} ({numbers}): {}", d.kind.as_str(), d.spec.text)
                        }
                    })
                    .unwrap_or_default()
            ));
            for hit in &change.hits {
                out.push_str(&format!("  - {hit}\n"));
                let reference = hit.split(" (").next().unwrap_or(hit);
                if let Ok(path) = self.evidence_relative(&sync.from, reference) {
                    files.insert(path.to_owned());
                }
            }
        }
        let files: Vec<String> = files.into_iter().collect();
        let mut diff_args = vec![
            "diff".to_owned(),
            "-U2".into(),
            "--no-color".into(),
            since.into(),
            to.into(),
            "--".into(),
        ];
        diff_args.extend(files.iter().cloned());
        let refs: Vec<&str> = diff_args.iter().map(String::as_str).collect();
        let diff = String::from_utf8_lossy(&git(root, &refs)?).into_owned();
        let copy: Vec<&str> = diff
            .lines()
            .filter(|l| {
                (l.starts_with('+') || l.starts_with('-'))
                    && !l.starts_with("+++")
                    && !l.starts_with("---")
                    && has_copy_literal(l)
            })
            .collect();
        out.push_str("\n## Copy changes\n\n");
        if copy.is_empty() {
            out.push_str("No changed string literals.\n");
        } else {
            out.push_str("```diff\n");
            for line in copy.iter().take(60) {
                out.push_str(line.trim_end());
                out.push('\n');
            }
            out.push_str("```\n");
        }
        let mut log_args = vec![
            "log".to_owned(),
            "--format=%H%x09%s".into(),
            format!("{since}..{to}"),
            "--".into(),
        ];
        log_args.extend(files.iter().cloned());
        let refs: Vec<&str> = log_args.iter().map(String::as_str).collect();
        out.push_str("\n## Commits\n\n");
        for line in lines(git(root, &refs)?) {
            let (sha, subject) = line.split_once('\t').unwrap_or((&line, ""));
            match commit_url(self, &sync.from, sha) {
                Some(url) => out.push_str(&format!("- [{}]({url}) {subject}\n", short(sha))),
                None => out.push_str(&format!("- `{}` {subject}\n", short(sha))),
            }
        }
        if let (Some(ui), Some(captures)) = (ui, &sync.captures) {
            let mut listed = Vec::new();
            for screen in ui
                .screens
                .values()
                .filter(|s| s.entries.iter().any(|e| e == entry_id))
            {
                for state in screen.states.keys() {
                    for platform in [&sync.from, &sync.to_platform] {
                        let stem = format!("{}.{state}.{platform}", screen.id);
                        for candidate in [
                            captures.join(format!("{stem}.png")),
                            captures.join(platform).join(format!("{stem}.png")),
                        ] {
                            if candidate.is_file() {
                                listed.push(candidate.display().to_string());
                            }
                        }
                    }
                }
            }
            if !listed.is_empty() {
                out.push_str("\n## Captures\n\n");
                for path in listed {
                    out.push_str(&format!("- {path}\n"));
                }
            }
        }
        // Trim line ends so blank context lines don't leave trailing whitespace in the task file.
        let excerpt: Vec<&str> = diff.lines().take(120).map(str::trim_end).collect();
        out.push_str(&format!(
            "\n<details><summary>Diff excerpt ({} lines)</summary>\n\n```diff\n{}\n```\n\n</details>\n",
            diff.lines().count(),
            excerpt.join("\n")
        ));
        Ok(out)
    }

    /// Stale records older than their limit and without an intended difference.
    /// A detail uses the limit for its kind (default otherwise); an entry uses the
    /// strictest limit among its stale details, or the default. A limit of 0 days
    /// fails any stale record; otherwise a record fails when older than the limit.
    pub fn gate_release(
        &self,
        platform: &str,
        default_days: u32,
        per_kind: &BTreeMap<DetailKind, u32>,
        today: &str,
    ) -> Result<Vec<ReleaseFinding>> {
        if !self.config.platforms.contains_key(platform) {
            return fail(format!("unknown platform: {platform}"));
        }
        let now = parse_date(today).ok_or("today must be YYYY-MM-DD")?;
        let stale = |r: Option<&crate::Platform>| {
            r.filter(|r| r.status == Status::Stale && r.intended_difference.is_none())
                .cloned()
        };
        let mut findings = Vec::new();
        let mut check = |label: String, record: crate::Platform, limit: u32| {
            let age = record
                .since_date
                .as_deref()
                .and_then(parse_date)
                .map(|d| now - d);
            let fails = limit == 0 || age.is_none_or(|a| a > limit as i64);
            if fails {
                findings.push(ReleaseFinding {
                    label,
                    since_date: record.since_date.clone(),
                    age_days: age,
                    limit_days: limit,
                });
            }
        };
        for entry in self.entries.values() {
            let mut strictest: Option<u32> = None;
            for detail in &entry.details {
                if let Some(record) = stale(detail.platforms.get(platform)) {
                    let limit = per_kind.get(&detail.kind).copied().unwrap_or(default_days);
                    strictest = Some(strictest.map_or(limit, |s| s.min(limit)));
                    check(format!("{}#{}", entry.id, detail.id), record, limit);
                }
            }
            if let Some(record) = stale(entry.platforms.get(platform)) {
                check(entry.id.clone(), record, strictest.unwrap_or(default_days));
            }
            for (surface, inner) in entry
                .platforms
                .get(platform)
                .map(|p| &p.surfaces)
                .into_iter()
                .flatten()
            {
                if let Some(record) = stale(Some(inner)) {
                    check(
                        format!("{}@{surface}", entry.id),
                        record,
                        strictest.unwrap_or(default_days),
                    );
                }
            }
        }
        Ok(findings)
    }
}

/// A double-quoted literal that reads like interface copy: it contains a space or starts
/// with an uppercase letter (identifiers such as "notStarted" are skipped).
fn has_copy_literal(line: &str) -> bool {
    line.split('"').skip(1).step_by(2).any(|literal| {
        literal.contains(' ') || literal.chars().next().is_some_and(char::is_uppercase)
    })
}

fn gh_upsert(title: &str, entry_id: &str, label: &str, body: &Path) -> Result<String> {
    let run = |args: &[&str]| -> Result<String> {
        let output = Command::new("gh").args(args).output()?;
        if !output.status.success() {
            return fail(format!(
                "gh {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let body = body.to_str().ok_or("non-UTF8 task path")?;
    let search = format!("\"{entry_id}\" in:title");
    let existing = run(&[
        "issue",
        "list",
        "--state",
        "open",
        "--label",
        label,
        "--search",
        &search,
        "--json",
        "number",
        "--jq",
        ".[0].number",
    ])?;
    if existing.is_empty() {
        let url = run(&[
            "issue",
            "create",
            "--title",
            title,
            "--label",
            label,
            "--body-file",
            body,
        ])?;
        Ok(format!("created {url}"))
    } else {
        run(&["issue", "edit", &existing, "--body-file", body])?;
        Ok(format!("updated #{existing}"))
    }
}

/// Set `[baselines.<platform>] commit` in catalog.toml, editing only that line.
pub(crate) fn advance_pin(path: &Path, platform: &str, commit: &str) -> Result<()> {
    let text = fs::read_to_string(path)?;
    let header = format!("[baselines.{platform}]");
    let mut out = Vec::new();
    let mut in_section = false;
    let mut done = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_section && !done {
                out.push(format!("commit = \"{commit}\""));
                done = true;
            }
            in_section = trimmed == header;
        }
        if in_section
            && !done
            && trimmed.starts_with("commit")
            && trimmed[6..].trim_start().starts_with('=')
        {
            out.push(format!("commit = \"{commit}\""));
            done = true;
            continue;
        }
        out.push(line.to_owned());
    }
    if in_section && !done {
        out.push(format!("commit = \"{commit}\""));
        done = true;
    }
    if !done {
        out.push(String::new());
        out.push(header);
        out.push(format!("commit = \"{commit}\""));
    }
    let mut result = out.join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    fs::write(path, result)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn civil_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(
            parse_date("2026-11-20").expect("date") - parse_date("2026-11-13").expect("date"),
            7
        );
        assert_eq!(
            parse_date("2024-03-01").expect("d") - parse_date("2024-02-28").expect("d"),
            2
        );
        assert_eq!(today().len(), 10);
    }
}
