use crate::{
    is_capture_key, is_test_path, safe_relative, schema, Catalog, Curve, Entry, Platform, Result,
    Status,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Issue {
    pub group: String,
    pub entry: String,
    pub platform: String,
    pub reference: String,
    pub message: String,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct ValidationReport {
    pub errors: Vec<Issue>,
    pub unchecked: Vec<Issue>,
}
impl ValidationReport {
    pub(crate) fn error(
        &mut self,
        group: &str,
        entry: &str,
        platform: &str,
        reference: &str,
        message: &str,
    ) {
        self.errors.push(Issue {
            group: group.into(),
            entry: entry.into(),
            platform: platform.into(),
            reference: reference.into(),
            message: message.into(),
        });
    }
    pub fn sort(&mut self) {
        self.errors.sort();
        self.unchecked.sort();
    }
    pub fn into_result(self) -> Result<()> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(Box::new(self))
        }
    }
}
impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut group = "";
        for issue in &self.errors {
            if group != issue.group {
                group = &issue.group;
                writeln!(f, "{group}:")?;
            }
            writeln!(
                f,
                "  {} {} {}: {}",
                issue.entry, issue.platform, issue.reference, issue.message
            )?;
        }
        for issue in &self.unchecked {
            writeln!(
                f,
                "{} {} {}: unchecked (root not supplied). {}",
                issue.entry, issue.platform, issue.reference, issue.message
            )?;
        }
        write!(
            f,
            "{} validation error(s); {} unchecked reference(s).",
            self.errors.len(),
            self.unchecked.len()
        )
    }
}
impl std::error::Error for ValidationReport {}

#[derive(Debug, Default, Clone)]
pub struct RootOptions {
    pub repo: Option<PathBuf>,
    pub roots: BTreeMap<String, PathBuf>,
    pub require_roots: bool,
}
impl RootOptions {
    /// Explicit CLI roots take precedence over environment variables.
    pub fn with_env(mut self, catalog: &Catalog) -> Self {
        for platform in catalog.config.platforms.keys() {
            let name = format!(
                "CONCORDANCE_ROOT_{}",
                platform.to_uppercase().replace('-', "_")
            );
            if let Some(root) = std::env::var_os(name) {
                self.roots
                    .entry(platform.clone())
                    .or_insert_with(|| root.into());
            }
        }
        self
    }
}
/// Source, file-based tests and verification journeys cited by a platform record.
pub(crate) fn evidence_refs(record: &Platform) -> impl Iterator<Item = &String> {
    record
        .source
        .iter()
        .chain(record.tests.iter().filter(|s| is_test_path(s)))
        .chain(
            record
                .verification
                .iter()
                .flat_map(|v| v.journeys.iter().filter(|s| is_test_path(s))),
        )
}

impl Catalog {
    /// Every platform record with a display label: entry IDs, plus `entry#detail` for details.
    pub fn records(&self) -> Vec<(String, &str, &Platform)> {
        let mut out = Vec::new();
        for entry in self.entries.values() {
            for (platform, record) in &entry.platforms {
                out.push((entry.id.clone(), platform.as_str(), record));
            }
            for detail in &entry.details {
                for (platform, record) in &detail.platforms {
                    out.push((
                        format!("{}#{}", entry.id, detail.id),
                        platform.as_str(),
                        record,
                    ));
                }
            }
        }
        out
    }

    /// Every file-based evidence reference: (label, platform, reference).
    /// Detail baseline evidence belongs to the report baseline platform.
    pub fn evidence_items(&self) -> Vec<(String, &str, &String)> {
        let mut out: Vec<_> = self
            .records()
            .into_iter()
            .flat_map(|(label, platform, record)| {
                evidence_refs(record).map(move |r| (label.clone(), platform, r))
            })
            .collect();
        let baseline = self.config.report.baseline.as_str();
        for entry in self.entries.values() {
            for detail in &entry.details {
                for reference in &detail.baseline {
                    out.push((format!("{}#{}", entry.id, detail.id), baseline, reference));
                }
            }
        }
        out
    }

    fn check_capture_files(
        &self,
        label: &str,
        platform: &str,
        record: &Platform,
        repo: std::result::Result<&Path, &std::io::Error>,
        report: &mut ValidationReport,
    ) {
        let Some(v) = &record.verification else {
            return;
        };
        for capture in v.captures.iter().filter(|c| !is_capture_key(c)) {
            if !safe_relative(capture, false) {
                report.error(
                    "verification",
                    label,
                    platform,
                    capture,
                    "unsafe capture path",
                );
                continue;
            }
            let message = match repo {
                Ok(repo) => match repo.join(capture).canonicalize() {
                    Ok(path) if path.starts_with(repo) && path.is_file() => continue,
                    Ok(_) => "capture is not a file within the repository root".to_owned(),
                    Err(error) => format!("missing capture: {error}"),
                },
                Err(error) => format!("repository root unavailable: {error}"),
            };
            report.error("verification", label, platform, capture, &message);
        }
    }

    pub(crate) fn evidence_relative<'a>(
        &self,
        platform: &str,
        reference: &'a str,
    ) -> Result<&'a str> {
        let path = crate::split_anchor(reference).0;
        let path = match self.config.path_prefixes.get(platform) {
            Some(prefix) => path
                .strip_prefix(prefix)
                .ok_or_else(|| format!("path must start with {prefix}: {reference}"))?,
            None => path,
        };
        if !safe_relative(path, false) {
            return crate::fail(format!("unsafe evidence path: {reference}"));
        }
        Ok(path)
    }

    /// Explicit roots may be anywhere; roots derived from catalog data stay in repo_root.
    pub fn platform_root(&self, platform: &str, options: &RootOptions) -> Result<Option<PathBuf>> {
        let relative = self
            .config
            .platforms
            .get(platform)
            .ok_or_else(|| format!("unknown platform: {platform}"))?;
        if let Some(root) = options.roots.get(platform) {
            return Ok(Some(root.canonicalize()?));
        }
        if self.config.external_roots.contains_key(platform) {
            return Ok(None);
        }
        if !safe_relative(relative, true) || Path::new(&self.config.repo_root).is_absolute() {
            return crate::fail("unsafe catalog root");
        }
        let repo = options
            .repo
            .clone()
            .unwrap_or_else(|| self.directory.join(&self.config.repo_root))
            .canonicalize()?;
        let root = repo.join(relative).canonicalize()?;
        if !root.starts_with(repo) {
            return crate::fail(format!("{platform} root escapes repo"));
        }
        Ok(Some(root))
    }

    pub fn validate_evidence(&self, options: &RootOptions) -> ValidationReport {
        let mut report = ValidationReport::default();
        for platform in options.roots.keys() {
            if !self.config.platforms.contains_key(platform) {
                report.error("roots", "", platform, "", "unknown platform");
            }
        }
        let roots: BTreeMap<_, _> = self
            .config
            .platforms
            .keys()
            .map(|p| (p, self.platform_root(p, options)))
            .collect();
        let repo = options
            .repo
            .clone()
            .unwrap_or_else(|| self.directory.join(&self.config.repo_root))
            .canonicalize();
        for (label, platform, record) in self.records() {
            self.check_capture_files(&label, platform, record, repo.as_deref(), &mut report);
        }
        for (entry_id, platform, reference) in self.evidence_items() {
            {
                {
                    let relative = match self.evidence_relative(platform, reference) {
                        Ok(path) => path,
                        Err(error) => {
                            report.error(
                                "evidence",
                                &entry_id,
                                platform,
                                reference,
                                &error.to_string(),
                            );
                            continue;
                        }
                    };
                    let Some(root) = roots.get(&platform.to_owned()) else {
                        continue;
                    };
                    match root {
                        Ok(Some(root)) => {
                            let checked = root.join(relative).canonicalize();
                            let message = match checked {
                                Ok(path) if path.starts_with(root) && path.is_file() => continue,
                                Ok(_) => "evidence is not a file within platform root".into(),
                                Err(error) => format!("missing evidence: {error}"),
                            };
                            report.error("evidence", &entry_id, platform, reference, &message);
                        }
                        Ok(None) => {
                            let hint = self.config.external_roots[platform].hint();
                            let message = format!(
                                "Supply --root {platform}=<path> or CONCORDANCE_ROOT_{}. {hint}",
                                platform.to_uppercase().replace('-', "_")
                            );
                            if options.require_roots {
                                report.error(
                                    "roots",
                                    &entry_id,
                                    platform,
                                    reference,
                                    &format!("root not supplied. {message}"),
                                );
                            } else {
                                report.unchecked.push(Issue {
                                    group: "roots".into(),
                                    entry: entry_id.clone(),
                                    platform: platform.to_owned(),
                                    reference: reference.clone(),
                                    message,
                                });
                            }
                        }
                        Err(error) => report.error(
                            "roots",
                            &entry_id,
                            platform,
                            reference,
                            &error.to_string(),
                        ),
                    }
                }
            }
        }
        report.sort();
        report
    }

    /// Explain declared checkouts that are unavailable without reading their evidence files.
    pub fn evidence_roots_section(&self, options: &RootOptions) -> Result<String> {
        let mut lines = Vec::new();
        for (platform, hint) in &self.config.external_roots {
            if options.roots.contains_key(platform) {
                continue;
            }
            let message = format!("{platform}: unchecked (root not supplied). Supply --root {platform}=<path> or CONCORDANCE_ROOT_{}. {}", platform.to_uppercase().replace('-', "_"), hint.hint());
            if options.require_roots {
                return crate::fail(message);
            }
            lines.push(format!("- {message}"));
        }
        if lines.is_empty() {
            Ok(String::new())
        } else {
            Ok(format!("\n## Evidence roots\n\n{}\n", lines.join("\n")))
        }
    }

    /// Validate recoverable structure and evidence together, even when some entries are invalid.
    pub fn validate(
        directory: impl AsRef<Path>,
        options: &RootOptions,
    ) -> Result<ValidationReport> {
        let (catalog, mut report) = Self::load_collect(directory)?;
        let paths = catalog.validate_evidence(options);
        report.errors.extend(paths.errors);
        report.unchecked.extend(paths.unchecked);
        report.sort();
        Ok(report)
    }
}

/// Check one platform record's verification semantics beyond the schema.
pub(crate) fn check_platform(
    entry: &str,
    platform: &str,
    record: &Platform,
    report: &mut ValidationReport,
) {
    let Some(v) = &record.verification else {
        return;
    };
    if matches!(record.status, Status::Missing | Status::NotApplicable) {
        report.error(
            "verification",
            entry,
            platform,
            "",
            &format!(
                "verification recorded for a {} status",
                record.status.as_str()
            ),
        );
    }
    if record.implemented_by.is_none() {
        report.error(
            "verification",
            entry,
            platform,
            "",
            "verification requires implemented_by",
        );
    }
    if record.implemented_by.as_deref() == Some(v.by.as_str()) {
        report.error(
            "verification",
            entry,
            platform,
            &v.by,
            "verifier must differ from implemented_by",
        );
    }
    for (field, commit) in [
        ("commit", &v.commit),
        ("baseline_commit", &v.baseline_commit),
    ] {
        if let Some(commit) = commit {
            if !schema::valid_commit(commit) {
                report.error(
                    "verification",
                    entry,
                    platform,
                    commit,
                    &format!("{field} must be a hexadecimal commit SHA"),
                );
            }
        }
    }
}

pub(crate) fn check_entry(entry: &Entry, report: &mut ValidationReport) {
    for (platform, record) in &entry.platforms {
        check_platform(&entry.id, platform, record, report);
    }
    let mut seen = std::collections::BTreeSet::new();
    for detail in &entry.details {
        let label = format!("{}#{}", entry.id, detail.id);
        if !seen.insert(detail.id.as_str()) {
            report.error("details", &label, "", "", "duplicate detail ID");
        }
        for (platform, record) in &detail.platforms {
            check_platform(&label, platform, record, report);
        }
        if let Some(m) = &detail.spec.motion {
            let problem = match m.curve {
                Curve::Spring if m.response.is_none() && m.duration.is_none() => {
                    Some("spring motion needs response (with damping) or duration (with bounce)")
                }
                Curve::Spring if m.response.is_some() && m.duration.is_some() => {
                    Some("spring motion uses response/damping or duration/bounce, not both")
                }
                Curve::Ease | Curve::Linear if m.duration.is_none() => {
                    Some("ease and linear motion need a duration")
                }
                Curve::Ease | Curve::Linear
                    if m.response.is_some() || m.damping.is_some() || m.bounce.is_some() =>
                {
                    Some("ease and linear motion take only a duration")
                }
                _ => None,
            };
            if let Some(problem) = problem {
                report.error("details", &label, "", "spec.motion", problem);
            }
        }
    }
}
