//! Reviewable comparisons: side-by-side images, reviewer notes, and proposed verification patches.
use super::compare::Image;
use super::UiCatalog;
use crate::{fail, schema, CaptureState, Result, Status, Verification};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufWriter;
use std::path::Path;

/// A reviewer's verdict on one baseline/target pair.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    #[default]
    Unreviewed,
    Same,
    Differs,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairReview {
    pub screen: String,
    pub state: String,
    #[serde(default)]
    pub verdict: Verdict,
    /// Free-text differences noted by the reviewer.
    #[serde(default)]
    pub differences: String,
    /// Side-by-side image, relative to the review file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}
/// Reviewer notes saved from the compare index (`review.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub version: u32,
    #[serde(default)]
    pub reviewer: String,
    #[serde(default)]
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_commit: Option<String>,
    pub baseline: String,
    pub target: String,
    pub pairs: Vec<PairReview>,
}
impl Review {
    pub fn load(path: &Path) -> Result<Self> {
        let review: Self = serde_json::from_str(&fs::read_to_string(path)?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if review.version != 1 {
            return fail(format!("{}: unsupported review version", path.display()));
        }
        Ok(review)
    }
}

/// One proposed change to an entry's target platform record. Never applied automatically.
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    pub id: String,
    pub file: String,
    pub platform: String,
    /// `verify`, `needs-work`, `not-implemented` or `rejected`.
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub pairs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
}
#[derive(Debug, Default, Serialize)]
pub struct VerifyOutcome {
    pub proposals: Vec<Proposal>,
    /// Reviewed pairs that cannot support verification, with the reason.
    pub excluded: Vec<String>,
}
impl VerifyOutcome {
    /// The proposed patch as commented YAML.
    pub fn patch(&self) -> Result<String> {
        let mut out = String::from("# Proposed by `concordance verify` from reviewer notes.\n# Nothing was applied: review each proposal, then edit the catalog by hand.\n");
        for line in &self.excluded {
            out.push_str(&format!("# Excluded pair: {line}\n"));
        }
        if self.proposals.is_empty() {
            out.push_str("[]\n");
        } else {
            out.push_str(&serde_yaml::to_string(&self.proposals)?);
        }
        Ok(out)
    }
    pub fn summary(&self) -> String {
        let count = |a: &str| self.proposals.iter().filter(|p| p.action == a).count();
        format!(
            "{} verify, {} needs-work, {} not-implemented, {} rejected; {} pair(s) excluded.",
            count("verify"),
            count("needs-work"),
            count("not-implemented"),
            count("rejected"),
            self.excluded.len()
        )
    }
}

impl UiCatalog {
    /// Fold reviewer notes into proposed verification records for linked entries.
    pub fn verify_review(&self, review: &Review, captures: &Path) -> Result<VerifyOutcome> {
        self.require_platform(&review.baseline)?;
        self.require_platform(&review.target)?;
        if review.reviewer.trim().is_empty() {
            return fail("review.json: reviewer is required");
        }
        if !schema::valid_date(&review.at) {
            return fail("review.json: at must be a YYYY-MM-DD date");
        }
        for commit in [&review.commit, &review.baseline_commit]
            .into_iter()
            .flatten()
        {
            if !schema::valid_commit(commit) {
                return fail(format!("review.json: invalid commit {commit:?}"));
            }
        }
        let coverage = self.coverage(captures, &[&review.baseline, &review.target])?;
        let states: BTreeMap<(String, String), &CaptureState> = coverage
            .records
            .iter()
            .map(|r| ((r.key(), r.platform.clone()), &r.capture))
            .collect();
        let mut outcome = VerifyOutcome::default();
        // Entry ID -> reviewed pairs (key, verdict, differences).
        let mut by_entry: BTreeMap<&str, Vec<(String, Verdict, &str)>> = BTreeMap::new();
        for pair in &review.pairs {
            let screen = self
                .screens
                .get(&pair.screen)
                .filter(|s| s.states.contains_key(&pair.state))
                .ok_or_else(|| {
                    format!(
                        "review.json: {}.{} is not a declared screen state",
                        pair.screen, pair.state
                    )
                })?;
            if pair.verdict == Verdict::Unreviewed {
                continue;
            }
            let key = format!("{}.{}", pair.screen, pair.state);
            let blocked = [&review.baseline, &review.target]
                .into_iter()
                .find_map(|p| match states.get(&(key.clone(), p.clone())) {
                    Some(CaptureState::Produced) => None,
                    Some(CaptureState::Skipped(r)) => Some(format!("{p} capture skipped: {r}")),
                    Some(CaptureState::Missing(r)) => Some(format!("{p} capture missing: {r}")),
                    None => Some(format!("{p} capture unknown")),
                });
            if let Some(reason) = blocked {
                outcome.excluded.push(format!("{key}: {reason}"));
                continue;
            }
            for entry in &screen.entries {
                by_entry.entry(entry).or_default().push((
                    key.clone(),
                    pair.verdict,
                    pair.differences.trim(),
                ));
            }
        }
        for (id, pairs) in by_entry {
            let entry = self.catalog.entry(id)?;
            let area = &self.catalog.areas[id];
            let record = entry.platforms.get(&review.target);
            let keys: Vec<String> = pairs.iter().map(|p| p.0.clone()).collect();
            let differences: Vec<String> = pairs
                .iter()
                .filter(|p| p.1 == Verdict::Differs)
                .map(|p| {
                    format!(
                        "{}: {}",
                        p.0,
                        if p.2.is_empty() {
                            "differs (no note)"
                        } else {
                            p.2
                        }
                    )
                })
                .collect();
            let mut proposal = Proposal {
                id: id.to_owned(),
                file: self.catalog.area_files[area].clone(),
                platform: review.target.clone(),
                action: String::new(),
                reason: None,
                pairs: keys.clone(),
                verification: None,
                gap: None,
            };
            if record.is_none_or(|r| r.status != Status::Shipped) {
                proposal.action = "not-implemented".into();
                proposal.reason = Some(format!(
                    "{} status is {}",
                    review.target,
                    record.map(|r| r.status.as_str()).unwrap_or("unrecorded")
                ));
                proposal.gap = (!differences.is_empty()).then(|| differences.join("; "));
            } else if record.and_then(|r| r.implemented_by.as_deref())
                == Some(review.reviewer.as_str())
            {
                proposal.action = "rejected".into();
                proposal.reason = Some("reviewer is the implementer".into());
            } else if !differences.is_empty() {
                proposal.action = "needs-work".into();
                proposal.gap = Some(differences.join("; "));
            } else {
                proposal.action = "verify".into();
                let existing = record.and_then(|r| r.verification.as_ref());
                let captures: BTreeSet<String> = existing
                    .map(|v| v.captures.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .chain(keys)
                    .collect();
                let notes: Vec<&str> = pairs
                    .iter()
                    .map(|p| p.2)
                    .filter(|n| !n.is_empty())
                    .collect();
                proposal.verification = Some(Verification {
                    by: review.reviewer.clone(),
                    at: review.at.clone(),
                    commit: review.commit.clone(),
                    baseline_commit: review.baseline_commit.clone(),
                    captures: captures.into_iter().collect(),
                    journeys: existing.map(|v| v.journeys.clone()).unwrap_or_default(),
                    notes: Some(if notes.is_empty() {
                        format!(
                            "Side-by-side review against {}: no differences noted.",
                            review.baseline
                        )
                    } else {
                        format!(
                            "Side-by-side review against {}: {}",
                            review.baseline,
                            notes.join("; ")
                        )
                    }),
                });
            }
            outcome.proposals.push(proposal);
        }
        Ok(outcome)
    }
}

/// Render the baseline | target pair at equal height with a label bar, as a PNG.
pub(super) fn write_side_by_side(
    path: &Path,
    left: (Option<&Image>, &str),
    right: (Option<&Image>, &str),
    caption: &str,
) -> Result<()> {
    const MAX_HEIGHT: u32 = 1600;
    const GAP: u32 = 16;
    let heights: Vec<u32> = [left.0, right.0]
        .iter()
        .flatten()
        .map(|i| i.height)
        .collect();
    let height = heights.iter().copied().min().unwrap_or(800).min(MAX_HEIGHT);
    let width_for = |image: Option<&Image>, other: Option<&Image>| -> u32 {
        match image.or(other) {
            Some(i) => ((i.width as u64 * height as u64) / i.height.max(1) as u64).max(1) as u32,
            None => height / 2,
        }
    };
    let lw = width_for(left.0, right.0);
    let rw = width_for(right.0, left.0);
    let scale = (height / 400).clamp(2, 4);
    let line = 7 * scale;
    let pad = 3 * scale;
    let bar = pad * 3 + line * 2;
    let total_w = lw + GAP + rw;
    let total_h = bar + height;
    let mut canvas = vec![[255u8; 3]; (total_w * total_h) as usize];
    for (image, x0, w) in [(left.0, 0, lw), (right.0, lw + GAP, rw)] {
        match image {
            Some(image) => blit_scaled(&mut canvas, total_w, image, x0, bar, w, height),
            None => {
                for y in bar..total_h {
                    for x in x0..x0 + w {
                        canvas[(y * total_w + x) as usize] = [220, 220, 220];
                    }
                }
                let text = "NO CAPTURE";
                let tw = text_width(text, scale);
                draw_text(
                    &mut canvas,
                    total_w,
                    x0 + w.saturating_sub(tw) / 2,
                    bar + height / 2,
                    text,
                    scale,
                    w,
                );
            }
        }
    }
    for x in lw..lw + GAP {
        for y in 0..total_h {
            canvas[(y * total_w + x) as usize] = [40, 40, 40];
        }
    }
    draw_text(
        &mut canvas,
        total_w,
        pad,
        pad,
        left.1,
        scale,
        lw.saturating_sub(pad),
    );
    draw_text(
        &mut canvas,
        total_w,
        lw + GAP + pad,
        pad,
        right.1,
        scale,
        rw.saturating_sub(pad),
    );
    draw_text(
        &mut canvas,
        total_w,
        pad,
        pad * 2 + line,
        caption,
        scale,
        lw.saturating_sub(pad),
    );
    draw_text(
        &mut canvas,
        total_w,
        lw + GAP + pad,
        pad * 2 + line,
        caption,
        scale,
        rw.saturating_sub(pad),
    );
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), total_w, total_h);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&canvas.concat())?;
    Ok(())
}

/// Box-filter scaling, composited over white.
fn blit_scaled(
    canvas: &mut [[u8; 3]],
    stride: u32,
    image: &Image,
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
) {
    let (sw, sh) = (image.width as u64, image.height as u64);
    for y in 0..h as u64 {
        let ya = y * sh / h as u64;
        let yb = ((y + 1) * sh / h as u64).max(ya + 1).min(sh);
        for x in 0..w as u64 {
            let xa = x * sw / w as u64;
            let xb = ((x + 1) * sw / w as u64).max(xa + 1).min(sw);
            let mut sum = [0u64; 3];
            let mut n = 0u64;
            for sy in ya..yb {
                for sx in xa..xb {
                    let p = image.pixels[(sy * sw + sx) as usize];
                    let a = p[3] as u64;
                    for c in 0..3 {
                        sum[c] += (p[c] as u64 * a + 255 * (255 - a)) / 255;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            canvas[((y0 as u64 + y) * stride as u64 + x0 as u64 + x) as usize] =
                [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8];
        }
    }
}

fn text_width(text: &str, scale: u32) -> u32 {
    text.chars().count() as u32 * 6 * scale
}

/// Draw uppercase ASCII with a 5x7 bitmap font, clipped to `max_width`.
fn draw_text(
    canvas: &mut [[u8; 3]],
    stride: u32,
    x: u32,
    y: u32,
    text: &str,
    scale: u32,
    max_width: u32,
) {
    let height = canvas.len() as u32 / stride;
    for (index, c) in text.chars().enumerate() {
        let cx = x + index as u32 * 6 * scale;
        if cx + 5 * scale > x + max_width {
            break;
        }
        let rows = glyph(c.to_ascii_uppercase());
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (0b10000 >> col) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = cx + col * scale + dx;
                        let py = y + row as u32 * scale + dy;
                        if px < stride && py < height {
                            canvas[(py * stride + px) as usize] = [0, 0, 0];
                        }
                    }
                }
            }
        }
    }
}

fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 17, 25, 21, 19, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [31, 2, 4, 2, 1, 17, 14],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [6, 8, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 2, 12],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '|' => [4, 4, 4, 4, 4, 4, 4],
        ' ' => [0; 7],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
}
