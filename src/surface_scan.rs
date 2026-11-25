//! `surfaces scan`: what an app declares to the system (widgets, controls, shortcuts, watch
//! and car targets...), read lexically from Swift, Info.plist and Android XML.
use crate::drift::{git, glob};
use crate::surfaces::wildcard;
use crate::{Catalog, Result, Status, SurfaceScanConfig};
use regex::Regex;

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// A regex compiled once per call site. Patterns are literals covered by tests.
macro_rules! re {
    ($pattern:expr) => {{
        static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
            Regex::new($pattern).unwrap_or_else(|e| panic!("bundled pattern: {e}"))
        });
        &*RE
    }};
}

/// Files under a platform root: the working tree, or a Git revision.
pub enum Files {
    Tree(PathBuf),
    Git { root: PathBuf, revision: String },
}
impl Files {
    fn list(&self, globs: &[String]) -> Result<Vec<String>> {
        let mut out = BTreeSet::new();
        for pattern in globs {
            let prefix = literal_prefix(pattern);
            let candidates = match self {
                Self::Tree(root) => {
                    let mut found = Vec::new();
                    walk(root, &prefix, &mut found)?;
                    found
                }
                Self::Git { root, revision } => {
                    let mut args = vec!["ls-tree", "-r", "--name-only", revision.as_str(), "--"];
                    if !prefix.is_empty() {
                        args.push(prefix.as_str());
                    }
                    String::from_utf8_lossy(&git(root, &args)?)
                        .lines()
                        .map(str::to_owned)
                        .collect()
                }
            };
            out.extend(
                candidates
                    .into_iter()
                    .filter(|p| glob(pattern.as_bytes(), p.as_bytes())),
            );
        }
        Ok(out.into_iter().collect())
    }
    fn read(&self, path: &str) -> Option<String> {
        if path.split('/').any(|p| p == "..") {
            return None;
        }
        match self {
            Self::Tree(root) => fs::read_to_string(root.join(path)).ok(),
            Self::Git { root, revision } => git(root, &["show", &format!("{revision}:./{path}")])
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
        }
    }
}

/// Directory components of a glob before the first wildcard.
fn literal_prefix(pattern: &str) -> String {
    let parts: Vec<&str> = pattern.split('/').collect();
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if part.contains(['*', '?', '[', '{']) || i + 1 == parts.len() {
            break;
        }
        out.push(*part);
    }
    out.join("/")
}
fn walk(root: &Path, relative: &str, out: &mut Vec<String>) -> Result<()> {
    let dir = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let Ok(read) = fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut items: Vec<_> = read.filter_map(|e| e.ok()).collect();
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let name = item.file_name().to_string_lossy().into_owned();
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        let kind = item.file_type()?;
        if kind.is_dir() {
            if name == ".git" || name == "build" || name == ".gradle" || name == "DerivedData" {
                continue;
            }
            walk(root, &path, out)?;
        } else if kind.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// One thing the app declares to the system.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Declaration {
    pub signal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub id: String,
    pub file: String,
    pub line: usize,
}
impl Declaration {
    /// `signal[:detail][@context]#id`, stable across line moves.
    pub fn key(&self) -> String {
        let mut out = self.signal.clone();
        if let Some(d) = &self.detail {
            out.push(':');
            out.push_str(d);
        }
        if let Some(c) = &self.context {
            out.push('@');
            out.push_str(c);
        }
        out.push('#');
        out.push_str(&self.id);
        out
    }
}
fn decl(signal: &str, detail: Option<&str>, id: &str, file: &str, line: usize) -> Declaration {
    Declaration {
        signal: signal.into(),
        detail: detail.map(str::to_owned),
        context: None,
        id: id.into(),
        file: file.into(),
        line,
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

// ---------- Swift ----------

/// Replace comments with spaces (keeping newlines) so offsets and lines survive.
fn strip_swift_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut string: Option<&[u8]> = None;
    while i < b.len() {
        if let Some(end) = string {
            if b[i] == b'\\' && end == b"\"" && i + 1 < b.len() {
                out.extend_from_slice(&b[i..i + 2]);
                i += 2;
                continue;
            }
            if b[i..].starts_with(end) {
                out.extend_from_slice(end);
                i += end.len();
                string = None;
                continue;
            }
            out.push(b[i]);
            i += 1;
            continue;
        }
        if b[i..].starts_with(b"\"\"\"") {
            string = Some(b"\"\"\"");
            out.extend_from_slice(b"\"\"\"");
            i += 3;
        } else if b[i] == b'"' {
            string = Some(b"\"");
            out.push(b'"');
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            let mut depth = 0;
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Offset just past the brace matching the `{` at `open`, skipping string literals.
fn matching_brace(code: &str, open: usize) -> usize {
    let b = code.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let triple = b[i..].starts_with(b"\"\"\"");
                let end: &[u8] = if triple { b"\"\"\"" } else { b"\"" };
                i += end.len();
                while i < b.len() && !b[i..].starts_with(end) {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += end.len();
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

fn swift_constants(code: &str, out: &mut BTreeMap<String, String>) {
    re!(r#"\b(?:let|var)\s+([A-Za-z_]\w*)\s*(?::\s*String\s*)?=\s*"([^"\\]*)""#)
        .captures_iter(code)
        .for_each(|c| {
            out.entry(c[1].to_owned())
                .or_insert_with(|| c[2].to_owned());
        });
}

const FAMILIES: &[&str] = &[
    "systemSmall",
    "systemMedium",
    "systemLarge",
    "systemExtraLarge",
    "systemExtraLargePortrait",
    "accessoryCircular",
    "accessoryRectangular",
    "accessoryInline",
    "accessoryCorner",
];

/// The first `kind:` argument that resolves: a literal, or a string constant (from the
/// type's own body first, then from every scanned file).
fn resolve_kind(body: &str, constants: &BTreeMap<String, String>) -> Option<String> {
    let mut local = BTreeMap::new();
    swift_constants(body, &mut local);
    re!(r#"\bkind\s*:\s*(?:"([^"\\]*)"|([A-Za-z_][\w.]*))"#)
        .captures_iter(body)
        .find_map(|c| {
            if let Some(literal) = c.get(1) {
                return Some(literal.as_str().to_owned());
            }
            let ident = c.get(2)?.as_str();
            let last = ident.rsplit('.').next().unwrap_or(ident);
            local.get(last).or_else(|| constants.get(last)).cloned()
        })
}

fn families(body: &str) -> Vec<String> {
    let token = re!(r"\.(system\w+|accessory\w+)\b");
    let known = |s: &str| FAMILIES.contains(&s);
    if let Some(m) = re!(r"\.supportedFamilies\s*\(").find(body) {
        // The argument list up to the matching parenthesis.
        let rest = &body[m.end()..];
        let mut depth = 1;
        let mut end = rest.len();
        for (i, c) in rest.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let mut list: Vec<String> = token
            .captures_iter(&rest[..end])
            .map(|c| c[1].to_owned())
            .filter(|f| known(f))
            .collect();
        if list.is_empty() {
            // A families constant: take every family token in the widget's body.
            list = token
                .captures_iter(body)
                .map(|c| c[1].to_owned())
                .filter(|f| known(f))
                .collect();
        }
        let mut seen = BTreeSet::new();
        list.retain(|f| seen.insert(f.clone()));
        return list;
    }
    vec![
        "systemSmall".into(),
        "systemMedium".into(),
        "systemLarge".into(),
    ]
}

/// Declarations in one Swift file. `constants` holds string constants from every file.
pub fn parse_swift(
    file: &str,
    text: &str,
    constants: &BTreeMap<String, String>,
) -> Vec<Declaration> {
    let code = strip_swift_comments(text);
    let mut out = Vec::new();
    let types = re!(
        r"\b(?:struct|class|enum|actor|extension)\s+([A-Za-z_][\w.]*)\s*(?:<[^>{]*>)?\s*:\s*([^{]+)\{"
    );
    for c in types.captures_iter(&code) {
        let (Some(whole), Some(name)) = (c.get(0), c.get(1)) else {
            continue;
        };
        let name = name.as_str();
        let conforms: Vec<&str> = c[2]
            .split([',', '&'])
            .map(|s| s.split_whitespace().next().unwrap_or("").trim())
            .collect();
        let has = |p: &str| {
            conforms
                .iter()
                .any(|c| *c == p || c.ends_with(&format!(".{p}")))
        };
        let open = whole.end() - 1;
        let body = &code[open..matching_brace(&code, open)];
        let line = line_of(&code, whole.start());
        if has("Widget") {
            if let Some(a) =
                re!(r"ActivityConfiguration\s*\(\s*for\s*:\s*([A-Za-z_][\w.]*?)(?:\.self)?\s*[,)]")
                    .captures(body)
            {
                out.push(decl("apple.live-activity", None, &a[1], file, line));
            } else {
                let kind = resolve_kind(body, constants).unwrap_or_else(|| name.to_owned());
                for family in families(body) {
                    out.push(decl("apple.widget", Some(&family), &kind, file, line));
                }
            }
        }
        if has("ControlWidget") {
            let kind = resolve_kind(body, constants).unwrap_or_else(|| name.to_owned());
            out.push(decl("apple.control", None, &kind, file, line));
        }
        if has("AppShortcutsProvider") {
            let shortcut = re!(r"AppShortcut\s*\(\s*intent\s*:\s*([A-Za-z_]\w*)");
            let mut seen = BTreeSet::new();
            for s in shortcut.captures_iter(body) {
                if seen.insert(s[1].to_owned()) {
                    let at = s.get(0).map_or(0, |m| m.start()) + open;
                    out.push(decl(
                        "apple.app-shortcut",
                        None,
                        &s[1],
                        file,
                        line_of(&code, at),
                    ));
                }
            }
        }
        if has("CPTemplateApplicationSceneDelegate") {
            out.push(decl("apple.carplay", None, name, file, line));
        }
        if has("CLKComplicationDataSource") {
            out.push(decl("apple.complication", None, name, file, line));
        }
    }
    let menu = re!(r#"\bMenuBarExtra\s*\(\s*(?:"([^"\\]*)")?"#);
    let mut seen = BTreeSet::new();
    for m in menu.captures_iter(&code) {
        let id = m.get(1).map_or("MenuBarExtra", |t| t.as_str()).to_owned();
        if seen.insert(id.clone()) {
            let at = m.get(0).map_or(0, |x| x.start());
            out.push(decl("apple.menu-bar", None, &id, file, line_of(&code, at)));
        }
    }
    out
}

// ---------- XML ----------

#[derive(Debug, Default, Clone)]
pub(crate) struct Element {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
    pub line: usize,
}
impl Element {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    fn descendants<'a>(&'a self, name: &'a str) -> Vec<&'a Element> {
        let mut out = Vec::new();
        let mut stack: Vec<&Element> = self.children.iter().rev().collect();
        while let Some(e) = stack.pop() {
            if e.name == name {
                out.push(e);
            }
            stack.extend(e.children.iter().rev());
        }
        out
    }
    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A small, forgiving XML reader: elements, attributes and text; skips comments,
/// declarations, DOCTYPE and processing instructions. Returns a synthetic root.
pub(crate) fn parse_xml(text: &str) -> Element {
    let mut stack = vec![Element::default()];
    let b = text.as_bytes();
    let mut i = 0;
    let attr = re!(r#"([\w:.-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#);
    while i < b.len() {
        if b[i] != b'<' {
            let end = text[i..].find('<').map_or(b.len(), |n| i + n);
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&unescape(&text[i..end]));
            }
            i = end;
            continue;
        }
        let rest = &text[i..];
        let skip_to = |marker: &str| rest.find(marker).map_or(b.len(), |n| i + n + marker.len());
        if rest.starts_with("<!--") {
            i = skip_to("-->");
        } else if rest.starts_with("<![CDATA[") {
            let end = rest.find("]]>").unwrap_or(rest.len());
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&rest[9.min(end)..end]);
            }
            i = skip_to("]]>");
        } else if rest.starts_with("<?") {
            i = skip_to("?>");
        } else if rest.starts_with("<!") {
            i = skip_to(">");
        } else if rest.starts_with("</") {
            i = skip_to(">");
            if stack.len() > 1 {
                if let Some(done) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(done);
                    }
                }
            }
        } else {
            // A start tag; find its end outside quotes.
            let mut j = i + 1;
            let mut quote = None;
            while j < b.len() {
                match (quote, b[j]) {
                    (None, b'"' | b'\'') => quote = Some(b[j]),
                    (Some(q), c) if c == q => quote = None,
                    (None, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            let tag = &text[i + 1..j.min(b.len())];
            let self_closing = tag.ends_with('/');
            let tag = tag.trim_end_matches('/');
            let name = tag.split_whitespace().next().unwrap_or("").to_owned();
            let attributes = attr
                .captures_iter(tag)
                .map(|c| {
                    let value = c.get(2).or_else(|| c.get(3)).map_or("", |m| m.as_str());
                    (c[1].to_owned(), unescape(value))
                })
                .collect();
            let element = Element {
                name,
                attributes,
                line: line_of(text, i),
                ..Element::default()
            };
            if self_closing {
                if let Some(top) = stack.last_mut() {
                    top.children.push(element);
                }
            } else {
                stack.push(element);
            }
            i = j + 1;
        }
    }
    while stack.len() > 1 {
        if let Some(done) = stack.pop() {
            if let Some(parent) = stack.last_mut() {
                parent.children.push(done);
            }
        }
    }
    stack.pop().unwrap_or_default()
}

// ---------- Info.plist ----------

#[derive(Debug, Clone)]
enum PlistValue {
    Dict(Vec<(String, PlistValue)>),
    Array(Vec<PlistValue>),
    Text(String, usize),
    Bool(bool, usize),
}
impl PlistValue {
    fn get(&self, key: &str) -> Option<&PlistValue> {
        match self {
            Self::Dict(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(t, _) => Some(t),
            _ => None,
        }
    }
    fn line(&self) -> usize {
        match self {
            Self::Text(_, line) | Self::Bool(_, line) => *line,
            _ => 1,
        }
    }
    fn truthy(&self) -> bool {
        matches!(self, Self::Bool(true, _))
            || matches!(self, Self::Text(t, _) if t == "YES" || t == "true" || t == "1")
    }
    fn walk<'a>(&'a self, out: &mut Vec<&'a PlistValue>) {
        out.push(self);
        match self {
            Self::Dict(items) => items.iter().for_each(|(_, v)| v.walk(out)),
            Self::Array(items) => items.iter().for_each(|v| v.walk(out)),
            _ => {}
        }
    }
}
fn plist_value(e: &Element) -> PlistValue {
    match e.name.as_str() {
        "dict" => {
            let mut items = Vec::new();
            let mut key = None;
            for child in &e.children {
                if child.name == "key" {
                    key = Some(child.text.trim().to_owned());
                } else if let Some(k) = key.take() {
                    items.push((k, plist_value(child)));
                }
            }
            PlistValue::Dict(items)
        }
        "array" => PlistValue::Array(e.children.iter().map(plist_value).collect()),
        "true" => PlistValue::Bool(true, e.line),
        "false" => PlistValue::Bool(false, e.line),
        _ => PlistValue::Text(e.text.trim().to_owned(), e.line),
    }
}

pub fn parse_plist(file: &str, text: &str) -> Vec<Declaration> {
    let root = parse_xml(text);
    let Some(top) = root
        .descendants("plist")
        .first()
        .and_then(|p| p.children.first())
        .map(plist_value)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(PlistValue::Array(items)) = top.get("UIApplicationShortcutItems") {
        for item in items {
            if let Some(value) = item.get("UIApplicationShortcutItemType") {
                if let Some(kind) = value.text() {
                    out.push(decl("apple.quick-action", None, kind, file, value.line()));
                }
            }
        }
    }
    let watch = ["WKApplication", "WKWatchKitApp"]
        .iter()
        .any(|k| top.get(k).is_some_and(PlistValue::truthy))
        || top.get("WKCompanionAppBundleIdentifier").is_some();
    if watch {
        let id = top
            .get("CFBundleIdentifier")
            .and_then(PlistValue::text)
            .filter(|t| !t.contains("$("))
            .unwrap_or(file);
        let line = [
            "WKApplication",
            "WKWatchKitApp",
            "WKCompanionAppBundleIdentifier",
        ]
        .iter()
        .find_map(|k| top.get(k))
        .map_or(1, PlistValue::line);
        out.push(decl("apple.watch-app", None, id, file, line));
    }
    if let Some(scenes) = top.get("UIApplicationSceneManifest") {
        let mut all = Vec::new();
        scenes.walk(&mut all);
        let carplay = all.iter().any(|v| match v {
            PlistValue::Dict(items) => items
                .iter()
                .any(|(k, _)| k == "CPTemplateApplicationSceneSessionRoleApplication"),
            _ => false,
        });
        if carplay {
            let delegate = all
                .iter()
                .filter_map(|v| match v {
                    PlistValue::Dict(items) => items
                        .iter()
                        .find(|(k, _)| k == "CPTemplateApplicationSceneSessionRoleApplication")
                        .map(|(_, v)| v),
                    _ => None,
                })
                .flat_map(|v| {
                    let mut inner = Vec::new();
                    v.walk(&mut inner);
                    inner
                })
                .find_map(|v| v.get("UISceneDelegateClassName").and_then(PlistValue::text))
                .map(|d| d.rsplit('.').next().unwrap_or(d).to_owned());
            out.push(decl(
                "apple.carplay",
                None,
                delegate.as_deref().unwrap_or(file),
                file,
                1,
            ));
        }
    }
    if let Some(extension) = top.get("NSExtension") {
        let point = extension
            .get("NSExtensionPointIdentifier")
            .and_then(PlistValue::text);
        if point == Some("com.apple.usernotifications.content-extension") {
            out.push(decl("apple.notification", None, file, file, 1));
        }
    }
    out
}

// ---------- Android ----------

const ANDROID_NAME: &str = "android:name";

/// `res` directories for a manifest: the one beside it, then the configured extras.
fn res_dirs(manifest: &str, extra: &[String]) -> Vec<String> {
    let dir = manifest.rsplit_once('/').map_or("", |(d, _)| d);
    let mut out = vec![if dir.is_empty() {
        "res".to_owned()
    } else {
        format!("{dir}/res")
    }];
    out.extend(extra.iter().map(|d| d.trim_end_matches('/').to_owned()));
    out
}

/// Read `@xml/name` from the first `res` directory that has it: (path, text).
fn read_res_xml(
    manifest: &str,
    resource: &str,
    extra: &[String],
    files: &Files,
) -> Option<(String, String)> {
    let name = resource.strip_prefix("@xml/")?;
    res_dirs(manifest, extra).into_iter().find_map(|dir| {
        let path = format!("{dir}/xml/{name}.xml");
        files.read(&path).map(|text| (path, text))
    })
}

fn dimension(value: &str, manifest: &str, extra: &[String], files: &Files) -> Option<f64> {
    if let Some(name) = value.strip_prefix("@dimen/") {
        let found = res_dirs(manifest, extra).into_iter().find_map(|dir| {
            let xml = parse_xml(&files.read(&format!("{dir}/values/dimens.xml"))?);
            xml.descendants("dimen")
                .into_iter()
                .find(|d| d.attr("name") == Some(name))
                .map(|d| d.text.trim().to_owned())
        })?;
        if found.starts_with("@dimen/") {
            return None; // no chains
        }
        return dimension(&found, manifest, extra, files);
    }
    value
        .trim()
        .trim_end_matches("dip")
        .trim_end_matches("dp")
        .parse()
        .ok()
}

/// Launcher cells for a dimension in dp: `(dp + 30) / 70`, at least one.
fn cells(dp: f64) -> u32 {
    (((dp + 30.0) / 70.0).floor() as u32).max(1)
}

fn widget_size(
    provider: &Element,
    manifest: &str,
    extra: &[String],
    files: &Files,
) -> Option<String> {
    let target = |k: &str| provider.attr(k).and_then(|v| v.trim().parse::<u32>().ok());
    if let (Some(w), Some(h)) = (
        target("android:targetCellWidth"),
        target("android:targetCellHeight"),
    ) {
        return Some(format!("{w}x{h}"));
    }
    let dp = |k: &str| {
        provider
            .attr(k)
            .and_then(|v| dimension(v, manifest, extra, files))
    };
    let (w, h) = (dp("android:minWidth")?, dp("android:minHeight")?);
    Some(format!("{}x{}", cells(w), cells(h)))
}

fn handles(component: &Element, action: &str) -> bool {
    component
        .descendants("action")
        .iter()
        .any(|a| a.attr(ANDROID_NAME) == Some(action))
}
fn meta_resource<'a>(component: &'a Element, name: &str) -> Option<&'a str> {
    component
        .children_named("meta-data")
        .find(|m| m.attr(ANDROID_NAME) == Some(name))
        .and_then(|m| m.attr("android:resource"))
}

/// Declarations in one `AndroidManifest.xml`; `res` lists extra resource directories.
pub fn parse_manifest(file: &str, text: &str, res: &[String], files: &Files) -> Vec<Declaration> {
    let root = parse_xml(text);
    let Some(manifest) = root.descendants("manifest").first().copied() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let package = manifest.attr("package");
    if manifest
        .children_named("uses-feature")
        .any(|f| f.attr(ANDROID_NAME) == Some("android.hardware.type.watch"))
    {
        out.push(decl(
            "android.wear-app",
            None,
            package.unwrap_or(file),
            file,
            manifest.line,
        ));
    }
    let Some(application) = manifest.children_named("application").next() else {
        return out;
    };
    for component in &application.children {
        let name = component.attr(ANDROID_NAME).unwrap_or("");
        let line = component.line;
        match component.name.as_str() {
            "receiver" => {
                if let Some(resource) = meta_resource(component, "android.appwidget.provider") {
                    let size = read_res_xml(file, resource, res, files)
                        .map(|(_, xml)| parse_xml(&xml))
                        .and_then(|xml| {
                            xml.descendants("appwidget-provider")
                                .first()
                                .and_then(|p| widget_size(p, file, res, files))
                        })
                        .unwrap_or_else(|| "unknown".into());
                    out.push(decl("android.widget", Some(&size), name, file, line));
                }
            }
            "service" => {
                for (action, signal) in [
                    (
                        "androidx.media3.session.MediaLibraryService",
                        "android.media-library",
                    ),
                    (
                        "android.media.browse.MediaBrowserService",
                        "android.media-library",
                    ),
                    (
                        "android.service.quicksettings.action.QS_TILE",
                        "android.qs-tile",
                    ),
                    (
                        "androidx.wear.tiles.action.BIND_TILE_PROVIDER",
                        "android.wear-tile",
                    ),
                    (
                        "android.support.wearable.complications.ACTION_COMPLICATION_UPDATE_REQUEST",
                        "android.complication",
                    ),
                ] {
                    if handles(component, action)
                        && !out.iter().any(|d| d.signal == signal && d.id == name)
                    {
                        out.push(decl(signal, None, name, file, line));
                    }
                }
            }
            "activity" | "activity-alias" => {
                if let Some(resource) = meta_resource(component, "android.app.shortcuts") {
                    let Some((path, xml)) = read_res_xml(file, resource, res, files) else {
                        continue;
                    };
                    let xml = parse_xml(&xml);
                    for s in xml.descendants("shortcut") {
                        if let Some(id) = s.attr("android:shortcutId") {
                            out.push(decl("android.shortcut", None, id, &path, s.line));
                        }
                    }
                    for c in xml.descendants("capability") {
                        if let Some(id) = c.attr(ANDROID_NAME) {
                            out.push(decl("android.app-action", None, id, &path, c.line));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(resource) = meta_resource(application, "com.google.android.gms.car.application") {
        let uses: Vec<(String, usize, String)> = read_res_xml(file, resource, res, files)
            .map(|(p, xml)| {
                parse_xml(&xml)
                    .descendants("uses")
                    .into_iter()
                    .filter_map(|u| u.attr("name").map(|n| (n.to_owned(), u.line, p.clone())))
                    .collect()
            })
            .unwrap_or_default();
        if uses.is_empty() {
            out.push(decl("android.car", None, resource, file, application.line));
        }
        for (use_name, line, path) in uses {
            out.push(decl("android.car", Some(&use_name), &use_name, &path, line));
        }
    }
    out
}

// ---------- Scan ----------

/// Every declaration on one platform, with contexts applied and ignores removed.
pub fn scan_platform(config: &SurfaceScanConfig, files: &Files) -> Result<Vec<Declaration>> {
    let mut out = Vec::new();
    let excluded = |path: &String| {
        config
            .exclude
            .iter()
            .any(|g| glob(g.as_bytes(), path.as_bytes()))
    };
    let list = |globs: &[String]| -> Result<Vec<String>> {
        Ok(files
            .list(globs)?
            .into_iter()
            .filter(|p| !excluded(p))
            .collect())
    };
    let swift = list(&config.swift)?;
    let mut sources = Vec::new();
    let mut constants = BTreeMap::new();
    for path in &swift {
        if let Some(text) = files.read(path) {
            swift_constants(&strip_swift_comments(&text), &mut constants);
            sources.push((path, text));
        }
    }
    for (path, text) in &sources {
        out.extend(parse_swift(path, text, &constants));
    }
    for path in list(&config.plist)? {
        if let Some(text) = files.read(&path) {
            out.extend(parse_plist(&path, &text));
        }
    }
    for path in list(&config.manifests)? {
        if let Some(text) = files.read(&path) {
            out.extend(parse_manifest(&path, &text, &config.res, files));
        }
    }
    for d in &mut out {
        d.context = config
            .contexts
            .iter()
            .find(|(_, globs)| globs.iter().any(|g| glob(g.as_bytes(), d.file.as_bytes())))
            .map(|(name, _)| name.clone());
    }
    out.retain(|d| !config.ignore.iter().any(|g| wildcard(g, &d.id)));
    out.sort();
    out.dedup_by(|a, b| a.key() == b.key());
    Ok(out)
}

/// A declaration and the surfaces it maps to.
#[derive(Debug, Clone, Serialize)]
pub struct Mapped {
    #[serde(flatten)]
    pub declaration: Declaration,
    pub surfaces: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingKind {
    Uncatalogued,
    Unmapped,
    NoLongerDeclared,
    StatusDisagrees,
    Coarse,
}
impl FindingKind {
    pub fn fails(self, strict: bool) -> bool {
        match self {
            Self::Uncatalogued | Self::Unmapped | Self::NoLongerDeclared => true,
            Self::StatusDisagrees | Self::Coarse => strict,
        }
    }
    fn heading(self) -> &'static str {
        match self {
            Self::Uncatalogued => "Uncatalogued",
            Self::Unmapped => "Unmapped",
            Self::NoLongerDeclared => "No longer declared",
            Self::StatusDisagrees => "Status disagrees",
            Self::Coarse => "Coarse",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// A declaration key, or `entry@surface` for records.
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The declaration key behind the finding, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declaration: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub platform: String,
    pub declarations: Vec<Mapped>,
    pub findings: Vec<Finding>,
}
impl ScanReport {
    pub fn failed(&self, strict: bool) -> bool {
        self.findings.iter().any(|f| f.kind.fails(strict))
    }
    pub fn markdown(&self, label: &str) -> String {
        let count = |k| self.findings.iter().filter(|f| f.kind == k).count();
        let mut out = format!(
            "## Declared surfaces: {label}\n\n{} declarations; {} uncatalogued, {} unmapped, {} no longer declared, {} status disagreements, {} coarse.\n",
            self.declarations.len(),
            count(FindingKind::Uncatalogued),
            count(FindingKind::Unmapped),
            count(FindingKind::NoLongerDeclared),
            count(FindingKind::StatusDisagrees),
            count(FindingKind::Coarse),
        );
        if !self.declarations.is_empty() {
            out.push_str("\n| Surface | Declaration | Location |\n| --- | --- | --- |\n");
            for m in &self.declarations {
                out.push_str(&format!(
                    "| {} | `{}` | `{}:{}` |\n",
                    if m.surfaces.is_empty() {
                        "(none)".to_owned()
                    } else {
                        m.surfaces
                            .iter()
                            .map(|s| format!("`{s}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                    m.declaration.key(),
                    m.declaration.file,
                    m.declaration.line
                ));
            }
        }
        for kind in [
            FindingKind::Uncatalogued,
            FindingKind::Unmapped,
            FindingKind::NoLongerDeclared,
            FindingKind::StatusDisagrees,
            FindingKind::Coarse,
        ] {
            let list: Vec<&Finding> = self.findings.iter().filter(|f| f.kind == kind).collect();
            if list.is_empty() {
                continue;
            }
            out.push_str(&format!("\n### {}\n\n", kind.heading()));
            for f in list {
                out.push_str(&format!(
                    "- `{}`{}{}: {}\n",
                    f.subject,
                    f.surface
                        .as_ref()
                        .map(|s| format!(" (`{s}`)"))
                        .unwrap_or_default(),
                    f.location
                        .as_ref()
                        .map(|l| format!(" at `{l}`"))
                        .unwrap_or_default(),
                    f.message
                ));
            }
        }
        out
    }
}

/// A surface record that can cover declarations: (entry@surface, record).
struct Claim<'a> {
    label: String,
    surface: &'a str,
    record: &'a crate::Platform,
}

impl Catalog {
    /// Surfaces whose platforms include `platform` and whose signals match `d`.
    pub fn surfaces_for(&self, platform: &str, d: &Declaration) -> Vec<String> {
        self.config
            .surfaces
            .iter()
            .filter(|(_, s)| s.platforms.iter().any(|p| p == platform))
            .filter(|(id, _)| {
                self.surface_signals(id)
                    .iter()
                    .any(|p| p.matches(&d.signal, d.detail.as_deref(), d.context.as_deref()))
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn claims<'a>(&'a self, platform: &str) -> Vec<Claim<'a>> {
        let mut out = Vec::new();
        for entry in self.entries.values() {
            let Some(record) = entry.platforms.get(platform) else {
                continue;
            };
            for (surface, inner) in &record.surfaces {
                out.push(Claim {
                    label: format!("{}@{surface}", entry.id),
                    surface,
                    record: inner,
                });
            }
        }
        out
    }

    /// Parser families configured for a platform (`apple` for Swift/plist, `android` for
    /// manifests); signals from other families are never expected there.
    fn scan_families(&self, platform: &str) -> BTreeSet<&'static str> {
        let mut out = BTreeSet::new();
        if let Some(c) = self.config.surface_scan.get(platform) {
            if !c.swift.is_empty() || !c.plist.is_empty() {
                out.insert("apple");
            }
            if !c.manifests.is_empty() {
                out.insert("android");
            }
        }
        out
    }

    /// Map declarations to surfaces and compare them with the catalog's records.
    pub fn compare_surfaces(&self, platform: &str, declarations: Vec<Declaration>) -> ScanReport {
        let mapped: Vec<Mapped> = declarations
            .into_iter()
            .map(|d| Mapped {
                surfaces: self.surfaces_for(platform, &d),
                declaration: d,
            })
            .collect();
        let claims = self.claims(platform);
        let mut findings = Vec::new();
        for m in &mapped {
            let d = &m.declaration;
            let location = Some(format!("{}:{}", d.file, d.line));
            if m.surfaces.is_empty() {
                findings.push(Finding {
                    kind: FindingKind::Unmapped,
                    subject: d.key(),
                    surface: None,
                    location,
                    declaration: Some(d.key()),
                    message: "matches no declared surface; add one to [surfaces] (or a signals pattern), or ignore it".into(),
                });
                continue;
            }
            for surface in &m.surfaces {
                let exact: Vec<&Claim> = claims
                    .iter()
                    .filter(|c| c.surface == surface && c.record.declared.contains(&d.id))
                    .collect();
                let coarse: Vec<&Claim> = claims
                    .iter()
                    .filter(|c| c.surface == surface && c.record.declared.is_empty())
                    .collect();
                let covering = if exact.is_empty() { &coarse } else { &exact };
                if covering.is_empty() {
                    let others: Vec<String> = claims
                        .iter()
                        .filter(|c| c.surface == surface)
                        .map(|c| format!("`{}`", c.label))
                        .collect();
                    let message = if others.is_empty() {
                        format!(
                            "no entry records {platform} `{surface}`; name `{surface}` in an entry's surfaces and add a surface record with declared: [{}]",
                            d.id
                        )
                    } else {
                        format!(
                            "{} list other declared IDs for {platform} `{surface}`; add `{}` to the right one's declared",
                            others.join(", "),
                            d.id
                        )
                    };
                    findings.push(Finding {
                        kind: FindingKind::Uncatalogued,
                        subject: d.key(),
                        surface: Some(surface.clone()),
                        location: location.clone(),
                        declaration: Some(d.key()),
                        message,
                    });
                    continue;
                }
                if exact.is_empty() {
                    findings.push(Finding {
                        kind: FindingKind::Coarse,
                        subject: d.key(),
                        surface: Some(surface.clone()),
                        location: location.clone(),
                        declaration: Some(d.key()),
                        message: format!(
                            "covered only by records without declared: {}",
                            coarse
                                .iter()
                                .map(|c| format!("`{}`", c.label))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    });
                }
                for c in covering {
                    if matches!(c.record.status, Status::Missing | Status::NotApplicable) {
                        findings.push(Finding {
                            kind: FindingKind::StatusDisagrees,
                            subject: c.label.clone(),
                            surface: Some(surface.clone()),
                            location: location.clone(),
                            declaration: Some(d.key()),
                            message: format!(
                                "recorded {} on {platform}, but `{}` is declared",
                                c.record.status.as_str(),
                                d.key()
                            ),
                        });
                    }
                }
            }
        }
        let families = self.scan_families(platform);
        for c in &claims {
            if !matches!(
                c.record.status,
                Status::Implemented | Status::Partial | Status::Stale
            ) {
                continue;
            }
            let scannable = self
                .surface_signals(c.surface)
                .iter()
                .any(|p| families.contains(p.family()));
            if !scannable {
                continue;
            }
            let on_surface: Vec<&Declaration> = mapped
                .iter()
                .filter(|m| m.surfaces.iter().any(|s| s == c.surface))
                .map(|m| &m.declaration)
                .collect();
            if c.record.declared.is_empty() {
                if on_surface.is_empty() {
                    findings.push(Finding {
                        kind: FindingKind::NoLongerDeclared,
                        subject: c.label.clone(),
                        surface: Some(c.surface.to_owned()),
                        location: None,
                        declaration: None,
                        message: format!(
                            "recorded {} on {platform}, but nothing declares `{}`",
                            c.record.status.as_str(),
                            c.surface
                        ),
                    });
                }
            } else {
                for id in &c.record.declared {
                    if !on_surface.iter().any(|d| &d.id == id) {
                        findings.push(Finding {
                            kind: FindingKind::NoLongerDeclared,
                            subject: c.label.clone(),
                            surface: Some(c.surface.to_owned()),
                            location: None,
                            declaration: None,
                            message: format!(
                                "declared `{id}` is not declared for `{}` on {platform}",
                                c.surface
                            ),
                        });
                    }
                }
            }
        }
        findings.sort_by(|a, b| (a.kind, &a.subject).cmp(&(b.kind, &b.subject)));
        ScanReport {
            platform: platform.to_owned(),
            declarations: mapped,
            findings,
        }
    }

    /// Scan a platform's working tree (or `revision` in Git) and compare with the catalog.
    pub fn scan_surfaces(
        &self,
        options: &crate::RootOptions,
        platform: &str,
        revision: Option<&str>,
    ) -> Result<ScanReport> {
        let config =
            self.config.surface_scan.get(platform).ok_or_else(|| {
                format!("{platform}: no [surface_scan.{platform}] in catalog.toml")
            })?;
        let files = self.surface_files(options, platform, revision)?;
        Ok(self.compare_surfaces(platform, scan_platform(config, &files)?))
    }

    pub(crate) fn surface_files(
        &self,
        options: &crate::RootOptions,
        platform: &str,
        revision: Option<&str>,
    ) -> Result<Files> {
        let root = self.platform_root(platform, options)?.ok_or_else(|| {
            format!("{platform}: root not supplied; use --root {platform}=<path>")
        })?;
        Ok(match revision {
            None => Files::Tree(root),
            Some(revision) => Files::Git {
                root,
                revision: revision.to_owned(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_widgets() {
        let text = r#"
// struct Fake: Widget { kind: "commented" }
enum Kinds { static let nowPlaying = "com.example.now-playing" }
struct NowPlayingWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: Kinds.nowPlaying, provider: P()) { e in V(e) }
            .supportedFamilies([.systemSmall, .systemMedium, .accessoryCircular])
    }
}
struct Plain: Widget {
    let kind: String = "plain"
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: P()) { _ in Text("}") }
    }
}
struct PlayControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: "com.example.play") { ControlWidgetButton(action: A()) { Label("Play", systemImage: "play") } }
    }
}
struct Activity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: PlaybackAttributes.self) { _ in EmptyView() } dynamicIsland: { _ in DynamicIsland {} }
    }
}
struct Shortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(intent: ResumeIntent(), phrases: ["Resume in \(.applicationName)"], shortTitle: "Resume", systemImageName: "play")
        AppShortcut(intent: CaptureIntent(), phrases: ["Capture"], shortTitle: "Capture", systemImageName: "mic")
    }
}
final class CarScene: UIResponder, CPTemplateApplicationSceneDelegate {}
@main struct MacApp: App { var body: some Scene { MenuBarExtra("Player", systemImage: "play") { Text("x") } } }
"#;
        let mut constants = BTreeMap::new();
        swift_constants(&strip_swift_comments(text), &mut constants);
        let keys: Vec<String> = parse_swift("W.swift", text, &constants)
            .iter()
            .map(Declaration::key)
            .collect();
        let expected = [
            "apple.widget:systemSmall#com.example.now-playing",
            "apple.widget:systemMedium#com.example.now-playing",
            "apple.widget:accessoryCircular#com.example.now-playing",
            "apple.widget:systemSmall#plain",
            "apple.widget:systemMedium#plain",
            "apple.widget:systemLarge#plain",
            "apple.control#com.example.play",
            "apple.live-activity#PlaybackAttributes",
            "apple.app-shortcut#ResumeIntent",
            "apple.app-shortcut#CaptureIntent",
            "apple.carplay#CarScene",
            "apple.menu-bar#Player",
        ];
        assert_eq!(keys, expected);
    }

    #[test]
    fn xml_reader() {
        let xml = parse_xml(
            "<?xml version=\"1.0\"?><!-- c --><a x=\"1\" y='a&amp;b'><b/><c>t<![CDATA[<z>]]></c></a>",
        );
        let a = &xml.children[0];
        assert_eq!(a.attr("y"), Some("a&b"));
        assert_eq!(a.children.len(), 2);
        assert_eq!(a.children[1].text, "t<z>");
    }

    #[test]
    fn widget_cells() {
        assert_eq!(cells(40.0), 1);
        assert_eq!(cells(110.0), 2);
        assert_eq!(cells(180.0), 3);
        assert_eq!(cells(250.0), 4);
    }

    #[test]
    fn prefixes() {
        assert_eq!(literal_prefix("apple/Widgets/**/*.swift"), "apple/Widgets");
        assert_eq!(literal_prefix("Info.plist"), "");
        assert_eq!(literal_prefix("a/b/Info.plist"), "a/b");
    }
}
