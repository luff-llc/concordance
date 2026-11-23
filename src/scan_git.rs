//! `concordance log scan-git`: facts stated by commit trailers (`Fixes:`, `Verifies:`,
//! `Reports:`) and release tags matching `[tracker] tag_pattern`, appended to the log.
//! Also the small git helpers the log uses (`rev-parse`, `merge-base --is-ancestor`).
use crate::log::{announce, timestamp_from_unix, Event, EventKind, LogWriter};
use crate::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run git in `repo` and return stdout. Errors name git's stderr, or say git is unavailable.
pub(crate) fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git unavailable: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether `ancestor` is contained in `descendant`. Errors when git cannot answer
/// (git missing, not a repository, unknown commit).
pub(crate) fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|e| format!("git unavailable: {e}"))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git merge-base --is-ancestor {ancestor} {descendant}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into()),
    }
}

/// Ancestry answers for one repository, cached. In a shallow clone a "not an ancestor"
/// answer may only mean the history is cut off, so it becomes unknown (`None`).
pub(crate) struct Ancestry {
    repo: PathBuf,
    shallow: Option<bool>,
    cache: BTreeMap<(String, String), Option<bool>>,
    resolved: BTreeMap<String, Option<String>>,
}
impl Ancestry {
    pub(crate) fn new(repo: &Path) -> Self {
        Self {
            repo: repo.to_path_buf(),
            shallow: None,
            cache: BTreeMap::new(),
            resolved: BTreeMap::new(),
        }
    }
    /// A ref as a full commit hash (`rev-parse --verify --end-of-options REF^{commit}`), or
    /// `None` when git cannot resolve it unambiguously. Errors only when git is missing.
    pub(crate) fn resolve(&mut self, reference: &str) -> Result<Option<String>> {
        if let Some(answer) = self.resolved.get(reference) {
            return Ok(answer.clone());
        }
        if self.ambiguous(reference)? {
            self.resolved.insert(reference.to_owned(), None);
            return Ok(None);
        }
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["rev-parse", "--verify", "--quiet", "--end-of-options"])
            .arg(format!("{reference}^{{commit}}"))
            .output()
            .map_err(|e| format!("git unavailable: {e}"))?;
        let full = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let answer = (output.status.success() && !full.is_empty()).then_some(full);
        self.resolved.insert(reference.to_owned(), answer.clone());
        Ok(answer)
    }
    /// A short name (not a full `refs/...` name, not a full 40- or 64-digit hash) that
    /// more than one of refs/heads/X, refs/tags/X and refs/remotes/X names, or, when it
    /// looks like hex, that names a ref and also abbreviates an object. Git would pick one
    /// silently.
    fn ambiguous(&self, reference: &str) -> Result<bool> {
        let base = reference
            .split(['~', '^', '@', ':'])
            .next()
            .unwrap_or(reference);
        let hex = !base.is_empty() && base.bytes().all(|c| c.is_ascii_hexdigit());
        if base.is_empty()
            || base == "HEAD"
            || base.starts_with("refs/")
            || (hex && matches!(base.len(), 40 | 64))
        {
            return Ok(false);
        }
        let candidates = [
            format!("refs/heads/{base}"),
            format!("refs/tags/{base}"),
            format!("refs/remotes/{base}"),
        ];
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["for-each-ref", "--format=%(refname)"])
            .args(&candidates)
            .output()
            .map_err(|e| format!("git unavailable: {e}"))?;
        let listed = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut found = listed
            .lines()
            .filter(|line| candidates.iter().any(|c| c == line))
            .count();
        if hex && found > 0 && base.len() >= 4 {
            // Objects this name abbreviates, besides the refs it names.
            let objects = Command::new("git")
                .arg("-C")
                .arg(&self.repo)
                .arg("rev-parse")
                .arg(format!("--disambiguate={base}"))
                .output()
                .map_err(|e| format!("git unavailable: {e}"))?;
            if !String::from_utf8_lossy(&objects.stdout).trim().is_empty() {
                found += 1;
            }
        }
        Ok(found > 1)
    }
    pub(crate) fn is_shallow(&mut self) -> bool {
        if self.shallow.is_none() {
            let shallow = git(&self.repo, &["rev-parse", "--is-shallow-repository"])
                .is_ok_and(|out| out.trim() == "true");
            self.shallow = Some(shallow);
        }
        self.shallow == Some(true)
    }
    /// Whether `descendant` contains `ancestor`: `Some(true)`, `Some(false)`, or `None`
    /// when a shallow history cannot say. Errors when git cannot answer at all.
    pub(crate) fn contains(&mut self, ancestor: &str, descendant: &str) -> Result<Option<bool>> {
        let key = (ancestor.to_owned(), descendant.to_owned());
        if let Some(answer) = self.cache.get(&key) {
            return Ok(*answer);
        }
        let answer = match is_ancestor(&self.repo, ancestor, descendant)? {
            true => Some(true),
            false if self.is_shallow() => None,
            false => Some(false),
        };
        self.cache.insert(key, answer);
        Ok(answer)
    }
}

/// Resolve `--since` to a commit hash, refusing option-shaped refs.
fn resolve_since(repo: &Path, since: &str) -> Result<String> {
    if since.starts_with('-') || since.trim().is_empty() {
        return Err(format!("--since {since:?} is not a git ref").into());
    }
    Ancestry::new(repo).resolve(since)?.ok_or_else(|| {
        format!("--since {since:?} does not name exactly one commit (unknown or ambiguous)").into()
    })
}

/// A tag glob with one `{version}` placeholder as a regex; `*` matches within one path
/// segment. The version must start with a digit.
pub fn tag_regex(pattern: &str) -> Result<regex::Regex> {
    let pieces: Vec<&str> = pattern.split("{version}").collect();
    if pieces.len() != 2 {
        return Err(format!("tag_pattern {pattern:?} needs exactly one {{version}}").into());
    }
    let literal = |text: &str| {
        text.split('*')
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join("[^/]*")
    };
    let source = format!(
        "^{}([0-9][0-9A-Za-z.+_-]*){}$",
        literal(pieces[0]),
        literal(pieces[1])
    );
    Ok(regex::Regex::new(&source)?)
}

/// One trailer item: an ID and optional parenthesised evidence.
fn trailer_items(value: &str) -> Vec<(String, Option<String>)> {
    // Split on commas outside parentheses.
    let mut items = Vec::new();
    let (mut depth, mut current) = (0usize, String::new());
    for c in value.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => items.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    items.push(current);
    items
        .into_iter()
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let end = item
                .find(|c: char| c.is_whitespace() || c == '(')
                .unwrap_or(item.len());
            let (id, rest) = item.split_at(end);
            let rest = rest.trim();
            let evidence = rest
                .strip_prefix('(')
                .and_then(|r| r.strip_suffix(')'))
                .map(|e| e.trim().to_owned())
                .filter(|e| !e.is_empty());
            Some((id.to_owned(), evidence))
        })
        .collect()
}

struct Tally {
    seen: usize,
    warnings: Vec<String>,
}
impl Tally {
    /// Queue an event; everything is written in one append at the end.
    fn add(&mut self, writer: &mut LogWriter, event: Event) -> Result<()> {
        writer.append(&event.with_id())?;
        self.seen += 1;
        Ok(())
    }
}

pub fn scan(writer: &mut LogWriter, repo: &Path, since: Option<&str>) -> Result<()> {
    let mut tally = Tally {
        seen: 0,
        warnings: vec![],
    };
    let since = since.map(|s| resolve_since(repo, s)).transpose()?;
    let mut ancestry = Ancestry::new(repo);
    // Commits, oldest first: hash, author, commit time, trailer block.
    let range = since.as_ref().map(|s| format!("{s}..HEAD"));
    let mut args = vec![
        "log",
        "--reverse",
        "--format=%H%x1f%an%x1f%ct%x1f%(trailers:only,unfold)%x1e",
    ];
    if let Some(range) = &range {
        args.push(range);
    }
    let text = git(repo, &args)?;
    for record in text.split('\u{1e}') {
        let record = record.trim_start_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.splitn(4, '\u{1f}').collect();
        let [sha, author, time, trailers] = fields[..] else {
            continue;
        };
        let at = timestamp_from_unix(time.trim().parse().unwrap_or(0));
        let short = crate::log::prefix(sha, 12);
        // Every Fixes: in a commit before its Verifies:, whatever the trailer order, so in
        // file order a commit's fix comes before its check.
        let mut items: Vec<(u8, String, String, Option<String>)> = Vec::new();
        for line in trailers.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let rank = match key.as_str() {
                "fixes" => 0,
                "verifies" => 1,
                "reports" => 2,
                _ => continue,
            };
            for (id, evidence) in trailer_items(value) {
                items.push((rank, key.clone(), id, evidence));
            }
        }
        items.sort_by_key(|(rank, ..)| *rank);
        // Each Verifies: item for an issue is its own fact: number them per issue.
        let mut verifies_seen: BTreeMap<String, usize> = BTreeMap::new();
        {
            for (_, key, id, evidence) in items {
                let Some(issue) = writer.catalog.tracker.issue(&id).cloned() else {
                    tally.warnings.push(format!(
                        "commit {short}: {}: {id} is not a known issue; nothing recorded",
                        key_label(&key)
                    ));
                    continue;
                };
                let mut event = match key.as_str() {
                    "fixes" => {
                        let mut e = Event::new(EventKind::Fixed, &id, &at, author);
                        e.commit = Some(sha.to_owned());
                        e
                    }
                    "verifies" => {
                        let mut e = Event::new(EventKind::Verified, &id, &at, author);
                        e.evidence = vec![evidence.unwrap_or_else(|| format!("commit {short}"))];
                        e.commit = Some(sha.to_owned());
                        e
                    }
                    _ => {
                        let mut e = Event::new(EventKind::Reported, &id, &at, author);
                        e.title = Some(issue.title.clone());
                        e
                    }
                };
                // The source key: one fact per commit, trailer kind and issue, whatever
                // the log says since (a pull or reopen never makes a rescan re-add it).
                event.key = Some(if key == "verifies" {
                    let n = verifies_seen.entry(id.clone()).or_default();
                    *n += 1;
                    format!("git:trailer:{sha}:Verifies:{id}:{n}")
                } else {
                    format!("git:trailer:{sha}:{}:{id}", key_label(&key))
                });
                tally.add(writer, event)?;
            }
        }
    }

    // Release tags.
    let patterns: Vec<String> = writer
        .catalog
        .config
        .tracker
        .as_ref()
        .map(|t| t.tag_pattern.patterns().to_vec())
        .unwrap_or_default();
    let regexes = patterns
        .iter()
        .map(|p| tag_regex(p))
        .collect::<Result<Vec<_>>>()?;
    if !regexes.is_empty() {
        let text = git(
            repo,
            &[
                "for-each-ref",
                "--format=%(refname)%1f%(creatordate:unix)%1f%(taggername)%1f%(authorname)",
                "refs/tags",
            ],
        )?;
        let mut tags = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\u{1f}').collect();
            let [refname, time, tagger, author] = fields[..] else {
                continue;
            };
            // The full refname, so a branch with the same short name cannot stand in.
            let Some(name) = refname.strip_prefix("refs/tags/") else {
                continue;
            };
            let Some(version) = regexes
                .iter()
                .find_map(|r| r.captures(name).and_then(|c| c.get(1)))
                .map(|m| m.as_str().to_owned())
            else {
                continue;
            };
            // Peel to a commit; a tag of a tree or blob is never a release.
            let Some(commit) = ancestry.resolve(refname)? else {
                tally
                    .warnings
                    .push(format!("tag {name} does not point at a commit; skipped"));
                continue;
            };
            let by = if tagger.is_empty() { author } else { tagger };
            let seconds: i64 = time.trim().parse().unwrap_or(0);
            tags.push((seconds, refname.to_owned(), version, commit, by.to_owned()));
        }
        tags.sort();
        for (seconds, refname, version, commit, by) in tags {
            let name = refname.strip_prefix("refs/tags/").unwrap_or(&refname);
            if let Some(since) = &since {
                match ancestry.contains(&commit, since)? {
                    Some(true) => continue,
                    Some(false) => {}
                    None => tally.warnings.push(format!(
                        "tag {name}: shallow history cannot tell whether --since contains it; scanned"
                    )),
                }
            }
            let at = timestamp_from_unix(seconds);
            let by = if by.trim().is_empty() {
                "unknown".to_owned()
            } else {
                by
            };
            let mut event = Event::new(EventKind::Released, &version, &at, &by);
            event.date = Some(crate::log::day(&at));
            event.key = Some(format!("git:tag:{refname}:{commit}"));
            event.commit = Some(commit);
            if let Err(error) = tally.add(writer, event) {
                tally.warnings.push(format!("tag {name}: {error}"));
            }
        }
    }

    let results = writer.flush()?;
    let recorded = results.iter().filter(|(_, added)| *added).count();
    for (event, _) in results.iter().filter(|(_, added)| *added) {
        announce(event, true);
    }
    for warning in &tally.warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "scan-git: {recorded} recorded, {} already recorded",
        tally.seen - recorded
    );
    Ok(())
}

fn key_label(key: &str) -> &'static str {
    match key {
        "fixes" => "Fixes",
        "verifies" => "Verifies",
        _ => "Reports",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn items() {
        let items = trailer_items(" bug.a (tests 13/13, journey), request.b ,bug.c");
        assert_eq!(
            items,
            vec![
                ("bug.a".to_owned(), Some("tests 13/13, journey".to_owned())),
                ("request.b".to_owned(), None),
                ("bug.c".to_owned(), None),
            ]
        );
    }
    #[test]
    fn tags() {
        let r = tag_regex("ios/v{version}").expect("pattern");
        assert_eq!(
            r.captures("ios/v1.2.0")
                .and_then(|c| c.get(1))
                .map(|m| m.as_str()),
            Some("1.2.0")
        );
        assert!(!r.is_match("android/v1.2.0"));
        assert!(!r.is_match("ios/vnext"));
        let r = tag_regex("taupe-{version}").expect("pattern");
        assert!(r.is_match("taupe-1.0"));
        let r = tag_regex("*/v{version}").expect("pattern");
        assert!(r.is_match("ios/v2.0.0"));
        assert!(!r.is_match("a/b/v2.0.0"));
        assert!(tag_regex("v1").is_err());
    }
}
