use crate::ledger::{Ledger, LedgerEntry};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditSummary {
    pub date: String,
    pub total_requests: usize,
    pub masked_events: usize,
    pub redacted_events: usize,
    pub alerts: usize,
    pub distinct_sessions: usize,
    pub tools: BTreeMap<String, usize>,
    pub entities: BTreeMap<String, usize>,
    pub packs: BTreeMap<String, usize>,
}

pub fn summarize(entries: &[LedgerEntry], date: &str) -> AuditSummary {
    let mut out = AuditSummary {
        date: date.to_string(),
        total_requests: entries.len(),
        masked_events: 0,
        redacted_events: 0,
        alerts: 0,
        distinct_sessions: 0,
        tools: BTreeMap::new(),
        entities: BTreeMap::new(),
        packs: BTreeMap::new(),
    };
    let mut sessions = std::collections::HashSet::new();
    for e in entries {
        sessions.insert(e.session.clone());
        *out.tools.entry(e.tool.clone()).or_insert(0) += 1;
        for p in &e.packs {
            *out.packs.entry(p.clone()).or_insert(0) += 1;
        }
        for ev in &e.events {
            match ev.action.as_str() {
                "mask" => out.masked_events += 1,
                "redact" => out.redacted_events += 1,
                _ => {}
            }
            if ev.alert {
                out.alerts += 1;
            }
            *out.entities.entry(ev.entity.clone()).or_insert(0) += 1;
        }
    }
    out.distinct_sessions = sessions.len();
    out
}

pub fn summarize_for_date(
    ledger_path: &Path,
    date: chrono::NaiveDate,
) -> anyhow::Result<AuditSummary> {
    let entries = Ledger::read_all(ledger_path)?;
    let filtered: Vec<LedgerEntry> = entries
        .into_iter()
        .filter(|e| crate::ledger::local_date(e.ts) == date)
        .collect();
    Ok(summarize(&filtered, &date.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::LedgerEvent;
    use chrono::{Duration, Utc};

    fn sample_entry(
        offset_days: i64,
        tool: &str,
        packs: &[&str],
        events: Vec<LedgerEvent>,
    ) -> LedgerEntry {
        LedgerEntry {
            ts: Utc::now() + Duration::days(offset_days),
            tool: tool.to_string(),
            session: format!("s_{offset_days}"),
            events,
            latency_ms: 42,
            packs: packs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn summarize_aggregates_counts() {
        let entries = vec![
            sample_entry(
                0,
                "claude-code",
                &["gdpr"],
                vec![LedgerEvent {
                    entity: "iban".into(),
                    placeholder: Some("[IBAN_1]".into()),
                    ph_hash: Some("deadbeef".into()),
                    action: "mask".into(),
                    alert: true,
                }],
            ),
            sample_entry(
                1,
                "opencode",
                &["gdpr"],
                vec![LedgerEvent {
                    entity: "api_key".into(),
                    placeholder: Some("[REDACTED_SECRET]".into()),
                    ph_hash: Some("cafe".into()),
                    action: "redact".into(),
                    alert: false,
                }],
            ),
        ];
        let s = summarize(&entries, "2026-08-01");
        assert_eq!(s.total_requests, 2);
        assert_eq!(s.masked_events, 1);
        assert_eq!(s.redacted_events, 1);
        assert_eq!(s.distinct_sessions, 2);
        assert_eq!(s.entities.get("iban"), Some(&1));
        assert_eq!(s.packs.get("gdpr"), Some(&2));
        assert_eq!(s.tools.get("claude-code"), Some(&1));
        assert_eq!(s.alerts, 1);
    }

    #[test]
    fn summarize_for_date_filters_to_that_date() {
        let path = std::env::temp_dir().join(format!("deectx_audit_{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let ledger = Ledger::new(path.clone(), 90).unwrap();
        ledger
            .append(&sample_entry(0, "today", &["default"], vec![]))
            .unwrap();
        ledger
            .append(&sample_entry(-1, "yesterday", &["default"], vec![]))
            .unwrap();
        let today = crate::ledger::local_date(Utc::now());
        let summary = summarize_for_date(&path, today).unwrap();
        assert_eq!(summary.total_requests, 1);
        assert_eq!(summary.tools.get("today"), Some(&1));
        let _ = std::fs::remove_file(&path);
    }
}
