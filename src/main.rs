use clap::{Parser, Subcommand, ValueEnum};
use concordance::{Catalog, Language, Result};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Native behavior catalogs: validate, plan, and generate shared identifiers and test journeys"
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
            let catalog = Catalog::load(dir)?;
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
fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
