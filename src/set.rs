//! `concordance set`: change a platform record's status and bookkeeping fields in place.
use crate::edit::edit_file;
use crate::{fail, Catalog, Issue, Result, Status};
use serde_yaml::{Mapping, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Fields `set` may clear. Verification is cleared here but only ever written by `verify`.
pub const CLEARABLE: &[&str] = &[
    "gap",
    "notes",
    "implemented_by",
    "blocked_by",
    "verification",
    "source",
    "tests",
    "reviewed_commit",
    "intended_difference",
    "previous_status",
    "since_commit",
    "since_date",
];

#[derive(Debug, Default, Clone)]
pub struct SetRequest {
    /// `entry.id`, `entry.id#detail` or `entry.id/detail`.
    pub target: String,
    pub platform: String,
    pub status: Option<Status>,
    pub implemented_by: Option<String>,
    pub gap: Option<String>,
    pub notes: Option<String>,
    pub blocked_by: Option<Vec<String>>,
    pub clear: Vec<String>,
    /// Record an intended difference; a stale record returns to its previous status.
    pub intended: Option<String>,
    /// Mark the record stale against the current baseline pin (triaging a review task).
    pub stale: bool,
    /// Record that the record needs no change at the current baseline pin.
    pub reviewed_unchanged: bool,
    /// Task directory from `sync --tasks-dir`; closing a stale record notes it there.
    pub tasks_dir: Option<PathBuf>,
    /// Commit that closed a stale record (default: HEAD of the platform root, if any).
    pub closing_commit: Option<String>,
    /// Fields to set to `null`, unsetting a value inherited from the parent platform.
    pub unset: Vec<String>,
    pub allow_reformat: bool,
}
#[derive(Debug)]
pub struct SetOutcome {
    pub file: PathBuf,
    pub changed: bool,
    /// Warnings for the edited record after the change.
    pub warnings: Vec<Issue>,
    /// Set when a stale record was closed: (previous stale since_commit, new status).
    pub closed: Option<String>,
}

pub(crate) fn key(s: &str) -> Value {
    Value::String(s.into())
}

fn find<'a>(items: &'a mut Value, id: &str) -> Option<&'a mut Mapping> {
    items
        .as_sequence_mut()?
        .iter_mut()
        .filter_map(Value::as_mapping_mut)
        .find(|m| m.get("id").and_then(Value::as_str) == Some(id))
}

/// The platform record mapping for `entry_id` (or its detail) in an area file's value.
/// Missing records are created with `create` as their status; `None` leaves them absent.
pub(crate) fn record_mut<'a>(
    value: &'a mut Value,
    entry_id: &str,
    detail_id: Option<&str>,
    platform: &str,
    create: Option<Status>,
) -> Result<Option<&'a mut Mapping>> {
    let entry = find(value, entry_id).ok_or("entry not found in its area file")?;
    let holder = match detail_id {
        None => entry,
        Some(detail) => entry
            .get_mut("details")
            .and_then(|d| find(d, detail))
            .ok_or_else(|| format!("unknown detail: {entry_id}#{detail}"))?,
    };
    if !holder.contains_key("platforms") {
        holder.insert(key("platforms"), Value::Mapping(Mapping::new()));
    }
    let platforms = holder
        .get_mut("platforms")
        .and_then(Value::as_mapping_mut)
        .ok_or("platforms is not a mapping")?;
    if !platforms.contains_key(platform) {
        let Some(status) = create else {
            return Ok(None);
        };
        let mut record = Mapping::new();
        record.insert(key("status"), key(status.as_str()));
        platforms.insert(key(platform), Value::Mapping(record));
    }
    Ok(platforms.get_mut(platform).and_then(Value::as_mapping_mut))
}

/// Split `entry`, `entry#detail` or `entry/detail`.
pub(crate) fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once(['#', '/']) {
        Some((entry, detail)) => (entry, Some(detail)),
        None => (target, None),
    }
}

impl Catalog {
    /// Edit one platform record in its area file, then reload; the edit is reverted if it
    /// introduces validation errors for that record.
    pub fn set_record(directory: &Path, request: &SetRequest) -> Result<SetOutcome> {
        for field in request.clear.iter().chain(&request.unset) {
            if !CLEARABLE.contains(&field.as_str()) {
                return fail(format!(
                    "cannot clear {field}; clearable: {}",
                    CLEARABLE.join(", ")
                ));
            }
        }
        let (entry_id, detail_id) = split_target(&request.target);
        let label = match detail_id {
            Some(d) => format!("{entry_id}#{d}"),
            None => entry_id.to_owned(),
        };
        let (catalog, before) = Catalog::load_collect(directory)?;
        let Some(platform_config) = catalog.config.platforms.get(&request.platform) else {
            return fail(format!("unknown platform: {}", request.platform));
        };
        if !request.unset.is_empty() && platform_config.inherits().is_none() {
            return fail(format!(
                "--unset needs an inheriting platform; {} inherits nothing (use --clear)",
                request.platform
            ));
        }
        let pin = catalog
            .baseline_commit(&catalog.config.report.baseline)
            .map(str::to_owned);
        if (request.stale || request.reviewed_unchanged) && pin.is_none() {
            return fail(format!(
                "--stale and --reviewed-unchanged need a [baselines.{}] commit",
                catalog.config.report.baseline
            ));
        }
        if request.stale && request.reviewed_unchanged {
            return fail("--stale and --reviewed-unchanged are exclusive");
        }
        let pin = pin.unwrap_or_default();
        let area = catalog
            .areas
            .get(entry_id)
            .ok_or_else(|| format!("unknown entry: {entry_id}"))?;
        let file = catalog.directory.join(&catalog.area_files[area]);
        let original = fs::read(&file)?;
        let mut closed: Option<(String, String)> = None;
        let changed = edit_file(&file, request.allow_reformat, |value| {
            let record = record_mut(
                value,
                entry_id,
                detail_id,
                &request.platform,
                request.status,
            )?
            .ok_or_else(|| {
                format!(
                    "{label} has no {} record; pass --status to create one",
                    request.platform
                )
            })?;
            for field in &request.clear {
                record.remove(field.as_str());
            }
            for field in &request.unset {
                record.insert(key(field), Value::Null);
            }
            let was_stale = record.get("status").and_then(Value::as_str) == Some("stale");
            let stale_since = record
                .get("since_commit")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if let Some(status) = request.status {
                record.insert(key("status"), key(status.as_str()));
            }
            for (field, value) in [
                ("implemented_by", &request.implemented_by),
                ("gap", &request.gap),
                ("notes", &request.notes),
            ] {
                if let Some(value) = value {
                    record.insert(key(field), key(value));
                }
            }
            if request.stale {
                let current = record
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if current != "stale" {
                    if !matches!(current.as_str(), "implemented" | "shipped" | "partial") {
                        return fail(format!(
                            "only implemented or partial records can become stale, not {current}"
                        ));
                    }
                    let current = if current == "shipped" {
                        "implemented".to_owned()
                    } else {
                        current
                    };
                    record.insert(key("status"), key("stale"));
                    record.insert(key("previous_status"), key(&current));
                    record.insert(key("since_commit"), key(&pin));
                    record.insert(key("since_date"), key(&crate::today()));
                }
                record.remove("reviewed_commit");
            }
            if request.reviewed_unchanged {
                record.insert(key("reviewed_commit"), key(&pin));
                // Reviewed and unchanged closes a stale record: it returns to its previous
                // status (the stale bookkeeping is dropped below), unless --status says otherwise.
                if request.status.is_none()
                    && record.get("status").and_then(Value::as_str) == Some("stale")
                {
                    let previous = record
                        .get("previous_status")
                        .and_then(Value::as_str)
                        .unwrap_or("implemented")
                        .to_owned();
                    record.insert(key("status"), key(&previous));
                }
            }
            if let Some(reason) = &request.intended {
                record.insert(key("intended_difference"), key(reason));
                if record.get("status").and_then(Value::as_str) == Some("stale") {
                    let previous = record
                        .get("previous_status")
                        .and_then(Value::as_str)
                        .unwrap_or("implemented")
                        .to_owned();
                    if request.status.is_none() {
                        record.insert(key("status"), key(&previous));
                    }
                }
                if record.get("status").and_then(Value::as_str) != Some("stale") {
                    for field in ["previous_status", "since_commit", "since_date"] {
                        record.remove(field);
                    }
                }
            }
            if let Some(keys) = &request.blocked_by {
                record.insert(
                    key("blocked_by"),
                    Value::Sequence(keys.iter().map(|k| key(k)).collect()),
                );
            }
            // Leaving `stale` drops its bookkeeping: the record is current again.
            if was_stale && record.get("status").and_then(Value::as_str) != Some("stale") {
                for field in ["previous_status", "since_commit", "since_date"] {
                    record.remove(field);
                }
                let now = record
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                closed = Some((stale_since.clone(), now));
            }
            Ok(())
        })?;
        let mine = |issue: &Issue| issue.entry == label && issue.platform == request.platform;
        let after = Catalog::load_collect(directory).map(|(_, report)| report);
        let new_errors: Vec<Issue> = match &after {
            Ok(report) => report
                .errors
                .iter()
                .filter(|i| (mine(i) || i.entry == label) && !before.errors.contains(i))
                .cloned()
                .collect(),
            Err(error) => vec![Issue {
                group: "schema".into(),
                entry: label.clone(),
                platform: request.platform.clone(),
                reference: String::new(),
                message: error.to_string(),
            }],
        };
        if !new_errors.is_empty() {
            fs::write(&file, original)?;
            let text: Vec<String> = new_errors
                .iter()
                .map(|i| format!("{} {} {}: {}", i.entry, i.platform, i.reference, i.message))
                .collect();
            let hint = if text.iter().any(|t| t.contains("verification recorded")) {
                " (pass --clear verification)"
            } else {
                ""
            };
            return fail(format!(
                "not written; the edit would introduce errors{hint}:\n{}",
                text.join("\n")
            ));
        }
        let warnings = after
            .map(|r| r.warnings.into_iter().filter(|i| mine(i)).collect())
            .unwrap_or_default();
        let mut closed_note = None;
        if let Some((since, now)) = closed {
            let commit = request.closing_commit.clone().or_else(|| {
                catalog
                    .platform_root(&request.platform, &crate::RootOptions::default())
                    .ok()
                    .flatten()
                    .and_then(|root| crate::drift::git(&root, &["rev-parse", "HEAD"]).ok())
                    .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
            });
            let note = format!(
                "- Closed `{label}` on {} ({} → {now}) on {}{}{}.\n",
                request.platform,
                "stale",
                crate::today(),
                commit.map(|c| format!(" at `{c}`")).unwrap_or_default(),
                if since.is_empty() {
                    String::new()
                } else {
                    format!("; stale since `{since}`")
                }
            );
            if let Some(dir) = &request.tasks_dir {
                let path = dir.join(format!("{entry_id}.md"));
                if let Ok(mut text) = fs::read_to_string(&path) {
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&note);
                    fs::write(&path, text)?;
                }
            }
            closed_note = Some(note.trim_end().to_owned());
        }
        Ok(SetOutcome {
            file,
            changed,
            warnings,
            closed: closed_note,
        })
    }
}
