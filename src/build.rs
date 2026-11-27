//! Derive assertions from observations and emit the database and firm table.

use crate::model::{confidence_rank, Observation, Repo};
use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// One distinct value a field has had, with when it was first and last seen.
#[derive(Debug, Clone)]
pub struct Assertion {
    pub entity: String,
    pub field: String,
    pub value: String,
    pub first_seen: String,
    pub last_confirmed: String,
    pub confidence: String,
    pub sources: BTreeSet<String>,
    pub is_override: bool,
    pub is_current: bool,
}

pub type Facts = BTreeMap<(String, String), Vec<Assertion>>;

/// Group observations into assertions, then pick the current value per field.
/// Resolution order: reviewed override, then higher confidence, then the most
/// recently confirmed value.
pub fn derive(repo: &Repo) -> Facts {
    let mut grouped: BTreeMap<(String, String, String), Assertion> = BTreeMap::new();
    for o in repo.observations.values().flatten() {
        let entity = canonical_entity(repo, o);
        let key = (entity.clone(), o.field.clone(), o.value.clone());
        let a = grouped.entry(key).or_insert_with(|| Assertion {
            entity,
            field: o.field.clone(),
            value: o.value.clone(),
            first_seen: o.at.clone(),
            last_confirmed: o.at.clone(),
            confidence: o.confidence.clone(),
            sources: BTreeSet::new(),
            is_override: false,
            is_current: false,
        });
        if o.at < a.first_seen {
            a.first_seen = o.at.clone();
        }
        if o.at > a.last_confirmed {
            a.last_confirmed = o.at.clone();
        }
        if confidence_rank(&o.confidence) > confidence_rank(&a.confidence) {
            a.confidence = o.confidence.clone();
        }
        for u in o.url.split(';').map(str::trim).filter(|u| !u.is_empty()) {
            a.sources.insert(u.to_string());
        }
    }
    let mut facts: Facts = BTreeMap::new();
    for a in grouped.into_values() {
        facts.entry((a.entity.clone(), a.field.clone())).or_default().push(a);
    }
    for ov in &repo.overrides {
        if ov.value == "allow-address" {
            continue;
        }
        let entity = find_entity(repo, &ov.entity_id);
        facts.entry((entity.clone(), ov.field.clone())).or_default().push(Assertion {
            entity,
            field: ov.field.clone(),
            value: ov.value.clone(),
            first_seen: ov.decided_at.clone(),
            last_confirmed: ov.decided_at.clone(),
            confidence: "high".into(),
            sources: BTreeSet::from([format!("override: {}", ov.reason)]),
            is_override: true,
            is_current: false,
        });
    }
    for list in facts.values_mut() {
        let best = list
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| {
                (a.is_override, confidence_rank(&a.confidence), &a.last_confirmed)
                    .cmp(&(b.is_override, confidence_rank(&b.confidence), &b.last_confirmed))
            })
            .map(|(i, _)| i);
        if let Some(i) = best {
            list[i].is_current = true;
        }
    }
    facts
}

fn canonical_entity(repo: &Repo, o: &Observation) -> String {
    format!("{}:{}", o.entity_kind(), repo.canonical(o.entity_id()))
}

fn find_entity(repo: &Repo, id: &str) -> String {
    let id = repo.canonical(id);
    let kind = if repo.orgs.iter().any(|o| o.id == id) {
        "org"
    } else if repo.persons.iter().any(|p| p.id == id) {
        "person"
    } else if repo.tenures.iter().any(|t| t.id == id) {
        "tenure"
    } else {
        "post"
    };
    format!("{kind}:{id}")
}

pub fn current<'a>(facts: &'a Facts, entity: &str, field: &str) -> Option<&'a Assertion> {
    facts
        .get(&(entity.to_string(), field.to_string()))
        .and_then(|l| l.iter().find(|a| a.is_current))
}

pub const FIRM_COLUMNS: &[&str] = &[
    "id", "name", "office_address", "city", "office_phone", "website", "entity_type",
    "sos_entity_id", "formation_date", "attorney_count", "attorneys", "practice_areas",
    "closings", "mx_provider", "spf", "dmarc_policy", "confirmed_by", "sources", "last_confirmed",
];

/// Attorneys currently listed at an org, from tenures whose current status is incumbent.
pub fn org_attorneys(repo: &Repo, facts: &Facts, org_id: &str) -> Vec<String> {
    let mut names = Vec::new();
    for t in repo.tenures.iter().filter(|t| repo.canonical(&t.org_id) == org_id) {
        let te = format!("tenure:{}", t.id);
        let status = current(facts, &te, "status").map(|a| a.value.as_str()).unwrap_or("");
        if status != "incumbent" {
            continue;
        }
        let pid = repo.canonical(&t.person_id);
        let name = current(facts, &format!("person:{pid}"), "name")
            .map(|a| a.value.clone())
            .or_else(|| repo.persons.iter().find(|p| p.id == pid).map(|p| p.name.clone()));
        if let Some(n) = name {
            names.push(n);
        }
    }
    names.sort();
    names.dedup();
    names
}

pub fn firm_rows(repo: &Repo, facts: &Facts) -> Vec<BTreeMap<&'static str, String>> {
    let mut rows = Vec::new();
    for o in repo.orgs.iter().filter(|o| o.kind == "firm" && repo.canonical(&o.id) == o.id) {
        let ent = format!("org:{}", o.id);
        let mut row = BTreeMap::new();
        let mut sources = BTreeSet::new();
        let mut last = String::new();
        for col in FIRM_COLUMNS {
            let v = match *col {
                "id" => o.id.clone(),
                "attorneys" => org_attorneys(repo, facts, &o.id).join("; "),
                "sources" | "last_confirmed" => String::new(),
                _ => match current(facts, &ent, col) {
                    Some(a) => {
                        sources.extend(a.sources.iter().cloned());
                        if a.last_confirmed > last {
                            last = a.last_confirmed.clone();
                        }
                        a.value.clone()
                    }
                    None if *col == "name" => o.name.clone(),
                    None => String::new(),
                },
            };
            row.insert(*col, v);
        }
        row.insert("sources", sources.into_iter().collect::<Vec<_>>().join("; "));
        row.insert("last_confirmed", last);
        rows.push(row);
    }
    rows.sort_by(|a, b| a["name"].to_lowercase().cmp(&b["name"].to_lowercase()));
    rows
}

pub fn write_firms_csv(path: &Path, rows: &[BTreeMap<&'static str, String>]) -> Result<()> {
    let mut w = csv::Writer::from_path(path)?;
    w.write_record(FIRM_COLUMNS)?;
    for r in rows {
        w.write_record(FIRM_COLUMNS.iter().map(|c| r[c].as_str()))?;
    }
    w.flush()?;
    Ok(())
}

pub fn write_db(repo: &Repo, facts: &Facts, path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    let mut db = Connection::open(path)?;
    db.execute_batch(
        "CREATE TABLE jurisdictions (id TEXT PRIMARY KEY, kind TEXT, name TEXT, fips TEXT);
         CREATE TABLE jurisdiction_relations (child_id TEXT, parent_id TEXT, relation TEXT, valid_from TEXT, valid_to TEXT);
         CREATE TABLE orgs (id TEXT PRIMARY KEY, kind TEXT, name TEXT, jurisdiction_id TEXT);
         CREATE TABLE persons (id TEXT PRIMARY KEY, name TEXT, name_variants TEXT);
         CREATE TABLE posts (id TEXT PRIMARY KEY, org_id TEXT, title TEXT, place TEXT, selection TEXT, jurisdiction_id TEXT);
         CREATE TABLE tenures (id TEXT PRIMARY KEY, person_id TEXT, org_id TEXT, post_id TEXT, role TEXT);
         CREATE TABLE observations (file TEXT, line INTEGER, run TEXT, source TEXT, at TEXT, entity TEXT, field TEXT, value TEXT, url TEXT, confidence TEXT);
         CREATE TABLE assertions (entity TEXT, field TEXT, value TEXT, first_seen TEXT, last_confirmed TEXT, confidence TEXT, sources TEXT, is_override INTEGER, is_current INTEGER);
         CREATE VIEW current_facts AS SELECT entity, field, value, last_confirmed, confidence, sources FROM assertions WHERE is_current = 1;",
    )?;
    let tx = db.transaction()?;
    for j in &repo.jurisdictions {
        tx.execute("INSERT INTO jurisdictions VALUES (?1,?2,?3,?4)", params![j.id, j.kind, j.name, j.fips])?;
    }
    for r in &repo.relations {
        tx.execute(
            "INSERT INTO jurisdiction_relations VALUES (?1,?2,?3,?4,?5)",
            params![r.child_id, r.parent_id, r.relation, r.valid_from, r.valid_to],
        )?;
    }
    for o in &repo.orgs {
        tx.execute("INSERT INTO orgs VALUES (?1,?2,?3,?4)", params![o.id, o.kind, o.name, o.jurisdiction_id])?;
    }
    for p in &repo.persons {
        tx.execute("INSERT INTO persons VALUES (?1,?2,?3)", params![p.id, p.name, p.name_variants])?;
    }
    for p in &repo.posts {
        tx.execute(
            "INSERT INTO posts VALUES (?1,?2,?3,?4,?5,?6)",
            params![p.id, p.org_id, p.title, p.place, p.selection, p.jurisdiction_id],
        )?;
    }
    for t in &repo.tenures {
        tx.execute(
            "INSERT INTO tenures VALUES (?1,?2,?3,?4,?5)",
            params![t.id, t.person_id, t.org_id, t.post_id, t.role],
        )?;
    }
    for (file, lines) in &repo.observations {
        for (i, o) in lines.iter().enumerate() {
            tx.execute(
                "INSERT INTO observations VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![file, (i + 1) as i64, o.run, o.source, o.at, o.entity, o.field, o.value, o.url, o.confidence],
            )?;
        }
    }
    for a in facts.values().flatten() {
        tx.execute(
            "INSERT INTO assertions VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                a.entity,
                a.field,
                a.value,
                a.first_seen,
                a.last_confirmed,
                a.confidence,
                a.sources.iter().cloned().collect::<Vec<_>>().join("; "),
                a.is_override as i64,
                a.is_current as i64
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Org, Override};

    fn obs(at: &str, value: &str, conf: &str) -> Observation {
        Observation {
            v: 1,
            run: format!("{at}-t"),
            source: "manual".into(),
            at: at.into(),
            entity: "org:o1".into(),
            field: "office_phone".into(),
            value: value.into(),
            url: "manual:test".into(),
            confidence: conf.into(),
        }
    }

    fn repo_with(obs_list: Vec<Observation>) -> Repo {
        let mut r = Repo::default();
        r.orgs.push(Org { id: "o1".into(), kind: "firm".into(), name: "Test".into(), jurisdiction_id: String::new() });
        r.observations.insert("observations/t.jsonl".into(), obs_list);
        r
    }

    #[test]
    fn reconfirmation_updates_last_confirmed_only() {
        let r = repo_with(vec![obs("2026-01-01", "555-0100", "high"), obs("2026-04-01", "555-0100", "high")]);
        let f = derive(&r);
        let list = &f[&("org:o1".to_string(), "office_phone".to_string())];
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].first_seen, "2026-01-01");
        assert_eq!(list[0].last_confirmed, "2026-04-01");
    }

    #[test]
    fn confidence_beats_recency_and_override_beats_both() {
        let mut r = repo_with(vec![obs("2026-01-01", "555-0100", "high"), obs("2026-04-01", "555-0199", "low")]);
        let f = derive(&r);
        assert_eq!(current(&f, "org:o1", "office_phone").unwrap().value, "555-0100");
        r.overrides.push(Override {
            entity_id: "o1".into(),
            field: "office_phone".into(),
            value: "555-0123".into(),
            decided_at: "2026-05-01".into(),
            reason: "called office".into(),
        });
        let f = derive(&r);
        assert_eq!(current(&f, "org:o1", "office_phone").unwrap().value, "555-0123");
    }
}
