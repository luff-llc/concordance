use crate::{is_test_path, safe_relative, Catalog, Result};
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
impl Catalog {
    pub(crate) fn evidence_relative<'a>(
        &self,
        platform: &str,
        reference: &'a str,
    ) -> Result<&'a str> {
        let path = reference
            .split_once('#')
            .map(|(p, _)| p)
            .unwrap_or(reference);
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
        for entry in self.entries.values() {
            for (platform, evidence) in &entry.platforms {
                for reference in evidence
                    .source
                    .iter()
                    .chain(evidence.tests.iter().filter(|s| is_test_path(s)))
                {
                    let relative = match self.evidence_relative(platform, reference) {
                        Ok(path) => path,
                        Err(error) => {
                            report.error(
                                "evidence",
                                &entry.id,
                                platform,
                                reference,
                                &error.to_string(),
                            );
                            continue;
                        }
                    };
                    let Some(root) = roots.get(platform) else {
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
                            report.error("evidence", &entry.id, platform, reference, &message);
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
                                    &entry.id,
                                    platform,
                                    reference,
                                    &format!("root not supplied. {message}"),
                                );
                            } else {
                                report.unchecked.push(Issue {
                                    group: "roots".into(),
                                    entry: entry.id.clone(),
                                    platform: platform.clone(),
                                    reference: reference.clone(),
                                    message,
                                });
                            }
                        }
                        Err(error) => report.error(
                            "roots",
                            &entry.id,
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
