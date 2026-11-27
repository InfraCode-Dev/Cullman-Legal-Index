//! Deterministic collectors. Each run writes one new append-only file:
//! DNS facts become observations; news posts become leads for review.

use crate::build::{current, derive};
use crate::ingest::norm;
use crate::model::{Observation, Repo, SCHEMA_VERSION};
use crate::runs;
use anyhow::{bail, Context, Result};
use chrono::{Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

// ---------- DNS ----------

const DOH: &str = "https://cloudflare-dns.com/dns-query";

fn doh(agent: &ureq::Agent, name: &str, typ: &str) -> Result<Vec<String>> {
    let resp: Value = agent
        .get(DOH)
        .query("name", name)
        .query("type", typ)
        .set("accept", "application/dns-json")
        .call()
        .with_context(|| format!("DNS {typ} {name}"))?
        .into_json()?;
    let status = resp["Status"].as_i64().unwrap_or(-1);
    if status != 0 && status != 3 {
        bail!("DNS {typ} {name}: resolver status {status}");
    }
    let want = if typ == "MX" { 15 } else { 16 };
    Ok(resp["Answer"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|x| x["type"].as_i64() == Some(want))
                .filter_map(|x| x["data"].as_str().map(|s| s.replace('"', "")))
                .collect()
        })
        .unwrap_or_default())
}

pub fn classify_mx(mx: &[String]) -> String {
    if mx.is_empty() {
        return "none".into();
    }
    let all = mx.join(" ").to_lowercase();
    if all.contains("protection.outlook.com") {
        return "M365".into();
    }
    if all.contains("google.com") || all.contains("googlemail.com") {
        return "Google".into();
    }
    if all.contains("secureserver.net") {
        return "GoDaddy".into();
    }
    let host = mx[0].split_whitespace().last().unwrap_or("").trim_end_matches('.');
    let parts: Vec<&str> = host.split('.').collect();
    let tail = if parts.len() >= 2 { parts[parts.len() - 2..].join(".") } else { host.to_string() };
    format!("other ({tail})")
}

pub fn dmarc_policy(records: &[String]) -> String {
    for r in records {
        if r.contains("v=DMARC1") {
            for part in r.split(';') {
                if let Some((k, v)) = part.trim().split_once('=') {
                    if k.trim() == "p" {
                        return v.trim().to_string();
                    }
                }
            }
            return "invalid".into();
        }
    }
    "missing".into()
}

pub fn domain_of(website: &str) -> Option<String> {
    let w = website.trim();
    if w.is_empty() {
        return None;
    }
    let rest = w.split("://").nth(1).unwrap_or(w);
    let host = rest.split(['/', '?', '#', ':']).next()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    host.contains('.').then_some(host)
}

pub fn collect_dns(repo: &Repo) -> Result<(PathBuf, usize, Vec<String>)> {
    runs::ensure_not_paused(&repo.root)?;
    let facts = derive(repo);
    let agent = runs::http();
    let run = runs::new_run("dns");
    let today = Utc::now().date_naive().to_string();
    let mut obs = Vec::new();
    let mut notes = Vec::new();
    for o in repo.orgs.iter().filter(|o| o.kind == "firm" && repo.canonical(&o.id) == o.id) {
        let ent = format!("org:{}", o.id);
        let Some(site) = current(&facts, &ent, "website") else { continue };
        let Some(dom) = domain_of(&site.value) else {
            notes.push(format!("{}: website `{}` has no domain", o.name, site.value));
            continue;
        };
        let res = (|| -> Result<(String, String, String)> {
            let mx = doh(&agent, &dom, "MX")?;
            std::thread::sleep(std::time::Duration::from_millis(300));
            let txt = doh(&agent, &dom, "TXT")?;
            std::thread::sleep(std::time::Duration::from_millis(300));
            let dm = doh(&agent, &format!("_dmarc.{dom}"), "TXT")?;
            std::thread::sleep(std::time::Duration::from_millis(300));
            let spf = if txt.iter().any(|t| t.contains("v=spf1")) { "present" } else { "missing" };
            Ok((classify_mx(&mx), spf.to_string(), dmarc_policy(&dm)))
        })();
        match res {
            Ok((mx, spf, dmarc)) => {
                for (field, value, typ) in [("mx_provider", mx, "MX"), ("spf", spf, "TXT"), ("dmarc_policy", dmarc, "TXT")] {
                    let qname = if field == "dmarc_policy" { format!("_dmarc.{dom}") } else { dom.clone() };
                    obs.push(Observation {
                        v: SCHEMA_VERSION,
                        run: run.clone(),
                        source: "dns".into(),
                        at: today.clone(),
                        entity: ent.clone(),
                        field: field.into(),
                        value,
                        url: format!("{DOH}?name={qname}&type={typ}"),
                        confidence: "high".into(),
                    });
                }
            }
            Err(e) => notes.push(format!("{}: {e:#}", o.name)),
        }
    }
    if obs.is_empty() {
        bail!("no DNS observations collected; {} errors: {}", notes.len(), notes.join("; "));
    }
    let file = runs::write_observations(repo, &run, &obs)?;
    Ok((file, obs.len(), notes))
}

// ---------- Feeds ----------

#[derive(Debug, Deserialize)]
struct FeedConfig {
    default_lookback_days: i64,
    per_request_delay_ms: u64,
    sources: Vec<FeedSource>,
}

#[derive(Debug, Deserialize)]
struct FeedSource {
    name: String,
    posts_url: String,
    terms: Vec<String>,
    require_any: Vec<String>,
    /// Keep a post only if it names a registry entity or its headline has one of these.
    #[serde(default)]
    title_terms: Vec<String>,
}

/// A news post worth a human look. Leads are never facts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lead {
    pub v: u32,
    pub run: String,
    pub source: String,
    pub post_id: i64,
    pub date: String,
    pub title: String,
    pub url: String,
    pub terms: Vec<String>,
    pub entities: Vec<String>,
}

pub fn load_leads(root: &Path) -> Result<BTreeMap<String, Vec<Lead>>> {
    let mut out = BTreeMap::new();
    let dir = root.join("leads");
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                let mut v = Vec::new();
                for (i, line) in fs::read_to_string(&p)?.lines().enumerate() {
                    if !line.trim().is_empty() {
                        v.push(serde_json::from_str(line).with_context(|| format!("{rel} line {}", i + 1))?);
                    }
                }
                out.insert(rel, v);
            }
        }
    }
    Ok(out)
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&#8216;", "'")
        .replace("&#8217;", "'")
        .replace("&#8220;", "\"")
        .replace("&#8221;", "\"")
        .replace("&#8211;", "-")
        .replace("&#8212;", "-")
        .replace("&#038;", "&")
        .replace("&amp;", "&")
}

/// Names to look for in post text: current persons and orgs in the registry.
fn watchlist(repo: &Repo) -> Vec<(String, String)> {
    let mut w: Vec<(String, String)> = repo
        .persons
        .iter()
        .filter(|p| repo.canonical(&p.id) == p.id)
        .map(|p| (norm(&p.name), format!("person:{}", p.id)))
        .chain(
            repo.orgs
                .iter()
                .filter(|o| repo.canonical(&o.id) == o.id)
                .map(|o| (norm(&o.name), format!("org:{}", o.id))),
        )
        .filter(|(n, _)| n.split_whitespace().count() >= 2)
        .collect();
    w.sort();
    w
}

pub fn match_entities(text: &str, watch: &[(String, String)]) -> Vec<String> {
    let t = format!(" {} ", norm(text));
    let mut hits: BTreeSet<String> = BTreeSet::new();
    for (name, id) in watch {
        if t.contains(&format!(" {name} ")) {
            hits.insert(id.clone());
        }
    }
    hits.into_iter().collect()
}

pub fn collect_feeds(repo: &Repo, since: Option<NaiveDate>) -> Result<(PathBuf, usize, Vec<String>)> {
    runs::ensure_not_paused(&repo.root)?;
    let cfg: FeedConfig = serde_json::from_str(&fs::read_to_string(repo.root.join("schema/feeds.json"))?)?;
    let prior = load_leads(&repo.root)?;
    let seen: BTreeSet<(String, i64)> = prior.values().flatten().map(|l| (l.source.clone(), l.post_id)).collect();
    let watch = watchlist(repo);
    let agent = runs::http();
    let run = runs::new_run("feeds");
    let mut leads: BTreeMap<(String, i64), Lead> = BTreeMap::new();
    let mut notes = Vec::new();
    for src in &cfg.sources {
        let last = prior.values().flatten().filter(|l| l.source == src.name).map(|l| l.date.clone()).max();
        let from = match (since, last) {
            (Some(d), _) => d,
            (None, Some(d)) => NaiveDate::parse_from_str(&d[..10], "%Y-%m-%d").unwrap_or_else(|_| Utc::now().date_naive()),
            (None, None) => Utc::now().date_naive() - Duration::days(cfg.default_lookback_days),
        };
        let after = format!("{from}T00:00:00");
        for term in &src.terms {
            std::thread::sleep(std::time::Duration::from_millis(cfg.per_request_delay_ms));
            let resp = agent
                .get(&src.posts_url)
                .query("search", term)
                .query("after", &after)
                .query("per_page", "50")
                .query("_fields", "id,date,link,title,excerpt")
                .call();
            let posts: Vec<Value> = match resp {
                Ok(r) => match r.into_json() {
                    Ok(v) => v,
                    Err(e) => {
                        notes.push(format!("{} `{term}`: not JSON ({e}); site may have changed", src.name));
                        continue;
                    }
                },
                Err(e) => {
                    notes.push(format!("{} `{term}`: {e}", src.name));
                    continue;
                }
            };
            for p in posts {
                let Some(id) = p["id"].as_i64() else { continue };
                if seen.contains(&(src.name.clone(), id)) {
                    continue;
                }
                let title = strip_html(p["title"]["rendered"].as_str().unwrap_or("")).trim().to_string();
                let excerpt = strip_html(p["excerpt"]["rendered"].as_str().unwrap_or(""));
                let text = format!("{title} {excerpt}");
                if !src.require_any.is_empty()
                    && !src.require_any.iter().any(|r| text.to_lowercase().contains(&r.to_lowercase()))
                {
                    continue;
                }
                let entities = match_entities(&text, &watch);
                let tl = title.to_lowercase();
                if entities.is_empty()
                    && !src.title_terms.is_empty()
                    && !src.title_terms.iter().any(|w| tl.contains(&w.to_lowercase()))
                {
                    continue;
                }
                let lead = leads.entry((src.name.clone(), id)).or_insert_with(|| Lead {
                    v: SCHEMA_VERSION,
                    run: run.clone(),
                    source: src.name.clone(),
                    post_id: id,
                    date: p["date"].as_str().unwrap_or("").to_string(),
                    title,
                    url: p["link"].as_str().unwrap_or("").to_string(),
                    terms: Vec::new(),
                    entities,
                });
                if !lead.terms.contains(term) {
                    lead.terms.push(term.clone());
                }
            }
        }
    }
    let mut list: Vec<Lead> = leads.into_values().collect();
    list.sort_by(|a, b| (a.source.as_str(), a.date.as_str(), a.post_id).cmp(&(b.source.as_str(), b.date.as_str(), b.post_id)));
    if list.is_empty() {
        bail!("no new leads; {} notes: {}", notes.len(), notes.join("; "));
    }
    let file = runs::write_jsonl(&repo.root, "leads", &run, &list)?;
    Ok((file, list.len(), notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mx_and_dmarc_classification() {
        assert_eq!(classify_mx(&[]), "none");
        assert_eq!(classify_mx(&["0 example-com.mail.protection.outlook.com.".into()]), "M365");
        assert_eq!(classify_mx(&["10 mx1.mailhost.example.net.".into()]), "other (example.net)");
        assert_eq!(dmarc_policy(&["v=DMARC1; p=quarantine; rua=mailto:x@example.com".into()]), "quarantine");
        assert_eq!(dmarc_policy(&["v=spf1 -all".into()]), "missing");
    }

    #[test]
    fn domains_and_entity_matching() {
        assert_eq!(domain_of("https://www.Example.com/about").as_deref(), Some("example.com"));
        assert_eq!(domain_of(""), None);
        let watch = vec![("ann example".to_string(), "person:1".to_string())];
        assert_eq!(match_entities("Judge Ann Example sworn in", &watch), vec!["person:1".to_string()]);
        assert!(match_entities("Annex ample news", &watch).is_empty());
    }
}
