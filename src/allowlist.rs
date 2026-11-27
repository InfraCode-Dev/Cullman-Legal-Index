//! Per-source field allowlist: the privacy boundary as code.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Allowlist {
    pub forbidden_fields: Vec<String>,
    pub sales_fields: Vec<String>,
    pub sources: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    pub address_fields: Vec<String>,
    pub residential_markers: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Ok,
    Forbidden(String),
}

impl Allowlist {
    pub fn load(root: &Path) -> Result<Allowlist> {
        let p = root.join("schema/allowlist.json");
        let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn check(&self, source: &str, kind: &str, field: &str) -> Verdict {
        if self.forbidden_fields.iter().any(|f| f == field) {
            return Verdict::Forbidden(format!("field `{field}` is excluded by the privacy boundary"));
        }
        if self.sales_fields.iter().any(|f| f == field) {
            return Verdict::Forbidden(format!("field `{field}` is sales data and must never be committed"));
        }
        let Some(kinds) = self.sources.get(source) else {
            return Verdict::Forbidden(format!("source `{source}` is not in the allowlist"));
        };
        let Some(fields) = kinds.get(kind) else {
            return Verdict::Forbidden(format!("source `{source}` may not write `{kind}` entities"));
        };
        if fields.iter().any(|f| f == field) {
            Verdict::Ok
        } else {
            Verdict::Forbidden(format!("source `{source}` may not write `{kind}.{field}`"))
        }
    }

    pub fn is_address_field(&self, field: &str) -> bool {
        self.address_fields.iter().any(|f| f == field)
    }

    /// True when an address looks like a residence or non-office location and
    /// has no suite or unit marker. Such values need a reviewed override.
    pub fn looks_residential(&self, value: &str) -> bool {
        let v = format!(" {} ", value.to_lowercase().replace(['.', ','], " "));
        let has_unit = [" suite ", " ste ", " unit ", "#"].iter().any(|m| v.contains(m));
        let marker = self
            .residential_markers
            .iter()
            .any(|m| v.contains(&format!(" {}", m.replace('.', " ").trim_end())));
        marker && !has_unit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Allowlist {
        serde_json::from_str(
            r#"{
            "forbidden_fields": ["home_address", "email"],
            "sales_fields": ["next_action"],
            "sources": {"firm-table": {"org": ["name", "office_address"]}},
            "address_fields": ["office_address"],
            "residential_markers": ["county road", "hwy", "po box"]
        }"#,
        )
        .unwrap()
    }

    #[test]
    fn allows_listed_field() {
        assert_eq!(sample().check("firm-table", "org", "name"), Verdict::Ok);
    }

    #[test]
    fn rejects_forbidden_sales_and_unlisted() {
        let a = sample();
        assert!(matches!(a.check("firm-table", "org", "email"), Verdict::Forbidden(_)));
        assert!(matches!(a.check("firm-table", "org", "next_action"), Verdict::Forbidden(_)));
        assert!(matches!(a.check("firm-table", "org", "office_phone"), Verdict::Forbidden(_)));
        assert!(matches!(a.check("bar-lookup", "org", "name"), Verdict::Forbidden(_)));
        assert!(matches!(a.check("firm-table", "person", "name"), Verdict::Forbidden(_)));
    }

    #[test]
    fn residential_heuristic() {
        let a = sample();
        // Fictional addresses only; never use real records in tests.
        assert!(a.looks_residential("123 County Road 999, Exampleville"));
        assert!(a.looks_residential("4500 Hwy 99 W"));
        assert!(!a.looks_residential("4500 Hwy 99 W, Suite 2"));
        assert!(!a.looks_residential("100 Main St, Exampleville"));
    }
}
