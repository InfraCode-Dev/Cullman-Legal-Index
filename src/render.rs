//! Render the private, phone-first firm-card page.

use anyhow::Result;
use chrono::{NaiveDate, Utc};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::Path;

pub type SalesMap = BTreeMap<String, BTreeMap<String, String>>;

pub const SALES_COLUMNS: &[&str] = &["next_action", "last_touch", "decision_unit", "incumbent_it", "notes"];

pub fn sales_init(path: &Path) -> Result<()> {
    let db = Connection::open(path)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS firm_sales (
            org_id TEXT PRIMARY KEY,
            next_action TEXT, last_touch TEXT, decision_unit TEXT,
            incumbent_it TEXT, notes TEXT);
         CREATE TABLE IF NOT EXISTS contact_log (
            org_id TEXT, at TEXT, channel TEXT, summary TEXT);
         CREATE TABLE IF NOT EXISTS do_not_contact (
            org_id TEXT PRIMARY KEY, reason TEXT, since TEXT);",
    )?;
    Ok(())
}

pub fn load_sales(path: &Path) -> Result<SalesMap> {
    let db = Connection::open(path)?;
    let mut out = SalesMap::new();
    let mut stmt = db.prepare("SELECT org_id, next_action, last_touch, decision_unit, incumbent_it, notes FROM firm_sales")?;
    let rows = stmt.query_map([], |r| {
        let mut m = BTreeMap::new();
        for (i, c) in SALES_COLUMNS.iter().enumerate() {
            let v: Option<String> = r.get(i + 1)?;
            m.insert(c.to_string(), v.unwrap_or_default());
        }
        Ok((r.get::<_, String>(0)?, m))
    })?;
    for r in rows {
        let (id, m) = r?;
        out.insert(id, m);
    }
    let mut stmt = db.prepare("SELECT org_id, reason FROM do_not_contact")?;
    for r in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (id, reason) = r?;
        out.entry(id).or_default().insert("do_not_contact".into(), reason);
    }
    Ok(out)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn badge(label: &str, value: &str, good: &[&str], bad: &[&str]) -> String {
    let cls = if good.contains(&value) {
        "ok"
    } else if bad.contains(&value) {
        "warn"
    } else {
        "neutral"
    };
    let v = if value.is_empty() { "not checked" } else { value };
    format!("<span class=\"badge {cls}\">{} {}</span>", esc(label), esc(v))
}

pub fn render(rows: &[BTreeMap<&'static str, String>], sales: Option<&SalesMap>, civic: &crate::views::Civic) -> String {
    let today = Utc::now().date_naive();
    let mut cards = String::new();
    for r in rows {
        let stale = NaiveDate::parse_from_str(&r["last_confirmed"], "%Y-%m-%d")
            .map(|d| (today - d).num_days() > 90)
            .unwrap_or(true);
        let attorneys: Vec<&str> = r["attorneys"].split("; ").filter(|s| !s.is_empty()).collect();
        let mut c = format!(
            "<article class=\"card{}\" data-search=\"{}\">",
            if stale { " stale" } else { "" },
            esc(&format!("{} {} {}", r["name"], r["attorneys"], r["practice_areas"]).to_lowercase())
        );
        c += &format!("<h2>{}</h2>", esc(&r["name"]));
        if r["notes"].starts_with("DOUBTFUL") {
            c += "<p class=\"badges\"><span class=\"badge warn\">Unconfirmed: call first</span></p>";
        }
        let addr = if r["office_address"].is_empty() { r["city"].clone() } else { r["office_address"].clone() };
        c += &format!("<p class=\"meta\">{}</p>", esc(&addr));
        let mut links = Vec::new();
        if !r["office_phone"].is_empty() {
            let digits: String = r["office_phone"].chars().filter(|c| c.is_ascii_digit()).collect();
            links.push(format!("<a href=\"tel:{digits}\">{}</a>", esc(&r["office_phone"])));
        }
        if !r["website"].is_empty() {
            links.push(format!("<a href=\"{0}\" rel=\"noreferrer\" target=\"_blank\">website</a>", esc(&r["website"])));
        }
        if !links.is_empty() {
            c += &format!("<p class=\"links\">{}</p>", links.join(" · "));
        }
        c += "<p class=\"badges\">";
        c += &badge("Closings", &r["closings"], &["yes"], &[]);
        c += &badge("Mail", &r["mx_provider"], &[], &["none"]);
        c += &badge("DMARC", &r["dmarc_policy"], &["reject", "quarantine"], &["none", "missing"]);
        c += "</p>";
        if attorneys.is_empty() {
            c += "<p><strong>Attorneys</strong> <span class=\"meta\">not listed on a firm site</span></p>";
        } else {
            c += &format!("<p><strong>Attorneys ({})</strong> {}</p>", attorneys.len(), esc(&attorneys.join(", ")));
        }
        if !r["practice_areas"].is_empty() {
            c += &format!("<p><strong>Practice</strong> {}</p>", esc(&r["practice_areas"].replace(';', ",")));
        }
        if let Some(s) = sales.and_then(|m| m.get(&r["id"])) {
            c += "<div class=\"sales\">";
            if let Some(dnc) = s.get("do_not_contact").filter(|v| !v.is_empty()) {
                c += &format!("<p class=\"dnc\">Do not contact: {}</p>", esc(dnc));
            }
            for col in SALES_COLUMNS {
                if let Some(v) = s.get(*col).filter(|v| !v.is_empty()) {
                    c += &format!("<p><strong>{}</strong> {}</p>", esc(&col.replace('_', " ")), esc(v));
                }
            }
            c += "</div>";
        }
        let srcs: Vec<String> = r["sources"]
            .split("; ")
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with("http") {
                    format!("<li><a href=\"{0}\" rel=\"noreferrer\" target=\"_blank\">{0}</a></li>", esc(s))
                } else {
                    format!("<li>{}</li>", esc(s))
                }
            })
            .collect();
        let notes = if r["notes"].is_empty() {
            String::new()
        } else {
            format!("<p class=\"notes\">{}</p>", esc(&r["notes"].replace(" | ", " ")))
        };
        c += &format!(
            "<details><summary>Confirmed {} via {}{}</summary>{}<ul>{}</ul></details>",
            esc(if r["last_confirmed"].is_empty() { "never" } else { &r["last_confirmed"] }),
            esc(&if r["confirmed_by"].is_empty() { "unknown".to_string() } else { r["confirmed_by"].replace(';', ", ") }),
            if stale { " (over 90 days)" } else { "" },
            notes,
            srcs.join("")
        );
        c += "</article>";
        cards += &c;
    }
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Cullman Legal Index</title>
<style>
:root {{ --bg:#f7f6f3; --card:#ffffff; --ink:#1d1d1b; --muted:#6b6b66; --line:#e3e1db; --ok:#1f7a4d; --okbg:#e5f4e4; --warn:#9a3b12; --warnbg:#f9ece7; --neu:#3d4f7a; --neubg:#e7f1fb; }}
@media (prefers-color-scheme: dark) {{ :root:not([data-theme="light"]) {{ --bg:#151513; --card:#1f1f1c; --ink:#ecebe6; --muted:#a3a29b; --line:#33322e; --ok:#7fd3a3; --okbg:#1c3327; --warn:#f0a37f; --warnbg:#3a2219; --neu:#a9c1f0; --neubg:#1d2a40; }} }}
:root[data-theme="dark"] {{ --bg:#151513; --card:#1f1f1c; --ink:#ecebe6; --muted:#a3a29b; --line:#33322e; --ok:#7fd3a3; --okbg:#1c3327; --warn:#f0a37f; --warnbg:#3a2219; --neu:#a9c1f0; --neubg:#1d2a40; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--bg); color:var(--ink); font:16px/1.45 system-ui,-apple-system,Segoe UI,sans-serif; }}
header {{ padding:16px; position:sticky; top:0; background:var(--bg); border-bottom:1px solid var(--line); }}
h1 {{ font-size:20px; margin:0 0 8px; }}
nav {{ display:flex; gap:6px; margin:0 0 10px; flex-wrap:wrap; }}
nav button {{ font:inherit; font-size:14px; padding:6px 12px; border-radius:999px; border:1px solid var(--line); background:var(--card); color:var(--ink); cursor:pointer; }}
nav button[aria-selected="true"] {{ background:var(--ink); color:var(--bg); border-color:var(--ink); }}
.holders {{ list-style:none; padding:0; margin:4px 0 8px; }} .holders li {{ margin:4px 0; }}
input {{ width:100%; padding:10px 12px; font:inherit; border:1px solid var(--line); border-radius:8px; background:var(--card); color:var(--ink); }}
main[hidden] {{ display:none; }}
main {{ padding:16px; display:grid; gap:12px; grid-template-columns:repeat(auto-fill,minmax(300px,1fr)); }}
.card {{ background:var(--card); border:1px solid var(--line); border-radius:12px; padding:14px 16px; }}
.card.stale {{ opacity:.62; }}
.card h2 {{ font-size:17px; margin:0 0 4px; }}
.card p {{ margin:6px 0; }}
.meta {{ color:var(--muted); font-size:14px; }}
a {{ color:var(--neu); }}
.badge {{ display:inline-block; font-size:12px; padding:2px 8px; border-radius:999px; margin:0 4px 4px 0; }}
.badge.ok {{ background:var(--okbg); color:var(--ok); }} .badge.warn {{ background:var(--warnbg); color:var(--warn); }} .badge.neutral {{ background:var(--neubg); color:var(--neu); }}
.sales {{ border-top:1px dashed var(--line); margin-top:8px; padding-top:6px; }}
.dnc {{ color:var(--warn); font-weight:600; }}
details {{ font-size:13px; color:var(--muted); margin-top:8px; }} details ul {{ padding-left:18px; overflow-wrap:anywhere; }}
  .notes {{ font-size:13px; color:var(--muted); }}
footer {{ padding:0 16px 24px; color:var(--muted); font-size:13px; }}
</style></head>
<body><header><h1>Cullman legal index</h1>
<nav role="tablist"><button role="tab" data-tab="firms" aria-selected="true">Firms ({count})</button><button role="tab" data-tab="offices" aria-selected="false">Offices ({posts})</button><button role="tab" data-tab="people" aria-selected="false">People ({people})</button></nav>
<input id="q" type="search" placeholder="Filter by name, firm, office or practice" aria-label="Filter"></header>
<main id="firms" class="panel">{cards}</main>
<main id="offices" class="panel" hidden>{offices}</main>
<main id="people" class="panel" hidden>{people_html}</main>
<footer>Built {today}. Dimmed firm cards were last confirmed more than 90 days ago. Bar status is a civic reference and never feeds sales. {sales_note}</footer>
<script>
const q=document.getElementById('q');
function filt(){{const t=q.value.toLowerCase();document.querySelectorAll('.card').forEach(c=>{{c.style.display=c.dataset.search.includes(t)?'':'none';}});}}
q.addEventListener('input',filt);
document.querySelectorAll('[data-tab]').forEach(b=>b.addEventListener('click',()=>{{document.querySelectorAll('[data-tab]').forEach(x=>x.setAttribute('aria-selected',x===b));document.querySelectorAll('.panel').forEach(p=>p.hidden=p.id!==b.dataset.tab);try{{localStorage.setItem('tab',b.dataset.tab)}}catch(e){{}}}}));
try{{const t=localStorage.getItem('tab');if(t){{const b=document.querySelector('[data-tab="'+t+'"]');if(b)b.click();}}}}catch(e){{}}
</script></body></html>
"#,
        count = rows.len(),
        posts = civic.posts,
        people = civic.people,
        offices = civic.offices_html,
        people_html = civic.people_html,
        cards = cards,
        today = today,
        sales_note = if sales.is_some() { "Includes local sales notes: do not share this file." } else { "Civic facts only; no sales data." }
    )
}
