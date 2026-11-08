//! Optional design contracts and native capture comparison.
mod compare;
mod generate;
mod model;
pub use compare::{Comparison, PairResult, PlatformResult};
pub use model::*;

use crate::{fail, schema, Catalog, Result};
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct UiCatalog {
    pub catalog: Catalog,
    pub tokens: Tokens,
    pub screens: BTreeMap<String, Screen>,
}
fn load_yaml<T: DeserializeOwned>(path: &Path, schema: &str) -> Result<T> {
    let value: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(path)?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let value = serde_json::to_value(value)?;
    schema::validate_document(&value, schema).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(serde_json::from_value(value)?)
}
pub(super) fn name(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.')
}
pub(super) fn rgb(s: &str) -> Result<[f64; 3]> {
    if s.len() != 7 || !s.starts_with('#') || !s[1..].bytes().all(|c| c.is_ascii_hexdigit()) {
        return fail(format!("invalid opaque sRGB color {s:?}; expected #RRGGBB"));
    }
    Ok([
        u8::from_str_radix(&s[1..3], 16)? as f64,
        u8::from_str_radix(&s[3..5], 16)? as f64,
        u8::from_str_radix(&s[5..7], 16)? as f64,
    ])
}
pub(super) fn reference(root: &Path, relative: &str) -> Result<PathBuf> {
    if !crate::safe_relative(relative, false) {
        return fail(format!("unsafe capture path: {relative}"));
    }
    let path = root.join(relative);
    if path.exists() && !path.canonicalize()?.starts_with(root.canonicalize()?) {
        return fail(format!("capture escapes directory: {relative}"));
    }
    Ok(path)
}
impl UiCatalog {
    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        let catalog = Catalog::load(directory)?;
        let tokens: Tokens = load_yaml(
            &catalog.directory.join("tokens.yaml"),
            include_str!("../../schema/tokens.schema.json"),
        )?;
        tokens.validate()?;
        let mut files = fs::read_dir(catalog.directory.join("screens"))?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        files.retain(|p| matches!(p.extension().and_then(|s| s.to_str()), Some("yaml" | "yml")));
        files.sort();
        let mut screens = BTreeMap::new();
        for file in files {
            let screen: Screen =
                load_yaml(&file, include_str!("../../schema/screens.schema.json"))?;
            if !name(&screen.id) || screens.contains_key(&screen.id) {
                return fail(format!("invalid or duplicate screen ID: {}", screen.id));
            }
            screens.insert(screen.id.clone(), screen);
        }
        if screens.is_empty() {
            return fail("screens must not be empty");
        }
        let ui = Self {
            catalog,
            tokens,
            screens,
        };
        ui.validate()?;
        Ok(ui)
    }
    pub fn require_platform(&self, platform: &str) -> Result<()> {
        if !name(platform) || !self.catalog.config.platforms.contains_key(platform) {
            return fail(format!("unknown or unsafe UI platform: {platform}"));
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        let mut capture_stems = BTreeSet::new();
        for screen in self.screens.values() {
            let regions = screen.regions();
            let ids: BTreeSet<_> = regions.iter().map(|r| r.id.as_str()).collect();
            if ids.len() != regions.len() {
                return fail(format!("{}: duplicate region identifier", screen.id));
            }
            for entry in &screen.entries {
                self.catalog.entry(entry)?;
            }
            for platform in screen
                .notes
                .keys()
                .chain(regions.iter().flat_map(|r| r.notes.keys()))
            {
                self.require_platform(platform)?;
            }
            for region in &regions {
                if !name(&region.id) {
                    return fail(format!("invalid region ID: {}", region.id));
                }
                let t = &region.tokens;
                for (token, exists) in [
                    (
                        &t.color,
                        t.color
                            .as_ref()
                            .is_none_or(|n| self.tokens.colors.contains_key(n)),
                    ),
                    (
                        &t.typography,
                        t.typography
                            .as_ref()
                            .is_none_or(|n| self.tokens.typography.contains_key(n)),
                    ),
                    (
                        &t.spacing,
                        t.spacing
                            .as_ref()
                            .is_none_or(|n| self.tokens.spacing.contains_key(n)),
                    ),
                    (
                        &t.radius,
                        t.radius
                            .as_ref()
                            .is_none_or(|n| self.tokens.radii.contains_key(n)),
                    ),
                    (
                        &t.material,
                        t.material
                            .as_ref()
                            .is_none_or(|n| self.tokens.materials.contains_key(n)),
                    ),
                    (
                        &t.motion,
                        t.motion
                            .as_ref()
                            .is_none_or(|n| self.tokens.motions.contains_key(n)),
                    ),
                ] {
                    if !exists {
                        return fail(format!("{}: unknown token {token:?}", region.id));
                    }
                }
                for other in [&region.layout.below, &region.layout.right_of]
                    .into_iter()
                    .flatten()
                {
                    if other == &region.id || !ids.contains(other.as_str()) {
                        return fail(format!("{}: invalid layout reference {other}", region.id));
                    }
                }
                for (min, max) in [
                    (region.layout.min_width, region.layout.max_width),
                    (region.layout.min_height, region.layout.max_height),
                ] {
                    if min.zip(max).is_some_and(|(a, b)| a > b) {
                        return fail("layout minimum exceeds maximum");
                    }
                }
            }
            for (state_id, state) in &screen.states {
                if !name(state_id) {
                    return fail(format!("invalid state ID: {state_id}"));
                }
                let mut absent = BTreeSet::new();
                for id in &state.absent {
                    if !ids.contains(id.as_str()) || !absent.insert(id) {
                        return fail(format!("invalid absent region: {id}"));
                    }
                }
                for region in &regions {
                    if absent.contains(&region.id)
                        && region.children.iter().any(|r| !absent.contains(&r.id))
                    {
                        return fail(format!(
                            "{}: absent parent requires absent children",
                            region.id
                        ));
                    }
                }
                for platform in self.catalog.config.platforms.keys() {
                    let stem = format!("{}.{state_id}.{platform}", screen.id);
                    if !capture_stems.insert(stem.clone()) {
                        return fail(format!("ambiguous capture filename: {stem}"));
                    }
                    self.require_platform(platform)?;
                    let path = state.captures.get(platform).ok_or_else(|| {
                        format!(
                            "{}.{}: missing reference for {platform}",
                            screen.id, state_id
                        )
                    })?;
                    reference(&self.catalog.directory, path)?;
                    if Path::new(path).file_name().and_then(|s| s.to_str())
                        != Some(&format!("{}.{}.{platform}.png", screen.id, state_id))
                    {
                        return fail(format!(
                            "capture filename does not follow convention: {path}"
                        ));
                    }
                }
                for platform in state.captures.keys() {
                    self.require_platform(platform)?;
                }
            }
        }
        Ok(())
    }
}
impl Tokens {
    pub fn load(directory: impl AsRef<Path>) -> Result<Self> {
        let tokens: Self = load_yaml(
            &directory.as_ref().join("tokens.yaml"),
            include_str!("../../schema/tokens.schema.json"),
        )?;
        tokens.validate()?;
        Ok(tokens)
    }
    fn validate(&self) -> Result<()> {
        for key in self
            .colors
            .keys()
            .chain(self.typography.keys())
            .chain(self.spacing.keys())
            .chain(self.radii.keys())
            .chain(self.materials.keys())
            .chain(self.motions.keys())
        {
            if !name(key) {
                return fail(format!("invalid token name: {key}"));
            }
        }
        for color in self.colors.values() {
            rgb(&color.light)?;
            rgb(&color.dark)?;
        }
        for style in self.typography.values() {
            if ![
                "largeTitle",
                "title1",
                "title2",
                "title3",
                "headline",
                "subheadline",
                "body",
                "callout",
                "footnote",
                "caption1",
                "caption2",
            ]
            .contains(&style.dynamic_type.as_str())
            {
                return fail(format!(
                    "unknown Dynamic Type style: {}",
                    style.dynamic_type
                ));
            }
        }
        for motion in self.motions.values() {
            if !matches!(
                (motion.duration, motion.response, motion.damping),
                (Some(_), None, None) | (None, Some(_), Some(_))
            ) {
                return fail("motion requires duration OR response and damping");
            }
        }
        Ok(())
    }
}
