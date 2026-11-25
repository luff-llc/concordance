use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Implemented; verification is recorded separately. `shipped` is a deprecated read alias.
    #[serde(alias = "shipped")]
    Implemented,
    Partial,
    Missing,
    NotApplicable,
    /// The baseline changed after this was implemented; see `previous_status` and `since_*`.
    Stale,
}
impl Status {
    /// Work remains: missing, partial, or stale after a baseline change.
    pub fn is_gap(self) -> bool {
        matches!(self, Self::Missing | Self::Partial | Self::Stale)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Partial => "partial",
            Self::Missing => "missing",
            Self::NotApplicable => "not-applicable",
            Self::Stale => "stale",
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
    /// Keys of `[blockers]` entries holding this work back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// For `stale` records: the status before the baseline changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_status: Option<Status>,
    /// For `stale` records: the baseline commit whose change made this stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_commit: Option<String>,
    /// For `stale` records: YYYY-MM-DD of that commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_date: Option<String>,
    /// Baseline commit at which a reviewer confirmed this record needs no change after a
    /// file-level evidence change (set with `set --reviewed-unchanged`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_commit: Option<String>,
    /// A recorded, intended difference from the baseline (set with `set --intended`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intended_difference: Option<String>,
    /// Per-surface records (`[surfaces]` IDs this entry names), keyed by surface ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub surfaces: BTreeMap<String, Platform>,
    /// On a surface record: the declaration IDs `surfaces scan` reports for it (widget
    /// kinds, shortcut types, receiver classes...). Empty means a coarse match.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declared: Vec<String>,
    /// True when the record was copied whole from an inherited platform.
    #[serde(skip)]
    pub inherited: bool,
    /// True for the not-applicable record that stands in for a platform outside the
    /// entry's `applies_to`; never written or serialized.
    #[serde(skip)]
    pub out_of_scope: bool,
}

/// Serialize platform records without the stand-ins for platforms outside `applies_to`.
fn scoped_records<S: serde::Serializer>(
    records: &BTreeMap<String, Platform>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(None)?;
    for (platform, record) in records.iter().filter(|(_, r)| !r.out_of_scope) {
        map.serialize_entry(platform, record)?;
    }
    map.end()
}
fn all_scoped(records: &BTreeMap<String, Platform>) -> bool {
    records.values().all(|r| r.out_of_scope)
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
        if self.status != Status::Implemented {
            return Some("not implemented");
        }
        let Some(v) = &self.verification else {
            return Some("no verification record");
        };
        if v.captures.is_empty() && v.journeys.is_empty() {
            return Some("verification has no captures or journeys");
        }
        match self.implemented_by.as_deref() {
            None => return Some("implemented_by not recorded"),
            Some(by) if by == v.by => return Some("verifier is the implementer"),
            Some(_) => {}
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
    /// The platforms this feature exists on. Absent: every configured platform. Platforms
    /// outside it need no record and count nowhere (they read as not-applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_to: Option<Vec<String>>,
    #[serde(serialize_with = "scoped_records")]
    pub platforms: BTreeMap<String, Platform>,
    pub acceptance: Vec<Acceptance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Evidence a verification must include. Default: a capture, plus a journey when the
    /// entry changes state (non-empty `data.writes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_requires: Option<Vec<VerifyEvidence>>,
    /// Measurable details (motion, haptics, geometry, copy...) tracked per platform.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<Detail>,
    /// The same feature in other apps, as `<app>:<entry-id>`; resolved by `suite`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerifyEvidence {
    Capture,
    Journey,
}
impl Entry {
    /// Whether the feature exists on a platform (`applies_to`, or every platform).
    pub fn applies(&self, platform: &str) -> bool {
        self.applies_to
            .as_ref()
            .is_none_or(|scope| scope.iter().any(|p| p == platform))
    }
    /// Explicit `verify_requires`, or the inferred default: purely visual entries need a
    /// capture; entries whose interactions write state need a capture and a journey.
    pub fn verification_requirements(&self) -> Vec<VerifyEvidence> {
        match &self.verify_requires {
            Some(explicit) => explicit.clone(),
            None if self.data.writes.is_empty() => vec![VerifyEvidence::Capture],
            None => vec![VerifyEvidence::Capture, VerifyEvidence::Journey],
        }
    }
    /// Which required kinds of evidence a verification record lacks.
    pub fn missing_verification_evidence(&self, v: &Verification) -> Vec<&'static str> {
        self.verification_requirements()
            .into_iter()
            .filter_map(|r| match r {
                VerifyEvidence::Capture if v.captures.is_empty() => Some("capture"),
                VerifyEvidence::Journey if v.journeys.is_empty() => Some("journey"),
                _ => None,
            })
            .collect()
    }
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
    #[serde(
        default,
        skip_serializing_if = "all_scoped",
        serialize_with = "scoped_records"
    )]
    pub platforms: BTreeMap<String, Platform>,
    /// Evidence a verification must include. Default: a journey for motion, haptic and
    /// gesture details (a still capture cannot prove them); a capture or journey otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_requires: Option<Vec<VerifyEvidence>>,
}
impl Detail {
    pub fn verification_requirements(&self) -> Vec<VerifyEvidence> {
        match (&self.verify_requires, self.kind) {
            (Some(explicit), _) => explicit.clone(),
            (None, DetailKind::Motion | DetailKind::Haptic | DetailKind::Gesture) => {
                vec![VerifyEvidence::Journey]
            }
            (None, _) => vec![],
        }
    }
    /// Why this detail's platform record is not verified, if it is not.
    pub fn unverified_reason(&self, record: &Platform) -> Option<String> {
        if let Some(reason) = record.unverified_reason() {
            return Some(reason.into());
        }
        let v = record.verification.as_ref()?;
        let lacking: Vec<&str> = self
            .verification_requirements()
            .into_iter()
            .filter_map(|r| match r {
                VerifyEvidence::Capture if v.captures.is_empty() => Some("capture"),
                VerifyEvidence::Journey if v.journeys.is_empty() => Some("journey"),
                _ => None,
            })
            .collect();
        (!lacking.is_empty()).then(|| {
            format!(
                "verification lacks a required {} (verify_requires)",
                lacking.join(" and a ")
            )
        })
    }
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
    /// One measurement or several (for example width, height and corner radius).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Geometry>,
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
#[serde(untagged)]
pub enum Geometry {
    One(GeometrySpec),
    Many(Vec<GeometrySpec>),
}
impl Geometry {
    pub fn items(&self) -> &[GeometrySpec] {
        match self {
            Self::One(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionSpec {
    /// A named design-token motion (for example `Motion.fold`), with or without numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curve>,
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
    /// String-catalog key (for example an `.xcstrings` key) for `strings export`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}
fn number(value: f64) -> String {
    format!("{value}")
}
impl DetailSpec {
    /// The structured numbers as one compact line, for briefs and work queues.
    pub fn numbers(&self) -> String {
        let mut parts = Vec::new();
        if let Some(m) = &self.motion {
            let mut p = vec!["motion".to_owned()];
            if let Some(token) = &m.token {
                p.push(format!("token {token}"));
            }
            if let Some(curve) = m.curve {
                p.push(
                    match curve {
                        Curve::Spring => "spring",
                        Curve::Ease => "ease",
                        Curve::Linear => "linear",
                    }
                    .into(),
                );
            }
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
            for g in g.items() {
                parts.push(format!("{} = {}{}", g.property, number(g.value), g.unit));
            }
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
    pub platforms: BTreeMap<String, PlatformConfig>,
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
    pub source_globs: BTreeMap<String, SourceGlobs>,
    /// Registry of decisions and dependencies that block work, referenced by `blocked_by`.
    #[serde(default)]
    pub blockers: BTreeMap<String, Blocker>,
    #[serde(default)]
    pub ui: UiConfig,
    /// One shared marketing version that every platform must ship.
    #[serde(default)]
    pub version: Option<VersionConfig>,
    /// `[tracker]`: release tag patterns for `log scan-git`.
    #[serde(default)]
    pub tracker: Option<crate::log::TrackerConfig>,
    /// `[surfaces.<id>]`: system surfaces (widgets, controls, watch, car...) and the
    /// platforms that can have each. See docs/surfaces.md.
    #[serde(default)]
    pub surfaces: BTreeMap<String, SurfaceConfig>,
    /// `[surface_scan.<platform>]`: where `surfaces scan` and `gate` read declarations.
    #[serde(default)]
    pub surface_scan: BTreeMap<String, SurfaceScanConfig>,
    /// `[gate.<platform>]`: what `gate` enforces, reports or skips on each platform.
    #[serde(default)]
    pub gate: BTreeMap<String, GateConfig>,
}

/// How `gate` treats one kind of finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateMode {
    /// Findings fail the gate.
    Enforce,
    /// Findings are listed and the gate still passes.
    Report,
    /// Not checked.
    Off,
}
impl GateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Report => "report",
            Self::Off => "off",
        }
    }
}
/// `[gate.<platform>]`. Defaults: `files` enforce when the platform's source globs name
/// user-visible files or string catalogs (or are a plain list), otherwise off; `surfaces`
/// enforce when the platform has `[surface_scan]`, otherwise off.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateConfig {
    pub files: Option<GateMode>,
    pub surfaces: Option<GateMode>,
}
/// The modes `gate` applies on a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GateModes {
    pub files: GateMode,
    pub surfaces: GateMode,
}

/// One system surface a feature can reach.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceConfig {
    #[serde(default)]
    pub title: Option<String>,
    /// Catalog platforms that can have this surface.
    pub platforms: Vec<String>,
    /// Scanner signal patterns (`signal[:detail][@context]`); defaults for well-known IDs.
    #[serde(default)]
    pub signals: Option<Vec<String>>,
    #[serde(default)]
    pub release: SurfaceRelease,
}
impl SurfaceConfig {
    pub fn title<'a>(&'a self, id: &'a str) -> &'a str {
        self.title.as_deref().unwrap_or(id)
    }
}
/// Whether a gap on the surface fails `gate-release`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceRelease {
    #[default]
    Optional,
    Required,
}

/// Source globs (relative to the platform root) read by the declared-surface parsers.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SurfaceScanConfig {
    pub swift: Vec<String>,
    pub plist: Vec<String>,
    pub manifests: Vec<String>,
    /// Extra Android `res` directories searched after the one beside each manifest (for
    /// Gradle source sets that share resources).
    pub res: Vec<String>,
    /// Context name -> globs; declarations from matching files carry the context.
    pub contexts: BTreeMap<String, Vec<String>>,
    /// Context name -> widget families assumed when a widget has no `.supportedFamilies`
    /// (for example accessory families in a watch target). Otherwise system small, medium
    /// and large.
    pub default_families: BTreeMap<String, Vec<String>>,
    /// File globs to leave out (for example sources built only for another platform).
    pub exclude: Vec<String>,
    /// Declaration ID globs to leave out.
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionConfig {
    /// Repository-relative file holding the version string.
    pub source: String,
    #[serde(default)]
    pub platforms: BTreeMap<String, VersionChecks>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum VersionChecks {
    One(VersionCheck),
    Many(Vec<VersionCheck>),
}
impl VersionChecks {
    pub fn checks(&self) -> &[VersionCheck] {
        match self {
            Self::One(one) => std::slice::from_ref(one),
            Self::Many(many) => many,
        }
    }
}
/// How to read one platform's version: a file (optionally with a regex whose first capture
/// group is the version), or a command that prints it. `reads_source` instead asserts the
/// file refers to the source file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionCheck {
    /// Relative to the platform root.
    pub file: Option<String>,
    pub pattern: Option<String>,
    /// Run with `sh -c` from the platform root; the last non-empty stdout line is the version.
    pub command: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub reads_source: bool,
}

/// Catalog-wide UI comparison settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub tolerance: UiTolerance,
    /// Categories a reviewer may give an accepted `platform` difference.
    pub platform_categories: Vec<String>,
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            tolerance: UiTolerance::default(),
            platform_categories: DEFAULT_PLATFORM_CATEGORIES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        }
    }
}
pub const DEFAULT_PLATFORM_CATEGORIES: &[&str] = &[
    "status-bar",
    "gesture-inset",
    "font-metrics",
    "glyph",
    "touch-target",
    "native-affordance",
];
/// Equivalence tolerances for cross-platform geometry, in logical points/dp.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiTolerance {
    pub position_dp: Option<f64>,
    pub size_ratio: Option<f64>,
    pub radius_dp: Option<f64>,
}

/// Root-relative globs for a platform: a plain list (every source file), or a table that
/// also names user-visible files and string catalogs for `gate`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SourceGlobs {
    All(Vec<String>),
    Table {
        #[serde(default)]
        all: Vec<String>,
        #[serde(default)]
        user_visible: Vec<String>,
        #[serde(default)]
        strings: Vec<String>,
        /// Test files, for `inventory` (files matching `all` that look like tests also count).
        #[serde(default)]
        tests: Vec<String>,
    },
}
impl SourceGlobs {
    pub fn all(&self) -> &[String] {
        match self {
            Self::All(all) | Self::Table { all, .. } => all,
        }
    }
    pub fn user_visible(&self) -> &[String] {
        match self {
            Self::All(_) => &[],
            Self::Table { user_visible, .. } => user_visible,
        }
    }
    pub fn strings(&self) -> &[String] {
        match self {
            Self::All(_) => &[],
            Self::Table { strings, .. } => strings,
        }
    }
    pub fn tests(&self) -> &[String] {
        match self {
            Self::All(_) => &[],
            Self::Table { tests, .. } => tests,
        }
    }
}

/// A platform's root directory, optionally inheriting records from another platform.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PlatformConfig {
    Root(String),
    Table {
        #[serde(default = "dot")]
        root: String,
        inherits: Option<String>,
    },
}
impl PlatformConfig {
    pub fn root(&self) -> &str {
        match self {
            Self::Root(root) | Self::Table { root, .. } => root,
        }
    }
    pub fn inherits(&self) -> Option<&str> {
        match self {
            Self::Root(_) => None,
            Self::Table { inherits, .. } => inherits.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockerStatus {
    #[default]
    Open,
    Decided,
    Resolved,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockerKind {
    Decision,
    Dependency,
    Sdk,
    Platform,
    Hardware,
}
/// Something outside the implementer's control that blocks work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocker {
    pub title: String,
    #[serde(default)]
    pub kind: Option<BlockerKind>,
    #[serde(default)]
    pub owner: Option<String>,
    /// Where the decision is tracked.
    #[serde(default, alias = "decision")]
    pub link: Option<String>,
    #[serde(default)]
    pub status: BlockerStatus,
}
impl Blocker {
    /// Only open blockers hold work back; decided and resolved ones do not.
    pub fn blocks(&self) -> bool {
        self.status == BlockerStatus::Open
    }
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
