//! Types for the registry, decisions and observation files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Jurisdiction {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub fips: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JurisdictionRelation {
    pub child_id: String,
    pub parent_id: String,
    pub relation: String,
    pub valid_from: String,
    pub valid_to: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Org {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub jurisdiction_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub name_variants: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Post {
    pub id: String,
    pub org_id: String,
    pub title: String,
    pub place: String,
    pub selection: String,
    pub jurisdiction_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tenure {
    pub id: String,
    pub person_id: String,
    pub org_id: String,
    pub post_id: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExternalId {
    pub entity_id: String,
    pub scheme: String,
    pub value: String,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SameAs {
    pub loser_id: String,
    pub winner_id: String,
    pub decided_at: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Override {
    pub entity_id: String,
    pub field: String,
    pub value: String,
    pub decided_at: String,
    pub reason: String,
}

/// One line of an observation file: what one source said about one field of
/// one entity at one moment. Observation files are append-only.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Observation {
    pub v: u32,
    pub run: String,
    pub source: String,
    /// Date the source was read (YYYY-MM-DD).
    pub at: String,
    /// `kind:id`, for example `org:01J...` or `person:01J...`.
    pub entity: String,
    pub field: String,
    pub value: String,
    pub url: String,
    /// high, med or low.
    pub confidence: String,
}

impl Observation {
    pub fn entity_kind(&self) -> &str {
        self.entity.split_once(':').map(|(k, _)| k).unwrap_or("")
    }
    pub fn entity_id(&self) -> &str {
        self.entity.split_once(':').map(|(_, i)| i).unwrap_or("")
    }
}

pub fn confidence_rank(c: &str) -> u8 {
    match c {
        "high" => 3,
        "med" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// Everything canonical in the repo, loaded into memory.
#[derive(Debug, Default)]
pub struct Repo {
    pub root: PathBuf,
    pub jurisdictions: Vec<Jurisdiction>,
    pub relations: Vec<JurisdictionRelation>,
    pub orgs: Vec<Org>,
    pub persons: Vec<Person>,
    pub posts: Vec<Post>,
    pub tenures: Vec<Tenure>,
    pub external_ids: Vec<ExternalId>,
    pub same_as: Vec<SameAs>,
    pub overrides: Vec<Override>,
    /// Observation file path (relative to root) to its lines, in file order.
    pub observations: BTreeMap<String, Vec<Observation>>,
}

pub fn read_csv<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let mut rdr = csv::Reader::from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for (i, rec) in rdr.deserialize().enumerate() {
        out.push(rec.with_context(|| format!("{} row {}", path.display(), i + 2))?);
    }
    Ok(out)
}

pub fn write_csv<T: Serialize>(path: &Path, rows: &[T], header: &[&str]) -> Result<()> {
    let mut wtr = csv::WriterBuilder::new().has_headers(false).from_path(path)?;
    wtr.write_record(header)?;
    for r in rows {
        wtr.serialize(r)?;
    }
    wtr.flush()?;
    Ok(())
}

impl Repo {
    pub fn load(root: &Path) -> Result<Repo> {
        let r = |p: &str| root.join(p);
        let mut repo = Repo {
            root: root.to_path_buf(),
            jurisdictions: read_csv(&r("registry/jurisdictions.csv"))?,
            relations: read_csv(&r("registry/jurisdiction_relations.csv"))?,
            orgs: read_csv(&r("registry/orgs.csv"))?,
            persons: read_csv(&r("registry/persons.csv"))?,
            posts: read_csv(&r("registry/posts.csv"))?,
            tenures: read_csv(&r("registry/tenures.csv"))?,
            external_ids: read_csv(&r("registry/external_ids.csv"))?,
            same_as: read_csv(&r("decisions/same_as.csv"))?,
            overrides: read_csv(&r("decisions/overrides.csv"))?,
            observations: BTreeMap::new(),
        };
        let mut files = Vec::new();
        collect_jsonl(&r("observations"), &mut files)?;
        files.sort();
        for f in files {
            let rel = f.strip_prefix(root).unwrap_or(&f).to_string_lossy().replace('\\', "/");
            let text = fs::read_to_string(&f)?;
            let mut lines = Vec::new();
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let o: Observation = serde_json::from_str(line)
                    .with_context(|| format!("{} line {}", rel, i + 1))?;
                lines.push(o);
            }
            repo.observations.insert(rel, lines);
        }
        Ok(repo)
    }

    pub fn save_registry(&self) -> Result<()> {
        let r = |p: &str| self.root.join(p);
        write_csv(&r("registry/orgs.csv"), &self.orgs, &["id", "kind", "name", "jurisdiction_id"])?;
        write_csv(&r("registry/persons.csv"), &self.persons, &["id", "name", "name_variants"])?;
        write_csv(&r("registry/tenures.csv"), &self.tenures, &["id", "person_id", "org_id", "post_id", "role"])?;
        write_csv(&r("registry/posts.csv"), &self.posts, &["id", "org_id", "title", "place", "selection", "jurisdiction_id"])?;
        write_csv(&r("registry/external_ids.csv"), &self.external_ids, &["entity_id", "scheme", "value", "first_seen", "last_seen"])?;
        Ok(())
    }

    /// Follow same_as decisions to the surviving id.
    pub fn canonical<'a>(&'a self, id: &'a str) -> &'a str {
        let mut cur = id;
        for _ in 0..16 {
            match self.same_as.iter().find(|s| s.loser_id == cur) {
                Some(s) => cur = &s.winner_id,
                None => break,
            }
        }
        cur
    }
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect_jsonl(&p, out)?;
        } else if p.extension().map(|x| x == "jsonl").unwrap_or(false) {
            out.push(p);
        }
    }
    Ok(())
}
