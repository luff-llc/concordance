//! System surfaces: the features × platforms × surfaces dimension (docs/surfaces.md).
use crate::validation::ValidationReport;
use crate::{Catalog, Entry, Platform, Status, SurfaceRelease};
use serde::Serialize;

/// Built-in signal patterns for well-known surface IDs.
pub fn default_signals(id: &str) -> &'static [&'static str] {
    match id {
        "widget.small" => &[
            "apple.widget:systemSmall@*",
            "android.widget:1x1@*",
            "android.widget:1x2@*",
            "android.widget:2x1@*",
            "android.widget:2x2@*",
        ],
        "widget.medium" => &[
            "apple.widget:systemMedium@*",
            "android.widget:3x1@*",
            "android.widget:3x2@*",
            "android.widget:4x1@*",
            "android.widget:4x2@*",
            "android.widget:5x1@*",
            "android.widget:5x2@*",
        ],
        "widget.large" => &[
            "apple.widget:systemLarge@*",
            "android.widget:*x3@*",
            "android.widget:*x4@*",
            "android.widget:*x5@*",
        ],
        "widget.extra-large" => &["apple.widget:systemExtraLarge*@*"],
        "lock-screen" => &["apple.widget:accessory*"],
        "complication" => &[
            "apple.widget:accessory*@watch",
            "apple.complication@*",
            "android.complication@*",
        ],
        "control" => &["apple.control@*", "android.qs-tile@*"],
        "quick-action" => &["apple.quick-action@*", "android.shortcut@*"],
        "app-shortcut" => &["apple.app-shortcut@*", "android.app-action@*"],
        "watch-app" => &["apple.watch-app@*", "android.wear-app@*"],
        "wear-tile" => &["android.wear-tile@*"],
        "car" => &[
            "apple.carplay@*",
            "android.car@*",
            "android.media-library@*",
        ],
        "menu-bar" => &["apple.menu-bar@*"],
        "live-activity" => &["apple.live-activity@*"],
        "notification" => &["apple.notification@*"],
        _ => &[],
    }
}

/// `signal[:detail][@context]`; `*` is a wildcard in detail and context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalPattern {
    pub signal: String,
    pub detail: Option<String>,
    /// `None`: only declarations without a context. `Some("*")`: any.
    pub context: Option<String>,
}
impl SignalPattern {
    pub fn parse(text: &str) -> Option<Self> {
        let (rest, context) = match text.split_once('@') {
            Some((rest, context)) if !context.is_empty() => (rest, Some(context.to_owned())),
            Some(_) => return None,
            None => (text, None),
        };
        let (signal, detail) = match rest.split_once(':') {
            Some((signal, detail)) if !detail.is_empty() => (signal, Some(detail.to_owned())),
            Some(_) => return None,
            None => (rest, None),
        };
        let ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        };
        if !ok(signal) || !(signal.starts_with("apple.") || signal.starts_with("android.")) {
            return None;
        }
        Some(Self {
            signal: signal.to_owned(),
            detail,
            context,
        })
    }
    pub fn matches(&self, signal: &str, detail: Option<&str>, context: Option<&str>) -> bool {
        if self.signal != signal {
            return false;
        }
        if let Some(pattern) = &self.detail {
            match detail {
                Some(d) if wildcard(pattern, d) => {}
                _ => return false,
            }
        }
        match (&self.context, context) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(p), Some(c)) => wildcard(p, c),
            (Some(p), None) => p == "*",
        }
    }
    /// The parser family that can produce this signal.
    pub fn family(&self) -> &str {
        self.signal.split('.').next().unwrap_or_default()
    }
}

/// `*` matches any run of characters (including none).
pub(crate) fn wildcard(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p {
            [] => t.is_empty(),
            [b'*', rest @ ..] => (0..=t.len()).any(|i| go(rest, &t[i..])),
            [c, rest @ ..] => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

pub(crate) fn valid_surface_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-'))
        && !id.starts_with(['.', '-'])
        && !id.ends_with(['.', '-'])
}

/// One entry × surface × platform obligation, and its record if any.
#[derive(Debug, Clone, Serialize)]
pub struct SurfaceCell<'a> {
    pub entry: &'a str,
    pub surface: &'a str,
    pub platform: &'a str,
    #[serde(skip)]
    pub record: Option<&'a Platform>,
}
impl SurfaceCell<'_> {
    /// `unrecorded` when the entry names the surface but the platform has no record.
    pub fn status(&self) -> &'static str {
        self.record.map_or("unrecorded", |r| r.status.as_str())
    }
    pub fn is_gap(&self) -> bool {
        self.record.is_none_or(|r| r.status.is_gap())
    }
    pub fn label(&self) -> String {
        format!("{}@{}", self.entry, self.surface)
    }
}

/// A gap on a `release = "required"` surface.
#[derive(Debug, Clone, Serialize)]
pub struct SurfaceReleaseFinding {
    pub label: String,
    pub surface: String,
    pub status: String,
}

impl Catalog {
    /// Signal patterns for a surface: explicit `signals`, or the built-in defaults.
    pub fn surface_signals(&self, id: &str) -> Vec<SignalPattern> {
        let Some(config) = self.config.surfaces.get(id) else {
            return Vec::new();
        };
        match &config.signals {
            Some(list) => list
                .iter()
                .filter_map(|s| SignalPattern::parse(s))
                .collect(),
            None => default_signals(id)
                .iter()
                .filter_map(|s| SignalPattern::parse(s))
                .collect(),
        }
    }

    /// Declared surface IDs an entry names, in entry order.
    pub fn entry_surfaces<'a>(&self, entry: &'a Entry) -> Vec<&'a str> {
        entry
            .surfaces
            .iter()
            .map(String::as_str)
            .filter(|s| self.config.surfaces.contains_key(*s))
            .collect()
    }

    /// The entry's surface obligations on one platform: surfaces it names that the platform
    /// can have, unless the entry is not applicable there.
    pub fn surface_cells<'a>(
        &'a self,
        entry: &'a Entry,
        platform: &'a str,
    ) -> Vec<SurfaceCell<'a>> {
        let Some(record) = entry.platforms.get(platform) else {
            return Vec::new();
        };
        if record.status == Status::NotApplicable {
            return Vec::new();
        }
        self.entry_surfaces(entry)
            .into_iter()
            .filter(|s| {
                self.config.surfaces[*s]
                    .platforms
                    .iter()
                    .any(|p| p == platform)
            })
            .map(|surface| SurfaceCell {
                entry: &entry.id,
                surface,
                platform,
                record: record.surfaces.get(surface),
            })
            .collect()
    }

    /// Surface gaps (missing, partial, stale, unrecorded) for an entry on a platform.
    pub fn surface_gaps<'a>(&'a self, entry: &'a Entry, platform: &'a str) -> Vec<SurfaceCell<'a>> {
        self.surface_cells(entry, platform)
            .into_iter()
            .filter(SurfaceCell::is_gap)
            .collect()
    }

    pub(crate) fn check_surfaces(&self, report: &mut ValidationReport) {
        let config = &self.config;
        for (id, surface) in &config.surfaces {
            if !valid_surface_id(id) {
                report.error("surfaces", "", "", id, "invalid surface ID");
            }
            if surface.platforms.is_empty() {
                report.error(
                    "surfaces",
                    "",
                    "",
                    id,
                    "surface platforms must not be empty",
                );
            }
            for platform in &surface.platforms {
                if !config.platforms.contains_key(platform) {
                    report.error("surfaces", "", platform, id, "unknown platform");
                }
            }
            for signal in surface.signals.iter().flatten() {
                if SignalPattern::parse(signal).is_none() {
                    report.error(
                        "surfaces",
                        "",
                        "",
                        signal,
                        &format!("{id}: signal must be apple.*|android.*[:detail][@context]"),
                    );
                }
            }
        }
        for (platform, scan) in &config.surface_scan {
            if !config.platforms.contains_key(platform) {
                report.error("surfaces", "", platform, "surface_scan", "unknown platform");
            }
            let globs = scan
                .swift
                .iter()
                .chain(&scan.plist)
                .chain(&scan.manifests)
                .chain(&scan.res)
                .chain(&scan.exclude)
                .chain(scan.contexts.values().flatten());
            for glob in globs {
                if glob.is_empty()
                    || glob.starts_with('/')
                    || glob.split('/').any(|part| part == "..")
                {
                    report.error("surfaces", "", platform, glob, "unsafe surface_scan glob");
                }
            }
        }
        for entry in self.entries.values() {
            for (platform, record) in &entry.platforms {
                if record.inherited {
                    continue;
                }
                for surface in record.surfaces.keys() {
                    let label = format!("{}@{surface}", entry.id);
                    let Some(declared) = config.surfaces.get(surface) else {
                        report.error(
                            "surfaces",
                            &label,
                            platform,
                            surface,
                            "not a [surfaces] ID in catalog.toml",
                        );
                        continue;
                    };
                    if !entry.surfaces.contains(surface) {
                        report.error(
                            "surfaces",
                            &label,
                            platform,
                            surface,
                            "the entry's surfaces list does not name this surface",
                        );
                    }
                    if !declared.platforms.contains(platform) {
                        report.error(
                            "surfaces",
                            &label,
                            platform,
                            surface,
                            "the surface does not list this platform",
                        );
                    }
                }
            }
        }
    }

    /// Gaps on `release = "required"` surfaces for a platform.
    pub fn gate_release_surfaces(&self, platform: &str) -> Vec<SurfaceReleaseFinding> {
        let mut out = Vec::new();
        for entry in self.entries.values() {
            for cell in self.surface_gaps(entry, platform) {
                if self.config.surfaces[cell.surface].release != SurfaceRelease::Required {
                    continue;
                }
                // Stale records are judged by their age limit, not here.
                if cell.record.is_some_and(|r| r.status == Status::Stale) {
                    continue;
                }
                out.push(SurfaceReleaseFinding {
                    label: cell.label(),
                    surface: cell.surface.to_owned(),
                    status: cell.status().to_owned(),
                });
            }
        }
        out
    }

    /// The `## Surfaces` report section; empty when no surfaces are declared.
    pub fn surfaces_section(&self) -> String {
        if self.config.surfaces.is_empty() {
            return String::new();
        }
        let labels = &self.config.report;
        // Columns: the report baseline and target first, then the other platforms; only
        // platforms that at least one surface lists.
        let mut platforms: Vec<&String> = self.config.platforms.keys().collect();
        platforms.sort_by_key(|p| {
            (
                **p != labels.baseline,
                **p != labels.target,
                p.as_str().to_owned(),
            )
        });
        let columns: Vec<&String> = platforms
            .into_iter()
            .filter(|p| {
                self.config
                    .surfaces
                    .values()
                    .any(|s| s.platforms.contains(p))
            })
            .collect();
        let mut lines = vec![
            String::new(),
            "## Surfaces".into(),
            String::new(),
            "Entries that name a declared surface, per platform: implemented over total, verified, and gaps (missing, partial, stale or unrecorded). A dash: the platform cannot have the surface. Run `concordance surfaces scan` to compare with what each app declares.".into(),
            String::new(),
            format!(
                "| Surface | {} |",
                columns
                    .iter()
                    .map(|p| labels.label(p))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            format!("| --- |{}", " --- |".repeat(columns.len())),
        ];
        let mut gaps = Vec::new();
        for (id, surface) in &self.config.surfaces {
            let mut cells = Vec::new();
            for platform in &columns {
                if !surface.platforms.contains(platform) {
                    cells.push("—".to_owned());
                    continue;
                }
                let (mut total, mut implemented, mut verified, mut open) = (0, 0, 0, 0);
                for entry in self.entries.values() {
                    for cell in self.surface_cells(entry, platform) {
                        if cell.surface != id {
                            continue;
                        }
                        if cell
                            .record
                            .is_some_and(|r| r.status == Status::NotApplicable)
                        {
                            continue;
                        }
                        total += 1;
                        if cell.record.is_some_and(|r| r.status == Status::Implemented) {
                            implemented += 1;
                        }
                        if cell.record.is_some_and(Platform::verified) {
                            verified += 1;
                        }
                        if cell.is_gap() {
                            open += 1;
                            gaps.push((
                                id.as_str(),
                                labels.label(platform).to_owned(),
                                cell.entry.to_owned(),
                                cell.status(),
                                cell.record.and_then(|r| r.gap.clone()),
                            ));
                        }
                    }
                }
                if total == 0 {
                    cells.push("none catalogued".to_owned());
                    continue;
                }
                let mut text = format!("{implemented}/{total}");
                if verified > 0 {
                    text.push_str(&format!(" ({verified} verified)"));
                }
                if open > 0 {
                    text.push_str(&format!(
                        " · {open} gap{}",
                        if open == 1 { "" } else { "s" }
                    ));
                }
                cells.push(text);
            }
            lines.push(format!(
                "| {} (`{id}`) | {} |",
                surface.title(id).replace('|', "\\|"),
                cells.join(" | ")
            ));
        }
        if !gaps.is_empty() {
            lines.extend([String::new(), "### Surface gaps".into(), String::new()]);
            lines.push("| Surface | Platform | Entry | Status | Gap |".into());
            lines.push("| --- | --- | --- | --- | --- |".into());
            for (surface, platform, entry, status, gap) in gaps {
                lines.push(format!(
                    "| `{surface}` | {platform} | `{entry}` | {status} | {} |",
                    gap.unwrap_or_default()
                        .replace('|', "\\|")
                        .replace(['\n', '\r'], " ")
                ));
            }
        }
        lines.push(String::new());
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let p = SignalPattern::parse("apple.widget:accessory*@watch").expect("pattern");
        assert!(p.matches("apple.widget", Some("accessoryCircular"), Some("watch")));
        assert!(!p.matches("apple.widget", Some("accessoryCircular"), None));
        let p = SignalPattern::parse("apple.widget:accessory*").expect("pattern");
        assert!(p.matches("apple.widget", Some("accessoryInline"), None));
        assert!(!p.matches("apple.widget", Some("accessoryInline"), Some("watch")));
        let p = SignalPattern::parse("android.widget:*x3@*").expect("pattern");
        assert!(p.matches("android.widget", Some("4x3"), None));
        assert!(p.matches("android.widget", Some("2x3"), Some("wear")));
        assert!(!p.matches("android.widget", Some("4x2"), None));
        assert!(SignalPattern::parse("widget").is_none());
        assert!(SignalPattern::parse("apple.widget:").is_none());
        assert!(SignalPattern::parse("apple.widget@").is_none());
        for id in [
            "widget.small",
            "widget.medium",
            "widget.large",
            "widget.extra-large",
            "lock-screen",
            "complication",
            "control",
            "quick-action",
            "app-shortcut",
            "watch-app",
            "wear-tile",
            "car",
            "menu-bar",
            "live-activity",
            "notification",
        ] {
            for s in default_signals(id) {
                assert!(SignalPattern::parse(s).is_some(), "{s}");
            }
        }
    }

    #[test]
    fn ids() {
        assert!(valid_surface_id("widget.small"));
        assert!(valid_surface_id("car"));
        assert!(!valid_surface_id("Widget"));
        assert!(!valid_surface_id(".x"));
        assert!(!valid_surface_id("a@b"));
    }
}
