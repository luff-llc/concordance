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
    pub allow_reformat: bool,
}
#[derive(Debug)]
pub struct SetOutcome {
    pub file: PathBuf,
    pub changed: bool,
    /// Warnings for the edited record after the change.
    pub warnings: Vec<Issue>,
}

fn key(s: &str) -> Value {
    Value::String(s.into())
}

fn find<'a>(items: &'a mut Value, id: &str) -> Option<&'a mut Mapping> {
    items
        .as_sequence_mut()?
        .iter_mut()
        .filter_map(Value::as_mapping_mut)
        .find(|m| m.get("id").and_then(Value::as_str) == Some(id))
}

impl Catalog {
    /// Edit one platform record in its area file, then reload; the edit is reverted if it
    /// introduces validation errors for that record.
    pub fn set_record(directory: &Path, request: &SetRequest) -> Result<SetOutcome> {
        for field in &request.clear {
            if !CLEARABLE.contains(&field.as_str()) {
                return fail(format!(
                    "cannot clear {field}; clearable: {}",
                    CLEARABLE.join(", ")
                ));
            }
        }
        let (entry_id, detail_id) = match request.target.split_once(['#', '/']) {
            Some((entry, detail)) => (entry, Some(detail)),
            None => (request.target.as_str(), None),
        };
        let label = match detail_id {
            Some(d) => format!("{entry_id}#{d}"),
            None => entry_id.to_owned(),
        };
        let (catalog, before) = Catalog::load_collect(directory)?;
        if !catalog.config.platforms.contains_key(&request.platform) {
            return fail(format!("unknown platform: {}", request.platform));
        }
        let area = catalog
            .areas
            .get(entry_id)
            .ok_or_else(|| format!("unknown entry: {entry_id}"))?;
        let file = catalog.directory.join(&catalog.area_files[area]);
        let original = fs::read(&file)?;
        let changed = edit_file(&file, request.allow_reformat, |value| {
            let entry = find(value, entry_id).ok_or("entry not found in its area file")?;
            let holder = match detail_id {
                None => entry,
                Some(detail) => entry
                    .get_mut("details")
                    .and_then(|d| find(d, detail))
                    .ok_or_else(|| format!("unknown detail: {label}"))?,
            };
            if !holder.contains_key("platforms") {
                holder.insert(key("platforms"), Value::Mapping(Mapping::new()));
            }
            let platforms = holder
                .get_mut("platforms")
                .and_then(Value::as_mapping_mut)
                .ok_or("platforms is not a mapping")?;
            if !platforms.contains_key(request.platform.as_str()) {
                let Some(status) = request.status else {
                    return fail(format!(
                        "{label} has no {} record; pass --status to create one",
                        request.platform
                    ));
                };
                let mut record = Mapping::new();
                record.insert(key("status"), key(status.as_str()));
                platforms.insert(key(&request.platform), Value::Mapping(record));
            }
            let record = platforms
                .get_mut(request.platform.as_str())
                .and_then(Value::as_mapping_mut)
                .ok_or("platform record is not a mapping")?;
            for field in &request.clear {
                record.remove(field.as_str());
            }
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
            if let Some(keys) = &request.blocked_by {
                record.insert(
                    key("blocked_by"),
                    Value::Sequence(keys.iter().map(|k| key(k)).collect()),
                );
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
        Ok(SetOutcome {
            file,
            changed,
            warnings,
        })
    }
}
