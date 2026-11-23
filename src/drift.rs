use crate::{Catalog, Result, RootOptions};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// How precisely a changed evidence reference was matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    /// An anchored reference (`#Symbol` or `:Lstart-Lend`) whose lines changed.
    Precise,
    /// A file-level reference (or an unresolvable anchor) in a changed file.
    Coarse,
}
#[derive(Debug, Clone, Serialize)]
pub struct EvidenceHit {
    pub reference: String,
    pub file: String,
    pub precision: Precision,
    /// Baseline line span that was matched, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<(u32, u32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct EntryDrift {
    /// Entry ID, or `entry#detail` for detail evidence.
    pub id: String,
    pub files: Vec<String>,
    pub hits: Vec<EvidenceHit>,
}
impl EntryDrift {
    pub fn precise(&self) -> bool {
        self.hits.iter().any(|h| h.precision == Precision::Precise)
    }
}
#[derive(Debug, Serialize)]
pub struct PlatformDrift {
    pub platform: String,
    pub since: String,
    pub to: String,
    pub entries: Vec<EntryDrift>,
    pub uncatalogued: Vec<String>,
    /// Reference-level counts.
    pub precise_hits: usize,
    pub coarse_hits: usize,
    /// Anchored references in changed files whose anchored lines did not change.
    pub anchored_unaffected: usize,
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
            let precise_entries = platform.entries.iter().filter(|e| e.precise()).count();
            out.push_str(&format!(
                "\n### {} (`{}` → `{}`)\n\n",
                platform.platform, platform.since, platform.to
            ));
            out.push_str(&format!(
                "{} entries: {precise_entries} with precise hits, {} file-level (coarse) only. References: {} precise, {} coarse; {} anchored references in changed files were unaffected.\n\n",
                platform.entries.len(),
                platform.entries.len() - precise_entries,
                platform.precise_hits,
                platform.coarse_hits,
                platform.anchored_unaffected
            ));
            if platform.entries.is_empty() {
                out.push_str("No cited evidence changed.\n");
            }
            for entry in &platform.entries {
                out.push_str(&format!(
                    "- `{}`: {}\n",
                    entry.id,
                    entry
                        .hits
                        .iter()
                        .map(|h| {
                            let mut text = format!("`{}`", h.reference);
                            match (h.precision, h.lines) {
                                (Precision::Precise, Some((a, b))) => {
                                    text.push_str(&format!(" precise (L{a}-L{b})"))
                                }
                                (Precision::Precise, None) => text.push_str(" precise"),
                                (Precision::Coarse, _) => text.push_str(" file-level (coarse)"),
                            }
                            if let Some(note) = &h.note {
                                text.push_str(&format!(" — {note}"));
                            }
                            text
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
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

/// Old-side hunk ranges from `git diff -U0`: (start, count); count 0 is an insertion after start.
pub(crate) fn hunks(diff: &str) -> Vec<(u32, u32)> {
    diff.lines()
        .filter_map(|line| line.strip_prefix("@@ -"))
        .filter_map(|rest| {
            let old = rest.split_whitespace().next()?;
            let (start, count) = old.split_once(',').unwrap_or((old, "1"));
            Some((start.parse().ok()?, count.parse().ok()?))
        })
        .collect()
}

/// Whether a hunk touches the inclusive line span [start, end].
pub(crate) fn touches(hunk: (u32, u32), span: (u32, u32)) -> bool {
    let (a, count) = hunk;
    if count == 0 {
        // Lines inserted after line `a` land inside the span only between its lines.
        span.0 <= a && a < span.1
    } else {
        a <= span.1 && a + count > span.0
    }
}

const DECLARATION_KEYWORDS: &[&str] = &[
    "func",
    "fun",
    "class",
    "struct",
    "enum",
    "protocol",
    "extension",
    "interface",
    "object",
    "var",
    "let",
    "val",
    "const",
    "def",
    "fn",
    "type",
    "typealias",
    "case",
    "init",
    "actor",
    "trait",
    "impl",
    "mod",
    "function",
    "macro",
    "record",
];

fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| !w.is_empty())
}

/// Heuristic declaration spans for `symbol` in source text (1-based, inclusive).
///
/// A declaration line contains the symbol as a whole word after a declaration keyword
/// (`func`, `fun`, `class`, `struct`, `var`, `val`, `def`, `fn`, ...). Its block is
/// brace-delimited when a `{` opens on the declaration line or on a continuation line of
/// its signature (open parentheses, a trailing `,` `(` `->` `:` `=`, or a next line starting
/// with `{`, `where`, `throws` or `->`); braces in `//` comments and double-quoted strings
/// are ignored. Otherwise
/// it is the following lines indented deeper than the declaration. Without a block,
/// the span is the declaration line alone.
pub fn symbol_spans(text: &str, symbol: &str) -> Vec<(u32, u32)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let tokens: Vec<&str> = words(line).collect();
        let Some(position) = tokens.iter().position(|w| *w == symbol) else {
            continue;
        };
        if !tokens[..position]
            .iter()
            .any(|w| DECLARATION_KEYWORDS.contains(w))
        {
            continue;
        }
        spans.push((index as u32 + 1, block_end(&lines, index) as u32 + 1));
    }
    spans
}

/// Type and function declarations in source text: (name, start, end), 1-based inclusive,
/// using the same block heuristic as `symbol_spans`. Variables and cases are skipped.
pub(crate) fn declarations(text: &str) -> Vec<(String, u32, u32)> {
    const TYPE_OR_FUNCTION: &[&str] = &[
        "func",
        "fun",
        "class",
        "struct",
        "enum",
        "protocol",
        "extension",
        "interface",
        "object",
        "def",
        "fn",
        "actor",
        "trait",
        "impl",
        "function",
        "record",
        "mod",
    ];
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let tokens: Vec<&str> = words(line).collect();
        if let Some(position) = tokens.iter().position(|w| TYPE_OR_FUNCTION.contains(w)) {
            if let Some(name) = tokens.get(position + 1) {
                out.push((
                    (*name).to_owned(),
                    index as u32 + 1,
                    block_end(&lines, index) as u32 + 1,
                ));
            }
        }
    }
    out
}

fn block_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i64;
    let mut parens = 0i64;
    let mut opened = false;
    for (offset, line) in lines[start..].iter().enumerate() {
        let mut in_string = false;
        let mut previous = '\0';
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if in_string {
                if c == '"' && previous != '\\' {
                    in_string = false;
                }
            } else if c == '"' {
                in_string = true;
            } else if c == '/' && chars.peek() == Some(&'/') {
                break;
            } else if c == '(' {
                parens += 1;
            } else if c == ')' {
                parens -= 1;
            } else if c == '{' {
                depth += 1;
                opened = true;
            } else if c == '}' {
                depth -= 1;
            }
            previous = c;
        }
        if opened && depth <= 0 {
            return start + offset;
        }
        if !opened {
            // Look ahead only through a multi-line signature or an Allman-style brace.
            let trimmed = line.split("//").next().unwrap_or("").trim_end();
            let next = lines.get(start + offset + 1).map(|l| l.trim_start());
            let continues = parens > 0
                || [",", "(", "->", ":", "="]
                    .iter()
                    .any(|t| trimmed.ends_with(t))
                || next.is_some_and(|n| {
                    n.starts_with('{')
                        || n.starts_with("where")
                        || n.starts_with("throws")
                        || n.starts_with("->")
                });
            if !continues || offset >= 8 {
                break;
            }
        }
    }
    if opened {
        return lines.len().saturating_sub(1);
    }
    let indent = |l: &str| l.len() - l.trim_start().len();
    let base = indent(lines[start]);
    let mut end = start;
    for (offset, line) in lines[start + 1..].iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if indent(line) <= base {
            break;
        }
        end = start + 1 + offset;
    }
    end
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
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
pub(crate) fn glob(pattern: &[u8], path: &[u8]) -> bool {
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
            let mut by_label: BTreeMap<String, Vec<EvidenceHit>> = BTreeMap::new();
            let (mut precise_hits, mut coarse_hits, mut anchored_unaffected) = (0, 0, 0);
            let mut file_hunks: BTreeMap<String, Option<Vec<(u32, u32)>>> = BTreeMap::new();
            let mut file_text: BTreeMap<String, Option<String>> = BTreeMap::new();
            for (label, platform, reference) in self.evidence_items() {
                if platform != name {
                    continue;
                }
                let relative = self.evidence_relative(name, reference)?;
                cited.insert(relative.to_owned());
                if !changed.contains(relative) {
                    continue;
                }
                let anchor = crate::split_anchor(reference).1;
                let hit = |precision, lines, note: Option<String>| EvidenceHit {
                    reference: reference.clone(),
                    file: relative.to_owned(),
                    precision,
                    lines,
                    note,
                };
                let Some(anchor) = anchor else {
                    coarse_hits += 1;
                    by_label
                        .entry(label)
                        .or_default()
                        .push(hit(Precision::Coarse, None, None));
                    continue;
                };
                if !file_hunks.contains_key(relative) {
                    // Pathspecs are relative to the platform root (the git working directory).
                    let diff = git(
                        &root,
                        &[
                            "diff",
                            "-U0",
                            "--no-renames",
                            "--no-color",
                            &from,
                            &end,
                            "--",
                            relative,
                        ],
                    )?;
                    file_hunks.insert(
                        relative.to_owned(),
                        Some(hunks(&String::from_utf8_lossy(&diff))),
                    );
                    let text = git(&root, &["show", &format!("{from}:./{relative}")])
                        .ok()
                        .map(|b| String::from_utf8_lossy(&b).into_owned());
                    file_text.insert(relative.to_owned(), text);
                }
                let changes = file_hunks[relative].as_deref().unwrap_or_default();
                let spans = match &anchor {
                    crate::Anchor::Lines(a, b) => Some(vec![(*a, *b)]),
                    crate::Anchor::Symbol(symbol) => file_text[relative]
                        .as_deref()
                        .map(|text| symbol_spans(text, crate::Anchor::symbol_name(symbol))),
                };
                let spans = spans.unwrap_or_default();
                if spans.is_empty() {
                    coarse_hits += 1;
                    let note = match &anchor {
                        crate::Anchor::Symbol(_) => "symbol not found at baseline",
                        crate::Anchor::Lines(..) => "baseline file unavailable",
                    };
                    by_label.entry(label).or_default().push(hit(
                        Precision::Coarse,
                        None,
                        Some(note.into()),
                    ));
                    continue;
                }
                let deleted =
                    git(&root, &["cat-file", "-e", &format!("{end}:./{relative}")]).is_err();
                match spans
                    .iter()
                    .find(|span| changes.iter().any(|h| touches(*h, **span)))
                {
                    Some(span) => {
                        precise_hits += 1;
                        by_label.entry(label).or_default().push(hit(
                            Precision::Precise,
                            Some(*span),
                            deleted.then(|| "file deleted".into()),
                        ));
                    }
                    None => anchored_unaffected += 1,
                }
            }
            let entries: Vec<EntryDrift> = by_label
                .into_iter()
                .map(|(id, hits)| EntryDrift {
                    id,
                    files: hits
                        .iter()
                        .map(|h| h.file.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                    hits,
                })
                .collect();
            let patterns = self.config.source_globs.get(name).map(|g| g.all());
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
                precise_hits,
                coarse_hits,
                anchored_unaffected,
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
        self.report_with_roots_at(options, None)
    }
    /// Like `report_with_roots`, with links relative to the directory of `out`.
    pub fn report_with_roots_at(
        &self,
        options: &RootOptions,
        out: Option<&Path>,
    ) -> Result<String> {
        let context = crate::ReportContext {
            link_base: out.map(crate::report::link_base),
            ..Default::default()
        };
        Ok(self.report_with(&context)?
            + &self.evidence_roots_section(options)?
            + &self.baseline_drift_section(options)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hunk_parsing_and_intersection() {
        let diff = "@@ -3 +3 @@\n-a\n+b\n@@ -10,0 +11,2 @@\n+c\n@@ -20,4 +22,0 @@\n";
        assert_eq!(hunks(diff), [(3, 1), (10, 0), (20, 4)]);
        assert!(touches((3, 1), (1, 3)));
        assert!(!touches((4, 1), (1, 3)));
        assert!(touches((10, 0), (5, 12)));
        assert!(!touches((12, 0), (5, 12)));
        assert!(touches((20, 4), (23, 30)));
    }
    #[test]
    fn symbol_spans_use_braces_then_indentation() {
        let swift = "// pause() is here\nfinal class Player {\n    func pause() { // {\n        let s = \"}\"\n    }\n}\n";
        assert_eq!(symbol_spans(swift, "pause"), [(3, 5)]);
        assert_eq!(symbol_spans(swift, "Player"), [(2, 6)]);
        let python = "def outer():\n    x = 1\n\n    return x\ny = 2\n";
        assert_eq!(symbol_spans(python, "outer"), [(1, 4)]);
        let kotlin = "val threshold = 0.4f\nfun other() {}\n";
        assert_eq!(symbol_spans(kotlin, "threshold"), [(1, 1)]);
        assert!(symbol_spans(kotlin, "missing").is_empty());
    }
}
