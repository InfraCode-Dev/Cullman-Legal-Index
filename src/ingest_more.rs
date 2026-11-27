//! Ingest reviewed CSVs for public offices and for bar credential lookups.

use crate::allowlist::{Allowlist, Verdict};
use crate::ingest::norm;
use crate::model::{ExternalId, Observation, Org, Person, Post, Repo, Tenure, SCHEMA_VERSION};
use crate::runs;
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use ulid::Ulid;

pub struct Outcome {
    pub file: PathBuf,
    pub observations: usize,
    pub notes: Vec<String>,
}

const OFFICE_COLUMNS: &[&str] = &[
    "org_key", "org_name", "org_kind", "org_jurisdiction", "org_office_address", "org_office_phone",
    "org_website", "post_key", "post_title", "post_place", "selection", "post_jurisdiction",
    "person_name", "role", "status", "valid_from", "valid_to", "source", "url", "confidence", "notes",
];

fn rows(path: &Path, required: &[&str]) -> Result<Vec<BTreeMap<String, String>>> {
    let mut rdr = csv::Reader::from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let headers: Vec<String> = rdr.headers()?.iter().map(str::to_string).collect();
    for need in required {
        if !headers.iter().any(|h| h == need) {
            bail!("{} is missing the `{need}` column", path.display());
        }
    }
    let mut out = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        out.push(headers.iter().cloned().zip(rec.iter().map(|v| v.trim().to_string())).collect());
    }
    Ok(out)
}

fn conf(v: &str) -> Option<&'static str> {
    match v.to_lowercase().as_str() {
        "high" => Some("high"),
        "med" | "medium" => Some("med"),
        "low" => Some("low"),
        _ => None,
    }
}

/// Public office holders: orgs, posts, persons and tenures, one row per holding.
pub fn ingest_offices(repo: &mut Repo, allow: &Allowlist, path: &Path, at: &str) -> Result<Outcome> {
    let jur: BTreeSet<String> = repo.jurisdictions.iter().map(|j| j.id.clone()).collect();
    let run = runs::new_run("offices");
    let mut obs = Vec::new();
    let mut notes = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, r) in rows(path, OFFICE_COLUMNS)?.into_iter().enumerate() {
        let line = i + 2;
        let g = |k: &str| r.get(k).cloned().unwrap_or_default();
        let (source, url) = (g("source"), g("url"));
        let Some(c) = conf(&g("confidence")) else {
            notes.push(format!("row {line}: skipped, confidence must be high, med or low"));
            continue;
        };
        if url.is_empty() || g("org_name").is_empty() || g("post_title").is_empty() {
            notes.push(format!("row {line}: skipped, needs org_name, post_title and url"));
            continue;
        }
        for j in [g("org_jurisdiction"), g("post_jurisdiction")] {
            if !j.is_empty() && !jur.contains(&j) {
                bail!("row {line}: unknown jurisdiction `{j}`");
            }
        }
        let kind = match g("org_kind").as_str() {
            k @ ("court" | "office" | "agency") => k.to_string(),
            other => bail!("row {line}: org_kind `{other}` must be court, office or agency"),
        };
        let org_id = match repo.orgs.iter().find(|o| o.kind == kind && norm(&o.name) == norm(&g("org_name"))) {
            Some(o) => o.id.clone(),
            None => {
                let id = Ulid::generate().to_string();
                repo.orgs.push(Org { id: id.clone(), kind: kind.clone(), name: g("org_name"), jurisdiction_id: g("org_jurisdiction") });
                id
            }
        };
        let post_id = match repo.posts.iter().find(|p| p.org_id == org_id && norm(&p.title) == norm(&g("post_title")) && p.place == g("post_place")) {
            Some(p) => p.id.clone(),
            None => {
                let id = Ulid::generate().to_string();
                repo.posts.push(Post {
                    id: id.clone(),
                    org_id: org_id.clone(),
                    title: g("post_title"),
                    place: g("post_place"),
                    selection: g("selection"),
                    jurisdiction_id: g("post_jurisdiction"),
                });
                id
            }
        };
        let mut push = |entity: String, field: &str, value: String, notes: &mut Vec<String>| {
            if value.is_empty() {
                return;
            }
            let kind = entity.split(':').next().unwrap_or("");
            match allow.check(&source, kind, field) {
                Verdict::Ok => obs.push(Observation {
                    v: SCHEMA_VERSION,
                    run: run.clone(),
                    source: source.clone(),
                    at: at.to_string(),
                    entity,
                    field: field.to_string(),
                    value,
                    url: url.clone(),
                    confidence: c.to_string(),
                }),
                Verdict::Forbidden(why) => notes.push(format!("row {line}: not stored: {why}")),
            }
        };
        if seen.insert(format!("{org_id}|{source}")) {
            push(format!("org:{org_id}"), "name", g("org_name"), &mut notes);
            if allow.looks_residential(&g("org_office_address")) {
                notes.push(format!("row {line}: office address looks residential; not stored"));
            } else {
                push(format!("org:{org_id}"), "office_address", g("org_office_address"), &mut notes);
            }
            push(format!("org:{org_id}"), "office_phone", g("org_office_phone"), &mut notes);
            push(format!("org:{org_id}"), "website", g("org_website"), &mut notes);
        }
        if g("person_name").is_empty() {
            continue;
        }
        let pid = match repo.persons.iter().find(|p| norm(&p.name) == norm(&g("person_name"))) {
            Some(p) => p.id.clone(),
            None => {
                let id = Ulid::generate().to_string();
                repo.persons.push(Person { id: id.clone(), name: g("person_name"), name_variants: String::new() });
                id
            }
        };
        let tid = match repo.tenures.iter().find(|t| t.person_id == pid && t.post_id == post_id) {
            Some(t) => t.id.clone(),
            None => {
                let id = Ulid::generate().to_string();
                repo.tenures.push(Tenure {
                    id: id.clone(),
                    person_id: pid.clone(),
                    org_id: org_id.clone(),
                    post_id: post_id.clone(),
                    role: g("role"),
                });
                id
            }
        };
        push(format!("person:{pid}"), "name", g("person_name"), &mut notes);
        for f in ["role", "status", "valid_from", "valid_to"] {
            push(format!("tenure:{tid}"), f, g(f), &mut notes);
        }
    }
    let file = runs::write_observations(repo, &run, &obs)?;
    repo.save_registry()?;
    Ok(Outcome { file, observations: obs.len(), notes })
}

const BAR_FIELDS: &[&str] = &["bar_status", "admit_date", "law_school", "firm_name_listed", "city_of_record", "discipline_page_id"];

/// Bar credential lookups for people already in the registry. Only exact
/// matches are stored, and only the allowlisted credential fields.
pub fn ingest_bar(repo: &mut Repo, allow: &Allowlist, path: &Path) -> Result<Outcome> {
    let run = runs::new_run("bar-lookup");
    let mut obs = Vec::new();
    let mut notes = Vec::new();
    for r in rows(path, &["person_id", "match", "searched_at", "url"])? {
        let g = |k: &str| r.get(k).cloned().unwrap_or_default();
        let pid = g("person_id");
        if !repo.persons.iter().any(|p| p.id == pid) {
            bail!("person {pid} is not in the registry");
        }
        if g("match") != "exact" {
            notes.push(format!("{}: {} match, nothing stored", g("person_name"), g("match")));
            continue;
        }
        for f in BAR_FIELDS {
            let v = g(f);
            if v.is_empty() {
                continue;
            }
            if allow.check("bar-lookup", "person", f) != Verdict::Ok {
                bail!("bar-lookup may not write person.{f}");
            }
            obs.push(Observation {
                v: SCHEMA_VERSION,
                run: run.clone(),
                source: "bar-lookup".into(),
                at: g("searched_at"),
                entity: format!("person:{pid}"),
                field: f.to_string(),
                value: v,
                url: g("url"),
                confidence: "high".into(),
            });
        }
        // Composite key: the bar's on-page number is random per request.
        let key = format!("{}|{}|{}", norm(&g("person_name")), g("admit_date"), norm(&g("law_school")));
        if !g("admit_date").is_empty() {
            match repo.external_ids.iter_mut().find(|x| x.entity_id == pid && x.scheme == "bar_composite") {
                Some(x) => x.last_seen = g("searched_at"),
                None => repo.external_ids.push(ExternalId {
                    entity_id: pid.clone(),
                    scheme: "bar_composite".into(),
                    value: key,
                    first_seen: g("searched_at"),
                    last_seen: g("searched_at"),
                }),
            }
        }
        let dpid = g("discipline_page_id");
        if !dpid.is_empty() && !repo.external_ids.iter().any(|x| x.entity_id == pid && x.scheme == "bar_discipline_page") {
            repo.external_ids.push(ExternalId {
                entity_id: pid.clone(),
                scheme: "bar_discipline_page".into(),
                value: dpid,
                first_seen: g("searched_at"),
                last_seen: g("searched_at"),
            });
        }
    }
    let file = runs::write_observations(repo, &run, &obs)?;
    repo.save_registry()?;
    Ok(Outcome { file, observations: obs.len(), notes })
}
