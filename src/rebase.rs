//! `rebase-anchors`: move evidence anchors from the pinned baseline to a new commit.
use crate::drift::{git, symbol_spans, touches};
use crate::edit::{edit_file, replace_strings};
use crate::{fail, split_anchor, Anchor, Catalog, Result, RootOptions};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnchorFate {
    /// Same text and position.
    Unchanged,
    /// Line range shifted by edits above it, or the file was renamed.
    Moved,
    /// Lines inside the range changed; kept as is for review.
    Changed,
    /// The file was deleted or the symbol no longer exists.
    Lost,
}
#[derive(Debug, Clone, Serialize)]
pub struct AnchorMove {
    pub reference: String,
    pub fate: AnchorFate,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Entries (or entry#detail) citing the reference.
    pub cited_by: Vec<String>,
}
#[derive(Debug, Default, Serialize)]
pub struct RebaseReport {
    pub platform: String,
    pub from: String,
    pub to: String,
    pub moves: Vec<AnchorMove>,
    /// Files rewritten with `--write`.
    pub written: Vec<String>,
}
impl RebaseReport {
    pub fn count(&self, fate: AnchorFate) -> usize {
        self.moves.iter().filter(|m| m.fate == fate).count()
    }
    pub fn markdown(&self) -> String {
        let mut out = format!(
            "## Anchor rebase: {} `{}` → `{}`\n\n{} references: {} unchanged, {} moved, {} changed (review), {} lost.\n",
            self.platform,
            self.from,
            self.to,
            self.moves.len(),
            self.count(AnchorFate::Unchanged),
            self.count(AnchorFate::Moved),
            self.count(AnchorFate::Changed),
            self.count(AnchorFate::Lost)
        );
        for (fate, title) in [
            (AnchorFate::Moved, "Moved"),
            (
                AnchorFate::Changed,
                "Changed: anchored lines were edited; review and re-anchor by hand",
            ),
            (AnchorFate::Lost, "Lost: file deleted or symbol gone"),
        ] {
            out.push_str(&format!("\n### {title}\n\n"));
            let rows: Vec<&AnchorMove> = self.moves.iter().filter(|m| m.fate == fate).collect();
            if rows.is_empty() {
                out.push_str("None.\n");
            }
            for m in rows {
                out.push_str(&format!(
                    "- `{}`{}{} ({})\n",
                    m.reference,
                    m.new_reference
                        .as_ref()
                        .map(|n| format!(" → `{n}`"))
                        .unwrap_or_default(),
                    m.note
                        .as_ref()
                        .map(|n| format!(": {n}"))
                        .unwrap_or_default(),
                    m.cited_by.join(", ")
                ));
            }
        }
        if self.written.is_empty() {
            out.push_str("\nNothing was written. Rerun with --write to apply moves, then update the baseline pin.\n");
        } else {
            out.push_str(&format!(
                "\nRewrote {}. Update the platform's baseline pin to `{}` in catalog.toml.\n",
                self.written.join(", "),
                self.to
            ));
        }
        out
    }
}

/// A hunk's old and new ranges, each (start, count).
pub(crate) type Hunk = ((u32, u32), (u32, u32));

/// Old/new hunk ranges from `git diff -U0`.
fn hunks_both(diff: &str) -> Vec<Hunk> {
    let range = |text: &str| -> Option<(u32, u32)> {
        let (start, count) = text.split_once(',').unwrap_or((text, "1"));
        Some((start.parse().ok()?, count.parse().ok()?))
    };
    diff.lines()
        .filter_map(|line| line.strip_prefix("@@ -"))
        .filter_map(|rest| {
            let mut parts = rest.split_whitespace();
            let old = range(parts.next()?)?;
            let new = range(parts.next()?.strip_prefix('+')?)?;
            Some((old, new))
        })
        .collect()
}

/// Shift a line range through hunks that do not touch it; `None` when it was edited.
pub(crate) fn shift_range(hunks: &[Hunk], span: (u32, u32)) -> Option<(u32, u32)> {
    let mut delta: i64 = 0;
    for &(old, new) in hunks {
        if touches(old, span) {
            return None;
        }
        let ends_before = if old.1 == 0 {
            old.0 < span.0
        } else {
            old.0 + old.1 - 1 < span.0
        };
        if ends_before {
            delta += new.1 as i64 - old.1 as i64;
        }
    }
    let shift = |n: u32| (n as i64 + delta).max(1) as u32;
    Some((shift(span.0), shift(span.1)))
}

fn format_lines(original: &str, span: (u32, u32)) -> String {
    let l = if original.contains(":L") { "L" } else { "" };
    if span.0 == span.1 {
        format!("{l}{}", span.0)
    } else {
        format!("{l}{}-{l}{}", span.0, span.1)
    }
}

impl Catalog {
    /// Map anchored and file-level evidence on `platform` from `from` (default: its pin)
    /// to `to`. With `write`, rewrite moved references in the area files.
    pub fn rebase_anchors(
        &self,
        options: &RootOptions,
        platform: &str,
        from: Option<&str>,
        to: &str,
        write: bool,
        allow_reformat: bool,
    ) -> Result<RebaseReport> {
        let from = from
            .or_else(|| self.baseline_commit(platform))
            .ok_or_else(|| format!("{platform}: no baseline pin; pass --from"))?;
        let root = self.platform_root(platform, options)?.ok_or_else(|| {
            format!("{platform}: root not supplied; use --root {platform}=<path>")
        })?;
        let resolve = |rev: &str| -> Result<String> {
            let out = git(
                &root,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{rev}^{{commit}}"),
                ],
            )?;
            Ok(String::from_utf8(out)?.trim().to_owned())
        };
        let (old, new) = (resolve(from)?, resolve(to)?);
        let prefix = String::from_utf8(git(&root, &["rev-parse", "--show-prefix"])?)?;
        let prefix = prefix.trim_end_matches('\n').to_owned();
        // Renames and deletions, root-relative.
        let status = git(
            &root,
            &["diff", "--name-status", "-M", "-z", &old, &new, "--", "."],
        )?;
        let fields: Vec<String> = status
            .split(|b| *b == 0)
            .filter(|f| !f.is_empty())
            .map(|f| String::from_utf8_lossy(f).into_owned())
            .collect();
        let (mut renamed, mut deleted, mut changed) =
            (BTreeMap::new(), BTreeSet::new(), BTreeSet::new());
        let mut i = 0;
        while i < fields.len() {
            let code = fields[i].clone();
            let strip = |p: &str| p.strip_prefix(prefix.as_str()).unwrap_or(p).to_owned();
            if code.starts_with('R') && i + 2 < fields.len() {
                renamed.insert(strip(&fields[i + 1]), strip(&fields[i + 2]));
                changed.insert(strip(&fields[i + 1]));
                i += 3;
            } else if i + 1 < fields.len() {
                let path = strip(&fields[i + 1]);
                if code.starts_with('D') {
                    deleted.insert(path.clone());
                }
                changed.insert(path);
                i += 2;
            } else {
                break;
            }
        }
        let mut cited: BTreeMap<&String, BTreeSet<String>> = BTreeMap::new();
        for (label, p, reference) in self.evidence_items() {
            if p == platform {
                cited.entry(reference).or_default().insert(label);
            }
        }
        let mut report = RebaseReport {
            platform: platform.into(),
            from: from.into(),
            to: to.into(),
            ..Default::default()
        };
        for (reference, labels) in cited {
            let relative = self.evidence_relative(platform, reference)?.to_owned();
            let (path_part, anchor) = split_anchor(reference);
            let new_relative = renamed
                .get(&relative)
                .cloned()
                .unwrap_or_else(|| relative.clone());
            let new_path = path_part.replacen(&relative, &new_relative, 1);
            let mut m = AnchorMove {
                reference: reference.clone(),
                fate: AnchorFate::Unchanged,
                new_reference: None,
                note: None,
                cited_by: labels.into_iter().collect(),
            };
            if deleted.contains(&relative) {
                m.fate = AnchorFate::Lost;
                m.note = Some("file deleted".into());
            } else if !changed.contains(&relative) {
                // Untouched file: nothing to do.
            } else {
                let suffix = match &anchor {
                    None => Some(String::new()),
                    Some(Anchor::Symbol(symbol)) => {
                        let text = git(&root, &["show", &format!("{new}:./{new_relative}")])
                            .map(|b| String::from_utf8_lossy(&b).into_owned())
                            .unwrap_or_default();
                        if symbol_spans(&text, Anchor::symbol_name(symbol)).is_empty() {
                            m.fate = AnchorFate::Lost;
                            m.note = Some(format!("symbol {symbol} not found at {to}"));
                            None
                        } else {
                            Some(format!("#{symbol}"))
                        }
                    }
                    Some(Anchor::Lines(a, b)) => {
                        let diff = git(
                            &root,
                            &[
                                "diff",
                                "-U0",
                                "-M",
                                "--no-color",
                                &old,
                                &new,
                                "--",
                                &relative,
                                &new_relative,
                            ],
                        )?;
                        match shift_range(&hunks_both(&String::from_utf8_lossy(&diff)), (*a, *b)) {
                            Some(span) => Some(format!(":{}", format_lines(reference, span))),
                            None => {
                                m.fate = AnchorFate::Changed;
                                m.note = Some(format!("lines {a}-{b} were edited"));
                                None
                            }
                        }
                    }
                };
                if let Some(suffix) = suffix {
                    let candidate = format!("{new_path}{suffix}");
                    if candidate != *reference {
                        m.fate = AnchorFate::Moved;
                        m.new_reference = Some(candidate);
                    }
                }
            }
            report.moves.push(m);
        }
        if write {
            let map: BTreeMap<String, String> = report
                .moves
                .iter()
                .filter_map(|m| m.new_reference.clone().map(|n| (m.reference.clone(), n)))
                .collect();
            if !map.is_empty() {
                for file in self.area_files.values() {
                    let path = self.directory.join(file);
                    if edit_file(&path, allow_reformat, |value| {
                        replace_strings(value, &map);
                        Ok(())
                    })? {
                        report.written.push(file.clone());
                    }
                }
            }
        }
        if report.moves.is_empty() && !self.config.platforms.contains_key(platform) {
            return fail("unknown platform");
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_shift_through_untouching_hunks() {
        // 3 lines inserted after line 2, one line deleted at 40.
        let hunks = [((2, 0), (3, 3)), ((40, 1), (42, 0))];
        assert_eq!(shift_range(&hunks, (10, 20)), Some((13, 23)));
        assert_eq!(shift_range(&hunks, (1, 2)), Some((1, 2)));
        assert_eq!(shift_range(&hunks, (35, 45)), None);
        assert_eq!(shift_range(&hunks, (50, 50)), Some((52, 52)));
    }
}

/// A proposed `#Symbol` anchor for file-level evidence.
#[derive(Debug, Clone, Serialize)]
pub struct AnchorSuggestion {
    pub platform: String,
    pub reference: String,
    /// The anchored reference to use instead, or `None` when nothing matched.
    pub suggestion: Option<String>,
    /// Identifiers or copy found inside the symbol, with their line numbers.
    pub evidence: Vec<String>,
}

impl Catalog {
    /// Propose `path#Symbol` anchors for an entry's file-level evidence: the innermost type
    /// or function declaration containing the entry's accessibility identifier, label or copy.
    pub fn suggest_anchors(
        &self,
        options: &RootOptions,
        entry_id: &str,
        platform: Option<&str>,
    ) -> Result<Vec<AnchorSuggestion>> {
        let entry = self.entry(entry_id)?;
        let mut needles: BTreeSet<&str> = BTreeSet::new();
        needles.insert(&entry.accessibility.identifier);
        needles.insert(&entry.accessibility.label);
        needles.extend(entry.copy.iter().map(String::as_str));
        for detail in &entry.details {
            if let Some(copy) = &detail.spec.copy {
                needles.insert(&copy.text);
                if let Some(key) = &copy.key {
                    needles.insert(key);
                }
            }
        }
        needles.retain(|n| n.chars().count() >= 3);
        let mut out = Vec::new();
        for (name, record) in &entry.platforms {
            if platform.is_some_and(|p| p != name) || record.inherited {
                continue;
            }
            let Some(root) = self.platform_root(name, options)? else {
                continue;
            };
            for reference in record.source.iter().chain(&record.tests) {
                if split_anchor(reference).1.is_some() || !crate::is_test_path(reference) {
                    continue;
                }
                let relative = self.evidence_relative(name, reference)?;
                let Ok(text) = std::fs::read_to_string(root.join(relative)) else {
                    continue;
                };
                let declarations = crate::drift::declarations(&text);
                let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for (index, line) in text.lines().enumerate() {
                    let number = index as u32 + 1;
                    for needle in needles.iter().filter(|n| line.contains(**n)) {
                        let innermost = declarations
                            .iter()
                            .filter(|(symbol, start, end)| {
                                *start <= number
                                    && number <= *end
                                    && !matches!(symbol.as_str(), "body" | "init")
                            })
                            .max_by_key(|(_, start, _)| *start);
                        if let Some((symbol, _, _)) = innermost {
                            found
                                .entry(symbol.clone())
                                .or_default()
                                .push(format!("{needle:?} at L{number}"));
                        }
                    }
                }
                if found.is_empty() {
                    out.push(AnchorSuggestion {
                        platform: name.clone(),
                        reference: reference.clone(),
                        suggestion: None,
                        evidence: vec![
                            "no type or function contains the entry's identifier, label or copy"
                                .into(),
                        ],
                    });
                }
                let path = split_anchor(reference).0;
                for (symbol, evidence) in found {
                    out.push(AnchorSuggestion {
                        platform: name.clone(),
                        reference: reference.clone(),
                        suggestion: Some(format!("{path}#{symbol}")),
                        evidence,
                    });
                }
            }
        }
        Ok(out)
    }
}
