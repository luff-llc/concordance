//! `strings export`: Android string resources from an Apple string catalog (`.xcstrings`).
use crate::{fail, Catalog, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// FNV-1a 32-bit hash, rendered as 8 lowercase hex digits; stable across runs and platforms.
pub fn fnv32(text: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

/// Android resource name for a string-catalog key.
///
/// `<app>_` + slug + `_` + hash: the slug is the key's first five words, lowercased, with
/// every run of characters outside `[a-z0-9]` replaced by `_` and trimmed, cut to 32
/// characters at a `_` boundary when possible; the hash is FNV-1a 32-bit of the exact key
/// (UTF-8) as 8 hex digits. A key with no ASCII letters or digits gets the slug `s`.
/// `"Play %@"` in app `taupe` → `taupe_play_<hash>`.
pub fn resource_name(app: &str, key: &str) -> String {
    let mut slug = String::new();
    for word in key.split_whitespace().take(5) {
        for c in word.chars() {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                slug.push(c);
            } else if !slug.ends_with('_') {
                slug.push('_');
            }
        }
        if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let mut slug = slug.trim_matches('_').to_owned();
    if slug.len() > 32 {
        let cut = &slug[..32];
        slug = match cut.rfind('_') {
            Some(i) if i >= 8 => cut[..i].to_owned(),
            _ => cut.to_owned(),
        };
    }
    if slug.is_empty() {
        slug = "s".into();
    }
    let app: String = app
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{app}_{slug}_{}", fnv32(key))
}

/// Android `values-*` directory for an Apple locale identifier.
pub fn values_dir(locale: &str, source: &str) -> String {
    if locale == source {
        return "values".into();
    }
    let parts: Vec<&str> = locale.split(['-', '_']).collect();
    match parts.as_slice() {
        ["zh", "Hans"] => "values-zh-rCN".into(),
        ["zh", "Hant"] => "values-zh-rTW".into(),
        [lang] => format!("values-{}", lang.to_ascii_lowercase()),
        [lang, region] if region.len() == 2 && region.chars().all(|c| c.is_ascii_alphabetic()) => {
            format!(
                "values-{}-r{}",
                lang.to_ascii_lowercase(),
                region.to_ascii_uppercase()
            )
        }
        _ => format!("values-b+{}", parts.join("+")),
    }
}

/// Convert Apple format specifiers to Android ones and escape for a string resource.
pub fn android_value(value: &str) -> String {
    // Count non-positional placeholders to decide whether to number them.
    let mut out = String::new();
    let chars: Vec<char> = value.chars().collect();
    let mut placeholders = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' {
            if chars.get(i + 1) == Some(&'%') {
                placeholders.push(None);
                i += 2;
                continue;
            }
            let mut j = i + 1;
            let mut position = String::new();
            while j < chars.len() && chars[j].is_ascii_digit() {
                position.push(chars[j]);
                j += 1;
            }
            let positional = chars.get(j) == Some(&'$') && !position.is_empty();
            if positional {
                j += 1;
            } else {
                j = i + 1;
            }
            let mut spec = String::new();
            while j < chars.len() && "lhqzjt0123456789.-+ #".contains(chars[j]) {
                spec.push(chars[j]);
                j += 1;
            }
            if let Some(&conv) = chars.get(j) {
                let android = match conv {
                    '@' => Some('s'),
                    'd' | 'i' | 'u' => Some('d'),
                    'f' | 'e' | 'g' | 'x' | 'X' | 'o' | 'c' | 's' => Some(conv),
                    _ => None,
                };
                if let Some(android) = android {
                    let flags: String = spec.chars().filter(|c| !"lhqzjt".contains(*c)).collect();
                    placeholders.push(Some((
                        positional.then(|| position.clone()),
                        flags,
                        android,
                        i,
                        j + 1,
                    )));
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    let numbered = placeholders
        .iter()
        .flatten()
        .filter(|p| p.0.is_none())
        .count()
        > 1;
    let mut cursor = 0;
    let mut raw = String::new();
    for (index, p) in placeholders.iter().flatten().enumerate() {
        let (position, flags, conv, start, end) = p;
        raw.extend(&chars[cursor..*start]);
        let counter = index + 1;
        let position = position
            .clone()
            .or_else(|| numbered.then(|| counter.to_string()));
        raw.push('%');
        if let Some(position) = position {
            raw.push_str(&position);
            raw.push('$');
        }
        raw.push_str(flags);
        raw.push(*conv);
        cursor = *end;
    }
    raw.extend(&chars[cursor..]);
    for (index, c) in raw.chars().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '@' | '?' if index == 0 => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn xml_comment(text: &str) -> String {
    text.replace("--", "- -")
}

#[derive(Debug, Default)]
pub struct ExportResult {
    /// (path, contents) for every generated file.
    pub files: Vec<(PathBuf, String)>,
    pub keys: usize,
    /// Keys requested by the catalog but absent from the string catalog.
    pub missing: Vec<String>,
    /// Keys with plural or device variations, which are not exported.
    pub unsupported: Vec<String>,
}

impl Catalog {
    /// Copy-detail keys (`copy.key`) and, with `match_text`, copy texts without a key.
    pub fn copy_keys(&self, match_text: bool) -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        for detail in self.entries.values().flat_map(|e| &e.details) {
            if let Some(copy) = &detail.spec.copy {
                match &copy.key {
                    Some(key) => {
                        keys.insert(key.clone());
                    }
                    None if match_text => {
                        keys.insert(copy.text.clone());
                    }
                    None => {}
                }
            }
        }
        keys
    }

    /// Android resources for the catalog's copy keys from an `.xcstrings` file.
    pub fn export_strings(
        &self,
        xcstrings: &Path,
        app: &str,
        out: &Path,
        match_text: bool,
    ) -> Result<ExportResult> {
        let document: Value = serde_json::from_str(&fs::read_to_string(xcstrings)?)
            .map_err(|e| format!("{}: {e}", xcstrings.display()))?;
        let source = document
            .get("sourceLanguage")
            .and_then(Value::as_str)
            .unwrap_or("en")
            .to_owned();
        let strings = document
            .get("strings")
            .and_then(Value::as_object)
            .ok_or("xcstrings has no strings object")?;
        let mut result = ExportResult::default();
        // locale -> (name, key, value, comment)
        let mut by_locale: BTreeMap<String, Vec<(String, String, String, String)>> =
            BTreeMap::new();
        let mut names: BTreeMap<String, String> = BTreeMap::new();
        let requested = self.copy_keys(match_text);
        for key in &requested {
            let Some(entry) = strings.get(key) else {
                if !match_text
                    || self
                        .entries
                        .values()
                        .flat_map(|e| &e.details)
                        .any(|d| d.spec.copy.as_ref().and_then(|c| c.key.as_ref()) == Some(key))
                {
                    result.missing.push(key.clone());
                }
                continue;
            };
            let name = resource_name(app, key);
            if let Some(other) = names.insert(name.clone(), key.clone()) {
                return fail(format!(
                    "resource name collision {name}: {other:?} and {key:?}"
                ));
            }
            let comment = entry
                .get("comment")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let localizations = entry.get("localizations").and_then(Value::as_object);
            let mut source_seen = false;
            let mut unsupported = false;
            for (locale, localization) in localizations.into_iter().flatten() {
                if localization.get("variations").is_some() {
                    unsupported = true;
                    continue;
                }
                let Some(unit) = localization.get("stringUnit") else {
                    continue;
                };
                let state = unit
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("translated");
                let Some(value) = unit.get("value").and_then(Value::as_str) else {
                    continue;
                };
                if locale != &source && state != "translated" {
                    continue;
                }
                source_seen |= locale == &source;
                by_locale.entry(locale.clone()).or_default().push((
                    name.clone(),
                    key.clone(),
                    value.to_owned(),
                    comment.clone(),
                ));
            }
            if unsupported {
                result.unsupported.push(key.clone());
            }
            if !source_seen && !unsupported {
                // The key is the source-language text when no source localization is stored.
                by_locale.entry(source.clone()).or_default().push((
                    name,
                    key.clone(),
                    key.clone(),
                    comment,
                ));
            }
            result.keys += 1;
        }
        for (locale, mut rows) in by_locale {
            rows.sort();
            let mut xml = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- Generated by concordance strings export from {}. Do not edit; regenerate. -->\n<resources>\n",
                xml_comment(&xcstrings.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default())
            );
            for (name, key, value, comment) in rows {
                let note = if comment.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", comment)
                };
                xml.push_str(&format!(
                    "    <!-- {}{} -->\n",
                    xml_comment(&key),
                    xml_comment(&note)
                ));
                xml.push_str(&format!(
                    "    <string name=\"{name}\">{}</string>\n",
                    android_value(&value)
                ));
            }
            xml.push_str("</resources>\n");
            let path = out
                .join(values_dir(&locale, &source))
                .join(format!("strings_{}_shared.xml", app));
            result.files.push((path, xml));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_values_and_locales() {
        assert_eq!(
            resource_name("taupe", "Play %@"),
            format!("taupe_play_{}", fnv32("Play %@"))
        );
        assert!(resource_name(
            "taupe",
            "Couldn't load this episode. Try again later, please"
        )
        .starts_with("taupe_couldn_t_load_this_episode_try_"));
        assert!(resource_name("taupe", "…").starts_with("taupe_s_"));
        assert_eq!(fnv32(""), "811c9dc5");
        assert_eq!(android_value("Play %@"), "Play %s");
        assert_eq!(android_value("%@ of %lld"), "%1$s of %2$d");
        assert_eq!(android_value("%2$@ by %1$@"), "%2$s by %1$s");
        assert_eq!(
            android_value("100%% & <b> \"it's\""),
            "100%% &amp; &lt;b&gt; \\\"it\\'s\\\""
        );
        assert_eq!(android_value("@home?\nok"), "\\@home?\\nok");
        assert_eq!(values_dir("zh-Hans", "en"), "values-zh-rCN");
        assert_eq!(values_dir("pt-BR", "en"), "values-pt-rBR");
        assert_eq!(values_dir("es-419", "en"), "values-b+es+419");
        assert_eq!(values_dir("en", "en"), "values");
    }
}
