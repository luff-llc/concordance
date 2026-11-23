//! `concordance init` and `concordance guide`: bring Concordance into a repository and print
//! the agent kit. See docs/adopt.md.
use crate::inventory::{is_test_file, slug, walk_with};
use crate::{fail, Catalog, Config, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// What `init` scans and where it writes.
#[derive(Debug, Clone)]
pub struct InitOptions {
    pub repo: PathBuf,
    /// Default: the repository directory's name.
    pub app: Option<String>,
    /// Catalog directory, relative to the repository. Default: `spec/<app>`.
    pub out: Option<PathBuf>,
}

/// Globs written to `[source_globs.<platform>]`, relative to the platform root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobPlan {
    pub all: Vec<String>,
    pub user_visible: Vec<String>,
    pub strings: Vec<String>,
    pub tests: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DetectedPlatform {
    pub name: String,
    /// `apple`, `android`, `web` or `cli`.
    pub kind: &'static str,
    /// Relative to the repository; `.` for the repository itself.
    pub root: String,
    pub markers: Vec<String>,
    pub globs: GlobPlan,
    pub user_visible_files: usize,
}

#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// Relative to the repository.
    pub path: String,
    absolute: PathBuf,
    /// `None` for a directory.
    pub content: Option<String>,
    pub exists: bool,
}

#[derive(Debug, Clone)]
pub struct InitPlan {
    pub app: String,
    /// Catalog directory relative to the repository.
    pub out: String,
    pub repo_root: String,
    pub platforms: Vec<DetectedPlatform>,
    pub tag_pattern: Option<String>,
    pub version_tags: usize,
    pub files: Vec<PlannedFile>,
    /// `catalog.toml` already exists: nothing is written; `notes` say what would be added.
    pub adopted: bool,
    pub notes: Vec<String>,
}

const WEB_EXTENSIONS: &[&str] = &["tsx", "jsx", "vue", "svelte", "ts", "js"];
const WEB_DIRS: &[&str] = &["src", "app", "pages", "components", "lib"];
const UI_IMPORTS_APPLE: &[&str] = &[
    "import SwiftUI",
    "import UIKit",
    "import AppKit",
    "import WidgetKit",
];
const UI_IMPORTS_ANDROID: &[&str] = &[
    "androidx.compose",
    "android.widget",
    "android.view",
    "android.app.Activity",
];

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
fn extension(path: &str) -> &str {
    file_name(path)
        .rsplit_once('.')
        .map(|(_, e)| e)
        .unwrap_or("")
}
fn inside(dir: &str, path: &str) -> bool {
    dir.is_empty() || path.starts_with(&format!("{dir}/"))
}
fn under<'a>(dir: &str, path: &'a str) -> &'a str {
    if dir.is_empty() {
        path
    } else {
        path.strip_prefix(&format!("{dir}/")).unwrap_or(path)
    }
}
fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_owned()
    } else {
        format!("{dir}/{rest}")
    }
}
fn common_ancestor<'a>(dirs: impl IntoIterator<Item = &'a str>) -> String {
    let mut common: Option<Vec<&str>> = None;
    for dir in dirs {
        let parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
        common = Some(match common {
            None => parts,
            Some(c) => c
                .iter()
                .zip(&parts)
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| *a)
                .collect(),
        });
    }
    common.unwrap_or_default().join("/")
}
fn is_test_dir(name: &str) -> bool {
    matches!(
        name,
        "test" | "tests" | "Tests" | "__tests__" | "spec" | "e2e"
    ) || name.ends_with("Tests")
        || name.ends_with("Test")
}
fn reads_any(repo: &Path, files: &[&String], needles: &[&str]) -> bool {
    files.iter().any(|f| {
        fs::read_to_string(repo.join(f)).is_ok_and(|text| needles.iter().any(|n| text.contains(n)))
    })
}

/// Group files (relative to `root`) by their top-level directory under it (`""` for files
/// at the root).
fn by_top_dir<'a>(root: &str, files: &[&'a String]) -> BTreeMap<String, Vec<&'a String>> {
    let mut groups: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for file in files {
        let rest = under(root, file);
        let top = rest.split_once('/').map(|(d, _)| d).unwrap_or("");
        groups.entry(top.to_owned()).or_default().push(file);
    }
    groups
}

fn detect_apple(
    repo: &Path,
    files: &BTreeSet<String>,
    bundles: &BTreeSet<String>,
) -> Option<DetectedPlatform> {
    let mut markers: Vec<String> = files
        .iter()
        .filter(|f| matches!(file_name(f), "Package.swift" | "project.yml"))
        .cloned()
        .chain(bundles.iter().cloned())
        .collect();
    markers.sort();
    if markers.is_empty() {
        return None;
    }
    let root = common_ancestor(markers.iter().map(|m| parent(m)));
    let swift: Vec<&String> = files
        .iter()
        .filter(|f| extension(f) == "swift" && inside(&root, f) && file_name(f) != "Package.swift")
        .collect();
    if swift.is_empty() {
        return None;
    }
    // ios or macos only when the project files name exactly one of them.
    let (mut ios, mut macos) = (false, false);
    for marker in &markers {
        let path = if marker.ends_with(".xcodeproj") {
            repo.join(marker).join("project.pbxproj")
        } else {
            repo.join(marker)
        };
        let text = fs::read_to_string(path).unwrap_or_default();
        ios |=
            text.contains(".iOS(") || text.contains("iphoneos") || text.contains("platform: iOS");
        macos |=
            text.contains(".macOS(") || text.contains("macosx") || text.contains("platform: macOS");
    }
    let name = match (ios, macos) {
        (true, false) => "ios",
        (false, true) => "macos",
        _ => "apple",
    };
    let mut globs = GlobPlan::default();
    let mut user_visible_files = 0;
    for (top, group) in by_top_dir(&root, &swift) {
        let pattern = if top.is_empty() {
            "*.swift".to_owned()
        } else {
            format!("{top}/**/*.swift")
        };
        if !top.is_empty() && is_test_dir(&top) {
            globs.tests.push(pattern);
            continue;
        }
        let sources: Vec<&String> = group
            .iter()
            .copied()
            .filter(|f| !is_test_file(under(&root, f)))
            .collect();
        if sources.is_empty() {
            globs.tests.push(pattern);
            continue;
        }
        globs.all.push(pattern.clone());
        if reads_any(repo, &sources, UI_IMPORTS_APPLE) {
            globs.user_visible.push(pattern);
            user_visible_files += sources.len();
        }
    }
    let rel: Vec<&str> = files
        .iter()
        .filter(|f| inside(&root, f))
        .map(|f| under(&root, f))
        .collect();
    if rel.iter().any(|f| f.ends_with(".xcstrings")) {
        globs.strings.push("**/*.xcstrings".into());
    }
    if rel.iter().any(|f| file_name(f) == "Localizable.strings") {
        globs.strings.push("**/Localizable.strings".into());
    }
    Some(DetectedPlatform {
        name: name.into(),
        kind: "apple",
        root: if root.is_empty() { ".".into() } else { root },
        markers,
        globs,
        user_visible_files,
    })
}

fn detect_android(repo: &Path, files: &BTreeSet<String>) -> Option<DetectedPlatform> {
    let manifests: Vec<&String> = files
        .iter()
        .filter(|f| file_name(f) == "AndroidManifest.xml")
        .collect();
    let gradle: Vec<&String> = files
        .iter()
        .filter(|f| matches!(file_name(f), "build.gradle" | "build.gradle.kts"))
        .filter(|g| manifests.iter().any(|m| inside(parent(g), m)))
        .collect();
    if gradle.is_empty() {
        return None;
    }
    let modules: BTreeSet<&str> = gradle.iter().map(|g| parent(g)).collect();
    let settings: Vec<&str> = files
        .iter()
        .filter(|f| matches!(file_name(f), "settings.gradle" | "settings.gradle.kts"))
        .map(|f| parent(f))
        .filter(|s| modules.iter().all(|m| *m == *s || inside(s, m)))
        .collect();
    let root = settings
        .iter()
        .max_by_key(|s| (s.len(), std::cmp::Reverse(**s)))
        .map(|s| (*s).to_owned())
        .unwrap_or_else(|| common_ancestor(modules.iter().copied()));
    let mut markers: Vec<String> = gradle
        .iter()
        .map(|g| (*g).clone())
        .chain(
            manifests
                .iter()
                .filter(|m| inside(&root, m))
                .map(|m| (*m).clone()),
        )
        .collect();
    markers.sort();
    let mut globs = GlobPlan::default();
    let mut user_visible_files = 0;
    // The innermost module owns each source file.
    let module_of = |path: &str| -> Option<&str> {
        modules
            .iter()
            .filter(|m| inside(m, path))
            .max_by_key(|m| m.len())
            .copied()
    };
    for module in &modules {
        let rel = under(&root, module);
        let rel = if *module == root { "" } else { rel };
        let owned: Vec<&String> = files
            .iter()
            .filter(|f| module_of(f) == Some(module))
            .collect();
        for ext in ["kt", "java"] {
            let sources: Vec<&String> = owned
                .iter()
                .copied()
                .filter(|f| extension(f) == ext && under(module, f).starts_with("src/main/"))
                .collect();
            if !sources.is_empty() {
                let pattern = join(rel, &format!("src/main/**/*.{ext}"));
                globs.all.push(pattern.clone());
                if reads_any(repo, &sources, UI_IMPORTS_ANDROID) {
                    globs.user_visible.push(pattern);
                    user_visible_files += sources.len();
                }
            }
            for set in ["test", "androidTest"] {
                if owned.iter().any(|f| {
                    extension(f) == ext && under(module, f).starts_with(&format!("src/{set}/"))
                }) {
                    globs
                        .tests
                        .push(join(rel, &format!("src/{set}/**/*.{ext}")));
                }
            }
        }
        if owned.iter().any(|f| {
            let r = under(module, f);
            r.starts_with("src/main/res/values") && file_name(r) == "strings.xml"
        }) {
            globs
                .strings
                .push(join(rel, "src/main/res/values*/strings.xml"));
        }
    }
    if globs.all.is_empty() {
        return None;
    }
    Some(DetectedPlatform {
        name: "android".into(),
        kind: "android",
        root: if root.is_empty() { ".".into() } else { root },
        markers,
        globs,
        user_visible_files,
    })
}

fn detect_web(files: &BTreeSet<String>) -> Option<DetectedPlatform> {
    let mut best: Option<(usize, String, Vec<&String>)> = None;
    for package in files.iter().filter(|f| file_name(f) == "package.json") {
        let root = parent(package);
        let sources: Vec<&String> = files
            .iter()
            .filter(|f| {
                let rest = under(root, f);
                inside(root, f)
                    && WEB_EXTENSIONS.contains(&extension(f))
                    && !f.ends_with(".d.ts")
                    && rest
                        .split_once('/')
                        .is_some_and(|(top, _)| WEB_DIRS.contains(&top))
            })
            .collect();
        if sources.is_empty() {
            continue;
        }
        let better = match &best {
            None => true,
            Some((count, current, _)) => {
                sources.len() > *count || (sources.len() == *count && root.len() < current.len())
            }
        };
        if better {
            best = Some((sources.len(), root.to_owned(), sources));
        }
    }
    let (_, root, sources) = best?;
    let mut globs = GlobPlan::default();
    let mut user_visible_files = 0;
    for (top, group) in by_top_dir(&root, &sources) {
        let exts: BTreeSet<&str> = group.iter().map(|f| extension(f)).collect();
        for ext in WEB_EXTENSIONS.iter().filter(|e| exts.contains(*e)) {
            let pattern = format!("{top}/**/*.{ext}");
            let count = group
                .iter()
                .filter(|f| extension(f) == *ext && !is_test_file(f))
                .count();
            globs.all.push(pattern.clone());
            if matches!(*ext, "tsx" | "jsx" | "vue" | "svelte") {
                globs.user_visible.push(pattern);
                user_visible_files += count;
            }
        }
    }
    if globs.user_visible.is_empty() {
        globs.user_visible = globs.all.clone();
        user_visible_files = sources.iter().filter(|f| !is_test_file(f)).count();
    }
    let rel: Vec<&str> = files
        .iter()
        .filter(|f| inside(&root, f))
        .map(|f| under(&root, f))
        .collect();
    for dir in ["locales", "i18n", "lang", "translations", "messages"] {
        if rel
            .iter()
            .any(|f| extension(f) == "json" && f.split('/').any(|c| c == dir))
        {
            globs.strings.push(format!("**/{dir}/**/*.json"));
        }
    }
    for ext in WEB_EXTENSIONS {
        for kind in ["test", "spec"] {
            let suffix = format!(".{kind}.{ext}");
            if rel.iter().any(|f| f.ends_with(&suffix)) {
                globs.tests.push(format!("**/*{suffix}"));
            }
        }
    }
    Some(DetectedPlatform {
        name: "web".into(),
        kind: "web",
        root: if root.is_empty() {
            ".".into()
        } else {
            root.clone()
        },
        markers: vec![join(&root, "package.json")],
        globs,
        user_visible_files,
    })
}

fn detect_cli(files: &BTreeSet<String>) -> Vec<DetectedPlatform> {
    let mut out = Vec::new();
    for (marker, language, ext) in [
        ("Cargo.toml", "rust", "rs"),
        ("go.mod", "go", "go"),
        ("pyproject.toml", "python", "py"),
    ] {
        let Some(root) = files
            .iter()
            .filter(|f| file_name(f) == marker)
            .map(|f| parent(f))
            .filter(|root| files.iter().any(|f| inside(root, f) && extension(f) == ext))
            .min_by_key(|r| (r.split('/').count(), r.len(), *r))
        else {
            continue;
        };
        let rel: Vec<&str> = files
            .iter()
            .filter(|f| inside(root, f) && extension(f) == ext)
            .map(|f| under(root, f))
            .collect();
        let mut globs = GlobPlan::default();
        match language {
            "rust" => {
                globs.all.push("src/**/*.rs".into());
                if rel.iter().any(|f| f.starts_with("tests/")) {
                    globs.tests.push("tests/**/*.rs".into());
                }
            }
            "go" => {
                globs.all.push("**/*.go".into());
                if rel.iter().any(|f| f.ends_with("_test.go")) {
                    globs.tests.push("**/*_test.go".into());
                }
            }
            _ => {
                let tops: BTreeSet<&str> = rel
                    .iter()
                    .filter_map(|f| f.split_once('/').map(|(d, _)| d))
                    .collect();
                for top in tops {
                    let pattern = format!("{top}/**/*.py");
                    if is_test_dir(top) {
                        globs.tests.push(pattern);
                    } else {
                        globs.all.push(pattern);
                    }
                }
                if rel.iter().any(|f| !f.contains('/')) {
                    globs.all.push("*.py".into());
                }
            }
        }
        let count = rel
            .iter()
            .filter(|f| {
                !is_test_file(f)
                    && globs
                        .all
                        .iter()
                        .any(|g| crate::drift::glob(g.as_bytes(), f.as_bytes()))
            })
            .count();
        globs.user_visible = globs.all.clone();
        out.push(DetectedPlatform {
            name: language.into(),
            kind: "cli",
            root: if root.is_empty() {
                ".".into()
            } else {
                root.to_owned()
            },
            markers: vec![join(root, marker)],
            globs,
            user_visible_files: count,
        });
    }
    let single = out.len() == 1;
    for platform in &mut out {
        platform.name = if single {
            "cli".into()
        } else {
            format!("cli-{}", platform.name)
        };
    }
    out
}

/// Platforms found in a repository, in name order.
pub fn detect(repo: &Path) -> Result<Vec<DetectedPlatform>> {
    let mut files = BTreeSet::new();
    let mut bundles = BTreeSet::new();
    walk_with(repo, "", &mut files, &mut bundles)?;
    let mut platforms: Vec<DetectedPlatform> = detect_apple(repo, &files, &bundles)
        .into_iter()
        .chain(detect_android(repo, &files))
        .chain(detect_web(&files))
        .chain(detect_cli(&files))
        .collect();
    platforms.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(platforms)
}

/// A release-tag regex from existing tags: the most common text before the version.
pub fn guess_tag_pattern(tags: &[String]) -> Option<(String, usize)> {
    let version = regex::Regex::new(r"\d+\.\d+(?:\.\d+)?").ok()?;
    let mut prefixes: BTreeMap<String, usize> = BTreeMap::new();
    for tag in tags {
        if let Some(m) = version.find(tag) {
            *prefixes.entry(tag[..m.start()].to_owned()).or_default() += 1;
        }
    }
    let total = prefixes.values().sum();
    let (prefix, _) = prefixes.into_iter().max_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| b.0.len().cmp(&a.0.len()))
            .then_with(|| b.0.cmp(&a.0))
    })?;
    Some((
        format!(
            r"^{}(\d+\.\d+(?:\.\d+)?(?:-[0-9A-Za-z.]+)?)$",
            regex::escape(&prefix)
        ),
        total,
    ))
}

fn git_tags(repo: &Path) -> Vec<String> {
    let run = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    // Tags belong to the whole Git repository; only read them when `repo` is its top level.
    let top = run(&["rev-parse", "--show-toplevel"])
        .and_then(|t| PathBuf::from(t.trim()).canonicalize().ok());
    if top.is_none() || top != repo.canonicalize().ok() {
        return Vec::new();
    }
    run(&["tag", "--list"])
        .map(|t| t.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn toml_list(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|i| toml_string(i))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
fn label(platform: &str) -> String {
    match platform {
        "ios" => "iOS".into(),
        "macos" => "macOS".into(),
        "apple" => "Apple".into(),
        "android" => "Android".into(),
        "web" => "Web".into(),
        "cli" => "CLI".into(),
        "cli-rust" => "CLI (Rust)".into(),
        "cli-go" => "CLI (Go)".into(),
        "cli-python" => "CLI (Python)".into(),
        other => other.into(),
    }
}
fn title_case(app: &str) -> String {
    let mut chars = app.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Baseline (most user-visible files) and target (next).
fn baseline_target(
    platforms: &[DetectedPlatform],
) -> (Option<&DetectedPlatform>, Option<&DetectedPlatform>) {
    let mut ranked: Vec<&DetectedPlatform> = platforms.iter().collect();
    ranked.sort_by(|a, b| {
        b.user_visible_files
            .cmp(&a.user_visible_files)
            .then_with(|| a.name.cmp(&b.name))
    });
    (ranked.first().copied(), ranked.get(1).copied())
}

fn catalog_toml(
    app: &str,
    repo_root: &str,
    platforms: &[DetectedPlatform],
    tag_pattern: Option<&str>,
) -> String {
    let mut out = vec![
        format!("# Concordance catalog for {app}, written by `concordance init`."),
        "# Format: https://github.com/luff-llc/concordance/blob/main/docs/format.md".into(),
        format!("repo_root = {}", toml_string(repo_root)),
        String::new(),
        "# Platform roots, relative to repo_root.".into(),
        "[platforms]".into(),
    ];
    for p in platforms {
        out.push(format!("{} = {}", p.name, toml_string(&p.root)));
    }
    out.extend([
        String::new(),
        "# Relative to each platform root. `all` feeds drift; `user_visible` and `strings` feed gate".into(),
        "# and inventory; `tests` feeds inventory.".into(),
    ]);
    for p in platforms {
        out.push(format!("[source_globs.{}]", p.name));
        for (key, items) in [
            ("all", &p.globs.all),
            ("user_visible", &p.globs.user_visible),
            ("strings", &p.globs.strings),
            ("tests", &p.globs.tests),
        ] {
            out.push(format!("{key} = {}", toml_list(items)));
        }
        out.push(String::new());
    }
    let (baseline, target) = baseline_target(platforms);
    out.push("[report]".into());
    out.push(format!(
        "title = {}",
        toml_string(&format!("{} native parity", title_case(app)))
    ));
    if let Some(b) = baseline {
        out.push(format!(
            "baseline = {}  # most user-visible files ({})",
            toml_string(&b.name),
            b.user_visible_files
        ));
    }
    match target {
        Some(t) => out.push(format!("target = {}", toml_string(&t.name))),
        None => out.push("# target = \"<second platform>\"".into()),
    }
    out.extend([String::new(), "[report.labels]".into()]);
    for p in platforms {
        out.push(format!("{} = {}", p.name, toml_string(&label(&p.name))));
    }
    out.extend([
        String::new(),
        "# Git tags that mark releases; `concordance log scan-git` records matching tags as released.".into(),
    ]);
    match tag_pattern {
        Some(pattern) => {
            out.push("[tracker]".into());
            out.push(format!("tag_pattern = {}", toml_string(pattern)));
        }
        None => {
            out.push("# [tracker]".into());
            out.push("# tag_pattern = \"^v(\\\\d+\\\\.\\\\d+\\\\.\\\\d+)$\"".into());
        }
    }
    out.join("\n") + "\n"
}

/// Parse catalog.toml text into `Config` plus its `[tracker] tag_pattern` (a string or a
/// list). `[tracker]` is read here from the raw TOML so this module does not depend on how
/// the tracker log types it.
pub fn parse_config(text: &str) -> Result<(Config, Vec<String>)> {
    let mut table: toml::Table = toml::from_str(text)?;
    let mut patterns = Vec::new();
    if let Some(toml::Value::Table(tracker)) = table.remove("tracker") {
        match tracker.get("tag_pattern") {
            Some(toml::Value::String(one)) => patterns.push(one.clone()),
            Some(toml::Value::Array(many)) => {
                patterns.extend(many.iter().filter_map(|v| v.as_str().map(str::to_owned)))
            }
            _ => {}
        }
    }
    Ok((table.try_into()?, patterns))
}

/// The line for the repository's own agent guide (printed, never written).
pub fn pointer_line(dir: &str) -> String {
    format!("Product state is kept by Concordance; run `concordance guide {dir}`.")
}

fn relative_inside(repo: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(repo).map_err(|_| {
        format!(
            "{} is not inside the repository {}",
            path.display(),
            repo.display()
        )
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str().ok_or("non-UTF8 path")?.to_owned()),
            Component::CurDir => {}
            _ => return fail(format!("unsupported catalog path: {}", path.display())),
        }
    }
    Ok(parts.join("/"))
}

/// Scan the repository and plan the catalog directory, without writing.
pub fn plan_init(options: &InitOptions) -> Result<InitPlan> {
    let repo = options
        .repo
        .canonicalize()
        .map_err(|e| format!("{}: {e}", options.repo.display()))?;
    let app = match &options.app {
        Some(app) => app.clone(),
        None => slug(repo.file_name().and_then(|n| n.to_str()).unwrap_or("app")),
    };
    if app.is_empty() {
        return fail("--app must not be empty");
    }
    let out_path = match &options.out {
        Some(out) if out.is_absolute() => out.clone(),
        Some(out) => repo.join(out),
        None => repo.join("spec").join(&app),
    };
    // Resolve `..` lexically; the directory may not exist yet.
    let mut normal = PathBuf::new();
    for component in out_path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    let out = relative_inside(&repo, &normal)?;
    let depth = out.split('/').filter(|p| !p.is_empty()).count();
    let repo_root = if depth == 0 {
        ".".to_owned()
    } else {
        vec![".."; depth].join("/")
    };
    let platforms = detect(&repo)?;
    let tags = git_tags(&repo);
    let guessed = guess_tag_pattern(&tags);
    let (tag_pattern, version_tags) = match guessed {
        Some((pattern, count)) => (Some(pattern), count),
        None => (None, 0),
    };
    let display = |name: &str| join(&out, name);
    let config_path = normal.join("catalog.toml");
    let adopted = config_path.exists();
    let toml_text = catalog_toml(&app, &repo_root, &platforms, tag_pattern.as_deref());
    let (config, patterns) = parse_config(&toml_text)?;
    let dir_display = if out.is_empty() {
        ".".to_owned()
    } else {
        out.clone()
    };
    let kit = guide_text(&app, &dir_display, &config, &patterns);
    let readme = format!(
        "Catalog area files (`<area>.yaml`) go here. Start with `concordance inventory {dir_display} --uncatalogued` and `concordance plan-audit {dir_display}`; run `concordance validate {dir_display}` after each edit.\n"
    );
    let mut files = Vec::new();
    for (name, content) in [
        ("catalog.toml", Some(toml_text)),
        ("catalog", None),
        ("catalog/README.md", Some(readme)),
        ("tracker", None),
        ("AGENTS.md", Some(kit)),
    ] {
        let absolute = normal.join(name);
        files.push(PlannedFile {
            path: display(name),
            exists: absolute.exists(),
            absolute,
            content,
        });
    }
    let mut notes = Vec::new();
    if adopted {
        match fs::read_to_string(&config_path)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_config(&t).map_err(|e| e.to_string()))
        {
            Ok((existing, existing_patterns)) => {
                for p in &platforms {
                    match existing.platforms.get(&p.name) {
                        None => notes.push(format!(
                            "platform `{}` (root `{}`): add `{} = {}` under [platforms] and its [source_globs.{}]",
                            p.name,
                            p.root,
                            p.name,
                            toml_string(&p.root),
                            p.name
                        )),
                        Some(c) if !existing.source_globs.contains_key(&p.name) => notes.push(format!(
                            "[source_globs.{}] for `{}`: all = {}, user_visible = {}, strings = {}, tests = {}",
                            p.name,
                            c.root(),
                            toml_list(&p.globs.all),
                            toml_list(&p.globs.user_visible),
                            toml_list(&p.globs.strings),
                            toml_list(&p.globs.tests)
                        )),
                        Some(_) => {}
                    }
                }
                if existing_patterns.is_empty() {
                    if let Some(pattern) = &tag_pattern {
                        notes.push(format!("[tracker] tag_pattern = {}", toml_string(pattern)));
                    }
                }
            }
            Err(error) => notes.push(format!("existing catalog.toml does not parse: {error}")),
        }
        for f in files
            .iter()
            .filter(|f| !f.exists && !f.path.ends_with("catalog.toml"))
        {
            notes.push(format!(
                "{}{}",
                f.path,
                if f.content.is_none() { "/" } else { "" }
            ));
        }
    }
    Ok(InitPlan {
        app,
        out: dir_display,
        repo_root,
        platforms,
        tag_pattern,
        version_tags,
        files,
        adopted,
        notes,
    })
}

impl InitPlan {
    /// Human-readable plan; `written` says whether files were (or will be) created.
    pub fn render(&self, dry_run: bool) -> String {
        let mut out = vec![
            format!("Concordance init: {}", self.app),
            format!(
                "Catalog directory: {} (repo_root = {})",
                self.out,
                toml_string(&self.repo_root)
            ),
            String::new(),
            "Platforms:".into(),
        ];
        if self.platforms.is_empty() {
            out.push("- none detected (add [platforms] by hand)".into());
        }
        for p in &self.platforms {
            let mut markers: Vec<String> = p.markers.iter().take(4).cloned().collect();
            if p.markers.len() > 4 {
                markers.push(format!("and {} more", p.markers.len() - 4));
            }
            out.push(format!(
                "- {}: `{}` ({}; {} user-visible files; markers: {})",
                p.name,
                p.root,
                p.kind,
                p.user_visible_files,
                markers.join(", ")
            ));
        }
        let (baseline, target) = baseline_target(&self.platforms);
        if let Some(b) = baseline {
            out.push(format!(
                "Report: baseline {}{}.",
                b.name,
                target
                    .map(|t| format!(", target {}", t.name))
                    .unwrap_or_default()
            ));
        }
        out.push(match &self.tag_pattern {
            Some(pattern) => format!(
                "Releases: tag_pattern {} from {} version tags.",
                toml_string(pattern),
                self.version_tags
            ),
            None => {
                "Releases: no version tags found; [tracker] tag_pattern is a commented example."
                    .into()
            }
        });
        out.extend([String::new(), "Files:".into()]);
        if self.adopted {
            out.push(format!(
                "Already adopted ({}/catalog.toml exists); nothing is written.",
                self.out
            ));
            if self.notes.is_empty() {
                out.push("Nothing to add.".into());
            } else {
                out.push("Would add:".into());
                out.extend(self.notes.iter().map(|n| format!("- {n}")));
            }
        } else {
            for f in &self.files {
                let slash = if f.content.is_none() { "/" } else { "" };
                out.push(if f.exists {
                    format!("- keep {}{slash} (exists)", f.path)
                } else if dry_run {
                    format!("- would create {}{slash}", f.path)
                } else {
                    format!("- create {}{slash}", f.path)
                });
            }
        }
        if dry_run && !self.adopted {
            if let Some(toml) = self.files.first().and_then(|f| f.content.as_ref()) {
                out.extend([String::new(), "catalog.toml:".into(), String::new()]);
                out.extend(
                    toml.lines()
                        .map(|l| format!("    {l}").trim_end().to_owned()),
                );
            }
            out.extend([String::new(), "Dry run: nothing written.".into()]);
        }
        out.extend([
            String::new(),
            "Add this line to the repository's own agent guide (init never writes it):".into(),
            String::new(),
            format!("    {}", pointer_line(&self.out)),
        ]);
        if !self.adopted {
            out.extend([
                String::new(),
                format!(
                    "Next: concordance inventory {0} --uncatalogued, then concordance plan-audit {0}.",
                    self.out
                ),
            ]);
        }
        out.join("\n") + "\n"
    }

    /// Create every planned file that does not exist; never overwrites. Returns the
    /// created paths. Does nothing on an adopted repository.
    pub fn apply(&self) -> Result<Vec<String>> {
        let mut created = Vec::new();
        if self.adopted {
            return Ok(created);
        }
        for f in &self.files {
            match &f.content {
                None => {
                    if !f.absolute.exists() {
                        fs::create_dir_all(&f.absolute)?;
                        created.push(f.path.clone());
                    }
                }
                Some(content) => {
                    if let Some(parent) = f.absolute.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&f.absolute)
                    {
                        Ok(mut file) => {
                            file.write_all(content.as_bytes())?;
                            created.push(f.path.clone());
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }
        Ok(created)
    }
}

/// The agent kit for a catalog, printed by `guide` and written by `init` as AGENTS.md.
/// `dir` is the catalog directory as agents should type it (from the repository root).
pub fn guide_text(app: &str, dir: &str, config: &Config, tag_patterns: &[String]) -> String {
    let report = &config.report;
    let mut platforms = Vec::new();
    for (name, platform) in &config.platforms {
        let mut line = format!(
            "- `{name}` ({}): root `{}`",
            report.label(name),
            platform.root()
        );
        if let Some(parent) = platform.inherits() {
            line.push_str(&format!(", inherits `{parent}`"));
        }
        if config.external_roots.contains_key(name) {
            line.push_str(&format!(
                "; external checkout, pass `--root {name}=<checkout>` (or CONCORDANCE_ROOT_{})",
                name.to_uppercase().replace('-', "_")
            ));
        }
        platforms.push(line);
    }
    let has = |p: &str| config.platforms.contains_key(p);
    let baseline = if has(&report.baseline) {
        report.baseline.as_str()
    } else {
        "<baseline>"
    };
    let target = if has(&report.target) {
        report.target.as_str()
    } else {
        "<platform>"
    };
    let tags = match tag_patterns {
        [] => "No release tag pattern is configured; set `[tracker] tag_pattern` in catalog.toml so `concordance log scan-git` can record tagged releases.".to_owned(),
        patterns => format!(
            "Release tags match {} (`[tracker] tag_pattern`); `concordance log scan-git` records each matching tag as `released`.",
            patterns.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(" or ")
        ),
    };
    format!(
        r#"# Concordance agent kit: {app}

Product state for {app} is kept by Concordance in `{dir}`. Paths here are relative to the
directory `concordance guide` ran in (for the copy `init` writes, the repository root). Plans are written, facts are recorded, status is derived: no command sets a
status, and nobody edits a status that a recorded fact can establish. This kit is printed by
`concordance guide {dir}`; when this copy and that output differ, the output wins.

## Layout

- `{dir}/catalog.toml`: platforms, source globs, report and tracker settings.
- `{dir}/catalog/<area>.yaml`: contract entries (what a feature does, its states, copy,
  identifiers, acceptance, and each platform's status and evidence).
- `{dir}/tracker/releases.yaml`: the release plan (written by people).
- `{dir}/tracker/log.jsonl`: facts, append-only, written by `concordance log`. Never edit it.

Platforms (baseline `{baseline}`, target `{target}`):

{platforms}

## Before work

    concordance next {dir} --platform {target} --limit 10 --details
    concordance entry {dir} <ENTRY-ID> --format brief --details

Read the brief before touching code. Keep the entry's accessibility identifiers and copy
exactly; tests on every platform find elements by those identifiers.

## Catalog entries

- Entry format: https://github.com/luff-llc/concordance/blob/main/docs/format.md. IDs are
  `area.verb-object`, stable, never reused (retire them in `retired-ids.json`).
- Cite evidence relative to the platform root, anchored where possible (`path#Symbol`).
- After any catalog edit: `concordance validate {dir}`.
- What nobody has catalogued yet: `concordance inventory {dir} --uncatalogued`.
- One audit brief per candidate area: `concordance plan-audit {dir}` (add
  `--target {target}` for a parity audit against `{baseline}`).
- Change a platform record with `concordance set {dir} <ENTRY-ID> --platform <p> --status ...`;
  never write `verification` for your own work.

## Facts

Each command appends one event to `tracker/log.jsonl`; repeating a fact is a no-op. `log`
finds the catalog from `--catalog {dir}`, else `$CONCORDANCE_CATALOG`, else `./catalog.toml`,
else the only `spec/*/catalog.toml`; the commands below pass it explicitly.

    concordance log --catalog {dir} report bug|request "Title" [--entry ID]... [--platform P]... [--impact I] [--source S]
    concordance log --catalog {dir} fixed ISSUE --commit SHA [--by WHO]
    concordance log --catalog {dir} verified ISSUE --evidence TEXT... --by WHO
    concordance log --catalog {dir} reopened ISSUE --reason TEXT
    concordance log --catalog {dir} uploaded RELEASE --platform P --build B [--commit SHA]
    concordance log --catalog {dir} released RELEASE [--platform P] [--date D] [--commit SHA]
    concordance log --catalog {dir} pulled RELEASE --reason TEXT
    concordance log --catalog {dir} scan-git [--since REF]

- When a person reports something: `concordance log --catalog {dir} report ...`.
- When committing a fix, add a trailer; a test runner or reviewer adds `Verifies:` (or runs
  `log verified`). Verification must come from someone other than the fixer.

      Fixes: bug.<slug>
      Verifies: bug.<slug> (<evidence, such as a test name and result>)
      Reports: request.<slug>

- When releasing, release tooling runs `log uploaded` and `log released`.
- {tags}

## Status

- `concordance track {dir}`: releases, open bugs and requests, unverified fixes and gaps.
- An issue is open once reported, fixed when a fix commit is recorded, verified when someone
  other than the fixer records a verification, and shipped when a released build contains
  the fix commit. A catalog record is `implemented` by claim and verified only through an
  independent verification record.
"#,
        platforms = platforms.join("\n"),
    )
}

impl Catalog {
    /// The agent kit for this catalog; `dir` is how agents should type its directory.
    pub fn guide(&self, dir: &str) -> String {
        let patterns = fs::read_to_string(self.directory.join("catalog.toml"))
            .ok()
            .and_then(|t| parse_config(&t).ok())
            .map(|(_, p)| p)
            .unwrap_or_default();
        guide_text(
            &crate::inventory::app_name(self),
            dir,
            &self.config,
            &patterns,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tag_patterns() {
        let tags: Vec<String> = ["ios/v0.1.0-rc.1", "ios/v0.3.0", "random"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let (pattern, count) = guess_tag_pattern(&tags).expect("pattern");
        assert_eq!(pattern, r"^ios/v(\d+\.\d+(?:\.\d+)?(?:-[0-9A-Za-z.]+)?)$");
        assert_eq!(count, 2);
        let re = regex::Regex::new(&pattern).expect("valid");
        assert!(re.is_match("ios/v1.2.0"));
        assert!(guess_tag_pattern(&["main".to_owned()]).is_none());
    }
    #[test]
    fn toml_strings_escape() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
