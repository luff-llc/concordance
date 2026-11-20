use clap::{Parser, Subcommand, ValueEnum};
use concordance::{
    ui::{Tokens, UiCatalog},
    Catalog, Language, Result, RootOptions,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Native product contracts: behavior, design tokens, screens, capture plans, and parity"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Validate schema, IDs, dependency DAG, retirement records and evidence files.
    Validate {
        dir: PathBuf,
        #[command(flatten)]
        roots: Roots,
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Fail on warnings (resolved blockers, implemented records with open work).
        #[arg(long)]
        strict: bool,
    },
    /// Generate the configured baseline-to-target parity report.
    Report {
        dir: PathBuf,
        #[command(flatten)]
        roots: Roots,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    /// Generate design tokens (swift, kotlin or css), preserving every existing file.
    Tokens {
        dir: PathBuf,
        #[arg(long, value_enum)]
        platform: TokenPlatform,
        #[arg(long)]
        out: PathBuf,
    },
    /// Print required screen/state pairs and create skipped tests for missing captures.
    CapturePlan {
        dir: PathBuf,
        #[arg(long)]
        platform: String,
        /// Native test language (defaults to Swift for ios, Kotlin for android).
        #[arg(long, value_enum)]
        language: Option<Native>,
        #[arg(long)]
        captures: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print per-area and per-screen capture coverage (Markdown) instead of the plan,
        /// without writing stubs. --platform may list several platforms, comma-separated.
        #[arg(long)]
        coverage: bool,
    },
    /// Compare native captures and write an HTML report; similarity never gates parity.
    Compare {
        dir: PathBuf,
        #[arg(long)]
        captures: PathBuf,
        #[arg(long)]
        baseline: Option<String>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        check: bool,
    },
    /// Fold reviewer notes (review.json from compare) into proposed verification records.
    /// Writes a patch for a person to apply; never edits the catalog.
    Verify {
        dir: PathBuf,
        #[arg(long)]
        review: PathBuf,
        /// Capture directory used to refuse skipped or missing captures (default DIR/captures).
        #[arg(long)]
        captures: Option<PathBuf>,
        /// Patch output path (default: stdout).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Set or clear a platform record's status and bookkeeping in place (never verification).
    Set {
        dir: PathBuf,
        /// `entry.id`, `entry.id#detail` or `entry.id/detail`.
        target: String,
        #[arg(long)]
        platform: String,
        #[arg(long, value_enum)]
        status: Option<StatusArg>,
        #[arg(long, alias = "by")]
        implemented_by: Option<String>,
        #[arg(long)]
        gap: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        /// Comma-separated blocker keys.
        #[arg(long, value_delimiter = ',')]
        blocked_by: Option<Vec<String>>,
        /// Field to remove: gap, notes, implemented_by, blocked_by, verification, source, tests.
        #[arg(long)]
        clear: Vec<String>,
        /// Set a field to null on an inheriting platform, unsetting the parent's value.
        #[arg(long)]
        unset: Vec<String>,
        /// Allow rewriting a file whose layout is not canonical (comments are lost).
        #[arg(long)]
        allow_reformat: bool,
    },
    /// Re-locate line and symbol anchors from the baseline pin to a new commit.
    RebaseAnchors {
        dir: PathBuf,
        #[command(flatten)]
        roots: Roots,
        #[arg(long)]
        to: String,
        /// Starting commit (default: the platform's baseline pin).
        #[arg(long)]
        from: Option<String>,
        /// Platform (default: the report baseline).
        #[arg(long)]
        platform: Option<String>,
        /// Rewrite moved references in the area files.
        #[arg(long)]
        write: bool,
        #[arg(long)]
        allow_reformat: bool,
        #[arg(long, value_enum, default_value = "markdown")]
        format: DriftFormat,
    },
    /// Edit reviewer notes (review.json) without the browser.
    Review {
        #[command(subcommand)]
        action: ReviewAction,
    },
    /// Generate parity.md with a fresh screen summary for every platform.
    UiReport {
        dir: PathBuf,
        #[command(flatten)]
        roots: Roots,
        #[arg(long)]
        captures: Option<PathBuf>,
        #[arg(long)]
        baseline: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Check report freshness without writing (same semantics as report --check).
        #[arg(long)]
        check: bool,
    },
    /// Print an entry or a native implementation brief.
    Entry {
        dir: PathBuf,
        #[command(flatten)]
        roots: Roots,
        id: String,
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
        /// Include detail items (numbers, baseline evidence, per-platform status) in briefs.
        #[arg(long)]
        details: bool,
    },
    /// Pull missing/partial work, prerequisites first, then highest impact.
    Next {
        dir: PathBuf,
        #[command(flatten)]
        roots: Roots,
        #[arg(long)]
        platform: String,
        #[arg(long)]
        area: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Also select implemented entries with open details and print one row per open detail.
        #[arg(long)]
        details: bool,
        /// Include work held back by open blockers (marked in the title column).
        #[arg(long)]
        include_blocked: bool,
    },
    /// List entries with changed baseline evidence and uncatalogued source changes.
    Drift {
        dir: PathBuf,
        #[command(flatten)]
        roots: Roots,
        #[arg(long)]
        platform: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value = "HEAD")]
        to: String,
        #[arg(long, value_enum, default_value = "markdown")]
        format: DriftFormat,
    },
    /// Generate native accessibility-identifier constants.
    Ids {
        dir: PathBuf,
        #[arg(long, value_enum)]
        platform: Native,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        package: Option<String>,
    },
    /// Create TODO acceptance test skeletons, preserving all existing files.
    Journeys {
        dir: PathBuf,
        #[arg(long, value_enum)]
        platform: Native,
        #[arg(long)]
        out: PathBuf,
    },
}
#[derive(Subcommand)]
enum ReviewAction {
    /// Set a pair's verdict and note in <dir>/review.json (the compare --out directory).
    Set {
        dir: PathBuf,
        /// Pair key, `screen.state`.
        key: String,
        #[arg(long, value_enum)]
        verdict: VerdictArg,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        /// Review date, YYYY-MM-DD.
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        baseline_commit: Option<String>,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum StatusArg {
    Implemented,
    Partial,
    Missing,
    NotApplicable,
}
#[derive(Clone, Copy, ValueEnum)]
enum VerdictArg {
    Unreviewed,
    Same,
    Differs,
}
#[derive(clap::Args)]
struct Roots {
    /// Evidence checkout root, PLATFORM=PATH; repeat for multiple platforms.
    #[arg(long = "root", value_parser = parse_root)]
    roots: Vec<(String, PathBuf)>,
    /// Fail when a declared external checkout has not been supplied.
    #[arg(long)]
    require_roots: bool,
}
fn parse_root(value: &str) -> std::result::Result<(String, PathBuf), String> {
    match value.split_once('=') {
        Some((platform, path)) if !platform.is_empty() && !path.is_empty() => {
            Ok((platform.into(), path.into()))
        }
        _ => Err("expected PLATFORM=PATH".into()),
    }
}
impl Roots {
    fn options(self, catalog: &Catalog) -> Result<RootOptions> {
        let options = RootOptions {
            repo: None,
            roots: self.roots.into_iter().collect::<BTreeMap<_, _>>(),
            require_roots: self.require_roots,
        }
        .with_env(catalog);
        for platform in options.roots.keys() {
            if !catalog.config.platforms.contains_key(platform) {
                return Err(format!("unknown platform: {platform}").into());
            }
        }
        if options.require_roots {
            for platform in catalog.config.external_roots.keys() {
                if !options.roots.contains_key(platform) {
                    return Err(format!(
                        "{platform}: root not supplied; use --root {platform}=<path>"
                    )
                    .into());
                }
            }
        }
        Ok(options)
    }
}
#[derive(Clone, Copy, ValueEnum)]
enum DriftFormat {
    Markdown,
    Json,
}
#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Yaml,
    Json,
    Brief,
}
#[derive(Clone, Copy, ValueEnum)]
enum Native {
    Kotlin,
    Swift,
}
#[derive(Clone, Copy, ValueEnum)]
enum TokenPlatform {
    Css,
    Kotlin,
    Swift,
}
impl From<Native> for Language {
    fn from(value: Native) -> Self {
        match value {
            Native::Kotlin => Self::Kotlin,
            Native::Swift => Self::Swift,
        }
    }
}
fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Validate {
            dir,
            repo,
            roots,
            strict,
        } => {
            let (catalog, mut report) = Catalog::load_collect(&dir)?;
            // Validation accumulates missing roots per reference, rather than failing in option parsing.
            let options = RootOptions {
                repo,
                roots: roots.roots.into_iter().collect(),
                require_roots: roots.require_roots,
            }
            .with_env(&catalog);
            let evidence = catalog.validate_evidence(&options);
            report.errors.extend(evidence.errors);
            report.unchecked.extend(evidence.unchecked);
            if dir.join("tokens.yaml").exists() || dir.join("screens").exists() {
                match UiCatalog::load(&dir) {
                    Ok(ui) => {
                        report.errors.extend(ui.verification_issues());
                        report.warnings.extend(ui.motion_token_warnings());
                    }
                    Err(error) => report.errors.push(concordance::Issue {
                        group: "ui".into(),
                        entry: String::new(),
                        platform: String::new(),
                        reference: String::new(),
                        message: error.to_string(),
                    }),
                }
            }
            report.sort();
            if strict {
                report = report.strict();
            }
            let failed = !report.errors.is_empty();
            if failed {
                return Err(Box::new(report));
            }
            println!("{report}");
            println!(
                "Valid: {} entries in {} areas.",
                catalog.entries.len(),
                catalog.area_files.len()
            );
        }
        Command::Drift {
            dir,
            roots,
            platform,
            since,
            to,
            format,
        } => {
            let catalog = Catalog::load(&dir)?;
            let options = roots.options(&catalog)?;
            let drift = catalog.drift(&options, platform.as_deref(), since.as_deref(), &to)?;
            match format {
                DriftFormat::Markdown => print!("{}", drift.markdown()),
                DriftFormat::Json => println!("{}", serde_json::to_string_pretty(&drift)?),
            }
        }
        Command::Report {
            dir,
            out,
            check,
            roots,
        } => {
            let catalog = Catalog::load(&dir)?;
            let out = out.unwrap_or_else(|| dir.join("parity.md"));
            let report = catalog.report_with_roots_at(&roots.options(&catalog)?, Some(&out))?;
            if check {
                if fs::read_to_string(&out).ok().as_deref() != Some(&report) {
                    return Err(format!(
                        "stale report: {}; regenerate with report --out {}",
                        out.display(),
                        out.display()
                    )
                    .into());
                }
                println!("Current: {}", out.display());
            } else {
                fs::write(&out, report)?;
                println!("Wrote {}", out.display());
            }
        }
        Command::Tokens { dir, platform, out } => {
            Catalog::load(&dir)?;
            let tokens = Tokens::load(&dir)?;
            let result = match platform {
                TokenPlatform::Css => tokens.write_css(&dir.display().to_string(), &out)?,
                TokenPlatform::Kotlin => tokens.write_native(Language::Kotlin, &out)?,
                TokenPlatform::Swift => tokens.write_native(Language::Swift, &out)?,
            };
            print_generation(result);
        }
        Command::CapturePlan {
            dir,
            platform,
            language,
            captures,
            out,
            coverage,
        } => {
            let ui = UiCatalog::load(&dir)?;
            if coverage {
                let captures = captures.unwrap_or_else(|| dir.join("captures"));
                let platforms: Vec<&str> = platform.split(',').map(str::trim).collect();
                print!("{}", ui.coverage(&captures, &platforms)?.markdown(&ui));
                return Ok(());
            }
            let language = match language {
                Some(language) => language,
                None => match platform.as_str() {
                    "ios" | "swift" => Native::Swift,
                    "android" | "kotlin" => Native::Kotlin,
                    _ => return Err("use --language swift|kotlin for this platform".into()),
                },
            };
            print!("{}", ui.capture_plan(&platform)?);
            let captures = captures.unwrap_or_else(|| dir.join("captures"));
            let out = out.unwrap_or_else(|| dir.join("capture-tests").join(&platform));
            let result = ui.capture_stubs(&platform, language.into(), &captures, &out)?;
            // stdout remains a machine-readable plan.
            eprintln!(
                "{} created; {} unchanged; {} existing files preserved.",
                result.created.len(),
                result.unchanged.len(),
                result.preserved.len()
            );
            for path in result.preserved {
                eprintln!("Preserved: {}", path.display());
            }
        }
        Command::Compare {
            dir,
            captures,
            baseline,
            target,
            out,
            check,
        } => {
            let ui = UiCatalog::load(&dir)?;
            let baseline = baseline
                .as_deref()
                .unwrap_or(&ui.catalog.config.report.baseline);
            let target = target
                .as_deref()
                .unwrap_or(&ui.catalog.config.report.target);
            let comparison = ui.compare(&captures, baseline, target, Some(&out))?;
            if let Some(warning) = &comparison.fixture_warning {
                eprintln!("WARNING: {warning}");
            }
            print!("{}", comparison.summary());
            println!("Report: {}", out.join("index.html").display());
            if check && comparison.failed() {
                return Err(
                    "UI comparison failed: structural or token failures; see report".into(),
                );
            }
        }
        Command::RebaseAnchors {
            dir,
            roots,
            to,
            from,
            platform,
            write,
            allow_reformat,
            format,
        } => {
            let catalog = Catalog::load(&dir)?;
            let options = roots.options(&catalog)?;
            let platform = platform.unwrap_or_else(|| catalog.config.report.baseline.clone());
            let report = catalog.rebase_anchors(
                &options,
                &platform,
                from.as_deref(),
                &to,
                write,
                allow_reformat,
            )?;
            match format {
                DriftFormat::Markdown => print!("{}", report.markdown()),
                DriftFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
            }
        }
        Command::Set {
            dir,
            target,
            platform,
            status,
            implemented_by,
            gap,
            notes,
            blocked_by,
            clear,
            unset,
            allow_reformat,
        } => {
            use concordance::Status;
            let request = concordance::SetRequest {
                target: target.clone(),
                platform,
                status: status.map(|s| match s {
                    StatusArg::Implemented => Status::Implemented,
                    StatusArg::Partial => Status::Partial,
                    StatusArg::Missing => Status::Missing,
                    StatusArg::NotApplicable => Status::NotApplicable,
                }),
                implemented_by,
                gap,
                notes,
                blocked_by,
                clear,
                unset,
                allow_reformat,
            };
            let outcome = Catalog::set_record(&dir, &request)?;
            println!(
                "{} {}",
                if outcome.changed {
                    "Updated"
                } else {
                    "Unchanged:"
                },
                outcome.file.display()
            );
            for warning in outcome.warnings {
                eprintln!(
                    "warning: {} {} {}: {}",
                    warning.entry, warning.platform, warning.reference, warning.message
                );
            }
        }
        Command::Review {
            action:
                ReviewAction::Set {
                    dir,
                    key,
                    verdict,
                    note,
                    reviewer,
                    at,
                    commit,
                    baseline_commit,
                },
        } => {
            use concordance::ui::{Review, Verdict};
            let path = dir.join("review.json");
            let mut review = Review::load(&path)?;
            review.set(
                &key,
                match verdict {
                    VerdictArg::Unreviewed => Verdict::Unreviewed,
                    VerdictArg::Same => Verdict::Same,
                    VerdictArg::Differs => Verdict::Differs,
                },
                note.as_deref(),
            )?;
            if let Some(reviewer) = reviewer {
                review.reviewer = reviewer;
            }
            if let Some(at) = at {
                if !concordance::schema::valid_date(&at) {
                    return Err("--at must be YYYY-MM-DD".into());
                }
                review.at = at;
            }
            for (field, value) in [
                (&mut review.commit, commit),
                (&mut review.baseline_commit, baseline_commit),
            ] {
                if let Some(value) = value {
                    if !concordance::schema::valid_commit(&value) {
                        return Err(format!("invalid commit {value:?}").into());
                    }
                    *field = Some(value);
                }
            }
            review.save(&path)?;
            println!("Updated {} in {}", key, path.display());
        }
        Command::Verify {
            dir,
            review,
            captures,
            out,
        } => {
            let ui = UiCatalog::load(&dir)?;
            let review = concordance::ui::Review::load(&review)?;
            let captures = captures.unwrap_or_else(|| dir.join("captures"));
            let outcome = ui.verify_review(&review, &captures)?;
            let patch = outcome.patch()?;
            match out {
                Some(out) => {
                    if out.exists() {
                        return Err(
                            format!("{} exists; choose a new patch path", out.display()).into()
                        );
                    }
                    fs::write(&out, patch)?;
                    eprintln!("Wrote proposed patch {}", out.display());
                }
                None => print!("{patch}"),
            }
            eprintln!("{}", outcome.summary());
        }
        Command::UiReport {
            dir,
            roots,
            captures,
            baseline,
            out,
            check,
        } => {
            let ui = UiCatalog::load(&dir)?;
            let captures = captures.unwrap_or_else(|| dir.join("captures"));
            let baseline = baseline
                .as_deref()
                .unwrap_or(&ui.catalog.config.report.baseline);
            let options = roots.options(&ui.catalog)?;
            let out = out.unwrap_or_else(|| dir.join("parity.md"));
            let report = ui.ui_report_at(&captures, baseline, Some(&out))?
                + &ui.catalog.evidence_roots_section(&options)?
                + &ui.catalog.baseline_drift_section(&options)?;
            if check {
                if fs::read_to_string(&out).ok().as_deref() != Some(&report) {
                    return Err(format!("stale UI report: {}", out.display()).into());
                }
            } else {
                fs::write(&out, report)?;
            }
            println!(
                "{} {}",
                if check { "Current:" } else { "Wrote" },
                out.display()
            );
        }
        Command::Entry {
            dir,
            id,
            format,
            roots,
            details,
        } => {
            let catalog = Catalog::load(dir)?;
            roots.options(&catalog)?;
            match format {
                Format::Yaml => print!("{}", serde_yaml::to_string(catalog.entry(&id)?)?),
                Format::Json => println!("{}", serde_json::to_string_pretty(catalog.entry(&id)?)?),
                Format::Brief => print!("{}", catalog.brief_with(&id, details)?),
            }
        }
        Command::Next {
            dir,
            roots,
            platform,
            area,
            limit,
            details,
            include_blocked,
        } => {
            let catalog = Catalog::load(dir)?;
            roots.options(&catalog)?;
            let blocked = catalog.blocked_entries(&platform);
            let mark = |keys: Option<&std::collections::BTreeSet<String>>| -> String {
                keys.filter(|k| !k.is_empty())
                    .map(|k| {
                        format!(
                            " [blocked: {}]",
                            k.iter().cloned().collect::<Vec<_>>().join(", ")
                        )
                    })
                    .unwrap_or_default()
            };
            for entry in catalog.next_filtered(
                &platform,
                area.as_deref(),
                limit,
                details,
                include_blocked,
            )? {
                println!(
                    "{}\t{}\t{}\t{}{}",
                    entry.id,
                    entry.impact.as_str(),
                    entry.platforms[&platform].status.as_str(),
                    entry.title,
                    mark(blocked.get(&entry.id))
                );
                if details {
                    // Same four columns: entry#detail, detail:kind, status, spec with numbers.
                    for detail in catalog.open_details(entry, &platform) {
                        let keys: std::collections::BTreeSet<String> = detail
                            .platforms
                            .get(&platform)
                            .map(|p| {
                                catalog
                                    .open_blockers(p)
                                    .into_iter()
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !include_blocked && !keys.is_empty() {
                            continue;
                        }
                        let numbers = detail.spec.numbers();
                        println!(
                            "{}#{}\tdetail:{}\t{}\t{}{}{}",
                            entry.id,
                            detail.id,
                            detail.kind.as_str(),
                            detail
                                .platforms
                                .get(&platform)
                                .map(|p| p.status.as_str())
                                .unwrap_or("unrecorded"),
                            detail.spec.text,
                            if numbers.is_empty() {
                                String::new()
                            } else {
                                format!(" [{numbers}]")
                            },
                            mark(Some(&keys))
                        );
                    }
                }
            }
        }
        Command::Ids {
            dir,
            platform,
            out,
            package,
        } => {
            let catalog = Catalog::load(dir)?;
            fs::write(
                &out,
                catalog.identifiers(platform.into(), package.as_deref())?,
            )?;
            println!(
                "Wrote {} identifiers to {}",
                catalog.entries.len(),
                out.display()
            );
        }
        Command::Journeys { dir, platform, out } => {
            let catalog = Catalog::load(dir)?;
            let result = catalog.journeys(platform.into(), &out)?;
            println!(
                "{} created; {} unchanged; {} existing files preserved.",
                result.created.len(),
                result.unchanged.len(),
                result.preserved.len()
            );
            for path in result.preserved {
                eprintln!("Preserved: {}", path.display());
            }
        }
    }
    Ok(())
}
fn print_generation(result: concordance::JourneyResult) {
    println!(
        "{} created; {} unchanged; {} existing files preserved.",
        result.created.len(),
        result.unchanged.len(),
        result.preserved.len()
    );
    for path in result.preserved {
        eprintln!("Preserved: {}", path.display());
    }
}
fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
