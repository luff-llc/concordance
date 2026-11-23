//! `concordance plan-audit`: one audit brief per candidate area from the inventory. With a
//! target platform, each brief is a parity audit (baseline against target). Generalized from
//! the Taupe parity audits. See docs/adopt.md.
use crate::inventory::{Inventory, PlatformInventory};
use crate::{fail, Catalog, Result, RootOptions};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct AuditOptions {
    /// Platform to audit (the parity baseline with `target`). Default: `[report].baseline`.
    pub platform: Option<String>,
    pub target: Option<String>,
    /// Only this candidate area.
    pub area: Option<String>,
    /// Catalog directory as agents should type it (for commands in the briefs).
    pub dir: String,
}

#[derive(Debug, Default)]
pub struct AuditResult {
    pub created: Vec<PathBuf>,
    pub unchanged: Vec<PathBuf>,
    pub preserved: Vec<PathBuf>,
}

const ENTRY_FORMAT: &str = r#"Each area file `catalog/<area>.yaml` is a nonempty array of entries. Required fields:
`id` (`area.verb-object`), `title`, `summary`, `surfaces`, `interactions` (trigger → result),
`states`, `settings`, `data` (`reads`, `writes`), `copy` (exact strings), `accessibility`
(`label`, `identifier`, `notes`), `design`, `depends_on`, `impact`
(critical|high|medium|low), `platforms` (per platform: `status`
implemented|partial|missing|not-applicable, `source`, `tests`, `gap`) and `acceptance`
(given/when/then). Measurable touches go in `details` (kind motion|haptic|gesture|geometry|
copy|state|accessibility|layout|other, with numbers in `spec`).

```yaml
- id: home.open-item
  title: Open an item
  summary: Tapping a row opens the item at its saved position.
  surfaces: [home]
  interactions:
    - trigger: Tap a row
      result: The item opens at its saved position.
  states: [ready, empty, offline]
  settings: []
  data: {reads: [item, position], writes: []}
  copy: [Open, Nothing here yet]
  accessibility: {label: Open item, identifier: home.row, notes: []}
  design: []
  depends_on: []
  impact: high
  platforms:
    ios: {status: implemented, source: ["App/Home/HomeView.swift#HomeRow"], tests: []}
  acceptance:
    - given: An item with a saved position
      when: The person taps its row
      then: The item opens at that position.
  details:
    - id: open-spring
      kind: motion
      spec: {text: The item expands with a spring., motion: {curve: spring, response: 0.4, damping: 0.8}}
      baseline: ["App/Home/HomeView.swift#openTransition"]
```

Full format: https://github.com/luff-llc/concordance/blob/main/docs/format.md"#;

const RULES: &[&str] = &[
    "Read the source. Do not trust an existing status: `implemented` is a claim, often from an earlier coarse review. Audit at the level a picky designer would notice.",
    "For every screen and component in the area compare or record: layout and hierarchy, typography roles, colour and material, motion (named springs, durations, transitions, reduced-motion behaviour), haptics, gestures (swipe actions, long press and context menus, drag, scrub), menus and their items, toolbar items, empty, loading, error and offline states, copy, accessibility labels and identifiers, text scaling, landscape, tablet and wide layouts, system integration (media controls, notifications, share sheet, intents, widgets) and the settings that change behaviour.",
    "Give exact values: copy verbatim, spring or duration numbers, haptic kind, gesture thresholds, sizes, identifiers. A brief without numbers produces a flat port; put the numbers in `details`.",
    "Keep existing accessibility and test identifiers; tests on every platform find elements by them.",
    "For a surface that exists on only one platform, say what the native analogue on the other platform should be (for example a lock-screen live activity and an ongoing notification, a control-centre control and a quick-settings tile, a voice-assistant shortcut and an app action), or record `not-applicable` with the reason.",
    "Cite evidence relative to the platform root and anchor it where you can (`path#Symbol` or `path:L10-L20`). Every cited path must exist.",
    "Statuses are `implemented`, `partial`, `missing` or `not-applicable`, with a `gap` for partial and missing work. Never write `verification`: verification is recorded later by someone who did not implement the work.",
    "Stay inside this area. Something that belongs to another area becomes a `depends_on` link or a note, not a duplicate entry. Never reuse an ID; retire old IDs in `retired-ids.json`.",
    "Do not build, run or edit application source during the audit. Only write catalog entries and your findings file.",
    "Depth over breadth: an exhaustive account of this area beats a skim of several.",
];

fn file_lines(items: &[crate::inventory::FileItem], area: &str) -> Vec<String> {
    items
        .iter()
        .filter(|f| f.area == area)
        .map(|f| {
            let mark = if f.cited_by.is_empty() { " " } else { "x" };
            let cited = if f.cited_by.is_empty() {
                String::new()
            } else {
                format!(" — cited by {}", f.cited_by.join(", "))
            };
            format!("- [{mark}] `{}`{cited}", f.path)
        })
        .collect()
}

fn section(out: &mut Vec<String>, title: &str, lines: Vec<String>) {
    out.extend([String::new(), format!("## {title}"), String::new()]);
    if lines.is_empty() {
        out.push("None found.".into());
    } else {
        out.extend(lines);
    }
}

fn platform_lists(out: &mut Vec<String>, p: &PlatformInventory, area: &str, label: &str) {
    let title = |what: &str| -> String {
        if label.is_empty() {
            let mut chars = what.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        } else {
            format!("{label}{what}")
        }
    };
    let files = file_lines(&p.files, area);
    let cited = p
        .files
        .iter()
        .filter(|f| f.area == area && !f.cited_by.is_empty())
        .count();
    section(
        out,
        &title(&format!("files ({} of {} cited)", cited, files.len())),
        files,
    );
    section(
        out,
        &title("identifiers"),
        p.identifiers
            .iter()
            .filter(|i| i.areas.iter().any(|a| a == area))
            .map(|i| {
                let cited = if i.cited_by.is_empty() {
                    "not cited".to_owned()
                } else {
                    format!("cited by {}", i.cited_by.join(", "))
                };
                format!("- `{}` ({}) — {cited}", i.identifier, i.files.join(", "))
            })
            .collect(),
    );
    section(
        out,
        &title("strings used here"),
        p.strings
            .iter()
            .filter(|s| s.areas.iter().any(|a| a == area))
            .map(|s| {
                let value = if s.value == s.key {
                    String::new()
                } else {
                    format!(" = {:?}", s.value)
                };
                let cited = if s.cited_by.is_empty() {
                    "not cited".to_owned()
                } else {
                    format!("cited by {}", s.cited_by.join(", "))
                };
                format!("- {:?}{value} ({}) — {cited}", s.key, s.file)
            })
            .collect(),
    );
    section(out, &title("tests"), file_lines(&p.tests, area));
}

impl Catalog {
    fn audit_brief(
        &self,
        options: &AuditOptions,
        base: &PlatformInventory,
        target: Option<&PlatformInventory>,
        area: &str,
    ) -> String {
        let dir = &options.dir;
        let app = crate::inventory::app_name(self);
        let label = |p: &str| self.config.report.label(p).to_owned();
        let mut out = Vec::new();
        match target {
            Some(t) => out.push(format!(
                "# Parity audit brief: {area} ({} → {})",
                label(&base.platform),
                label(&t.platform)
            )),
            None => out.push(format!("# Audit brief: {area} ({})", label(&base.platform))),
        }
        out.push(String::new());
        out.push(format!(
            "App: {app}. Catalog: `{dir}`. {} root: `{}` (relative to the repository root{}).",
            label(&base.platform),
            base.root,
            if self.config.external_roots.contains_key(&base.platform) {
                format!(
                    "; external checkout, pass `--root {}=<checkout>`",
                    base.platform
                )
            } else {
                String::new()
            }
        ));
        if let Some(t) = target {
            out.push(format!("{} root: `{}`.", label(&t.platform), t.root));
        }
        out.push(
            "Written by `concordance plan-audit`; it is never overwritten, so edit freely.".into(),
        );
        out.extend([String::new(), "## Goal".into(), String::new()]);
        match target {
            Some(t) => out.push(format!(
                "Find everything in `{area}` that {} lacks or does worse than {}, at the level a picky designer would notice, and record it in the catalog so an engineer can build it without opening the {} code.",
                label(&t.platform),
                label(&base.platform),
                label(&base.platform)
            )),
            None => out.push(format!(
                "Describe every user-visible behaviour in `{area}` on {} as catalog entries, so the files, identifiers and strings below are all cited by an entry.",
                label(&base.platform)
            )),
        }
        let prefix = if target.is_some() {
            format!("{} ", label(&base.platform))
        } else {
            String::new()
        };
        platform_lists(&mut out, base, area, &prefix);

        // Existing entries: those citing files here, plus the area file of the same name.
        let mut entries: BTreeSet<&str> = base
            .areas
            .iter()
            .filter(|a| a.area == area)
            .flat_map(|a| a.entries.iter().map(String::as_str))
            .collect();
        entries.extend(
            self.areas
                .iter()
                .filter(|(_, a)| a.as_str() == area)
                .map(|(id, _)| id.as_str()),
        );
        section(
            &mut out,
            "Existing entries",
            entries
                .iter()
                .filter_map(|id| self.entries.get(*id))
                .map(|e| {
                    let statuses = e
                        .platforms
                        .iter()
                        .filter(|(p, _)| {
                            **p == base.platform || target.is_some_and(|t| t.platform == **p)
                        })
                        .map(|(p, r)| format!("{p} {}", r.status.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "- `{}` — {} (`{}`{})",
                        e.id,
                        e.title,
                        self.area_files
                            .get(&self.areas[&e.id])
                            .map(String::as_str)
                            .unwrap_or(""),
                        if statuses.is_empty() {
                            String::new()
                        } else {
                            format!("; {statuses}")
                        }
                    )
                })
                .collect(),
        );
        if let Some(t) = target {
            let mut lines: Vec<String> = Vec::new();
            for id in &entries {
                let Some(record) = self
                    .entries
                    .get(*id)
                    .and_then(|e| e.platforms.get(&t.platform))
                else {
                    lines.push(format!("- `{id}`: no {} record", t.platform));
                    continue;
                };
                let evidence: Vec<&String> = record.source.iter().chain(&record.tests).collect();
                lines.push(format!(
                    "- `{id}`: {}{}{}",
                    record.status.as_str(),
                    record
                        .gap
                        .as_ref()
                        .map(|g| format!(" — gap: {g}"))
                        .unwrap_or_default(),
                    if evidence.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " — {}",
                            evidence
                                .iter()
                                .map(|e| format!("`{e}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                ));
            }
            section(
                &mut out,
                &format!("{} records of these entries", label(&t.platform)),
                lines,
            );
            if t.unchecked.is_some() {
                out.extend([
                    String::new(),
                    format!(
                        "{} was not scanned ({}); find its counterparts by reading its source.",
                        label(&t.platform),
                        t.unchecked.as_deref().unwrap_or("")
                    ),
                ]);
            } else {
                out.extend([
                    String::new(),
                    format!(
                        "The {} lists below come from its candidate area of the same name; counterparts may live elsewhere.",
                        label(&t.platform)
                    ),
                ]);
                platform_lists(&mut out, t, area, &format!("{} ", label(&t.platform)));
            }
        }
        out.extend([String::new(), "## Entry format".into(), String::new()]);
        out.push(ENTRY_FORMAT.into());
        out.extend([String::new(), "## Rules".into(), String::new()]);
        for (i, rule) in RULES.iter().enumerate() {
            out.push(format!("{}. {rule}", i + 1));
        }
        out.extend([String::new(), "## Output".into(), String::new()]);
        match target {
            Some(t) => {
                out.push(format!(
                    "1. Findings beside this brief, in `{}`: a one-paragraph verdict; a gap table ordered by user impact (id `{area}-NN`, title, severity P0 broken or missing core / P1 obvious detail / P2 polish, {} refs, {} refs or \"absent\", an exact implementation spec, and the entry ID that covers it or the new ID to add); the {} files an implementer will touch, per gap cluster, so work can be split without collisions; shared-component gaps; and anything the catalog format cannot express.",
                    findings_name(&base.platform, Some(&t.platform), area),
                    label(&base.platform),
                    label(&t.platform),
                    label(&t.platform)
                ));
                out.push(format!(
                    "2. Catalog updates: `{}` records set to `partial` or `missing` with a `gap`, `details` with numbers for each touch, and new entries for {} behaviour nobody catalogued.",
                    t.platform,
                    label(&base.platform)
                ));
            }
            None => out.push(format!(
                "1. Entries in `{dir}/catalog/{area}.yaml` (or the existing area file that fits), with {} source and tests cited.",
                base.platform
            )),
        }
        out.push(format!(
            "{}. Check: `concordance validate {dir}` passes, and `concordance inventory {dir} --platform {} --uncatalogued` no longer lists this area's items.",
            if target.is_some() { 3 } else { 2 },
            base.platform
        ));
        out.join("\n") + "\n"
    }

    /// Write one brief per candidate area into `out` (default: `tasks` in the catalog
    /// directory), never overwriting a file.
    pub fn plan_audit(
        &self,
        roots: &RootOptions,
        options: &AuditOptions,
        out: Option<&Path>,
    ) -> Result<AuditResult> {
        // The catalog path must not reach the catalog through a link (for example a `spec`
        // linked into another checkout), by the same rule the log uses before writing.
        crate::log::check_catalog_path(&self.directory)?;
        let platform = options
            .platform
            .clone()
            .unwrap_or_else(|| self.config.report.baseline.clone());
        if !self.config.platforms.contains_key(&platform) {
            return fail(format!("unknown platform: {platform}"));
        }
        if let Some(target) = &options.target {
            if !self.config.platforms.contains_key(target) {
                return fail(format!("unknown platform: {target}"));
            }
            if *target == platform {
                return fail("--target must differ from the audited platform");
            }
        }
        let Inventory { platforms, .. } = self.inventory(roots, Some(&platform))?;
        let base = platforms.into_iter().next().ok_or("no inventory")?;
        if let Some(reason) = &base.unchecked {
            return fail(format!("{platform}: {reason}"));
        }
        let target = match &options.target {
            Some(t) => {
                let inventory = match self.inventory(roots, Some(t)) {
                    Ok(inventory) => inventory,
                    Err(error) => return fail(format!("{t}: {error}")),
                };
                inventory.platforms.into_iter().next()
            }
            None => None,
        };
        let areas: Vec<&str> = base.areas.iter().map(|a| a.area.as_str()).collect();
        let selected: Vec<&str> = match &options.area {
            Some(area) if areas.contains(&area.as_str()) => vec![area.as_str()],
            Some(area) => {
                return fail(format!(
                    "unknown candidate area: {area} (candidates: {})",
                    areas.join(", ")
                ))
            }
            None => areas,
        };
        // Same rules as init, never through a symbolic link. The allowed bases are the
        // catalog directory and the Git checkout that contains it; repo_root may be a shared
        // parent of several checkouts, so it is not a base.
        let catalog_dir = self.directory.canonicalize()?;
        let toplevel = git_toplevel(&catalog_dir);
        let safe = match out {
            None => crate::adopt::SafeDir::child(&catalog_dir, "tasks")?,
            Some(out) => {
                let mut chosen = None;
                for base in toplevel.iter().chain(std::iter::once(&catalog_dir)) {
                    match crate::adopt::SafeDir::new(base, out) {
                        Ok(safe) => {
                            chosen = Some(safe);
                            break;
                        }
                        Err(error) if error.to_string().contains("is not inside") => {}
                        Err(error) => return Err(error),
                    }
                }
                chosen.ok_or_else(|| {
                    format!(
                        "--out {} must be inside the catalog's Git checkout{} or the catalog directory {}",
                        out.display(),
                        toplevel
                            .as_ref()
                            .map(|t| format!(" {}", t.display()))
                            .unwrap_or_default(),
                        catalog_dir.display()
                    )
                })?
            }
        };
        safe.create()?;
        let mut result = AuditResult::default();
        for area in selected {
            let text = self.audit_brief(options, &base, target.as_ref(), area);
            let name = brief_name(&platform, options.target.as_deref(), area);
            // Write through exactly the checked path.
            let path = safe.file(&name)?;
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(text.as_bytes())?;
                    result.created.push(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
                        result.unchanged.push(path);
                    } else {
                        result.preserved.push(path);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(result)
    }
}

/// The top level of the Git checkout containing `dir`, canonicalized.
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        .canonicalize()
        .ok()
}

fn brief_name(platform: &str, target: Option<&str>, area: &str) -> String {
    match target {
        Some(t) => format!("parity-{platform}-{t}-{area}.md"),
        None => format!("audit-{platform}-{area}.md"),
    }
}
fn findings_name(platform: &str, target: Option<&str>, area: &str) -> String {
    brief_name(platform, target, area).replace(".md", "-findings.md")
}
