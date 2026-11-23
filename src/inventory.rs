//! `concordance inventory`: a deterministic map of what each platform exposes (source files
//! grouped into candidate areas, user-visible strings, accessibility and test identifiers,
//! test files) and which catalog entries cite each item. See docs/adopt.md.
use crate::drift::glob;
use crate::{Catalog, Result, RootOptions};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

/// Directories never scanned: dependencies, build output and VCS metadata. Hidden
/// directories (a leading `.`) and Xcode bundles are skipped too.
pub const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "build",
    ".build",
    "DerivedData",
    "target",
    "Pods",
    ".git",
    "dist",
];

/// Directory names that say nothing about the feature, skipped when naming an area.
const GENERIC_DIRS: &[&str] = &[
    "src",
    "source",
    "sources",
    "main",
    "kotlin",
    "java",
    "swift",
    "lib",
    "ui",
    "view",
    "views",
    "screen",
    "screens",
    "feature",
    "features",
    "scene",
    "scenes",
    "module",
    "modules",
    "presentation",
];

/// Source extensions scanned when a platform has no `[source_globs]`.
const SOURCE_EXTENSIONS: &[&str] = &[
    "swift", "kt", "kts", "java", "ts", "tsx", "js", "jsx", "vue", "svelte", "rs", "go", "py",
    "dart", "m", "mm",
];

pub(crate) fn skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
        || name.starts_with('.')
        || name.ends_with(".xcodeproj")
        || name.ends_with(".xcworkspace")
        || name.ends_with(".xcassets")
}

/// What a walk found besides files.
#[derive(Debug, Default)]
pub(crate) struct WalkExtras {
    /// Xcode project bundles (`*.xcodeproj`), recorded instead of descended into.
    pub bundles: BTreeSet<String>,
    /// Symbolic links not followed: directory links, dangling links and links that resolve
    /// outside the root (relative to the root).
    pub skipped_symlinks: BTreeSet<String>,
}

/// Every file below `root/start`, as sorted `/`-separated paths relative to `root`.
/// A symbolic link is followed only when it is a file inside the root; a fixed `start`
/// with a symbolic link in it is skipped. Skipped links are recorded in `extras`.
pub(crate) fn walk(
    root: &Path,
    start: &str,
    out: &mut BTreeSet<String>,
    extras: &mut WalkExtras,
) -> Result<()> {
    let canonical_root = root.canonicalize()?;
    // Never walk through a symbolic link in a fixed prefix, wherever it points.
    let mut current = root.to_path_buf();
    let mut walked = String::new();
    for part in start.split('/').filter(|p| !p.is_empty()) {
        current.push(part);
        walked = if walked.is_empty() {
            part.to_owned()
        } else {
            format!("{walked}/{part}")
        };
        match fs::symlink_metadata(&current) {
            Err(_) => return Ok(()), // a glob prefix that does not exist yet matches nothing
            Ok(meta) if meta.file_type().is_symlink() => {
                extras.skipped_symlinks.insert(walked);
                return Ok(());
            }
            Ok(meta) if !meta.is_dir() => return Ok(()),
            Ok(_) => {}
        }
    }
    walk_dir(root, &canonical_root, start, out, extras)
}

fn walk_dir(
    root: &Path,
    canonical_root: &Path,
    start: &str,
    out: &mut BTreeSet<String>,
    extras: &mut WalkExtras,
) -> Result<()> {
    let dir = if start.is_empty() {
        root.to_path_buf()
    } else {
        root.join(start)
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(());
    };
    let mut entries = entries.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let relative = if start.is_empty() {
            name.clone()
        } else {
            format!("{start}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            match entry.path().canonicalize() {
                Ok(target) if target.starts_with(canonical_root) && target.is_file() => {
                    out.insert(relative);
                }
                _ => {
                    extras.skipped_symlinks.insert(relative);
                }
            }
        } else if kind.is_dir() {
            if name.ends_with(".xcodeproj") {
                extras.bundles.insert(relative);
            } else if !skip_dir(&name) {
                walk_dir(root, canonical_root, &relative, out, extras)?;
            }
        } else if kind.is_file() {
            out.insert(relative);
        }
    }
    Ok(())
}

/// The module a path belongs to: everything before its `src/` directory (`""` when none).
fn module_of(path: &str) -> &str {
    if path.starts_with("src/") {
        return "";
    }
    path.find("/src/").map(|i| &path[..i]).unwrap_or("")
}

/// The directory a glob can match under: its components before the first wildcard (all
/// but the last component when there is none).
pub(crate) fn static_prefix(pattern: &str) -> String {
    let parts: Vec<&str> = pattern.split('/').collect();
    let fixed: Vec<&str> = match parts.iter().position(|p| p.contains(['*', '?'])) {
        Some(index) => parts[..index].to_vec(),
        None => parts[..parts.len().saturating_sub(1)].to_vec(),
    };
    fixed.join("/")
}

/// Kebab-case area name from a folder name: `NowPlaying` → `now-playing`.
pub fn slug(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_alphanumeric() {
            if c.is_ascii_uppercase() && i > 0 {
                let previous = chars[i - 1];
                let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
                if previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_ascii_uppercase() && next_lower)
                {
                    out.push('-');
                }
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    let mut collapsed = String::new();
    for c in out.chars() {
        if c == '-' && (collapsed.is_empty() || collapsed.ends_with('-')) {
            continue;
        }
        collapsed.push(c);
    }
    collapsed.trim_end_matches('-').to_owned()
}

/// A path that looks like a test by convention.
pub fn is_test_file(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    let (dirs, file) = parts.split_at(parts.len().saturating_sub(1));
    let file = file.first().copied().unwrap_or("");
    let stem = file.split('.').next().unwrap_or(file);
    dirs.iter().any(|d| {
        matches!(
            *d,
            "test" | "tests" | "Tests" | "androidTest" | "__tests__" | "spec" | "e2e"
        ) || d.ends_with("Tests")
            || d.ends_with("UITests")
            || d.ends_with("Test")
    }) || stem.ends_with("Tests")
        || stem.ends_with("Test")
        || stem.ends_with("_test")
        || stem.starts_with("test_")
        || file.contains(".test.")
        || file.contains(".spec.")
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => panic!("built-in pattern {pattern}: {error}"),
    })
}

/// Accessibility and test identifiers named by string literals in source.
pub fn identifiers_in(text: &str) -> Vec<String> {
    static CELL: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &CELL,
        r#"(?:accessibilityIdentifier\s*\(\s*|testTag\s*\(\s*|testTag\s*=\s*|accessibilityIdentifier\s*=\s*|data-testid\s*=\s*\{?\s*|testID\s*=\s*\{?\s*)(?:"((?:[^"\\\n]|\\.)*)"|'((?:[^'\\\n]|\\.)*)'|`([^`\n]*)`)"#,
    );
    let mut out = Vec::new();
    for captures in re.captures_iter(text) {
        if let Some(m) = captures
            .get(1)
            .or_else(|| captures.get(2))
            .or_else(|| captures.get(3))
        {
            let mut id = dynamic_to_wildcard(m.as_str());
            // `"prefix." + suffix` names a family of identifiers.
            let end = captures.get(0).map_or(m.end(), |all| all.end());
            if text[end..].trim_start().starts_with('+') {
                id.push('*');
            }
            if !id.is_empty() && id != "*" {
                out.push(id);
            }
        }
    }
    out
}

/// Replace interpolations (`\(x)`, `${x}`, `$x`) with `*`.
fn dynamic_to_wildcard(literal: &str) -> String {
    static CELL: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &CELL,
        r"\\\((?:[^()]|\([^()]*\))*\)|\$\{[^}]*\}|\$[A-Za-z_][A-Za-z0-9_.]*",
    );
    re.replace_all(literal, "*").into_owned()
}

/// A string with interpolations (`\\(x)`, `${x}`) and format placeholders (`%@`, `%lld`,
/// `%1$s`) replaced by one marker, so a source literal matches its string-catalog key.
fn normalize_text(text: &str) -> String {
    static CELL: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &CELL,
        r"\\\((?:[^()]|\([^()]*\))*\)|\$\{[^}]*\}|%(?:\d+\$)?(?:@|ll[dui]|l[dui]|[dDuUxXifFeEgGsc]|\.\d+[fF])",
    );
    re.replace_all(text, "\u{1}").into_owned()
}

/// Double-quoted string literals and Android `R.string.name` references in source.
fn usages_in(text: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    static LITERAL: OnceLock<Regex> = OnceLock::new();
    static RESOURCE: OnceLock<Regex> = OnceLock::new();
    let literal = regex(&LITERAL, r#""((?:[^"\\\n]|\\.)*)""#);
    let resource = regex(&RESOURCE, r"R\.string\.([A-Za-z0-9_]+)");
    (
        literal
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| normalize_text(m.as_str())))
            .collect(),
        resource
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
            .collect(),
    )
}

/// A user-visible string from a string catalog.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CatalogString {
    pub key: String,
    pub value: String,
    /// Android resource name (`R.string.<key>`), not text used in source.
    pub resource: bool,
}

fn is_locale(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    (bytes.len() == 2 && stem.bytes().all(|b| b.is_ascii_lowercase()))
        || (bytes.len() == 5
            && stem[..2].bytes().all(|b| b.is_ascii_lowercase())
            && matches!(bytes[2], b'-' | b'_'))
}

/// Strings in the source language of one string-catalog file; other locales are skipped.
pub fn strings_in(path: &str, text: &str) -> Vec<CatalogString> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let parent = path.rsplit('/').nth(1).unwrap_or("");
    let mut out = Vec::new();
    if name.ends_with(".xcstrings") {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
            return out;
        };
        let source = json
            .get("sourceLanguage")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("en");
        let Some(strings) = json.get("strings").and_then(serde_json::Value::as_object) else {
            return out;
        };
        for (key, item) in strings {
            if key.trim().is_empty()
                || item
                    .get("extractionState")
                    .and_then(serde_json::Value::as_str)
                    == Some("stale")
            {
                continue;
            }
            let value = item
                .pointer(&format!("/localizations/{source}/stringUnit/value"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(key);
            out.push(CatalogString {
                key: key.clone(),
                value: value.to_owned(),
                resource: false,
            });
        }
    } else if name.ends_with(".strings") {
        if parent.ends_with(".lproj") && !matches!(parent, "en.lproj" | "Base.lproj") {
            return out;
        }
        static CELL: OnceLock<Regex> = OnceLock::new();
        let re = regex(
            &CELL,
            r#"(?m)^\s*"((?:[^"\\]|\\.)*)"\s*=\s*"((?:[^"\\]|\\.)*)"\s*;"#,
        );
        for c in re.captures_iter(text) {
            if let (Some(key), Some(value)) = (c.get(1), c.get(2)) {
                out.push(CatalogString {
                    key: key.as_str().to_owned(),
                    value: value.as_str().to_owned(),
                    resource: false,
                });
            }
        }
    } else if name.ends_with(".xml") {
        if parent != "values" {
            return out; // values-xx directories are translations
        }
        static CELL: OnceLock<Regex> = OnceLock::new();
        let re = regex(
            &CELL,
            r#"(?s)<string\s+[^>]*?name\s*=\s*"([^"]+)"[^>]*>(.*?)</string>"#,
        );
        for c in re.captures_iter(text) {
            if let (Some(key), Some(value)) = (c.get(1), c.get(2)) {
                out.push(CatalogString {
                    key: key.as_str().to_owned(),
                    value: android_text(value.as_str()),
                    resource: true,
                });
            }
        }
    } else if name.ends_with(".json") {
        let stem = name.trim_end_matches(".json");
        if is_locale(stem) && !stem.starts_with("en") {
            return out;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
            return out;
        };
        fn flatten(prefix: &str, value: &serde_json::Value, out: &mut Vec<CatalogString>) {
            match value {
                serde_json::Value::String(text) => out.push(CatalogString {
                    key: prefix.to_owned(),
                    value: text.clone(),
                    resource: false,
                }),
                serde_json::Value::Object(map) => {
                    for (key, value) in map {
                        let key = if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}.{key}")
                        };
                        flatten(&key, value, out);
                    }
                }
                _ => {}
            }
        }
        flatten("", &json, &mut out);
    }
    out.sort();
    out.dedup_by(|a, b| a.key == b.key);
    out
}

/// Unescape the common Android string-resource escapes for display and matching.
fn android_text(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw);
    raw.replace("\\'", "'")
        .replace("\\\"", "\"")
        .replace("\\n", "\n")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub files: usize,
    pub files_cited: usize,
    pub identifiers: usize,
    pub identifiers_cited: usize,
    pub strings: usize,
    pub strings_cited: usize,
    pub tests: usize,
    pub tests_cited: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AreaSummary {
    pub area: String,
    pub files: usize,
    pub files_cited: usize,
    pub identifiers: usize,
    pub identifiers_cited: usize,
    pub strings: usize,
    pub strings_cited: usize,
    pub tests: usize,
    /// Entries citing a file in this candidate area.
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileItem {
    pub path: String,
    pub area: String,
    pub cited_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IdentifierItem {
    /// Interpolated parts appear as `*`.
    pub identifier: String,
    pub areas: Vec<String>,
    pub files: Vec<String>,
    pub cited_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StringItem {
    pub key: String,
    pub value: String,
    pub file: String,
    /// Candidate areas whose source uses the key (as a literal or `R.string.<key>`).
    pub areas: Vec<String>,
    pub cited_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlatformInventory {
    pub platform: String,
    /// The configured root, relative to repo_root.
    pub root: String,
    /// Why the platform was not scanned (an external root that was not supplied).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unchecked: Option<String>,
    pub summary: Summary,
    pub areas: Vec<AreaSummary>,
    pub files: Vec<FileItem>,
    pub identifiers: Vec<IdentifierItem>,
    pub strings: Vec<StringItem>,
    pub tests: Vec<FileItem>,
    /// Symbolic links not followed (relative to the platform root).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_symlinks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub app: String,
    /// Only items no entry cites are listed (summary counts still cover everything).
    pub uncatalogued: bool,
    pub platforms: Vec<PlatformInventory>,
}

/// The app name: the catalog directory's name.
pub fn app_name(catalog: &Catalog) -> String {
    catalog
        .directory
        .canonicalize()
        .unwrap_or_else(|_| catalog.directory.clone())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("app")
        .to_owned()
}

/// Globs in effect for one platform, with defaults when `[source_globs]` is absent.
struct Globs {
    sources: Vec<String>,
    all: Vec<String>,
    strings: Vec<String>,
    tests: Vec<String>,
}
impl Globs {
    fn of(catalog: &Catalog, platform: &str) -> Self {
        let configured = catalog.config.source_globs.get(platform);
        Self::from_lists(
            configured.map(|g| g.all().to_vec()).unwrap_or_default(),
            configured
                .map(|g| g.user_visible().to_vec())
                .unwrap_or_default(),
            configured.map(|g| g.strings().to_vec()).unwrap_or_default(),
            configured.map(|g| g.tests().to_vec()).unwrap_or_default(),
        )
    }
    /// The globs in effect for these `[source_globs]` lists, with inventory's defaults.
    fn from_lists(
        all: Vec<String>,
        user_visible: Vec<String>,
        strings: Vec<String>,
        tests: Vec<String>,
    ) -> Self {
        let all = if all.is_empty() && user_visible.is_empty() {
            SOURCE_EXTENSIONS
                .iter()
                .map(|e| format!("**/*.{e}"))
                .collect()
        } else {
            all
        };
        let strings = match strings {
            s if !s.is_empty() => s,
            _ => vec![
                "**/*.xcstrings".into(),
                "**/Localizable.strings".into(),
                "**/res/values/strings.xml".into(),
            ],
        };
        Self {
            sources: if user_visible.is_empty() {
                all.clone()
            } else {
                user_visible
            },
            all,
            strings,
            tests,
        }
    }
    fn prefixes(&self) -> Vec<String> {
        let mut prefixes: Vec<String> = self
            .sources
            .iter()
            .chain(&self.all)
            .chain(&self.strings)
            .chain(&self.tests)
            .map(|g| static_prefix(g))
            .collect();
        prefixes.sort();
        prefixes.dedup();
        // Drop prefixes nested in another prefix; the outer walk covers them.
        let mut kept: Vec<String> = Vec::new();
        for prefix in prefixes {
            if !kept
                .iter()
                .any(|k| k.is_empty() || prefix == *k || prefix.starts_with(&format!("{k}/")))
            {
                kept.push(prefix);
            }
        }
        kept
    }
}

fn matches(globs: &[String], path: &str) -> Option<usize> {
    globs
        .iter()
        .position(|g| glob(g.as_bytes(), path.as_bytes()))
}

/// Groups files by the glob that matched them and names each file's candidate area.
fn areas_for(files: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    // files: path -> group key (the matched glob's static prefix, extended through a
    // `kotlin/` or `java/` source directory).
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (path, group) in files {
        groups.entry(group).or_default().push(path);
    }
    let largest = groups
        .iter()
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| b.0.cmp(a.0)))
        .map(|(k, _)| *k);
    let mut out = BTreeMap::new();
    for (group, paths) in &groups {
        // In Kotlin and Java sources, strip the package directories every file shares.
        let package_root = group.ends_with("kotlin") || group.ends_with("java");
        let strip = |path: &str| -> Vec<String> {
            let rest = under(group, path);
            let mut dirs: Vec<String> = rest.split('/').map(str::to_owned).collect();
            dirs.pop();
            dirs
        };
        let common = if package_root {
            let all: Vec<Vec<String>> = paths.iter().map(|p| strip(p)).collect();
            let mut common = all.first().cloned().unwrap_or_default();
            for dirs in &all {
                let shared = common.iter().zip(dirs).take_while(|(a, b)| a == b).count();
                common.truncate(shared);
            }
            common.len()
        } else {
            0
        };
        let root_name = if Some(*group) == largest {
            "app".to_owned()
        } else {
            group
                .split('/')
                .rev()
                .find(|c| {
                    // A module or target folder may be called `feature`; skip only layout folders.
                    !c.is_empty()
                        && !matches!(
                            c.to_ascii_lowercase().as_str(),
                            "src" | "source" | "sources" | "main" | "kotlin" | "java" | "swift"
                        )
                })
                .map(slug)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "app".to_owned())
        };
        for path in paths {
            let dirs = strip(path);
            let area = dirs
                .iter()
                .skip(common)
                .find(|d| !GENERIC_DIRS.contains(&d.to_ascii_lowercase().as_str()))
                .map(|d| slug(d))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| root_name.clone());
            out.insert((*path).to_owned(), area);
        }
    }
    out
}

/// `path` relative to the directory `prefix` (unchanged when it is not inside it).
fn under<'a>(prefix: &str, path: &'a str) -> &'a str {
    if prefix.is_empty() {
        return path;
    }
    path.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(path)
}

fn group_key(prefix: &str, path: &str) -> String {
    let rest = under(prefix, path);
    let parts: Vec<&str> = rest.split('/').collect();
    let dirs = &parts[..parts.len().saturating_sub(1)];
    match dirs.iter().position(|d| *d == "kotlin" || *d == "java") {
        Some(index) => {
            let inner = dirs[..=index].join("/");
            if prefix.is_empty() {
                inner
            } else {
                format!("{prefix}/{inner}")
            }
        }
        None => prefix.to_owned(),
    }
}

/// A platform's files, sorted into inventory's kinds.
struct Classified {
    /// User-visible source path -> group key.
    sources: BTreeMap<String, String>,
    /// Test path -> group key.
    tests: BTreeMap<String, String>,
    string_files: Vec<String>,
    /// Every non-test source-like file (read for string usages).
    scanned: Vec<String>,
    extras: WalkExtras,
}

/// Walk a platform root and classify its files: string catalogs, then tests, then sources.
/// `init` counts with this too, so its numbers are inventory's.
fn classify(root: &Path, globs: &Globs) -> Result<Classified> {
    let mut paths = BTreeSet::new();
    let mut extras = WalkExtras::default();
    for prefix in globs.prefixes() {
        walk(root, &prefix, &mut paths, &mut extras)?;
    }
    // Classify: string catalogs, tests, then user-visible sources.
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    let mut tests: BTreeMap<String, String> = BTreeMap::new();
    let mut string_files = Vec::new();
    let mut scanned = Vec::new(); // every source-like file read for string usages
    for path in &paths {
        if matches(&globs.strings, path).is_some() {
            string_files.push(path.clone());
            continue;
        }
        if let Some(index) = matches(&globs.tests, path) {
            tests.insert(
                path.clone(),
                group_key(&static_prefix(&globs.tests[index]), path),
            );
            continue;
        }
        let in_sources = matches(&globs.sources, path);
        let in_all = matches(&globs.all, path);
        if in_sources.is_none() && in_all.is_none() {
            continue;
        }
        if is_test_file(path) {
            let pattern = in_all
                .map(|i| &globs.all[i])
                .or_else(|| in_sources.map(|i| &globs.sources[i]));
            let prefix = pattern.map(|g| static_prefix(g)).unwrap_or_default();
            tests.insert(path.clone(), group_key(&prefix, path));
            continue;
        }
        scanned.push(path.clone());
        if let Some(index) = in_sources {
            sources.insert(
                path.clone(),
                group_key(&static_prefix(&globs.sources[index]), path),
            );
        }
    }
    Ok(Classified {
        sources,
        tests,
        string_files,
        scanned,
        extras,
    })
}

/// The number of user-visible source files `inventory` would list for these globs.
pub(crate) fn count_sources(
    root: &Path,
    all: &[String],
    user_visible: &[String],
    strings: &[String],
    tests: &[String],
) -> Result<usize> {
    let globs = Globs::from_lists(
        all.to_vec(),
        user_visible.to_vec(),
        strings.to_vec(),
        tests.to_vec(),
    );
    Ok(classify(root, &globs)?.sources.len())
}

impl Catalog {
    /// Inventory one platform (or every configured platform).
    pub fn inventory(&self, options: &RootOptions, platform: Option<&str>) -> Result<Inventory> {
        let platforms: Vec<&String> = match platform {
            Some(p) => vec![self
                .config
                .platforms
                .get_key_value(p)
                .map(|(k, _)| k)
                .ok_or_else(|| format!("unknown platform: {p}"))?],
            None => self.config.platforms.keys().collect(),
        };
        let mut out = Inventory {
            app: app_name(self),
            uncatalogued: false,
            platforms: Vec::new(),
        };
        for name in platforms {
            let configured = self.config.platforms[name].root().to_owned();
            let root = match self.platform_root(name, options) {
                Ok(Some(root)) => root,
                Ok(None) => {
                    out.platforms.push(PlatformInventory::unchecked(
                        name,
                        &configured,
                        format!(
                            "root not supplied; pass --root {name}=<path> or CONCORDANCE_ROOT_{}",
                            name.to_uppercase().replace('-', "_")
                        ),
                    ));
                    continue;
                }
                Err(error) if platform.is_none() => {
                    out.platforms.push(PlatformInventory::unchecked(
                        name,
                        &configured,
                        format!("root unavailable: {error}"),
                    ));
                    continue;
                }
                Err(error) => return Err(format!("{name}: {error}").into()),
            };
            out.platforms
                .push(self.inventory_platform(name, &configured, &root)?);
        }
        Ok(out)
    }

    fn inventory_platform(
        &self,
        name: &str,
        configured: &str,
        root: &Path,
    ) -> Result<PlatformInventory> {
        let globs = Globs::of(self, name);
        let Classified {
            sources,
            tests,
            string_files,
            scanned,
            extras,
        } = classify(root, &globs)?;
        let source_areas = areas_for(&sources);
        let mut test_areas = areas_for(&tests);
        // A test named after a source area (`HomeUITests.swift`) belongs to that area.
        let known: BTreeSet<&String> = source_areas.values().collect();
        for (path, area) in &mut test_areas {
            let stem = slug(
                path.rsplit('/')
                    .next()
                    .unwrap_or(path)
                    .split('.')
                    .next()
                    .unwrap_or(""),
            );
            if let Some(best) = known
                .iter()
                .filter(|a| stem == ***a || stem.starts_with(&format!("{}-", a)))
                .max_by_key(|a| a.len())
            {
                *area = (*best).clone();
            }
        }

        // Citations on this platform: path -> entry IDs.
        let mut cited: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (label, platform, reference) in self.evidence_items() {
            if platform != name {
                continue;
            }
            if let Ok(path) = self.evidence_relative(name, reference) {
                let id = label.split('#').next().unwrap_or(&label).to_owned();
                cited.entry(path.to_owned()).or_default().insert(id);
            }
        }
        // Identifiers and copy that entries name.
        let mut entry_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut entry_copy: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in self.entries.values() {
            for id in entry
                .accessibility
                .identifier
                .split([',', ' ', '\n'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                entry_ids
                    .entry(id.to_owned())
                    .or_default()
                    .insert(entry.id.clone());
            }
            for text in &entry.copy {
                entry_copy
                    .entry(text.clone())
                    .or_default()
                    .insert(entry.id.clone());
            }
            for detail in &entry.details {
                if let Some(copy) = &detail.spec.copy {
                    for text in std::iter::once(&copy.text).chain(copy.key.as_ref()) {
                        entry_copy
                            .entry(text.clone())
                            .or_default()
                            .insert(entry.id.clone());
                    }
                }
            }
        }

        let mut identifiers: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> =
            BTreeMap::new();
        // Text (or resource name) -> module -> areas using it.
        type Usage = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;
        let mut literal_areas: Usage = BTreeMap::new();
        let mut resource_areas: Usage = BTreeMap::new();
        for path in &scanned {
            let Ok(text) = fs::read_to_string(root.join(path)) else {
                continue; // binary or unreadable
            };
            let area = source_areas.get(path).cloned();
            if let Some(area) = &area {
                for id in identifiers_in(&text) {
                    let slot = identifiers.entry(id).or_default();
                    slot.0.insert(area.clone());
                    slot.1.insert(path.clone());
                }
            }
            let area = area.unwrap_or_else(|| "(other)".to_owned());
            let module = module_of(path).to_owned();
            let (literals, resources) = usages_in(&text);
            for (map, keys) in [
                (&mut literal_areas, literals),
                (&mut resource_areas, resources),
            ] {
                for key in keys {
                    map.entry(key)
                        .or_default()
                        .entry(module.clone())
                        .or_default()
                        .insert(area.clone());
                }
            }
        }
        // A string's areas: uses in its own module, or anywhere when its module has none.
        let areas_of = |usage: Option<&BTreeMap<String, BTreeSet<String>>>, file: &str| {
            let Some(by_module) = usage else {
                return Vec::new();
            };
            match by_module.get(module_of(file)) {
                Some(own) => own.iter().cloned().collect(),
                None => by_module
                    .values()
                    .flatten()
                    .cloned()
                    .collect::<BTreeSet<String>>()
                    .into_iter()
                    .collect(),
            }
        };

        // Keyed by (file, key): two modules' `title` strings stay separate.
        let mut strings: BTreeMap<(String, String), StringItem> = BTreeMap::new();
        for file in &string_files {
            let Ok(text) = fs::read_to_string(root.join(file)) else {
                continue;
            };
            for s in strings_in(file, &text) {
                let usage = if s.resource {
                    resource_areas.get(&s.key)
                } else {
                    literal_areas.get(&normalize_text(&s.key))
                };
                let mut cited_by: BTreeSet<String> = BTreeSet::new();
                for text in [&s.key, &s.value] {
                    cited_by.extend(entry_copy.get(text).into_iter().flatten().cloned());
                }
                strings
                    .entry((s.key.clone(), file.clone()))
                    .or_insert(StringItem {
                        areas: areas_of(usage, file),
                        key: s.key,
                        value: s.value,
                        file: file.clone(),
                        cited_by: cited_by.into_iter().collect(),
                    });
            }
        }

        let file_items = |map: &BTreeMap<String, String>| -> Vec<FileItem> {
            map.iter()
                .map(|(path, area)| FileItem {
                    path: path.clone(),
                    area: area.clone(),
                    cited_by: cited.get(path).into_iter().flatten().cloned().collect(),
                })
                .collect()
        };
        let files = file_items(&source_areas);
        let tests = file_items(&test_areas);
        let identifiers: Vec<IdentifierItem> = identifiers
            .into_iter()
            .map(|(identifier, (areas, files))| {
                let cited_by: BTreeSet<String> = if identifier.contains('*') {
                    let pattern = identifier.replace("**", "*");
                    entry_ids
                        .iter()
                        .filter(|(id, _)| {
                            glob(pattern.as_bytes(), id.as_bytes()) || **id == identifier
                        })
                        .flat_map(|(_, e)| e.iter().cloned())
                        .collect()
                } else {
                    entry_ids.get(&identifier).cloned().unwrap_or_default()
                };
                IdentifierItem {
                    identifier,
                    areas: areas.into_iter().collect(),
                    files: files.into_iter().collect(),
                    cited_by: cited_by.into_iter().collect(),
                }
            })
            .collect();
        let strings: Vec<StringItem> = strings.into_values().collect();

        let mut areas: BTreeMap<String, AreaSummary> = BTreeMap::new();
        let mut entries_by_area: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        fn area<'a>(
            areas: &'a mut BTreeMap<String, AreaSummary>,
            name: &str,
        ) -> &'a mut AreaSummary {
            areas.entry(name.to_owned()).or_insert_with(|| AreaSummary {
                area: name.to_owned(),
                files: 0,
                files_cited: 0,
                identifiers: 0,
                identifiers_cited: 0,
                strings: 0,
                strings_cited: 0,
                tests: 0,
                entries: Vec::new(),
            })
        }
        for file in &files {
            let a = area(&mut areas, &file.area);
            a.files += 1;
            if !file.cited_by.is_empty() {
                a.files_cited += 1;
            }
            entries_by_area
                .entry(file.area.clone())
                .or_default()
                .extend(file.cited_by.iter().cloned());
        }
        for id in &identifiers {
            for name in &id.areas {
                let a = area(&mut areas, name);
                a.identifiers += 1;
                if !id.cited_by.is_empty() {
                    a.identifiers_cited += 1;
                }
            }
        }
        for s in &strings {
            for name in s.areas.iter().filter(|a| a.as_str() != "(other)") {
                let a = area(&mut areas, name);
                a.strings += 1;
                if !s.cited_by.is_empty() {
                    a.strings_cited += 1;
                }
            }
        }
        for test in &tests {
            if let Some(a) = areas.get_mut(&test.area) {
                a.tests += 1;
            }
        }
        for (name, entries) in entries_by_area {
            if let Some(a) = areas.get_mut(&name) {
                a.entries = entries.into_iter().collect();
            }
        }
        let count = |items: &[FileItem]| items.iter().filter(|f| !f.cited_by.is_empty()).count();
        let summary = Summary {
            files: files.len(),
            files_cited: count(&files),
            identifiers: identifiers.len(),
            identifiers_cited: identifiers
                .iter()
                .filter(|i| !i.cited_by.is_empty())
                .count(),
            strings: strings.len(),
            strings_cited: strings.iter().filter(|s| !s.cited_by.is_empty()).count(),
            tests: tests.len(),
            tests_cited: count(&tests),
        };
        Ok(PlatformInventory {
            platform: name.to_owned(),
            root: configured.to_owned(),
            unchecked: None,
            summary,
            areas: areas.into_values().collect(),
            files,
            identifiers,
            strings,
            tests,
            skipped_symlinks: extras.skipped_symlinks.into_iter().collect(),
        })
    }
}

impl PlatformInventory {
    fn unchecked(name: &str, root: &str, reason: String) -> Self {
        Self {
            platform: name.to_owned(),
            root: root.to_owned(),
            unchecked: Some(reason),
            summary: Summary::default(),
            areas: Vec::new(),
            files: Vec::new(),
            identifiers: Vec::new(),
            strings: Vec::new(),
            tests: Vec::new(),
            skipped_symlinks: Vec::new(),
        }
    }
}

fn md_code(text: &str) -> String {
    let flat = text.replace('\n', "\\n");
    if flat.contains('`') {
        format!("`` {flat} ``")
    } else {
        format!("`{flat}`")
    }
}
fn md_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}
fn cited_suffix(cited_by: &[String]) -> String {
    if cited_by.is_empty() {
        " — not cited".into()
    } else {
        format!(" — cited by {}", cited_by.join(", "))
    }
}

impl Inventory {
    /// Keep only items no entry cites; summary counts are unchanged.
    pub fn uncatalogued(mut self) -> Self {
        self.uncatalogued = true;
        for p in &mut self.platforms {
            p.files.retain(|f| f.cited_by.is_empty());
            p.tests.retain(|f| f.cited_by.is_empty());
            p.identifiers.retain(|i| i.cited_by.is_empty());
            p.strings.retain(|s| s.cited_by.is_empty());
        }
        self
    }

    pub fn markdown(&self) -> String {
        let mut out = vec![format!("# Inventory: {}", self.app), String::new()];
        out.push(
            "| Platform | Root | Files cited | Identifiers cited | Strings cited | Tests cited |"
                .into(),
        );
        out.push("| --- | --- | --- | --- | --- | --- |".into());
        for p in &self.platforms {
            match &p.unchecked {
                Some(reason) => out.push(format!(
                    "| {} | {} | unchecked: {} | | | |",
                    p.platform,
                    md_code(&p.root),
                    md_cell(reason)
                )),
                None => {
                    let s = &p.summary;
                    out.push(format!(
                        "| {} | {} | {}/{} | {}/{} | {}/{} | {}/{} |",
                        p.platform,
                        md_code(&p.root),
                        s.files_cited,
                        s.files,
                        s.identifiers_cited,
                        s.identifiers,
                        s.strings_cited,
                        s.strings,
                        s.tests_cited,
                        s.tests
                    ));
                }
            }
        }
        out.push(String::new());
        out.push(if self.uncatalogued {
            "Listing only what no catalog entry cites. Files are user-visible sources (`[source_globs] user_visible`, else `all`); areas are candidate areas named after feature folders.".into()
        } else {
            "Files are user-visible sources (`[source_globs] user_visible`, else `all`); areas are candidate areas named after feature folders.".into()
        });
        for p in self.platforms.iter().filter(|p| p.unchecked.is_none()) {
            out.extend([String::new(), format!("## {}", p.platform), String::new()]);
            if !p.skipped_symlinks.is_empty() {
                out.push(format!(
                    "Skipped {} symbolic link(s) (directory links, dangling links, or links that leave the platform root): {}.",
                    p.skipped_symlinks.len(),
                    p.skipped_symlinks
                        .iter()
                        .map(|l| md_code(l))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                out.push(String::new());
            }
            out.push(
                "| Area | Files cited | Identifiers cited | Strings cited | Tests | Entries |"
                    .into(),
            );
            out.push("| --- | --- | --- | --- | --- | --- |".into());
            for a in &p.areas {
                out.push(format!(
                    "| {} | {}/{} | {}/{} | {}/{} | {} | {} |",
                    a.area,
                    a.files_cited,
                    a.files,
                    a.identifiers_cited,
                    a.identifiers,
                    a.strings_cited,
                    a.strings,
                    a.tests,
                    a.entries.len()
                ));
            }
            let section = |out: &mut Vec<String>, title: &str, lines: Vec<String>| {
                out.extend([
                    String::new(),
                    format!("### {title} ({})", lines.len()),
                    String::new(),
                ]);
                if lines.is_empty() {
                    out.push("None.".into());
                }
                out.extend(lines);
            };
            section(
                &mut out,
                "Files",
                p.files
                    .iter()
                    .map(|f| {
                        format!(
                            "- {} ({}){}",
                            md_code(&f.path),
                            f.area,
                            cited_suffix(&f.cited_by)
                        )
                    })
                    .collect(),
            );
            section(
                &mut out,
                "Identifiers",
                p.identifiers
                    .iter()
                    .map(|i| {
                        format!(
                            "- {} ({}; {}){}",
                            md_code(&i.identifier),
                            i.areas.join(", "),
                            i.files.join(", "),
                            cited_suffix(&i.cited_by)
                        )
                    })
                    .collect(),
            );
            section(
                &mut out,
                "Strings",
                p.strings
                    .iter()
                    .map(|s| {
                        let value = if s.value == s.key {
                            String::new()
                        } else {
                            format!(" = {}", md_code(&s.value))
                        };
                        let areas = if s.areas.is_empty() {
                            "unused in scanned source".to_owned()
                        } else {
                            s.areas.join(", ")
                        };
                        format!(
                            "- {}{value} ({}; {areas}){}",
                            md_code(&s.key),
                            s.file,
                            cited_suffix(&s.cited_by)
                        )
                    })
                    .collect(),
            );
            section(
                &mut out,
                "Tests",
                p.tests
                    .iter()
                    .map(|f| {
                        format!(
                            "- {} ({}){}",
                            md_code(&f.path),
                            f.area,
                            cited_suffix(&f.cited_by)
                        )
                    })
                    .collect(),
            );
        }
        out.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slugs_and_prefixes() {
        assert_eq!(slug("NowPlaying"), "now-playing");
        assert_eq!(slug("TaupeUITests"), "taupe-ui-tests");
        assert_eq!(slug("ad_skip"), "ad-skip");
        assert_eq!(
            static_prefix("apps/suite/Taupe/**/*.swift"),
            "apps/suite/Taupe"
        );
        assert_eq!(static_prefix("**/*.kt"), "");
        assert_eq!(static_prefix("src/main.rs"), "src");
    }
    #[test]
    fn identifiers_and_tests() {
        let ids = identifiers_in(
            r#".accessibilityIdentifier("home.row.\(item.id)") Modifier.testTag("home.list") testTag = "x" <div data-testid="web.a">"#,
        );
        assert_eq!(ids, vec!["home.row.*", "home.list", "x", "web.a"]);
        assert!(is_test_file("App/AppUITests/HomeTests.swift"));
        assert!(is_test_file("app/src/test/kotlin/HomeTest.kt"));
        assert!(!is_test_file("App/Home/HomeView.swift"));
        let ids = identifiers_in(r#".accessibilityIdentifier("action.quick." + action.rawValue)"#);
        assert_eq!(ids, vec!["action.quick.*"]);
    }
    #[test]
    fn placeholders_match_interpolations() {
        assert_eq!(
            normalize_text("%lld episodes"),
            normalize_text(r"\(items.count) episodes")
        );
        assert_eq!(
            normalize_text("%1$@ of %2$@"),
            normalize_text(r"\(a) of \(b)")
        );
        assert_ne!(normalize_text("Play"), normalize_text("Pause"));
    }
}
