use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Implemented. `implemented` is accepted as a synonym; verification is recorded separately.
    #[serde(alias = "implemented")]
    Shipped,
    Partial,
    Missing,
    NotApplicable,
}
impl Status {
    pub fn is_gap(self) -> bool {
        matches!(self, Self::Missing | Self::Partial)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shipped => "shipped",
            Self::Partial => "partial",
            Self::Missing => "missing",
            Self::NotApplicable => "not-applicable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Impact {
    Critical,
    High,
    Medium,
    Low,
}
impl Impact {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interaction {
    pub trigger: String,
    pub result: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acceptance {
    pub given: String,
    pub when: String,
    pub then: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accessibility {
    pub label: String,
    pub identifier: String,
    pub notes: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    pub status: Status,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Who implemented this platform's behavior (agent or person ID).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implemented_by: Option<String>,
    /// Independent review evidence. `verified` is derived from it, never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
}

/// A reviewer's record that an implementation was checked against the baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub by: String,
    /// ISO date, YYYY-MM-DD.
    pub at: String,
    /// Target commit that was reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Baseline commit the target was compared against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_commit: Option<String>,
    /// Capture keys (`screen.state`) or capture file paths relative to the repository root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<String>,
    /// Test references that exercised the journey, in the same form as platform tests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub journeys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A capture key names a declared `screen.state`; anything path-like is a file.
pub fn is_capture_key(capture: &str) -> bool {
    !capture.contains('/') && !capture.ends_with(".png") && !capture.ends_with(".json")
}

impl Platform {
    /// Why this record does not count as verified, or `None` when it does.
    /// Capture availability is checked separately when capture coverage is known.
    pub fn unverified_reason(&self) -> Option<&'static str> {
        if self.status != Status::Shipped {
            return Some("not implemented");
        }
        let Some(v) = &self.verification else {
            return Some("no verification record");
        };
        if v.captures.is_empty() && v.journeys.is_empty() {
            return Some("verification has no captures or journeys");
        }
        if self.implemented_by.as_deref() == Some(v.by.as_str()) {
            return Some("verifier is the implementer");
        }
        None
    }
    pub fn verified(&self) -> bool {
        self.unverified_reason().is_none()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub surfaces: Vec<String>,
    pub interactions: Vec<Interaction>,
    pub states: Vec<String>,
    pub settings: Vec<String>,
    pub data: Data,
    pub copy: Vec<String>,
    pub accessibility: Accessibility,
    pub design: Vec<String>,
    pub depends_on: Vec<String>,
    pub impact: Impact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u16>,
    pub platforms: BTreeMap<String, Platform>,
    pub acceptance: Vec<Acceptance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Measurable details (motion, haptics, geometry, copy...) tracked per platform.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<Detail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DetailKind {
    Motion,
    Haptic,
    Gesture,
    Geometry,
    Copy,
    State,
    Accessibility,
    Layout,
    Other,
}
impl DetailKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Motion => "motion",
            Self::Haptic => "haptic",
            Self::Gesture => "gesture",
            Self::Geometry => "geometry",
            Self::Copy => "copy",
            Self::State => "state",
            Self::Accessibility => "accessibility",
            Self::Layout => "layout",
            Self::Other => "other",
        }
    }
}

/// One measurable detail of an entry, with baseline evidence and per-platform status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detail {
    /// Entry-scoped slug; referenced as `entry.id#detail-id`.
    pub id: String,
    pub kind: DetailKind,
    pub spec: DetailSpec,
    /// Evidence on the report baseline platform, usually `path:line` or `path#Symbol`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub platforms: BTreeMap<String, Platform>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailSpec {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion: Option<MotionSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haptic: Option<HapticSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gesture: Option<GestureSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<GeometrySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopySpec>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Curve {
    Spring,
    Ease,
    Linear,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionSpec {
    pub curve: Curve,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damping: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HapticSpec {
    pub kind: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GestureSpec {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeometrySpec {
    pub property: String,
    pub value: f64,
    pub unit: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopySpec {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
}
fn number(value: f64) -> String {
    format!("{value}")
}
impl DetailSpec {
    /// The structured numbers as one compact line, for briefs and work queues.
    pub fn numbers(&self) -> String {
        let mut parts = Vec::new();
        if let Some(m) = &self.motion {
            let mut p = vec![format!(
                "motion {}",
                match m.curve {
                    Curve::Spring => "spring",
                    Curve::Ease => "ease",
                    Curve::Linear => "linear",
                }
            )];
            for (name, value) in [
                ("response", m.response),
                ("damping", m.damping),
                ("duration", m.duration),
                ("bounce", m.bounce),
            ] {
                if let Some(v) = value {
                    p.push(format!("{name} {}", number(v)));
                }
            }
            parts.push(p.join(" "));
        }
        if let Some(h) = &self.haptic {
            parts.push(format!("haptic {}", h.kind));
        }
        if let Some(g) = &self.gesture {
            let mut text = format!("gesture {}", g.kind);
            if let Some(t) = g.threshold {
                text.push_str(&format!(
                    " threshold {}{}",
                    number(t),
                    g.unit.as_deref().unwrap_or("")
                ));
            }
            parts.push(text);
        }
        if let Some(g) = &self.geometry {
            parts.push(format!("{} = {}{}", g.property, number(g.value), g.unit));
        }
        if let Some(c) = &self.copy {
            parts.push(format!(
                "copy {:?}{}",
                c.text,
                c.locale
                    .as_ref()
                    .map(|l| format!(" ({l})"))
                    .unwrap_or_default()
            ));
        }
        parts.join("; ")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Relative to the directory containing catalog.toml; --repo overrides this.
    #[serde(default = "dot")]
    pub repo_root: String,
    pub platforms: BTreeMap<String, String>,
    /// Optional legacy prefix to strip before resolving a platform's evidence.
    #[serde(default)]
    pub path_prefixes: BTreeMap<String, String>,
    #[serde(default)]
    pub report: ReportConfig,
    #[serde(default)]
    pub external_roots: BTreeMap<String, ExternalRoot>,
    #[serde(default)]
    pub baseline: Option<Baseline>,
    #[serde(default)]
    pub baselines: BTreeMap<String, Baseline>,
    /// Root-relative glob patterns used to discover uncatalogued changes.
    #[serde(default)]
    pub source_globs: BTreeMap<String, Vec<String>>,
}
fn dot() -> String {
    ".".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReportConfig {
    pub title: String,
    pub baseline: String,
    pub target: String,
    pub labels: BTreeMap<String, String>,
    pub intro: String,
    pub area_order: Vec<String>,
    pub ordering_note: String,
    pub evidence_note: String,
    pub adaptation_platform: Option<String>,
}
impl Default for ReportConfig {
    fn default() -> Self {
        Self {
            title: "Native parity".into(),
            baseline: "ios".into(),
            target: "android".into(),
            labels: BTreeMap::from([("ios".into(), "iOS".into()), ("android".into(), "Android".into())]),
            intro: "Statuses are source-review findings, not a claim that a build or UI journey passed during this audit.\nAn empty test list is an evidence gap; a listed test is a reference, not a fresh test result.".into(),
            area_order: vec![],
            ordering_note: "Order: critical, high, medium, low. Within a tier, catalog priority comes first, then configured area order, then IDs.".into(),
            evidence_note: "Native implementations preserve behavior contracts using each platform's own UI conventions.".into(),
            adaptation_platform: None,
        }
    }
}
impl ReportConfig {
    pub fn label<'a>(&'a self, platform: &'a str) -> &'a str {
        self.labels
            .get(platform)
            .map(String::as_str)
            .unwrap_or(platform)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ExternalRoot {
    Hint(String),
    Details {
        #[serde(alias = "hint")]
        description: String,
        git: Option<String>,
        commit: Option<String>,
    },
}
impl ExternalRoot {
    pub fn hint(&self) -> String {
        match self {
            Self::Hint(hint) => hint.clone(),
            Self::Details {
                description,
                git,
                commit,
            } => format!(
                "{description}{}{}",
                git.as_ref()
                    .map(|s| format!("; git {s}"))
                    .unwrap_or_default(),
                commit
                    .as_ref()
                    .map(|s| format!("; commit {s}"))
                    .unwrap_or_default()
            ),
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub commit: String,
}
