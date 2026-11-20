//! Continuous parity: `gate` on every merge, `sync` after baseline merges, and `gate-release`.
use crate::drift::{git, glob, Precision};
use crate::edit::edit_file;
use crate::set::{key, record_mut, split_target};
use crate::{fail, Catalog, DetailKind, Result, RootOptions, Status};
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
            "## Sync {} → {}: `{}` → `{}`{}\n\n{} records with changed baseline evidence: {} newly stale, {} already stale, {} unchanged (not implemented).\n",
            self.from,
            self.to_platform,
            short(&self.since),
            short(&self.to),
            if self.applied { "" } else { " (dry run; pass --apply to write)" },
            self.changes.len(),
            count("stale"),
            self.changes.iter().filter(|c| c.before == "stale").count(),
            self.changes.iter().filter(|c| c.after != "stale").count()
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
        for (label, p, reference) in self.evidence_items() {
            if p == platform {
                let path = self.evidence_relative(platform, reference)?.to_owned();
                cited
                    .entry(path)
                    .or_default()
                    .insert(split_target(&label).0.to_owned());
            }
        }
        let changed_areas: BTreeSet<&String> = self
            .area_files
            .iter()
            .filter(|(_, file)| catalog_changed.contains(file))
            .map(|(area, _)| area)
            .collect();
        let any_area = !changed_areas.is_empty();
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
                .filter(|e| {
                    self.areas
                        .get(*e)
                        .is_some_and(|a| changed_areas.contains(a))
                })
                .collect();
            let (covered, reason) = match (entries.is_empty(), touched.is_empty()) {
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
        let waiver = messages.lines().find_map(|line| {
            let rest = line.trim().strip_prefix("Catalog:")?.trim_start();
            let rest = rest.strip_prefix("none")?.trim_start();
            let rest = rest.trim_start_matches(['—', '–', '-', ':']).trim();
            (!rest.is_empty()).then(|| rest.to_owned())
        });
        let passed = waiver.is_some() || user_visible.iter().all(|f| f.covered);
        Ok(GateReport {
            platform: platform.into(),
            base,
            head,
            changed: changed.len(),
            user_visible,
            catalog_changed,
            waiver,
            passed,
        })
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
        let mut edits: BTreeMap<String, Vec<(String, Option<String>, Status)>> = BTreeMap::new();
        for hit in &platform_drift.entries {
            let (entry_id, detail_id) = split_target(&hit.id);
            let Some(entry) = self.entries.get(entry_id) else {
                continue;
            };
            let record = match detail_id {
                None => entry.platforms.get(target),
                Some(d) => entry
                    .details
                    .iter()
                    .find(|x| x.id == d)
                    .and_then(|x| x.platforms.get(target)),
            };
            let before = record.map(|r| r.status);
            let after = match before {
                Some(Status::Implemented | Status::Partial | Status::Stale) => Status::Stale,
                Some(other) => other,
                None => continue,
            };
            report.changes.push(SyncChange {
                label: hit.id.clone(),
                before: before.map(Status::as_str).unwrap_or("unrecorded").into(),
                after: after.as_str().into(),
                hits: hit
                    .hits
                    .iter()
                    .map(|h| match (h.precision, h.lines) {
                        (Precision::Precise, Some((a, b))) => {
                            format!("{} (L{a}-L{b})", h.reference)
                        }
                        (Precision::Precise, None) => h.reference.clone(),
                        (Precision::Coarse, _) => format!("{} (file-level)", h.reference),
                    })
                    .collect(),
            });
            if let Some(previous) =
                before.filter(|s| matches!(s, Status::Implemented | Status::Partial))
            {
                let file = self.area_files[&self.areas[entry_id]].clone();
                edits.entry(file).or_default().push((
                    entry_id.to_owned(),
                    detail_id.map(str::to_owned),
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
                    for (entry_id, detail_id, previous) in list {
                        if let Some(record) =
                            record_mut(value, entry_id, detail_id.as_deref(), target, None)?
                        {
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
        let mut out = format!(
            "# Parity task: {} (`{entry_id}`)\n\nThe {} baseline changed between `{}` and `{}` ({date}). Bring {} back in line, then clear the stale status with `concordance set` (or record `--intended`).\n\n## Records\n\n",
            entry.title,
            sync.from,
            short(since),
            short(to),
            sync.to_platform
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
        let excerpt: Vec<&str> = diff.lines().take(120).collect();
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
