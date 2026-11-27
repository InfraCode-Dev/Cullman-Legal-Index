//! Repo validation: run on every pull request; any error blocks the merge.

use crate::allowlist::{Allowlist, Verdict};
use crate::model::{confidence_rank, Repo, SCHEMA_VERSION};
use anyhow::{bail, Result};
use chrono::{Datelike, NaiveDate, Utc};
use std::collections::BTreeSet;
use std::process::Command;

const DATE_FIELDS: &[&str] = &["valid_from", "valid_to", "formation_date", "admit_date", "discipline_date"];
const ENTITY_KINDS: &[&str] = &["org", "person", "tenure", "post"];

pub fn validate(repo: &Repo, allow: &Allowlist, base: Option<&str>) -> Vec<String> {
    let mut errs = Vec::new();
    check_schema_version(repo, &mut errs);
    check_registry(repo, &mut errs);
    check_observations(repo, allow, &mut errs);
    check_leads(repo, &mut errs);
    if let Some(b) = base {
        if let Err(e) = check_append_only(repo, b, &mut errs) {
            errs.push(format!("append-only check failed to run: {e}"));
        }
    }
    errs
}

fn check_schema_version(repo: &Repo, errs: &mut Vec<String>) {
    let p = repo.root.join("schema/schema_version");
    match std::fs::read_to_string(&p) {
        Ok(s) if s.trim() == SCHEMA_VERSION.to_string() => {}
        Ok(s) => errs.push(format!("schema_version is {} but this build expects {SCHEMA_VERSION}; run a migration", s.trim())),
        Err(e) => errs.push(format!("cannot read schema/schema_version: {e}")),
    }
}

fn dup_check<'a>(kind: &str, ids: impl Iterator<Item = &'a String>, errs: &mut Vec<String>) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if id.trim().is_empty() {
            errs.push(format!("{kind}: empty id"));
        } else if !seen.insert(id.clone()) {
            errs.push(format!("{kind}: duplicate id {id}"));
        }
    }
    seen
}

fn check_registry(repo: &Repo, errs: &mut Vec<String>) {
    let jur = dup_check("jurisdiction", repo.jurisdictions.iter().map(|j| &j.id), errs);
    let orgs = dup_check("org", repo.orgs.iter().map(|o| &o.id), errs);
    let persons = dup_check("person", repo.persons.iter().map(|p| &p.id), errs);
    let posts = dup_check("post", repo.posts.iter().map(|p| &p.id), errs);
    dup_check("tenure", repo.tenures.iter().map(|t| &t.id), errs);

    for r in &repo.relations {
        for id in [&r.child_id, &r.parent_id] {
            if !jur.contains(id) {
                errs.push(format!("jurisdiction relation references unknown jurisdiction {id}"));
            }
        }
        check_opt_date("jurisdiction relation", &r.valid_from, errs);
        check_opt_date("jurisdiction relation", &r.valid_to, errs);
    }
    for o in &repo.orgs {
        if !o.jurisdiction_id.is_empty() && !jur.contains(&o.jurisdiction_id) {
            errs.push(format!("org {} references unknown jurisdiction {}", o.id, o.jurisdiction_id));
        }
    }
    for p in &repo.posts {
        if !orgs.contains(&p.org_id) {
            errs.push(format!("post {} references unknown org {}", p.id, p.org_id));
        }
        if !p.jurisdiction_id.is_empty() && !jur.contains(&p.jurisdiction_id) {
            errs.push(format!("post {} references unknown jurisdiction {}", p.id, p.jurisdiction_id));
        }
    }
    for t in &repo.tenures {
        if !persons.contains(&t.person_id) {
            errs.push(format!("tenure {} references unknown person {}", t.id, t.person_id));
        }
        if !t.org_id.is_empty() && !orgs.contains(&t.org_id) {
            errs.push(format!("tenure {} references unknown org {}", t.id, t.org_id));
        }
        if !t.post_id.is_empty() && !posts.contains(&t.post_id) {
            errs.push(format!("tenure {} references unknown post {}", t.id, t.post_id));
        }
        if t.org_id.is_empty() && t.post_id.is_empty() {
            errs.push(format!("tenure {} needs an org or a post", t.id));
        }
    }
    let all: BTreeSet<&String> = orgs.iter().chain(persons.iter()).chain(posts.iter()).collect();
    for s in &repo.same_as {
        for id in [&s.loser_id, &s.winner_id] {
            if !all.contains(id) {
                errs.push(format!("same_as references unknown id {id}"));
            }
        }
    }
    for x in &repo.external_ids {
        if !all.contains(&x.entity_id) {
            errs.push(format!("external id {}:{} references unknown entity {}", x.scheme, x.value, x.entity_id));
        }
        // The bar search's on-page number is regenerated on every request.
        if x.scheme == "bar_code_random" || x.scheme == "bar_result_id" {
            errs.push(format!("external id scheme `{}` is not stable and must not be stored", x.scheme));
        }
        check_opt_date("external id", &x.first_seen, errs);
        check_opt_date("external id", &x.last_seen, errs);
    }
}

fn check_opt_date(ctx: &str, v: &str, errs: &mut Vec<String>) {
    if v.is_empty() {
        return;
    }
    match NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        Ok(d) => {
            let limit = Utc::now().date_naive().year() + 6;
            if d.year() < 1900 || d.year() > limit {
                errs.push(format!("{ctx}: implausible date {v}"));
            }
        }
        Err(_) => errs.push(format!("{ctx}: `{v}` is not a YYYY-MM-DD date")),
    }
}

fn check_observations(repo: &Repo, allow: &Allowlist, errs: &mut Vec<String>) {
    let ids: BTreeSet<String> = repo
        .orgs
        .iter()
        .map(|o| format!("org:{}", o.id))
        .chain(repo.persons.iter().map(|p| format!("person:{}", p.id)))
        .chain(repo.tenures.iter().map(|t| format!("tenure:{}", t.id)))
        .chain(repo.posts.iter().map(|p| format!("post:{}", p.id)))
        .collect();
    let today = Utc::now().date_naive();
    for (file, lines) in &repo.observations {
        let mut runs = BTreeSet::new();
        for (i, o) in lines.iter().enumerate() {
            let at = format!("{file}:{}", i + 1);
            runs.insert(o.run.clone());
            if o.v != SCHEMA_VERSION {
                errs.push(format!("{at}: schema version {} (expected {SCHEMA_VERSION})", o.v));
            }
            if !ENTITY_KINDS.contains(&o.entity_kind()) {
                errs.push(format!("{at}: unknown entity kind in `{}`", o.entity));
            } else if !ids.contains(&o.entity) {
                errs.push(format!("{at}: entity {} is not in the registry", o.entity));
            }
            if let Verdict::Forbidden(why) = allow.check(&o.source, o.entity_kind(), &o.field) {
                errs.push(format!("{at}: {why}"));
            }
            match NaiveDate::parse_from_str(&o.at, "%Y-%m-%d") {
                Ok(d) if d > today => errs.push(format!("{at}: observed date {} is in the future", o.at)),
                Ok(_) => {}
                Err(_) => errs.push(format!("{at}: `at` must be YYYY-MM-DD")),
            }
            if confidence_rank(&o.confidence) == 0 {
                errs.push(format!("{at}: confidence must be high, med or low"));
            }
            if o.url.trim().is_empty() {
                errs.push(format!("{at}: every observation needs a source url (or manual:<note>)"));
            }
            if DATE_FIELDS.contains(&o.field.as_str()) {
                check_opt_date(&at, &o.value, errs);
            }
            if allow.is_address_field(&o.field) && allow.looks_residential(&o.value) {
                let cleared = repo.overrides.iter().any(|ov| {
                    format!("{}:{}", o.entity_kind(), ov.entity_id) == o.entity
                        && ov.field == o.field
                        && ov.value == "allow-address"
                });
                if !cleared {
                    errs.push(format!(
                        "{at}: `{}` looks residential; store the city only, or add a reviewed override (field {}, value allow-address)",
                        o.value, o.field
                    ));
                }
            }
        }
        if runs.len() > 1 {
            errs.push(format!("{file}: one observation file must hold exactly one run"));
        }
    }
}

/// Observation files may only be added, never modified, renamed or deleted.
fn check_append_only(repo: &Repo, base: &str, errs: &mut Vec<String>) -> Result<()> {
    let out = Command::new("git")
        .args(["diff", "--name-status", "--no-renames", base, "--", "observations/", "leads/"])
        .current_dir(&repo.root)
        .output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr));
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");
        if status != "A" && path.ends_with(".jsonl") {
            errs.push(format!("{path}: observation and lead files are append-only (git status {status})"));
        }
    }
    Ok(())
}

fn check_leads(repo: &Repo, errs: &mut Vec<String>) {
    match crate::collect::load_leads(&repo.root) {
        Err(e) => errs.push(format!("leads: {e:#}")),
        Ok(files) => {
            for (file, leads) in files {
                let runs: BTreeSet<&str> = leads.iter().map(|l| l.run.as_str()).collect();
                if runs.len() > 1 {
                    errs.push(format!("{file}: one lead file must hold exactly one run"));
                }
                for (i, l) in leads.iter().enumerate() {
                    if l.v != SCHEMA_VERSION {
                        errs.push(format!("{file}:{}: schema version {}", i + 1, l.v));
                    }
                    if !l.url.starts_with("https://") {
                        errs.push(format!("{file}:{}: lead url must be https", i + 1));
                    }
                }
            }
        }
    }
}
