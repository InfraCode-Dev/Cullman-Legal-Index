//! lidx: build tool for the Cullman legal index.

mod allowlist;
mod build;
mod ingest;
mod model;
mod render;
mod validate;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "lidx", about = "Validate, build and render the Cullman legal index")]
struct Cli {
    /// Repository root.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check schema, registry, allowlists and (with --base) that observation files are append-only.
    Validate {
        /// Git ref to compare against, for example origin/main.
        #[arg(long)]
        base: Option<String>,
    },
    /// Derive assertions; write build/index.db, data/firms.csv and build/firms.html.
    Build {
        /// Local sales database (never committed). Adds sales notes to the page only.
        #[arg(long)]
        sales: Option<PathBuf>,
    },
    /// Ingest a reviewed firm-table CSV as one new observation file.
    IngestFirms {
        csv: PathBuf,
        /// Date the sources were read (YYYY-MM-DD).
        #[arg(long)]
        at: String,
    },
    /// Create an empty local sales database outside the repo.
    SalesInit { path: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.canonicalize()?;
    match cli.cmd {
        Cmd::Validate { base } => {
            let repo = model::Repo::load(&root)?;
            let allow = allowlist::Allowlist::load(&root)?;
            let errs = validate::validate(&repo, &allow, base.as_deref());
            let n: usize = repo.observations.values().map(Vec::len).sum();
            if errs.is_empty() {
                println!("ok: {} orgs, {} persons, {} tenures, {} observations in {} files",
                    repo.orgs.len(), repo.persons.len(), repo.tenures.len(), n, repo.observations.len());
            } else {
                for e in &errs {
                    eprintln!("error: {e}");
                }
                bail!("{} validation error(s)", errs.len());
            }
        }
        Cmd::Build { sales } => {
            let repo = model::Repo::load(&root)?;
            let allow = allowlist::Allowlist::load(&root)?;
            let errs = validate::validate(&repo, &allow, None);
            if !errs.is_empty() {
                for e in &errs {
                    eprintln!("error: {e}");
                }
                bail!("refusing to build: {} validation error(s)", errs.len());
            }
            if let Some(p) = &sales {
                let p = p.canonicalize()?;
                if p.starts_with(&root) {
                    bail!("the sales database must live outside the repo");
                }
            }
            let facts = build::derive(&repo);
            fs::create_dir_all(root.join("build"))?;
            build::write_db(&repo, &facts, &root.join("build/index.db"))?;
            let rows = build::firm_rows(&repo, &facts);
            build::write_firms_csv(&root.join("data/firms.csv"), &rows)?;
            let sales_map = match &sales {
                Some(p) => Some(render::load_sales(p)?),
                None => None,
            };
            fs::write(root.join("build/firms.html"), render::render(&rows, sales_map.as_ref()))?;
            let current = facts.values().flatten().filter(|a| a.is_current).count();
            println!("built: {} firms, {} current facts; wrote build/index.db, data/firms.csv, build/firms.html", rows.len(), current);
        }
        Cmd::IngestFirms { csv, at } => {
            if root.join("PAUSED").exists() {
                bail!("PAUSED file present; collection is paused");
            }
            chrono::NaiveDate::parse_from_str(&at, "%Y-%m-%d")?;
            let mut repo = model::Repo::load(&root)?;
            let allow = allowlist::Allowlist::load(&root)?;
            let s = ingest::ingest_firms(&mut repo, &allow, &csv, &at)?;
            println!("wrote {} ({} observations; {} new orgs, {} new persons)",
                s.file.strip_prefix(&root).unwrap_or(&s.file).display(), s.observations, s.new_orgs, s.new_persons);
            for n in s.notes {
                println!("note: {n}");
            }
        }
        Cmd::SalesInit { path } => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                if parent.canonicalize()?.starts_with(&root) {
                    bail!("the sales database must live outside the repo");
                }
            }
            render::sales_init(&path)?;
            println!("created {}", path.display());
        }
    }
    Ok(())
}
