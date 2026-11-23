//! `concordance log scan-git`: facts stated by commit trailers (`Fixes:`, `Verifies:`,
//! `Reports:`) and release tags matching `[tracker] tag_pattern`, appended to the log.
//! Also the small git helpers the log uses (`rev-parse`, `merge-base --is-ancestor`).
use crate::log::{announce, timestamp_from_unix, Event, EventKind, LogWriter};
use crate::Result;
use std::path::Path;
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
    recorded: usize,
    already: usize,
    warnings: Vec<String>,
}
impl Tally {
    fn add(&mut self, writer: &mut LogWriter, event: Event) -> Result<()> {
        let event = event.with_id();
        if writer.append(&event)? {
            announce(&event, true);
            self.recorded += 1;
        } else {
            self.already += 1;
        }
        Ok(())
    }
}

pub fn scan(writer: &mut LogWriter, repo: &Path, since: Option<&str>) -> Result<()> {
    let mut tally = Tally {
        recorded: 0,
        already: 0,
        warnings: vec![],
    };
    // Commits, oldest first: hash, author, commit time, trailer block.
    let range = since.map(|s| format!("{s}..HEAD"));
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
        let short = &sha[..sha.len().min(12)];
        for line in trailers.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            if !matches!(key.as_str(), "fixes" | "verifies" | "reports") {
                continue;
            }
            for (id, evidence) in trailer_items(value) {
                let Some(issue) = writer.catalog.tracker.issue(&id).cloned() else {
                    tally.warnings.push(format!(
                        "commit {short}: {}: {id} is not a known issue; nothing recorded",
                        key_label(&key)
                    ));
                    continue;
                };
                let event = match key.as_str() {
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
                "--format=%(refname:short)%1f%(objectname)%1f%(*objectname)%1f%(creatordate:unix)%1f%(taggername)%1f%(authorname)",
                "refs/tags",
            ],
        )?;
        let mut tags = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\u{1f}').collect();
            let [name, object, peeled, time, tagger, author] = fields[..] else {
                continue;
            };
            let Some(version) = regexes
                .iter()
                .find_map(|r| r.captures(name).and_then(|c| c.get(1)))
                .map(|m| m.as_str().to_owned())
            else {
                continue;
            };
            let commit = if peeled.is_empty() { object } else { peeled };
            let by = if tagger.is_empty() { author } else { tagger };
            let seconds: i64 = time.trim().parse().unwrap_or(0);
            tags.push((
                seconds,
                name.to_owned(),
                version,
                commit.to_owned(),
                by.to_owned(),
            ));
        }
        tags.sort();
        for (seconds, name, version, commit, by) in tags {
            if let Some(since) = since {
                if is_ancestor(repo, &commit, since)? {
                    continue;
                }
            }
            let at = timestamp_from_unix(seconds);
            let by = if by.trim().is_empty() {
                "unknown".to_owned()
            } else {
                by
            };
            let mut event = Event::new(EventKind::Released, &version, &at, &by);
            event.date = Some(at[..10].to_owned());
            event.commit = Some(commit);
            if let Err(error) = tally.add(writer, event) {
                tally.warnings.push(format!("tag {name}: {error}"));
            }
        }
    }

    for warning in &tally.warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "scan-git: {} recorded, {} already recorded",
        tally.recorded, tally.already
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
