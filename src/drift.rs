use crate::{is_test_path, Catalog, Result, RootOptions};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Serialize)]
pub struct EntryDrift {
    pub id: String,
    pub files: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct PlatformDrift {
    pub platform: String,
    pub since: String,
    pub to: String,
    pub entries: Vec<EntryDrift>,
    pub uncatalogued: Vec<String>,
}
#[derive(Debug, Default, Serialize)]
pub struct DriftReport {
    pub platforms: Vec<PlatformDrift>,
    pub unchecked: Vec<String>,
}
impl DriftReport {
    pub fn markdown(&self) -> String {
        let mut out = String::from(
            "## Baseline drift\n\nChanged baseline evidence may mean target status is stale.\n",
        );
        for platform in &self.platforms {
            out.push_str(&format!(
                "\n### {} (`{}` → `{}`)\n\n",
                platform.platform, platform.since, platform.to
            ));
            if platform.entries.is_empty() {
                out.push_str("No cited evidence changed.\n");
            }
            for entry in &platform.entries {
                out.push_str(&format!(
                    "- `{}`: {}\n",
                    entry.id,
                    entry
                        .files
                        .iter()
                        .map(|p| format!("`{p}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            out.push_str("\nUncatalogued changes:\n\n");
            if platform.uncatalogued.is_empty() {
                out.push_str("None.\n");
            }
            for path in &platform.uncatalogued {
                out.push_str(&format!("- `{path}`\n"));
            }
        }
        for message in &self.unchecked {
            out.push_str(&format!("\n- {message}\n"));
        }
        out
    }
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return crate::fail(format!(
            "git in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

// Root-relative globs: * matches within a component, ** across components, ? one character.
fn glob(pattern: &[u8], path: &[u8]) -> bool {
    match pattern {
        [] => path.is_empty(),
        [b'*', b'*', b'/', rest @ ..] => {
            glob(rest, path)
                || (0..path.len()).any(|i| path[i] == b'/' && glob(rest, &path[i + 1..]))
        }
        [b'*', rest @ ..] => {
            glob(rest, path)
                || (!path.is_empty()
                    && (rest.first() == Some(&b'*') || path[0] != b'/')
                    && glob(pattern, &path[1..]))
        }
        [b'?', rest @ ..] => !path.is_empty() && path[0] != b'/' && glob(rest, &path[1..]),
        [first, rest @ ..] => path.first() == Some(first) && glob(rest, &path[1..]),
    }
}
impl Catalog {
    pub fn baseline_commit(&self, platform: &str) -> Option<&str> {
        self.config
            .baselines
            .get(platform)
            .or_else(|| {
                (platform == self.config.report.baseline)
                    .then_some(self.config.baseline.as_ref())
                    .flatten()
            })
            .map(|b| b.commit.as_str())
            .or_else(|| match self.config.external_roots.get(platform) {
                Some(crate::ExternalRoot::Details {
                    commit: Some(commit),
                    ..
                }) => Some(commit.as_str()),
                _ => None,
            })
    }
    pub fn drift(
        &self,
        options: &RootOptions,
        platform: Option<&str>,
        since: Option<&str>,
        to: &str,
    ) -> Result<DriftReport> {
        if platform.is_some_and(|p| !self.config.platforms.contains_key(p)) {
            return crate::fail("unknown platform");
        }
        let mut report = DriftReport::default();
        for name in self
            .config
            .platforms
            .keys()
            .filter(|p| platform.is_none_or(|wanted| wanted == p.as_str()))
        {
            let Some(commit) = since.or_else(|| self.baseline_commit(name)) else {
                report
                    .unchecked
                    .push(format!("{name}: unchecked (baseline commit not supplied)"));
                continue;
            };
            let Some(root) = self.platform_root(name, options)? else {
                if options.require_roots {
                    return crate::fail(format!("{name}: root not supplied"));
                }
                report.unchecked.push(format!(
                    "{name}: unchecked (root not supplied). Supply --root {name}=<path>. {}",
                    self.config.external_roots[name].hint()
                ));
                continue;
            };
            // Resolve revisions first so values beginning with '-' cannot become diff options.
            let resolve = |revision: &str| -> Result<String> {
                let value = git(
                    &root,
                    &[
                        "rev-parse",
                        "--verify",
                        "--end-of-options",
                        &format!("{revision}^{{commit}}"),
                    ],
                )?;
                Ok(String::from_utf8(value)?.trim().to_owned())
            };
            let from = resolve(commit)?;
            let end = resolve(to)?;
            let bytes = git(
                &root,
                &[
                    "diff",
                    "--name-only",
                    "--no-renames",
                    "-z",
                    &from,
                    &end,
                    "--",
                ],
            )?;
            // Git paths are repository-relative; a platform root may be a repository subdirectory.
            let prefix = String::from_utf8(git(&root, &["rev-parse", "--show-prefix"])?)?;
            let prefix = prefix.trim_end_matches('\n');
            let changed: BTreeSet<String> = bytes
                .split(|b| *b == 0)
                .filter(|b| !b.is_empty())
                .map(|b| String::from_utf8(b.to_vec()))
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .filter_map(|p| p.strip_prefix(prefix).map(str::to_owned))
                .collect();
            let mut cited = BTreeSet::new();
            let mut entries = Vec::new();
            for entry in self.entries.values() {
                let Some(evidence) = entry.platforms.get(name) else {
                    continue;
                };
                let mut files = BTreeSet::new();
                for reference in evidence
                    .source
                    .iter()
                    .chain(evidence.tests.iter().filter(|s| is_test_path(s)))
                {
                    let relative = self.evidence_relative(name, reference)?;
                    cited.insert(relative.to_owned());
                    if changed.contains(relative) {
                        files.insert(relative.to_owned());
                    }
                }
                if !files.is_empty() {
                    entries.push(EntryDrift {
                        id: entry.id.clone(),
                        files: files.into_iter().collect(),
                    });
                }
            }
            let patterns = self.config.source_globs.get(name);
            let uncatalogued = changed
                .into_iter()
                .filter(|p| {
                    !cited.contains(p)
                        && patterns.is_some_and(|patterns| {
                            patterns.iter().any(|g| glob(g.as_bytes(), p.as_bytes()))
                        })
                })
                .collect();
            report.platforms.push(PlatformDrift {
                platform: name.clone(),
                since: commit.into(),
                to: to.into(),
                entries,
                uncatalogued,
            });
        }
        Ok(report)
    }
    pub fn baseline_drift_section(&self, options: &RootOptions) -> Result<String> {
        let platform = &self.config.report.baseline;
        if self.baseline_commit(platform).is_none() {
            return Ok(String::new());
        }
        match self.platform_root(platform, options) {
            Ok(Some(_)) => (),
            Ok(None) => return Ok(String::new()),
            Err(_) if !options.roots.contains_key(platform) => return Ok(String::new()),
            Err(error) => return Err(error),
        }
        Ok(format!(
            "\n{}",
            self.drift(options, Some(platform), None, "HEAD")?
                .markdown()
        ))
    }
    pub fn report_with_roots(&self, options: &RootOptions) -> Result<String> {
        Ok(self.report()?
            + &self.evidence_roots_section(options)?
            + &self.baseline_drift_section(options)?)
    }
}
