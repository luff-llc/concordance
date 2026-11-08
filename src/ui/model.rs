use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tokens {
    pub colors: BTreeMap<String, ColorRole>,
    pub typography: BTreeMap<String, TypeStyle>,
    pub spacing: BTreeMap<String, f64>,
    pub radii: BTreeMap<String, f64>,
    pub materials: BTreeMap<String, Material>,
    pub motions: BTreeMap<String, Motion>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorRole {
    pub light: String,
    pub dark: String,
    /// Artwork rules are implementation contracts; light/dark are fallback colors.
    pub rule: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeStyle {
    pub size: f64,
    pub line_height: f64,
    pub weight: u16,
    pub dynamic_type: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub swift: String,
    pub elevation: f64,
    pub blur: f64,
    pub opacity: f64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Motion {
    pub duration: Option<f64>,
    pub response: Option<f64>,
    pub damping: Option<f64>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    pub id: String,
    pub entries: Vec<String>,
    pub anatomy: Vec<Region>,
    pub states: BTreeMap<String, ScreenState>,
    #[serde(default)]
    pub tolerances: Tolerances,
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub id: String,
    pub role: String,
    pub priority: u16,
    #[serde(default)]
    pub tokens: TokenUses,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub notes: BTreeMap<String, String>,
    #[serde(default)]
    pub children: Vec<Region>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenUses {
    pub color: Option<String>,
    pub typography: Option<String>,
    pub spacing: Option<String>,
    pub radius: Option<String>,
    pub material: Option<String>,
    pub motion: Option<String>,
    /// Sample point relative to the region, defaults to its center.
    pub sample: Option<[f64; 2]>,
}
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub min_width: Option<f64>,
    pub max_width: Option<f64>,
    pub min_height: Option<f64>,
    pub max_height: Option<f64>,
    pub below: Option<String>,
    pub right_of: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenState {
    pub theme: Theme,
    #[serde(default = "one")]
    pub font_scale: f64,
    /// Platforms map to reference PNG paths relative to the catalog directory.
    pub captures: BTreeMap<String, String>,
    #[serde(default)]
    pub absent: Vec<String>,
}
fn one() -> f64 {
    1.0
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
}
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tolerances {
    /// Absolute normalized coordinate/size difference across platforms.
    pub geometry: f64,
    pub delta_e: f64,
    /// Absolute logical points/sp after applying the state's font_scale.
    pub text_size: f64,
}
impl Default for Tolerances {
    fn default() -> Self {
        Self {
            geometry: 0.05,
            delta_e: 5.0,
            text_size: 1.0,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// Pixel dimensions must match the PNG. Node frames use logical units.
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub nodes: Vec<Node>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub identifier: String,
    pub role: String,
    pub frame: Frame,
    pub text_size: Option<f64>,
    /// Resolved artwork-derived color, required when checking a rule-based role.
    pub resolved_color: Option<String>,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Frame {
    pub(super) fn normalized(self, snapshot: &Snapshot) -> Self {
        let w = snapshot.width as f64 / snapshot.scale;
        let h = snapshot.height as f64 / snapshot.scale;
        Self {
            x: self.x / w,
            y: self.y / h,
            width: self.width / w,
            height: self.height / h,
        }
    }
}
impl Screen {
    pub fn regions(&self) -> Vec<&Region> {
        fn walk<'a>(regions: &'a [Region], out: &mut Vec<&'a Region>) {
            for region in regions {
                out.push(region);
                walk(&region.children, out);
            }
        }
        let mut out = vec![];
        walk(&self.anatomy, &mut out);
        out
    }
}
