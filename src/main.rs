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
        Command::Validate { dir, repo, roots } => {
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
                    Ok(ui) => report.errors.extend(ui.verification_issues()),
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
            let report = catalog.report_with_roots(&roots.options(&catalog)?)?;
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
            print!("{}", comparison.summary());
            println!("Report: {}", out.join("index.html").display());
            if check && comparison.failed() {
                return Err(
                    "UI comparison failed: structural or token failures; see report".into(),
                );
            }
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
            let report = ui.ui_report(&captures, baseline)?
                + &ui.catalog.evidence_roots_section(&options)?
                + &ui.catalog.baseline_drift_section(&options)?;
            let out = out.unwrap_or_else(|| dir.join("parity.md"));
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
        } => {
            let catalog = Catalog::load(dir)?;
            roots.options(&catalog)?;
            for entry in catalog.next_with(&platform, area.as_deref(), limit, details)? {
                println!(
                    "{}\t{}\t{}\t{}",
                    entry.id,
                    entry.impact.as_str(),
                    entry.platforms[&platform].status.as_str(),
                    entry.title
                );
                if details {
                    // Same four columns: entry#detail, detail:kind, status, spec with numbers.
                    for detail in catalog.open_details(entry, &platform) {
                        let numbers = detail.spec.numbers();
                        println!(
                            "{}#{}\tdetail:{}\t{}\t{}{}",
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
                            }
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
