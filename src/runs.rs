//! Shared helpers for collection runs: run ids, the pause switch, and
//! writing one append-only file per run.

use crate::model::{Observation, Repo};
use anyhow::{bail, Result};
use chrono::Utc;
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn ensure_not_paused(root: &Path) -> Result<()> {
    if root.join("PAUSED").exists() {
        bail!("PAUSED file present at the repo root; collection is paused");
    }
    Ok(())
}

pub fn new_run(source: &str) -> String {
    format!("{}-{source}", Utc::now().format("%Y%m%dT%H%M%S"))
}

/// Write `lines` as JSONL to `<root>/<dir>/YYYY/MM/<run>.jsonl`. Never overwrites.
pub fn write_jsonl<T: Serialize>(root: &Path, dir: &str, run: &str, lines: &[T]) -> Result<PathBuf> {
    let now = Utc::now();
    let folder = root.join(format!("{dir}/{}/{}", now.format("%Y"), now.format("%m")));
    fs::create_dir_all(&folder)?;
    let file = folder.join(format!("{run}.jsonl"));
    if file.exists() {
        bail!("{} already exists; run files are never overwritten", file.display());
    }
    let mut f = fs::File::create(&file)?;
    for l in lines {
        writeln!(f, "{}", serde_json::to_string(l)?)?;
    }
    Ok(file)
}

pub fn write_observations(repo: &Repo, run: &str, obs: &[Observation]) -> Result<PathBuf> {
    if obs.is_empty() {
        bail!("nothing to write");
    }
    write_jsonl(&repo.root, "observations", run, obs)
}

/// A shared HTTP agent: honours HTTPS_PROXY and the system trust store,
/// sends a generic user agent with no personal details.
pub fn http() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("lidx/0.1 (private civic index; low-rate)")
        .build()
}
