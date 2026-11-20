//! Evidence references and their optional anchors.
use serde::Serialize;

/// A location within an evidence file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    /// `path#Symbol` (or `path#Suite.method`, whose last component is the symbol).
    Symbol(String),
    /// `path:L10-L20`, `path:10-20`, `path:L10` or `path:10`; 1-based and inclusive.
    Lines(u32, u32),
}
impl Anchor {
    /// The identifier searched for in source: the last dot-separated component.
    pub fn symbol_name(name: &str) -> &str {
        name.rsplit('.').next().unwrap_or(name)
    }
}

fn line_number(text: &str) -> Option<u32> {
    let text = text.strip_prefix('L').unwrap_or(text);
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
        .filter(|n| *n > 0)
}

/// Split a reference into its path and anchor. Unanchored references return `None`.
pub fn split_anchor(reference: &str) -> (&str, Option<Anchor>) {
    if let Some((path, symbol)) = reference.split_once('#') {
        let anchor = (!symbol.is_empty()).then(|| Anchor::Symbol(symbol.to_owned()));
        return (path, anchor);
    }
    if let Some((path, range)) = reference.rsplit_once(':') {
        let (start, end) = range.split_once('-').unwrap_or((range, range));
        if let (Some(start), Some(end)) = (line_number(start), line_number(end)) {
            if start <= end {
                return (path, Some(Anchor::Lines(start, end)));
            }
        }
    }
    (reference, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anchors() {
        assert_eq!(split_anchor("a/B.swift"), ("a/B.swift", None));
        assert_eq!(
            split_anchor("a/B.swift#Type.method"),
            ("a/B.swift", Some(Anchor::Symbol("Type.method".into())))
        );
        assert_eq!(
            split_anchor("a/B.swift:L10-L20"),
            ("a/B.swift", Some(Anchor::Lines(10, 20)))
        );
        assert_eq!(
            split_anchor("a/B.swift:7"),
            ("a/B.swift", Some(Anchor::Lines(7, 7)))
        );
        assert_eq!(split_anchor("a/B.swift:20-10").1, None);
        assert_eq!(split_anchor("a/B.swift:x").1, None);
        assert_eq!(Anchor::symbol_name("Suite.testOpen"), "testOpen");
    }
}
