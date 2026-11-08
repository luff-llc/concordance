use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
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
