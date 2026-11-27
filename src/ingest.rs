//! Turn a reviewed firm-table CSV into registry entries plus one observation file.

use crate::allowlist::{Allowlist, Verdict};
use crate::model::{Observation, Org, Person, Repo, Tenure, SCHEMA_VERSION};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use ulid::Ulid;

const SOURCE: &str = "firm-table";
const ORG_FIELDS: &[&str] = &[
    "office_address", "city", "office_phone", "website", "entity_type", "sos_entity_id",
    "formation_date", "attorney_count", "practice_areas", "closings", "mx_provider", "spf",
    "dmarc_policy", "places_place_id", "confirmed_by", "notes",
];

pub fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct Summary {
    pub file: PathBuf,
    pub new_orgs: usize,
    pub new_persons: usize,
    pub observations: usize,
    pub notes: Vec<String>,
}

pub fn ingest_firms(repo: &mut Repo, allow: &Allowlist, csv_path: &Path, at: &str) -> Result<Summary> {
    let mut rdr = csv::Reader::from_path(csv_path).with_context(|| format!("reading {}", csv_path.display()))?;
    let headers = rdr.headers()?.clone();
    for need in ["name", "sources", "confidence"] {
        if !headers.iter().any(|h| h == need) {
            bail!("firm CSV is missing the `{need}` column");
        }
    }
    let now = Utc::now();
    let run = format!("{}-{SOURCE}", now.format("%Y%m%dT%H%M%S"));
    let mut out: Vec<Observation> = Vec::new();
    let mut notes = Vec::new();
    let (mut new_orgs, mut new_persons) = (0, 0);

    for rec in rdr.records() {
        let rec = rec?;
        let row: BTreeMap<&str, &str> = headers.iter().zip(rec.iter()).collect();
        let name = row.get("name").copied().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let conf = match row.get("confidence").copied().unwrap_or("").to_lowercase().as_str() {
            "high" => "high",
            "med" | "medium" => "med",
            _ => {
                notes.push(format!("skipped {name}: confidence must be High or Med for the firm table"));
                continue;
            }
        };
        let url = row.get("sources").copied().unwrap_or("").trim().to_string();
        if url.is_empty() {
            notes.push(format!("skipped {name}: no sources"));
            continue;
        }
        let org_id = match repo.orgs.iter().find(|o| o.kind == "firm" && norm(&o.name) == norm(name)) {
            Some(o) => o.id.clone(),
            None => {
                let id = Ulid::generate().to_string();
                repo.orgs.push(Org {
                    id: id.clone(),
                    kind: "firm".into(),
                    name: name.to_string(),
                    jurisdiction_id: "jur-cullman-county".into(),
                });
                new_orgs += 1;
                id
            }
        };
        let mut push = |entity: String, field: &str, value: &str| {
            out.push(Observation {
                v: SCHEMA_VERSION,
                run: run.clone(),
                source: SOURCE.into(),
                at: at.into(),
                entity,
                field: field.into(),
                value: value.into(),
                url: url.clone(),
                confidence: conf.into(),
            });
        };
        push(format!("org:{org_id}"), "name", name);
        for f in ORG_FIELDS {
            let Some(v) = row.get(f).map(|v| v.trim()).filter(|v| !v.is_empty()) else { continue };
            if allow.check(SOURCE, "org", f) != Verdict::Ok {
                continue;
            }
            if allow.is_address_field(f) && allow.looks_residential(v) {
                notes.push(format!("{name}: address dropped as possibly residential; city kept"));
                continue;
            }
            push(format!("org:{org_id}"), f, v);
        }
        for attorney in row.get("attorneys").copied().unwrap_or("").split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let pid = match repo.persons.iter().find(|p| norm(&p.name) == norm(attorney)) {
                Some(p) => p.id.clone(),
                None => {
                    let id = Ulid::generate().to_string();
                    repo.persons.push(Person { id: id.clone(), name: attorney.into(), name_variants: String::new() });
                    new_persons += 1;
                    id
                }
            };
            let tid = match repo.tenures.iter().find(|t| t.person_id == pid && t.org_id == org_id && t.role == "attorney") {
                Some(t) => t.id.clone(),
                None => {
                    let id = Ulid::generate().to_string();
                    repo.tenures.push(Tenure {
                        id: id.clone(),
                        person_id: pid.clone(),
                        org_id: org_id.clone(),
                        post_id: String::new(),
                        role: "attorney".into(),
                    });
                    id
                }
            };
            push(format!("person:{pid}"), "name", attorney);
            push(format!("tenure:{tid}"), "role", "attorney");
            push(format!("tenure:{tid}"), "status", "incumbent");
        }
    }
    if out.is_empty() {
        bail!("nothing to ingest");
    }
    let dir = repo.root.join(format!("observations/{}/{}", now.format("%Y"), now.format("%m")));
    fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{run}.jsonl"));
    if file.exists() {
        bail!("{} already exists; observation files are never overwritten", file.display());
    }
    let mut f = fs::File::create(&file)?;
    for o in &out {
        writeln!(f, "{}", serde_json::to_string(o)?)?;
    }
    repo.save_registry()?;
    Ok(Summary { file, new_orgs, new_persons, observations: out.len(), notes })
}
