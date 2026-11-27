//! Civic views: an office chart (posts and their holders, including
//! officers-elect) and person profiles (roles over time, bar status).

use crate::build::{current, Facts};
use crate::model::Repo;
use std::collections::BTreeMap;

pub struct Civic {
    pub offices_html: String,
    pub people_html: String,
    pub posts: usize,
    pub people: usize,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn val(facts: &Facts, entity: &str, field: &str) -> String {
    current(facts, entity, field).map(|a| a.value.clone()).unwrap_or_default()
}

fn person_name(repo: &Repo, facts: &Facts, pid: &str) -> String {
    let pid = repo.canonical(pid);
    let v = val(facts, &format!("person:{pid}"), "name");
    if v.is_empty() {
        repo.persons.iter().find(|p| p.id == pid).map(|p| p.name.clone()).unwrap_or_default()
    } else {
        v
    }
}

fn status_badge(status: &str, from: &str, to: &str) -> String {
    match status {
        "incumbent" if !to.is_empty() => format!("<span class=\"badge neutral\">serving to {}</span>", esc(to)),
        "incumbent" => "<span class=\"badge ok\">serving</span>".into(),
        "elect" => format!("<span class=\"badge neutral\">elect{}</span>", if from.is_empty() { String::new() } else { format!(", takes office {}", esc(from)) }),
        "acting" => "<span class=\"badge neutral\">acting</span>".into(),
        "former" => format!("<span class=\"badge warn\">former{}</span>", if to.is_empty() { String::new() } else { format!(" to {}", esc(to)) }),
        "" => "<span class=\"badge neutral\">status not recorded</span>".into(),
        other => format!("<span class=\"badge neutral\">{}</span>", esc(other)),
    }
}

pub fn civic(repo: &Repo, facts: &Facts) -> Civic {
    // Offices: one card per public org, one line per post holder.
    let mut offices = String::new();
    let mut post_count = 0;
    let mut orgs: Vec<_> = repo
        .orgs
        .iter()
        .filter(|o| o.kind != "firm" && repo.canonical(&o.id) == o.id)
        .collect();
    orgs.sort_by_key(|o| o.name.to_lowercase());
    for o in orgs {
        let posts: Vec<_> = repo.posts.iter().filter(|p| p.org_id == o.id).collect();
        let ent = format!("org:{}", o.id);
        let mut lines = String::new();
        let mut search = o.name.clone();
        for p in &posts {
            post_count += 1;
            let title = if p.place.is_empty() {
                p.title.clone()
            } else if p.place.chars().all(|c| c.is_ascii_digit()) {
                format!("{}, Place {}", p.title, p.place)
            } else {
                format!("{}, {}", p.title, p.place)
            };
            let mut holders: Vec<(String, String)> = Vec::new();
            for t in repo.tenures.iter().filter(|t| t.post_id == p.id) {
                let te = format!("tenure:{}", t.id);
                let status = val(facts, &te, "status");
                if status == "former" {
                    continue;
                }
                let from = val(facts, &te, "valid_from");
                let to = val(facts, &te, "valid_to");
                let name = person_name(repo, facts, &t.person_id);
                search.push(' ');
                search.push_str(&name);
                let sort = if status == "elect" { "1" } else { "0" };
                let when = current(facts, &te, "status").map(|a| a.last_confirmed.clone()).unwrap_or_default();
                holders.push((sort.to_string(), format!(
                    "<li>{} {} <span class=\"meta\">confirmed {}</span></li>",
                    esc(&name), status_badge(&status, &from, &to), esc(&when)
                )));
            }
            holders.sort();
            let list: String = if holders.is_empty() {
                "<li class=\"meta\">holder not recorded</li>".into()
            } else {
                holders.into_iter().map(|(_, h)| h).collect()
            };
            search.push(' ');
            search.push_str(&title);
            lines += &format!("<p><strong>{}</strong></p><ul class=\"holders\">{}</ul>", esc(&title), list);
        }
        let addr = val(facts, &ent, "office_address");
        let phone = val(facts, &ent, "office_phone");
        let mut meta = Vec::new();
        if !addr.is_empty() {
            meta.push(esc(&addr));
        }
        if !phone.is_empty() {
            let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
            meta.push(format!("<a href=\"tel:{digits}\">{}</a>", esc(&phone)));
        }
        offices += &format!(
            "<article class=\"card\" data-search=\"{}\"><h2>{}</h2>{}{}</article>",
            esc(&search.to_lowercase()),
            esc(&o.name),
            if meta.is_empty() { String::new() } else { format!("<p class=\"meta\">{}</p>", meta.join(" · ")) },
            if lines.is_empty() { "<p class=\"meta\">no posts recorded</p>".into() } else { lines }
        );
    }

    // People: roles over time plus bar credentials (civic only).
    let mut people = String::new();
    let mut persons: Vec<_> = repo.persons.iter().filter(|p| repo.canonical(&p.id) == p.id).collect();
    persons.sort_by_key(|p| {
        let n = person_name(repo, facts, &p.id);
        (n.split_whitespace().last().unwrap_or("").to_lowercase(), n.to_lowercase())
    });
    let org_names: BTreeMap<&str, &str> = repo.orgs.iter().map(|o| (o.id.as_str(), o.name.as_str())).collect();
    for p in &persons {
        let name = person_name(repo, facts, &p.id);
        let pe = format!("person:{}", p.id);
        let mut roles = String::new();
        let mut search = name.clone();
        for t in repo.tenures.iter().filter(|t| repo.canonical(&t.person_id) == p.id) {
            let te = format!("tenure:{}", t.id);
            let status = val(facts, &te, "status");
            let org = org_names.get(repo.canonical(&t.org_id)).copied().unwrap_or("");
            let post = repo.posts.iter().find(|x| x.id == t.post_id).map(|x| x.title.clone());
            let role = post.unwrap_or_else(|| if t.role.is_empty() { "member".into() } else { t.role.clone() });
            search.push_str(&format!(" {org} {role}"));
            roles += &format!(
                "<li>{} at {} {}</li>",
                esc(&role),
                esc(org),
                status_badge(&status, &val(facts, &te, "valid_from"), &val(facts, &te, "valid_to"))
            );
        }
        let bar_status = val(facts, &pe, "bar_status");
        let admit = val(facts, &pe, "admit_date");
        let bar = if bar_status.is_empty() {
            "<p class=\"meta\">Bar status not looked up</p>".to_string()
        } else {
            let when = current(facts, &pe, "bar_status").map(|a| a.last_confirmed.clone()).unwrap_or_default();
            format!(
                "<p><strong>Bar</strong> {}{} <span class=\"meta\">checked {}</span></p>",
                esc(&bar_status),
                if admit.is_empty() { String::new() } else { format!(", admitted {}", esc(&admit)) },
                esc(&when)
            )
        };
        people += &format!(
            "<article class=\"card\" data-search=\"{}\"><h2>{}</h2><ul class=\"holders\">{}</ul>{}</article>",
            esc(&search.to_lowercase()),
            esc(&name),
            roles,
            bar
        );
    }
    Civic { offices_html: offices, people_html: people, posts: post_count, people: persons.len() }
}
