//! In-place catalog edits that keep key order and, for JSON-style area files, formatting.
use crate::{fail, Result};
use serde_yaml::Value;
use std::fs;
use std::path::Path;

/// How an area file is written back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Pretty JSON with two-space indentation (a YAML subset).
    Json,
    /// Block YAML as emitted by serde_yaml; comments are not preserved.
    Yaml,
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn json(value: &Value, depth: usize, out: &mut String) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&serde_json::to_string(s)?),
        Value::Sequence(items) if items.is_empty() => out.push_str("[]"),
        Value::Sequence(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&indent(depth + 1));
                json(item, depth + 1, out)?;
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&indent(depth));
            out.push(']');
        }
        Value::Mapping(map) if map.is_empty() => out.push_str("{}"),
        Value::Mapping(map) => {
            out.push_str("{\n");
            for (i, (key, item)) in map.iter().enumerate() {
                let Value::String(key) = key else {
                    return fail("non-string mapping key");
                };
                out.push_str(&indent(depth + 1));
                out.push_str(&serde_json::to_string(key)?);
                out.push_str(": ");
                json(item, depth + 1, out)?;
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            out.push_str(&indent(depth));
            out.push('}');
        }
        Value::Tagged(_) => return fail("YAML tags are not supported"),
    }
    Ok(())
}

pub fn render(value: &Value, style: Style, trailing: &str) -> Result<String> {
    Ok(match style {
        Style::Json => {
            let mut out = String::new();
            json(value, 0, &mut out)?;
            out + trailing
        }
        Style::Yaml => serde_yaml::to_string(value)?,
    })
}

fn style_of(text: &str) -> Style {
    if text.trim_start().starts_with('[') {
        Style::Json
    } else {
        Style::Yaml
    }
}

/// Apply `edit` to an area file. Refuses to write when re-rendering the unedited file would
/// change it (comments, flow style, other indentation) unless `allow_reformat` is set.
/// Returns whether the file changed.
pub fn edit_file(
    path: &Path,
    allow_reformat: bool,
    edit: impl FnOnce(&mut Value) -> Result<()>,
) -> Result<bool> {
    let text = fs::read_to_string(path)?;
    let style = style_of(&text);
    let trailing = &text[text.trim_end().len()..];
    let mut value: Value = serde_yaml::from_str(&text)?;
    if !allow_reformat && render(&value, style, trailing)? != text {
        return fail(format!(
            "{}: rewriting would change formatting beyond the edit (comments or non-canonical layout); pass --allow-reformat",
            path.display()
        ));
    }
    edit(&mut value)?;
    let updated = render(&value, style, trailing)?;
    if updated == text {
        return Ok(false);
    }
    fs::write(path, updated)?;
    Ok(true)
}

/// Replace every string equal to a key of `map` with its value. Returns replacements made.
pub fn replace_strings(
    value: &mut Value,
    map: &std::collections::BTreeMap<String, String>,
) -> usize {
    match value {
        Value::String(s) => match map.get(s.as_str()) {
            Some(new) => {
                *s = new.clone();
                1
            }
            None => 0,
        },
        Value::Sequence(items) => items.iter_mut().map(|v| replace_strings(v, map)).sum(),
        Value::Mapping(m) => m.iter_mut().map(|(_, v)| replace_strings(v, map)).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_rendering_matches_serde_json_pretty() {
        let text = "[\n  {\n    \"id\": \"a.b\",\n    \"list\": [],\n    \"map\": {},\n    \"n\": 0.4,\n    \"s\": \"x \\\"q\\\" \\\\(y)\"\n  }\n]\n";
        let value: Value = serde_yaml::from_str(text).expect("parse");
        assert_eq!(render(&value, Style::Json, "\n").expect("render"), text);
    }
}
