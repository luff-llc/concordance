use clap::{Parser, Subcommand, ValueEnum};
use concordance::{
    ui::{Tokens, UiCatalog},
    Catalog, Language, Result,
};
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
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Generate the configured baseline-to-target parity report.
    Report {
        dir: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    /// Generate native design tokens, preserving every existing file.
    Tokens {
        dir: PathBuf,
        #[arg(long, value_enum)]
        platform: Native,
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
        id: String,
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
    },
    /// Pull missing/partial work, prerequisites first, then highest impact.
    Next {
        dir: PathBuf,
        #[arg(long)]
        platform: String,
        #[arg(long)]
        area: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
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
        Command::Validate { dir, repo } => {
            let catalog = Catalog::load(&dir)?;
            if dir.join("tokens.yaml").exists() || dir.join("screens").exists() {
                UiCatalog::load(&dir)?;
            }
            catalog.validate_paths(repo.as_deref())?;
            println!(
                "Valid: {} entries in {} areas.",
                catalog.entries.len(),
                catalog.area_files.len()
            );
        }
        Command::Report { dir, out, check } => {
            let catalog = Catalog::load(&dir)?;
            let out = out.unwrap_or_else(|| dir.join("parity.md"));
            let report = catalog.report()?;
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
            let result = Tokens::load(&dir)?.write_native(platform.into(), &out)?;
            print_generation(result);
        }
        Command::CapturePlan {
            dir,
            platform,
            language,
            captures,
            out,
        } => {
            let ui = UiCatalog::load(&dir)?;
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
            let report = ui.ui_report(&captures, baseline)?;
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
        Command::Entry { dir, id, format } => {
            let catalog = Catalog::load(dir)?;
            match format {
                Format::Yaml => print!("{}", serde_yaml::to_string(catalog.entry(&id)?)?),
                Format::Json => println!("{}", serde_json::to_string_pretty(catalog.entry(&id)?)?),
                Format::Brief => print!("{}", catalog.brief(&id)?),
            }
        }
        Command::Next {
            dir,
            platform,
            area,
            limit,
        } => {
            let catalog = Catalog::load(dir)?;
            for entry in catalog.next(&platform, area.as_deref(), limit)? {
                println!(
                    "{}\t{}\t{}\t{}",
                    entry.id,
                    entry.impact.as_str(),
                    entry.platforms[&platform].status.as_str(),
                    entry.title
                );
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
