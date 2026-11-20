//! `version-check`: every platform ships the shared marketing version.
use crate::{fail, Catalog, Result, RootOptions, VersionCheck};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct VersionResult {
    pub platform: String,
    /// What was checked, for example `file android/app/build.gradle.kts` or `command …`.
    pub check: String,
    pub found: Option<String>,
    pub ok: bool,
    pub message: String,
}
#[derive(Debug, Default, Serialize)]
pub struct VersionReport {
    pub source: String,
    pub version: String,
    pub results: Vec<VersionResult>,
    pub unchecked: Vec<String>,
}
impl VersionReport {
    pub fn passed(&self) -> bool {
        self.results.iter().all(|r| r.ok)
    }
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "## Version check\n\nSource `{}`: **{}**. {}.\n\n| Platform | Check | Found | Result |\n| --- | --- | --- | --- |\n",
            self.source,
            self.version,
            if self.passed() { "All platforms match" } else { "Mismatch" }
        );
        for r in &self.results {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                r.platform,
                r.check.replace('|', "\\|"),
                r.found.as_deref().unwrap_or("—").replace('|', "\\|"),
                r.message.replace('|', "\\|")
            ));
        }
        for line in &self.unchecked {
            out.push_str(&format!("\n- {line}\n"));
        }
        out
    }
}

fn run(command: &str, dir: &Path, timeout: Duration) -> Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return fail(format!("timed out after {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut stdout)?;
    }
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            err.read_to_string(&mut stderr)?;
        }
        return fail(format!("exited with {status}: {}", stderr.trim()));
    }
    stdout
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "printed nothing".into())
}

impl Catalog {
    /// Compare every configured platform (or one) with the shared version source.
    pub fn version_check(
        &self,
        options: &RootOptions,
        platform: Option<&str>,
    ) -> Result<VersionReport> {
        let config = self
            .config
            .version
            .as_ref()
            .ok_or("catalog.toml has no [version] section")?;
        let repo = options
            .repo
            .clone()
            .unwrap_or_else(|| self.directory.join(&self.config.repo_root));
        let source_path = repo.join(&config.source);
        let version = fs::read_to_string(&source_path)
            .map_err(|e| format!("{}: {e}", source_path.display()))?
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .ok_or("version source is empty")?
            .to_owned();
        let mut report = VersionReport {
            source: config.source.clone(),
            version: version.clone(),
            ..Default::default()
        };
        if platform.is_some_and(|p| !self.config.platforms.contains_key(p)) {
            return fail(format!(
                "unknown platform: {}",
                platform.unwrap_or_default()
            ));
        }
        for (name, checks) in &config.platforms {
            if platform.is_some_and(|p| p != name) {
                continue;
            }
            let root = match self.platform_root(name, options) {
                Ok(Some(root)) => root,
                Ok(None) if !options.require_roots => {
                    report.unchecked.push(format!(
                        "{name}: unchecked (root not supplied); use --root {name}=<path>"
                    ));
                    continue;
                }
                Ok(None) => return fail(format!("{name}: root not supplied")),
                Err(error) => return Err(error),
            };
            for check in checks.checks() {
                report
                    .results
                    .push(self.one_check(name, check, &root, &config.source, &version));
            }
        }
        Ok(report)
    }

    fn one_check(
        &self,
        platform: &str,
        check: &VersionCheck,
        root: &Path,
        source: &str,
        version: &str,
    ) -> VersionResult {
        let mut result = VersionResult {
            platform: platform.into(),
            check: String::new(),
            found: None,
            ok: false,
            message: String::new(),
        };
        let outcome = (|| -> Result<Option<String>> {
            if let Some(command) = &check.command {
                result.check = format!("command `{command}`");
                let timeout = Duration::from_secs(check.timeout_seconds.unwrap_or(300));
                return Ok(Some(run(command, root, timeout)?));
            }
            let file = check
                .file
                .as_ref()
                .ok_or("a version check needs file or command")?;
            if !crate::safe_relative(file, false) {
                return fail(format!("unsafe version file path {file}"));
            }
            let text = fs::read_to_string(root.join(file)).map_err(|e| format!("{file}: {e}"))?;
            let pattern = check
                .pattern
                .as_deref()
                .map(regex::Regex::new)
                .transpose()
                .map_err(|e| format!("invalid pattern: {e}"))?;
            result.check = format!(
                "{} `{file}`{}",
                if check.reads_source {
                    "reads source:"
                } else {
                    "file"
                },
                check
                    .pattern
                    .as_ref()
                    .map(|p| format!(" /{p}/"))
                    .unwrap_or_default()
            );
            if check.reads_source {
                let name = Path::new(source)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(source);
                let wired = match &pattern {
                    Some(re) => re.find(&text).map(|m| m.as_str().to_owned()),
                    None => [source, name]
                        .into_iter()
                        .find(|n| text.contains(*n))
                        .map(str::to_owned),
                };
                return match wired {
                    Some(found) => Ok(Some(found)),
                    None => fail(format!("does not reference {source}")),
                };
            }
            Ok(Some(match &pattern {
                Some(re) => re
                    .captures(&text)
                    .and_then(|c| c.get(1).or_else(|| c.get(0)))
                    .map(|m| m.as_str().trim().to_owned())
                    .ok_or("pattern did not match")?,
                None => text
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("")
                    .to_owned(),
            }))
        })();
        match outcome {
            Ok(found) if check.reads_source => {
                result.ok = true;
                result.message = "reads the source".into();
                result.found = found;
            }
            Ok(Some(found)) => {
                result.ok = found == version;
                result.message = if result.ok {
                    "matches".into()
                } else {
                    format!("expected {version}")
                };
                result.found = Some(found);
            }
            Ok(None) => result.message = "no version found".into(),
            Err(error) => result.message = error.to_string(),
        }
        result
    }
}
