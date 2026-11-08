use super::{reference, rgb, Frame, Node, Region, Screen, ScreenState, Snapshot, Theme, UiCatalog};
use crate::{fail, Result};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufReader;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct PlatformResult {
    pub platform: String,
    pub captured: bool,
    pub failures: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct PairResult {
    pub screen: String,
    pub state: String,
    pub baseline: PlatformResult,
    pub target: PlatformResult,
    /// Downsampled luminance SSIM; informational, never a gate.
    pub similarity: Option<f64>,
    pub reviewer_prompt: String,
}
#[derive(Debug, Serialize)]
pub struct Comparison {
    pub pairs: Vec<PairResult>,
}
impl Comparison {
    pub fn failed(&self) -> bool {
        self.pairs
            .iter()
            .any(|p| !p.baseline.failures.is_empty() || !p.target.failures.is_empty())
    }
    pub fn summary(&self) -> String {
        screen_summary(self.pairs.iter().flat_map(|pair| {
            [
                Some(&pair.baseline),
                (pair.baseline.platform != pair.target.platform).then_some(&pair.target),
            ]
            .into_iter()
            .flatten()
        }))
    }
}
fn screen_summary<'a>(results: impl IntoIterator<Item = &'a PlatformResult>) -> String {
    let mut counts: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
    for result in results {
        let c = counts.entry(&result.platform).or_default();
        if result.captured {
            c[0] += 1;
            if result.failures.is_empty() {
                c[1] += 1;
            } else {
                c[2] += 1;
            }
        } else {
            c[3] += 1;
        }
    }
    let mut out = String::from("## Screen parity\n\nCounts are screen/state captures; matching means declared structural and token checks passed.\nSimilarity is a trend signal only; reviewer judgment is still required.\n\n| Platform | Captured | Matching | Drifting | Missing |\n| --- | ---: | ---: | ---: | ---: |\n");
    for (platform, c) in counts {
        out.push_str(&format!(
            "| {platform} | {} | {} | {} | {} |\n",
            c[0], c[1], c[2], c[3]
        ));
    }
    out
}

struct Image {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}
impl Image {
    fn load(path: &Path) -> Result<Self> {
        let mut decoder = png::Decoder::new(BufReader::new(fs::File::open(path)?));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        decoder.set_limits(png::Limits {
            bytes: 64 * 1024 * 1024,
        });
        let mut reader = decoder.read_info()?;
        let size = reader.output_buffer_size();
        if size > 64 * 1024 * 1024 {
            return fail("PNG exceeds 64 MiB decoded limit");
        }
        let mut buffer = vec![0; size];
        let info = reader.next_frame(&mut buffer)?;
        let channels = info.color_type.samples();
        let pixels = buffer[..info.buffer_size()]
            .chunks_exact(channels)
            .map(|p| match info.color_type {
                png::ColorType::Rgb => [p[0], p[1], p[2], 255],
                png::ColorType::Rgba => [p[0], p[1], p[2], p[3]],
                png::ColorType::Grayscale => [p[0], p[0], p[0], 255],
                png::ColorType::GrayscaleAlpha => [p[0], p[0], p[0], p[1]],
                png::ColorType::Indexed => unreachable!("palette expanded"),
            })
            .collect();
        Ok(Self {
            width: info.width,
            height: info.height,
            pixels,
        })
    }
    fn sample(&self, frame: Frame, point: [f64; 2], scale: f64) -> [u8; 4] {
        let x = ((frame.x + frame.width * point[0]) * scale)
            .floor()
            .clamp(0.0, (self.width - 1) as f64) as usize;
        let y = ((frame.y + frame.height * point[1]) * scale)
            .floor()
            .clamp(0.0, (self.height - 1) as f64) as usize;
        self.pixels[y * self.width as usize + x]
    }
    fn luminance(&self) -> Vec<f64> {
        let mut out = vec![0.0; 32 * 32];
        for y in 0..32 {
            for x in 0..32 {
                let x0 = x * self.width as usize / 32;
                let x1 = ((x + 1) * self.width as usize / 32).max(x0 + 1);
                let y0 = y * self.height as usize / 32;
                let y1 = ((y + 1) * self.height as usize / 32).max(y0 + 1);
                let mut sum = 0.0;
                for py in y0..y1 {
                    for px in x0..x1 {
                        let p = self.pixels[py * self.width as usize + px];
                        let a = p[3] as f64 / 255.0;
                        sum += (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64)
                            / 255.0
                            * a
                            + 1.0
                            - a;
                    }
                }
                out[y * 32 + x] = sum / ((x1 - x0) * (y1 - y0)) as f64;
            }
        }
        out
    }
}
fn similarity(a: &Image, b: &Image) -> f64 {
    let (a, b) = (a.luminance(), b.luminance());
    let mut score = 0.0;
    for wy in 0..4 {
        for wx in 0..4 {
            let mut aa = vec![];
            let mut bb = vec![];
            for y in wy * 8..wy * 8 + 8 {
                for x in wx * 8..wx * 8 + 8 {
                    aa.push(a[y * 32 + x]);
                    bb.push(b[y * 32 + x]);
                }
            }
            let ma = aa.iter().sum::<f64>() / 64.0;
            let mb = bb.iter().sum::<f64>() / 64.0;
            let va = aa.iter().map(|a| (a - ma).powi(2)).sum::<f64>() / 63.0;
            let vb = bb.iter().map(|b| (b - mb).powi(2)).sum::<f64>() / 63.0;
            let cov = aa
                .iter()
                .zip(&bb)
                .map(|(a, b)| (a - ma) * (b - mb))
                .sum::<f64>()
                / 63.0;
            score += ((2.0 * ma * mb + 0.0001) * (2.0 * cov + 0.0009))
                / ((ma * ma + mb * mb + 0.0001) * (va + vb + 0.0009));
        }
    }
    (score / 16.0).clamp(0.0, 1.0)
}
fn lab(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(|c| {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    let f = |v: f64| {
        if v > 216.0 / 24389.0 {
            v.cbrt()
        } else {
            (24389.0 / 27.0 * v + 16.0) / 116.0
        }
    };
    let x = f((0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047);
    let y = f(0.2126729 * r + 0.7151522 * g + 0.0721750 * b);
    let z = f((0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883);
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}
fn delta_e(a: [f64; 3], b: [f64; 3]) -> f64 {
    lab(a)
        .into_iter()
        .zip(lab(b))
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}
struct Evidence {
    result: PlatformResult,
    image: Option<Image>,
    snapshot: Option<Snapshot>,
}
fn valid_snapshot(snapshot: &Snapshot, image: &Image) -> Result<()> {
    if snapshot.width != image.width
        || snapshot.height != image.height
        || !snapshot.scale.is_finite()
        || snapshot.scale <= 0.0
    {
        return fail("snapshot dimensions/scale do not match PNG");
    }
    let mut ids = BTreeSet::new();
    let w = snapshot.width as f64 / snapshot.scale;
    let h = snapshot.height as f64 / snapshot.scale;
    for node in &snapshot.nodes {
        let f = node.frame;
        if node.identifier.is_empty()
            || node.role.trim().is_empty()
            || !ids.insert(&node.identifier)
        {
            return fail("empty or duplicate snapshot identifier/role");
        }
        if [f.x, f.y, f.width, f.height]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
            || f.width <= 0.0
            || f.height <= 0.0
            || f.x + f.width > w + 0.001
            || f.y + f.height > h + 0.001
        {
            return fail(format!("{}: invalid or offscreen frame", node.identifier));
        }
        if node.text_size.is_some_and(|s| !s.is_finite() || s <= 0.0) {
            return fail("invalid text_size");
        }
    }
    Ok(())
}
impl UiCatalog {
    fn evidence(
        &self,
        screen: &Screen,
        state_id: &str,
        state: &ScreenState,
        platform: &str,
        captures: &Path,
    ) -> Result<Evidence> {
        let stem = format!("{}.{state_id}.{platform}", screen.id);
        let png = reference(captures, &format!("{stem}.png"))?;
        let json = reference(captures, &format!("{stem}.json"))?;
        let mut result = PlatformResult {
            platform: platform.into(),
            captured: png.is_file() && json.is_file(),
            failures: vec![],
        };
        if !result.captured {
            result
                .failures
                .push("structural: missing PNG/JSON capture pair".into());
            return Ok(Evidence {
                result,
                image: None,
                snapshot: None,
            });
        }
        let image = match Image::load(&png) {
            Ok(image) => image,
            Err(e) => {
                result
                    .failures
                    .push(format!("structural: invalid PNG: {e}"));
                return Ok(Evidence {
                    result,
                    image: None,
                    snapshot: None,
                });
            }
        };
        let snapshot: Result<Snapshot> = (|| {
            let snapshot: Snapshot = serde_json::from_str(&fs::read_to_string(json)?)?;
            valid_snapshot(&snapshot, &image)?;
            Ok(snapshot)
        })();
        let snapshot = match snapshot {
            Ok(s) => s,
            Err(e) => {
                result
                    .failures
                    .push(format!("structural: invalid snapshot: {e}"));
                return Ok(Evidence {
                    result,
                    image: Some(image),
                    snapshot: None,
                });
            }
        };
        let nodes: BTreeMap<_, _> = snapshot
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.identifier.as_str(), (i, n)))
            .collect();
        let mut previous = None;
        for region in screen.regions() {
            if state.absent.contains(&region.id) {
                if nodes.contains_key(region.id.as_str()) {
                    result
                        .failures
                        .push(format!("structural: {} should be absent", region.id));
                }
                continue;
            }
            let Some(&(index, node)) = nodes.get(region.id.as_str()) else {
                result
                    .failures
                    .push(format!("structural: missing region {}", region.id));
                continue;
            };
            if previous.is_some_and(|i| index <= i) {
                result
                    .failures
                    .push(format!("structural: {} is out of anatomy order", region.id));
            }
            previous = Some(index);
            if node.role != region.role {
                result.failures.push(format!(
                    "structural: {} role {:?}, expected {:?}",
                    region.id, node.role, region.role
                ));
            }
            self.check_region(
                screen,
                state,
                region,
                node,
                &snapshot,
                &image,
                &nodes,
                &mut result.failures,
            );
        }
        check_containment(
            &screen.anatomy,
            &state.absent,
            &nodes,
            screen.tolerances.geometry,
            &snapshot,
            &mut result.failures,
        );
        Ok(Evidence {
            result,
            image: Some(image),
            snapshot: Some(snapshot),
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn check_region(
        &self,
        screen: &Screen,
        state: &ScreenState,
        region: &Region,
        node: &Node,
        snapshot: &Snapshot,
        image: &Image,
        nodes: &BTreeMap<&str, (usize, &Node)>,
        failures: &mut Vec<String>,
    ) {
        let frame = node.frame.normalized(snapshot);
        let l = &region.layout;
        for (label, value, min, max) in [
            ("width", frame.width, l.min_width, l.max_width),
            ("height", frame.height, l.min_height, l.max_height),
        ] {
            if min.is_some_and(|v| value + 1e-9 < v) || max.is_some_and(|v| value - 1e-9 > v) {
                failures.push(format!(
                    "structural: {} {label} {value:.3} outside declared bounds",
                    region.id
                ));
            }
        }
        for (other, below) in [(&l.below, true), (&l.right_of, false)] {
            if let Some(other) = other {
                if let Some((_, other)) = nodes.get(other.as_str()) {
                    let other = other.frame.normalized(snapshot);
                    let ordered = if below {
                        frame.y + screen.tolerances.geometry >= other.y + other.height
                    } else {
                        frame.x + screen.tolerances.geometry >= other.x + other.width
                    };
                    if !ordered {
                        failures.push(format!(
                            "structural: {} must be {} {}",
                            region.id,
                            if below { "below" } else { "right of" },
                            if below {
                                l.below.as_deref().unwrap_or("")
                            } else {
                                l.right_of.as_deref().unwrap_or("")
                            }
                        ));
                    }
                } else {
                    failures.push(format!(
                        "structural: {} layout reference {other} absent",
                        region.id
                    ));
                }
            }
        }
        if let Some(name) = &region.tokens.color {
            let color = &self.tokens.colors[name];
            let expected = if color.rule.is_some() {
                node.resolved_color.as_deref()
            } else {
                Some(
                    match state.theme {
                        Theme::Light => &color.light,
                        Theme::Dark => &color.dark,
                    }
                    .as_str(),
                )
            };
            match expected.map(rgb).transpose() {
                Ok(Some(expected)) => {
                    let sample = image.sample(
                        node.frame,
                        region.tokens.sample.unwrap_or([0.5, 0.5]),
                        snapshot.scale,
                    );
                    let difference = delta_e(
                        [sample[0] as f64, sample[1] as f64, sample[2] as f64],
                        expected,
                    );
                    if sample[3] != 255 || difference > screen.tolerances.delta_e {
                        failures.push(format!("token: {} color {name} ΔE {difference:.2} exceeds {:.2} or sample is transparent",region.id,screen.tolerances.delta_e));
                    }
                }
                Ok(None) => failures.push(format!(
                    "token: {} needs resolved_color for artwork rule {name}",
                    region.id
                )),
                Err(e) => {
                    failures.push(format!("token: {} invalid resolved_color: {e}", region.id))
                }
            }
        }
        if let Some(name) = &region.tokens.typography {
            let expected = self.tokens.typography[name].size * state.font_scale;
            if node
                .text_size
                .is_none_or(|s| (s - expected).abs() > screen.tolerances.text_size)
            {
                failures.push(format!(
                    "token: {} text_size {:?}; expected {expected:.2} ± {:.2} ({name})",
                    region.id, node.text_size, screen.tolerances.text_size
                ));
            }
        }
    }
    pub fn compare(
        &self,
        captures: &Path,
        baseline: &str,
        target: &str,
        out: Option<&Path>,
    ) -> Result<Comparison> {
        self.require_platform(baseline)?;
        self.require_platform(target)?;
        if let Some(out) = out {
            fs::create_dir_all(out.join("captures"))?;
        }
        let mut comparison = Comparison { pairs: vec![] };
        let mut html = String::from("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>Concordance screen parity</title><style>body{font:16px system-ui;margin:2rem;max-width:1200px;background:#fafafa;color:#171717}section{border-top:1px solid #aaa;padding:1rem 0}.pair{display:flex;gap:1rem;flex-wrap:wrap}.capture{width:min(45%,480px);min-width:240px}svg{width:100%;height:auto}rect{fill:none;stroke:#e00080;stroke-width:1}text{fill:#fff;stroke:#111;stroke-width:.2px;font-size:10px}pre{white-space:pre-wrap}li{margin:.3rem 0}</style><h1>Screen parity</h1><p>Structural and token checks gate parity. Luminance SSIM (0–1) is a trend only. Overlays mark anatomy; hide them to review the images.</p><label><input type=\"checkbox\" checked onchange=\"document.querySelectorAll('.overlay').forEach(e=>e.style.display=this.checked?'':'none')\"> Show anatomy</label>");
        for screen in self.screens.values() {
            for (state_id, state) in &screen.states {
                let base = self.evidence(screen, state_id, state, baseline, captures)?;
                let mut dest = self.evidence(screen, state_id, state, target, captures)?;
                if let (Some(a), Some(b)) = (&base.snapshot, &dest.snapshot) {
                    for region in screen.regions() {
                        if state.absent.contains(&region.id) {
                            continue;
                        }
                        let a_node = a.nodes.iter().find(|n| n.identifier == region.id);
                        let b_node = b.nodes.iter().find(|n| n.identifier == region.id);
                        if let (Some(an), Some(bn)) = (a_node, b_node) {
                            if let Some(color) = region
                                .tokens
                                .color
                                .as_ref()
                                .filter(|name| self.tokens.colors[*name].rule.is_some())
                            {
                                if let (Some(Ok(ac)), Some(Ok(bc))) = (
                                    an.resolved_color.as_deref().map(rgb),
                                    bn.resolved_color.as_deref().map(rgb),
                                ) {
                                    let difference = delta_e(ac, bc);
                                    if difference > screen.tolerances.delta_e {
                                        dest.result.failures.push(format!("token: {} artwork color {color} differs from {baseline}: ΔE {difference:.2} exceeds {:.2}", region.id, screen.tolerances.delta_e));
                                    }
                                }
                            }
                            let af = an.frame.normalized(a);
                            let bf = bn.frame.normalized(b);
                            for (axis, av, bv) in [
                                ("x", af.x, bf.x),
                                ("y", af.y, bf.y),
                                ("width", af.width, bf.width),
                                ("height", af.height, bf.height),
                            ] {
                                if (av - bv).abs() > screen.tolerances.geometry {
                                    dest.result.failures.push(format!("structural: {} {axis} differs from {baseline}: {av:.3} vs {bv:.3} (tolerance {:.3})",region.id,screen.tolerances.geometry));
                                }
                            }
                        }
                    }
                }
                if base.snapshot.is_none() && dest.snapshot.is_some() && baseline != target {
                    dest.result.failures.push(format!(
                        "structural: {baseline} snapshot unavailable for geometry comparison"
                    ));
                }
                let score = base
                    .image
                    .as_ref()
                    .zip(dest.image.as_ref())
                    .map(|(a, b)| similarity(a, b));
                let prompt = reviewer_prompt(screen, state_id, state, baseline, target);
                if let Some(out) = out {
                    html.push_str(&format!("<section><h2>{} · {}</h2><p>Trend SSIM: {} (never a gate)</p><div class=\"pair\">",escape(&screen.id),escape(state_id),score.map(|s| format!("{s:.4}")).unwrap_or_else(|| "unavailable".into())));
                    for evidence in [&base, &dest] {
                        html.push_str(&format!(
                            "<div class=\"capture\"><h3>{}</h3>",
                            escape(&evidence.result.platform)
                        ));
                        if let Some(image) = &evidence.image {
                            let filename = format!(
                                "{}.{}.{}.png",
                                screen.id, state_id, evidence.result.platform
                            );
                            let source = reference(captures, &filename)?;
                            let destination = out.join("captures").join(&filename);
                            if !destination.exists()
                                || source.canonicalize()? != destination.canonicalize()?
                            {
                                // Read first: even hard-linked input/output paths must not truncate evidence.
                                let bytes = fs::read(source)?;
                                fs::write(destination, bytes)?;
                            }
                            html.push_str(&format!("<svg viewBox=\"0 0 {} {}\" role=\"img\" aria-label=\"{} {} {} capture\"><image href=\"captures/{}\" width=\"{}\" height=\"{}\"/>",image.width,image.height,escape(&screen.id),escape(state_id),escape(&evidence.result.platform),filename,image.width,image.height));
                            if let Some(snapshot) = &evidence.snapshot {
                                html.push_str("<g class=\"overlay\">");
                                for region in screen.regions() {
                                    if let Some(node) =
                                        snapshot.nodes.iter().find(|n| n.identifier == region.id)
                                    {
                                        let f = node.frame;
                                        let s = snapshot.scale;
                                        html.push_str(&format!("<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/><text x=\"{}\" y=\"{}\">{}</text>",f.x*s,f.y*s,f.width*s,f.height*s,f.x*s+2.0,f.y*s+10.0,escape(&region.id)));
                                    }
                                }
                                html.push_str("</g>");
                            }
                            html.push_str("</svg>");
                        }
                        html.push_str("<ul>");
                        for failure in &evidence.result.failures {
                            html.push_str(&format!("<li>{}</li>", escape(failure)));
                        }
                        if evidence.result.failures.is_empty() {
                            html.push_str("<li>Declared checks passed.</li>");
                        }
                        html.push_str("</ul></div>");
                    }
                    html.push_str(&format!(
                        "</div><h3>For reviewers</h3><pre>{}</pre></section>",
                        escape(&prompt)
                    ));
                }
                comparison.pairs.push(PairResult {
                    screen: screen.id.clone(),
                    state: state_id.clone(),
                    baseline: base.result,
                    target: dest.result,
                    similarity: score,
                    reviewer_prompt: prompt,
                });
            }
        }
        if let Some(out) = out {
            html.push_str("</html>");
            fs::write(out.join("index.html"), html)?;
            fs::write(
                out.join("comparison.json"),
                serde_json::to_string_pretty(&comparison)?,
            )?;
        }
        Ok(comparison)
    }
    /// Fresh comparison summary for all configured platforms against the chosen baseline.
    pub fn ui_report(&self, captures: &Path, baseline: &str) -> Result<String> {
        let comparisons = self
            .catalog
            .config
            .platforms
            .keys()
            .map(|platform| self.compare(captures, baseline, platform, None))
            .collect::<Result<Vec<_>>>()?;
        let summary = screen_summary(
            comparisons
                .iter()
                .flat_map(|c| c.pairs.iter().map(|pair| &pair.target)),
        );
        let mut report = self.catalog.report()?;
        report.push('\n');
        report.push_str(&summary);
        Ok(report)
    }
}
fn check_containment(
    regions: &[Region],
    absent: &[String],
    nodes: &BTreeMap<&str, (usize, &Node)>,
    tolerance: f64,
    snapshot: &Snapshot,
    failures: &mut Vec<String>,
) {
    for region in regions {
        if let Some((_, parent)) = nodes.get(region.id.as_str()) {
            let p = parent.frame.normalized(snapshot);
            for child in &region.children {
                if absent.contains(&child.id) {
                    continue;
                }
                if let Some((_, child_node)) = nodes.get(child.id.as_str()) {
                    let c = child_node.frame.normalized(snapshot);
                    if c.x + tolerance < p.x
                        || c.y + tolerance < p.y
                        || c.x + c.width > p.x + p.width + tolerance
                        || c.y + c.height > p.y + p.height + tolerance
                    {
                        failures.push(format!(
                            "structural: {} escapes parent {}",
                            child.id, region.id
                        ));
                    }
                }
            }
        }
        check_containment(
            &region.children,
            absent,
            nodes,
            tolerance,
            snapshot,
            failures,
        );
    }
}
fn reviewer_prompt(
    screen: &Screen,
    state_id: &str,
    state: &ScreenState,
    baseline: &str,
    target: &str,
) -> String {
    let mut prompt=format!("Review {} / {state_id} ({:?}, font scale {}). Do {baseline} and {target} read as the same product using native components? Judge hierarchy, emphasis, artwork, type, spacing, materials and motion intent; explain meaningful drift and legitimate native adaptations. SSIM is not a verdict.\nAnatomy (preorder; priority 1 is highest):\n",screen.id,state.theme,state.font_scale);
    for r in screen.regions() {
        prompt.push_str(&format!("{}: {} (priority {}{}); tokens color={:?}, type={:?}, spacing={:?}, radius={:?}, material={:?}, motion={:?}\n",r.id,r.role,r.priority,if state.absent.contains(&r.id) { ", absent" } else { "" },r.tokens.color,r.tokens.typography,r.tokens.spacing,r.tokens.radius,r.tokens.material,r.tokens.motion));
        for (p, n) in &r.notes {
            prompt.push_str(&format!("  {p}: {n}\n"));
        }
    }
    for (p, n) in &screen.notes {
        prompt.push_str(&format!("{p}: {n}\n"));
    }
    prompt
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cie76_known_values() {
        assert!(delta_e([255.0; 3], [255.0; 3]) < 1e-9);
        assert!((delta_e([255.0; 3], [0.0; 3]) - 100.0).abs() < 0.001);
        assert!(delta_e([255.0, 0.0, 0.0], [0.0, 255.0, 0.0]) > 100.0);
    }
    #[test]
    fn ssim_identity_and_difference() {
        let a = Image {
            width: 2,
            height: 2,
            pixels: vec![[0, 0, 0, 255]; 4],
        };
        let b = Image {
            width: 2,
            height: 2,
            pixels: vec![[255, 255, 255, 255]; 4],
        };
        assert!((similarity(&a, &a) - 1.0).abs() < 1e-9);
        assert!(similarity(&a, &b) < 0.01);
    }
}
