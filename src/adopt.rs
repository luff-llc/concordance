//! `concordance init` and `concordance guide`: bring Concordance into a repository and print
//! the agent kit. See docs/adopt.md.
use crate::inventory::{is_test_file, slug, walk, WalkExtras};
use crate::{fail, Catalog, Config, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// What `init` scans and where it writes.
#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    pub repo: PathBuf,
    /// Default: the repository directory's name.
    pub app: Option<String>,
    /// Catalog directory, relative to the repository. Default: `spec/<app>`.
    pub out: Option<PathBuf>,
    /// Subtrees (relative to the repository) to search for markers and sources; empty means
    /// the whole repository. Marker files in their parent directories still count.
    pub scopes: Vec<PathBuf>,
}

/// One app found in the repository: an Apple application target or an Android
/// application module. Several of them suggest scoping `init`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppCandidate {
    /// Grouping key: the slugged name without platform suffixes (`TaupeWatch` → `taupe`).
    pub key: String,
    pub kind: &'static str,
    pub name: String,
    /// The project file or build file that declares it.
    pub declared_in: String,
    /// Source directories to pass as `--scope` (may be empty when none is found by name).
    pub scopes: Vec<String>,
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
    /// The `--scope` subtrees, relative to the repository.
    pub scopes: Vec<String>,
    /// Apps found when no scope was given (listed when there is more than one).
    pub candidates: Vec<AppCandidate>,
    repo: PathBuf,
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
    // String catalogs, scoped to the top-level folders that hold them.
    for (matches, tail) in [
        (
            &(|f: &str| f.ends_with(".xcstrings")) as &dyn Fn(&str) -> bool,
            "*.xcstrings",
        ),
        (
            &|f: &str| file_name(f) == "Localizable.strings",
            "Localizable.strings",
        ),
    ] {
        let tops: BTreeSet<&str> = rel
            .iter()
            .filter(|f| matches(f))
            .map(|f| f.split_once('/').map(|(d, _)| d).unwrap_or(""))
            .collect();
        for top in tops {
            globs.strings.push(if top.is_empty() {
                tail.to_owned()
            } else {
                format!("{top}/**/{tail}")
            });
        }
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

/// A Cargo member pattern as a `/`-separated relative glob: no leading `./`, no trailing
/// `/`, no empty or `.` components (`./crates//*/` → `crates/*`).
fn normalize_member(pattern: &str) -> String {
    pattern
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Member directories of a Cargo workspace manifest (relative to its directory), matched
/// against the crates found; `None` when the manifest has no `[workspace] members`.
fn cargo_members(repo: &Path, root: &str, files: &BTreeSet<String>) -> Option<Vec<String>> {
    let text = fs::read_to_string(repo.join(join(root, "Cargo.toml"))).ok()?;
    let table: toml::Table = text.parse().ok()?;
    let workspace = table.get("workspace")?.as_table()?;
    let list = |key: &str| -> Vec<String> {
        workspace
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(normalize_member)
                    .collect()
            })
            .unwrap_or_default()
    };
    let members = list("members");
    let exclude = list("exclude");
    if members.is_empty() {
        return None;
    }
    let crates: BTreeSet<&str> = files
        .iter()
        .filter(|f| file_name(f) == "Cargo.toml" && inside(root, f) && parent(f) != root)
        .map(|f| under(root, parent(f)))
        .collect();
    let hit = |patterns: &[String], dir: &str| {
        patterns
            .iter()
            .any(|p| crate::drift::glob(p.as_bytes(), dir.as_bytes()))
    };
    Some(
        crates
            .into_iter()
            .filter(|d| hit(&members, d) && !hit(&exclude, d))
            .map(str::to_owned)
            .collect(),
    )
}

fn detect_cli(repo: &Path, files: &BTreeSet<String>) -> Vec<DetectedPlatform> {
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
                // A workspace lists its member crates; a virtual workspace has no root `src/`.
                let mut crates = vec![String::new()];
                if let Some(members) = cargo_members(repo, root, files) {
                    if !rel.iter().any(|f| f.starts_with("src/")) {
                        crates.clear();
                    }
                    crates.extend(members);
                }
                for krate in &crates {
                    if rel
                        .iter()
                        .any(|f| inside(krate, f) && under(krate, f).starts_with("src/"))
                    {
                        globs.all.push(join(krate, "src/**/*.rs"));
                    }
                    if rel
                        .iter()
                        .any(|f| inside(krate, f) && under(krate, f).starts_with("tests/"))
                    {
                        globs.tests.push(join(krate, "tests/**/*.rs"));
                    }
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

/// Marker files looked for in the parent directories of a scope.
const MARKERS: &[&str] = &[
    "Package.swift",
    "project.yml",
    "settings.gradle",
    "settings.gradle.kts",
    "build.gradle",
    "build.gradle.kts",
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
];

/// Files (and Xcode bundles) of the repository, or of the scopes plus the marker files in
/// each scope's parent directories.
fn scan(repo: &Path, scopes: &[String]) -> Result<(BTreeSet<String>, WalkExtras)> {
    let mut files = BTreeSet::new();
    let mut extras = WalkExtras::default();
    if scopes.is_empty() {
        walk(repo, "", &mut files, &mut extras)?;
        return Ok((files, extras));
    }
    for scope in scopes {
        walk(repo, scope, &mut files, &mut extras)?;
        let mut dir = parent(scope).to_owned();
        loop {
            let path = if dir.is_empty() {
                repo.to_path_buf()
            } else {
                repo.join(&dir)
            };
            if let Ok(entries) = fs::read_dir(&path) {
                for entry in entries.flatten() {
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    if kind.is_file() && MARKERS.contains(&name.as_str()) {
                        files.insert(join(&dir, &name));
                    } else if kind.is_dir() && name.ends_with(".xcodeproj") {
                        extras.bundles.insert(join(&dir, &name));
                    }
                }
            }
            if dir.is_empty() {
                break;
            }
            dir = parent(&dir).to_owned();
        }
        // A scope inside an Android module's source set: the nearest module above it keeps
        // its manifest in `src/main`.
        let mut dir = scope.clone();
        loop {
            let has_gradle = ["build.gradle", "build.gradle.kts"]
                .iter()
                .any(|g| fs::symlink_metadata(repo.join(join(&dir, g))).is_ok_and(|m| m.is_file()));
            if has_gradle {
                let manifest = join(&dir, "src/main/AndroidManifest.xml");
                if fs::symlink_metadata(repo.join(&manifest)).is_ok_and(|m| m.is_file()) {
                    files.insert(manifest);
                }
                break;
            }
            if dir.is_empty() {
                break;
            }
            dir = parent(&dir).to_owned();
        }
    }
    Ok((files, extras))
}

/// Platforms found in a repository (or in its scopes), in name order.
pub fn detect(repo: &Path, scopes: &[String]) -> Result<Vec<DetectedPlatform>> {
    let (files, extras) = scan(repo, scopes)?;
    detect_in(repo, &files, &extras.bundles, scopes)
}

fn detect_in(
    repo: &Path,
    files: &BTreeSet<String>,
    bundles: &BTreeSet<String>,
    scopes: &[String],
) -> Result<Vec<DetectedPlatform>> {
    let mut platforms: Vec<DetectedPlatform> = detect_apple(repo, files, bundles)
        .into_iter()
        .chain(detect_android(repo, files))
        .chain(detect_web(files))
        .chain(detect_cli(repo, files))
        .collect();
    for platform in &mut platforms {
        narrow_to_scopes(platform, scopes, files);
    }
    platforms.retain(|p| !p.globs.all.is_empty() || !p.globs.user_visible.is_empty());
    for platform in &mut platforms {
        platform.user_visible_files = inventory_count(repo, platform)?;
    }
    platforms.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(platforms)
}

fn platform_dir(platform: &DetectedPlatform) -> &str {
    if platform.root == "." {
        ""
    } else {
        &platform.root
    }
}

/// Make every glob no broader than the scopes: a glob whose fixed prefix lies inside a scope
/// is kept; a scope inside a glob's fixed prefix narrows it to `<scope>/**/<pattern>`.
/// Globs that match no scanned file are dropped.
fn narrow_to_scopes(platform: &mut DetectedPlatform, scopes: &[String], files: &BTreeSet<String>) {
    let root = platform_dir(platform).to_owned();
    let mut relative = Vec::new();
    for scope in scopes {
        if scope == &root || inside(scope, &root) {
            return; // a scope at or above the root covers all of it
        }
        if inside(&root, scope) {
            relative.push(under(&root, scope).to_owned());
        }
    }
    if relative.is_empty() {
        return;
    }
    let rel_files: Vec<&str> = files
        .iter()
        .filter(|f| inside(&root, f))
        .map(|f| under(&root, f))
        .collect();
    let any_match = |g: &str| {
        rel_files
            .iter()
            .any(|f| crate::drift::glob(g.as_bytes(), f.as_bytes()))
    };
    let globs = &mut platform.globs;
    for list in [
        &mut globs.all,
        &mut globs.user_visible,
        &mut globs.strings,
        &mut globs.tests,
    ] {
        let mut narrowed: Vec<String> = Vec::new();
        for pattern in list.iter() {
            let prefix = crate::inventory::static_prefix(pattern);
            for scope in &relative {
                let candidate =
                    if prefix == *scope || (inside(scope, &prefix) && !prefix.is_empty()) {
                        pattern.clone()
                    } else if inside(&prefix, scope) {
                        let tail = under(&prefix, pattern);
                        let mut candidates = match tail.strip_prefix("**/") {
                            Some(rest) => {
                                let mut c = vec![format!("{scope}/**/{rest}")];
                                c.extend(anchored(scope, rest));
                                c
                            }
                            None => vec![format!("{scope}/**/{}", file_name(pattern))],
                        };
                        candidates.retain(|c| any_match(c));
                        for c in candidates {
                            if !narrowed.contains(&c) {
                                narrowed.push(c);
                            }
                        }
                        continue;
                    } else {
                        continue;
                    };
                if any_match(&candidate) && !narrowed.contains(&candidate) {
                    narrowed.push(candidate);
                }
            }
        }
        *list = narrowed;
    }
}

/// For a `**/<literals>/<remainder>` glob, anchor at a scope that already contains the
/// literal segments: scope `src/i18n` with `i18n/**/*.json` gives `src/i18n/**/*.json`.
/// When the literals end before the scope does, the remainder must start with `**/`.
fn anchored(scope: &str, rest: &str) -> Option<String> {
    let parts: Vec<&str> = rest.split('/').collect();
    let literals: Vec<&str> = parts
        .iter()
        .take(parts.len().saturating_sub(1))
        .take_while(|p| !p.contains(['*', '?']))
        .copied()
        .collect();
    if literals.is_empty() {
        return None;
    }
    let remainder = parts[literals.len()..].join("/");
    let components: Vec<&str> = scope.split('/').collect();
    let end = (literals.len()..=components.len())
        .rev()
        .find(|&end| components[end - literals.len()..end] == literals[..])?;
    if end == components.len() || remainder.starts_with("**/") {
        Some(format!("{scope}/{remainder}"))
    } else {
        None
    }
}

/// User-visible source files exactly as `inventory` counts them: matching `user_visible`
/// (else `all`), not a string catalog, not a test.
fn inventory_count(repo: &Path, platform: &DetectedPlatform) -> Result<usize> {
    let g = &platform.globs;
    crate::inventory::count_sources(
        &repo.join(platform_dir(platform)),
        &g.all,
        &g.user_visible,
        &g.strings,
        &g.tests,
    )
}

fn candidate_key(name: &str) -> String {
    let mut key = slug(name);
    loop {
        let before = key.clone();
        for suffix in [
            "-app", "-android", "-ios", "-macos", "-mac", "-watch", "-vision", "-tv",
        ] {
            if let Some(stripped) = key.strip_suffix(suffix) {
                if !stripped.is_empty() {
                    key = stripped.to_owned();
                }
            }
        }
        if key == before {
            return key;
        }
    }
}

/// `(name, productType)` of every native target in a `project.pbxproj` (quoted or not).
pub fn pbxproj_targets(text: &str) -> Vec<(String, String)> {
    let isa = regex::Regex::new(r#""?isa"?\s*=\s*"?PBXNativeTarget"?\s*;"#).ok();
    let next_isa = regex::Regex::new(r#""?isa"?\s*=\s*"?[A-Za-z]+"?\s*;"#).ok();
    let name = regex::Regex::new(r#"(?m)^\s*"?name"?\s*=\s*"?([^";]+)"?\s*;"#).ok();
    let product = regex::Regex::new(r#""?productType"?\s*=\s*"([^"]+)""#).ok();
    let (Some(isa), Some(next_isa), Some(name), Some(product)) = (isa, next_isa, name, product)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for m in isa.find_iter(text) {
        let rest = &text[m.end()..];
        let block = match next_isa.find(rest) {
            Some(next) => &rest[..next.start()],
            None => rest,
        };
        if let (Some(n), Some(p)) = (name.captures(block), product.captures(block)) {
            if let (Some(n), Some(p)) = (n.get(1), p.get(1)) {
                out.push((n.as_str().trim().to_owned(), p.as_str().to_owned()));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Application targets in a `project.yml` (XcodeGen): top-level target keys whose block
/// has `type: application`.
fn project_yml_apps(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_targets = false;
    let mut current: Option<String> = None;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.trim().is_empty() {
            in_targets = line.trim_end() == "targets:";
            current = None;
            continue;
        }
        if !in_targets {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();
        if indent == 2 && trimmed.ends_with(':') {
            current = Some(trimmed.trim_end_matches(':').trim_matches('"').to_owned());
        } else if trimmed == "type: application" {
            if let Some(name) = current.take() {
                out.push(name);
            }
        }
    }
    out
}

/// `rootProject.name = "…"` in a Gradle settings file.
fn root_project_name(text: &str) -> Option<String> {
    let re = regex::Regex::new(r#"rootProject\.name\s*=\s*["']([^"']+)["']"#).ok()?;
    re.captures(text)?.get(1).map(|m| m.as_str().to_owned())
}

/// Apple application targets and Android application modules.
fn app_candidates(
    repo: &Path,
    files: &BTreeSet<String>,
    bundles: &BTreeSet<String>,
) -> Vec<AppCandidate> {
    // Directories that hold Swift sources, by their last component.
    let mut source_dirs: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for file in files.iter().filter(|f| extension(f) == "swift") {
        let mut dir = parent(file);
        while !dir.is_empty() {
            source_dirs
                .entry(file_name(dir))
                .or_default()
                .push(dir.to_owned());
            dir = parent(dir);
        }
    }
    let dir_named = |name: &str| -> Option<String> {
        source_dirs
            .get(name)
            .and_then(|dirs| dirs.iter().min_by_key(|d| (d.len(), d.as_str())).cloned())
    };
    let mut out = Vec::new();
    for bundle in bundles {
        let first = out.len();
        let text =
            fs::read_to_string(repo.join(bundle).join("project.pbxproj")).unwrap_or_default();
        let targets = pbxproj_targets(&text);
        for (name, product) in &targets {
            if !product.starts_with("com.apple.product-type.application") {
                continue;
            }
            let mut scopes: Vec<String> = dir_named(name).into_iter().collect();
            for (other, kind) in &targets {
                if other != name && other.starts_with(name.as_str()) && kind.contains("extension") {
                    scopes.extend(dir_named(other));
                }
            }
            scopes.sort();
            scopes.dedup();
            out.push(AppCandidate {
                key: candidate_key(name),
                kind: "apple",
                name: name.clone(),
                declared_in: bundle.clone(),
                scopes,
            });
        }
        // A project holding one app: its folder is the scope for targets without a folder
        // of their own name.
        let keys: BTreeSet<&str> = out[first..].iter().map(|c| c.key.as_str()).collect();
        if keys.len() == 1 && !parent(bundle).is_empty() {
            let folder = parent(bundle).to_owned();
            for c in &mut out[first..] {
                if c.scopes.is_empty() {
                    c.scopes.push(folder.clone());
                }
            }
        }
    }
    for yml in files.iter().filter(|f| file_name(f) == "project.yml") {
        for name in project_yml_apps(&fs::read_to_string(repo.join(yml)).unwrap_or_default()) {
            out.push(AppCandidate {
                key: candidate_key(&name),
                kind: "apple",
                scopes: dir_named(&name).into_iter().collect(),
                name,
                declared_in: yml.clone(),
            });
        }
    }
    let apply_false = regex::Regex::new(r"\bapply\s+false\b").ok();
    for gradle in files
        .iter()
        .filter(|f| matches!(file_name(f), "build.gradle" | "build.gradle.kts"))
    {
        let text = fs::read_to_string(repo.join(gradle)).unwrap_or_default();
        // `id("com.android.application") apply false` in a root build file declares the
        // plugin without applying it.
        let applies = text.lines().any(|line| {
            let line = line.trim();
            !line.starts_with("//")
                && (line.contains("com.android.application")
                    || line.contains("android.application"))
                && !apply_false.as_ref().is_some_and(|re| re.is_match(line))
        });
        if !applies {
            continue;
        }
        let module = parent(gradle);
        let name = if module.is_empty() {
            "app"
        } else {
            file_name(module)
        };
        // The conventional `app` module takes its app's name from the Gradle root project.
        let mut key = candidate_key(name);
        if key == "app" {
            let mut dir = module.to_owned();
            loop {
                let named = ["settings.gradle.kts", "settings.gradle"]
                    .iter()
                    .find_map(|f| {
                        let path = join(&dir, f);
                        files
                            .contains(&path)
                            .then(|| {
                                root_project_name(
                                    &fs::read_to_string(repo.join(&path)).unwrap_or_default(),
                                )
                            })
                            .flatten()
                    });
                if let Some(named) = named {
                    key = candidate_key(&named);
                    break;
                }
                if dir.is_empty() {
                    break;
                }
                dir = parent(&dir).to_owned();
            }
        }
        out.push(AppCandidate {
            key,
            kind: "android",
            name: name.to_owned(),
            declared_in: gradle.clone(),
            scopes: vec![if module.is_empty() {
                ".".to_owned()
            } else {
                module.to_owned()
            }],
        });
    }
    out.sort();
    out.dedup();
    out
}

/// A release-tag glob (`<prefix>{version}`, as `log scan-git` reads it) from existing tags:
/// the most common text before the version.
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
    Some((format!("{prefix}{{version}}"), total))
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
        // One platform: target it too, so readiness never falls back to the model's
        // default target.
        None => {
            if let Some(b) = baseline {
                out.push(format!(
                    "target = {}  # the only platform",
                    toml_string(&b.name)
                ));
            }
        }
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
            out.push("# tag_pattern = \"v{version}\"".into());
        }
    }
    out.join("\n") + "\n"
}

/// Parse catalog.toml text into `Config` plus its `[tracker] tag_pattern` globs.
pub fn parse_config(text: &str) -> Result<(Config, Vec<String>)> {
    let config: Config = toml::from_str(text)?;
    let patterns = config
        .tracker
        .as_ref()
        .map(|t| t.tag_pattern.patterns().to_vec())
        .unwrap_or_default();
    Ok((config, patterns))
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

/// Refuse a user-supplied path with a `..` component: resolving it lexically can name a
/// different directory than the operating system would when a component is a link.
pub(crate) fn reject_parent_components(flag: &str, path: &Path) -> Result<()> {
    if path.components().any(|c| c == Component::ParentDir) {
        return fail(format!(
            "{flag} {} must not contain `..`; give a path inside the repository instead",
            path.display()
        ));
    }
    Ok(())
}

/// An output directory inside a base directory (a repository or catalog directory) that
/// init and plan-audit write into without following symbolic links. Every write goes
/// through `base` (canonical) joined with the checked relative path.
pub(crate) struct SafeDir {
    base: PathBuf,
    relative: String,
}
impl SafeDir {
    /// Refuse `dir` (absolute, or relative to the current directory; no `..`) unless it is
    /// inside `base`, no existing component below `base` is a symbolic link, and its nearest
    /// existing ancestor resolves inside `base`.
    pub(crate) fn new(base: &Path, dir: &Path) -> Result<Self> {
        reject_parent_components("--out", dir)?;
        let base = base
            .canonicalize()
            .map_err(|e| format!("{}: {e}", base.display()))?;
        let absolute = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            std::env::current_dir()?.canonicalize()?.join(dir)
        };
        // The part of `dir` that names `base` may go through links above the base (such as
        // `/var` → `/private/var`); take the shallowest ancestor that resolves to `base`, and
        // check every component below it.
        let anchor = absolute
            .ancestors()
            .filter(|a| a.canonicalize().is_ok_and(|c| c == base))
            .last()
            .ok_or_else(|| {
                format!(
                    "{} is not inside the repository {}",
                    absolute.display(),
                    base.display()
                )
            })?;
        let relative = relative_inside(anchor, &absolute)?;
        check_no_symlinks(&base, &relative)?;
        Ok(Self { base, relative })
    }
    /// `name` inside a canonical directory, e.g. `tasks` in the catalog directory.
    pub(crate) fn child(base: &Path, name: &str) -> Result<Self> {
        let base = base
            .canonicalize()
            .map_err(|e| format!("{}: {e}", base.display()))?;
        check_no_symlinks(&base, name)?;
        Ok(Self {
            base,
            relative: name.to_owned(),
        })
    }
    /// Create the directory one component at a time.
    pub(crate) fn create(&self) -> Result<()> {
        create_dirs(&self.base, &self.relative)
    }
    /// The path of `name` in the directory, after the same checks; write through it.
    pub(crate) fn file(&self, name: &str) -> Result<PathBuf> {
        let relative = join(&self.relative, name);
        check_no_symlinks(&self.base, &relative)?;
        Ok(self.base.join(relative))
    }
}

/// Refuse a path under the canonical `repo` when an existing component is a symbolic link,
/// or when its nearest existing ancestor resolves outside the repository.
fn check_no_symlinks(repo: &Path, relative: &str) -> Result<()> {
    let mut current = repo.to_path_buf();
    for part in relative.split('/').filter(|p| !p.is_empty()) {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return fail(format!(
                    "refusing to write through the symbolic link {}",
                    current.display()
                ))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    let mut probe = repo.join(relative);
    while fs::symlink_metadata(&probe).is_err() {
        if !probe.pop() {
            break;
        }
    }
    if !probe.canonicalize()?.starts_with(repo.canonicalize()?) {
        return fail(format!(
            "{} resolves outside the repository {}",
            repo.join(relative).display(),
            repo.display()
        ));
    }
    Ok(())
}

/// Create `relative` (a directory under the canonical `repo`) one component at a time,
/// refusing symbolic links and non-directories on the way.
fn create_dirs(repo: &Path, relative: &str) -> Result<()> {
    let mut current = repo.to_path_buf();
    for part in relative.split('/').filter(|p| !p.is_empty()) {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return fail(format!(
                    "refusing to write through the symbolic link {}",
                    current.display()
                ))
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return fail(format!("{} is not a directory", current.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Err(e) = fs::create_dir(&current) {
                    if e.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(e.into());
                    }
                    let meta = fs::symlink_metadata(&current)?;
                    if meta.file_type().is_symlink() || !meta.is_dir() {
                        return fail(format!("{} changed while creating it", current.display()));
                    }
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// A repository-relative directory from a `--scope` argument.
fn scope_path(repo: &Path, scope: &Path) -> Result<String> {
    let joined = if scope.is_absolute() {
        scope.to_path_buf()
    } else {
        repo.join(scope)
    };
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    let relative = relative_inside(repo, &normal)?;
    let canonical = normal
        .canonicalize()
        .map_err(|e| format!("--scope {}: {e}", scope.display()))?;
    if !canonical.starts_with(repo) || !canonical.is_dir() {
        return fail(format!(
            "--scope {} must be a directory inside the repository",
            scope.display()
        ));
    }
    Ok(relative)
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
    if let Some(out) = &options.out {
        reject_parent_components("--out", out)?;
    }
    for scope in &options.scopes {
        reject_parent_components("--scope", scope)?;
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
    check_no_symlinks(&repo, &out)?;
    let mut scopes = Vec::new();
    for scope in &options.scopes {
        let scope = scope_path(&repo, scope)?;
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    scopes.sort();
    let depth = out.split('/').filter(|p| !p.is_empty()).count();
    let repo_root = if depth == 0 {
        ".".to_owned()
    } else {
        vec![".."; depth].join("/")
    };
    let (scanned, extras) = scan(&repo, &scopes)?;
    let platforms = detect_in(&repo, &scanned, &extras.bundles, &scopes)?;
    let candidates = if scopes.is_empty() {
        app_candidates(&repo, &scanned, &extras.bundles)
    } else {
        Vec::new()
    };
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
        check_no_symlinks(&repo, &join(&out, name))?;
        files.push(PlannedFile {
            path: display(name),
            exists: fs::symlink_metadata(&absolute).is_ok(),
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
                            "[source_globs.{}] (relative to the detected root `{}`{}): all = {}, user_visible = {}, strings = {}, tests = {}",
                            p.name,
                            p.root,
                            if c.root() == p.root {
                                String::new()
                            } else {
                                format!("; configured root is `{}`", c.root())
                            },
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
        scopes,
        candidates,
        repo,
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
        ];
        if !self.scopes.is_empty() {
            out.push(format!("Scopes: {}", self.scopes.join(", ")));
            out.push(String::new());
        }
        out.push("Platforms:".into());
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
                    .unwrap_or_else(|| format!(", target {} (the only platform)", b.name))
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
        let mut groups: BTreeMap<&str, Vec<&AppCandidate>> = BTreeMap::new();
        for c in &self.candidates {
            groups.entry(c.key.as_str()).or_default().push(c);
        }
        if groups.len() > 1 {
            out.extend([
                String::new(),
                format!(
                    "Several apps found ({}); this catalog covers all of them. To catalog one app, rerun with --scope (paths relative to the repository):",
                    groups.len()
                ),
            ]);
            for (key, members) in &groups {
                let described: Vec<String> = members
                    .iter()
                    .map(|c| {
                        format!(
                            "{} ({} in {}{})",
                            c.name,
                            if c.kind == "apple" {
                                "Apple target"
                            } else {
                                "Android module"
                            },
                            c.declared_in,
                            if c.scopes.is_empty() {
                                "; no source folder found by name".to_owned()
                            } else {
                                format!("; {}", c.scopes.join(", "))
                            }
                        )
                    })
                    .collect();
                out.push(format!("- {key}: {}", described.join("; ")));
                let scopes: BTreeSet<&String> = members.iter().flat_map(|c| &c.scopes).collect();
                if !scopes.is_empty() {
                    out.push(format!(
                        "    concordance init --app {key}{}",
                        scopes
                            .iter()
                            .map(|s| format!(" --scope {s}"))
                            .collect::<String>()
                    ));
                }
            }
        }
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
            check_no_symlinks(&self.repo, &f.path)?;
            match &f.content {
                None => {
                    if fs::symlink_metadata(&f.absolute).is_err() {
                        create_dirs(&self.repo, &f.path)?;
                        created.push(f.path.clone());
                    }
                }
                Some(content) => {
                    create_dirs(&self.repo, parent(&f.path))?;
                    // create_new never follows a symbolic link at the final component.
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
        assert_eq!(pattern, "ios/v{version}");
        assert_eq!(count, 2);
        let re = crate::scan_git::tag_regex(&pattern).expect("valid");
        assert!(re.is_match("ios/v1.2.0"));
        assert!(!re.is_match("android/v1.2.0"));
        assert!(guess_tag_pattern(&["main".to_owned()]).is_none());
    }
    #[test]
    fn toml_strings_escape() {
        assert_eq!(toml_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
