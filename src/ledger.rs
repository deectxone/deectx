use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Serialize)]
pub struct LedgerEvent {
    pub entity: String,
    pub placeholder: Option<String>,
    pub ph_hash: Option<String>,
    pub action: String,
}

#[derive(Debug, Serialize)]
pub struct LedgerEntry {
    pub ts: DateTime<Utc>,
    pub tool: String,
    pub session: String,
    pub events: Vec<LedgerEvent>,
    pub latency_ms: u128,
    pub packs: Vec<String>,
}

pub struct Ledger {
    file: Mutex<File>,
}

impl Ledger {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file: Mutex::new(file) })
    }

    pub fn append(&self, entry: &LedgerEntry) -> io::Result<()> {
        let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
        line.push('\n');
        self.file.lock().unwrap_or_else(|p| p.into_inner()).write_all(line.as_bytes())
    }
}

pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_hash_only_jsonl() {
        let path = std::env::temp_dir().join(format!("deectx_ledger_{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let ledger = Ledger::new(path.clone()).unwrap();
        let entry = LedgerEntry {
            ts: Utc::now(),
            tool: "curl".into(),
            session: "s_abc12345".into(),
            events: vec![LedgerEvent {
                entity: "email".into(),
                placeholder: Some("[EMAIL_1]".into()),
                ph_hash: Some(sha256_hex("[EMAIL_1]")),
                action: "mask".into(),
            }],
            latency_ms: 3,
            packs: vec!["default".into()],
        };
        ledger.append(&entry).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let line: serde_json::Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(line["session"], "s_abc12345");
        assert_eq!(line["events"][0]["entity"], "email");
        assert!(content.contains("ph_hash"));
        assert!(!content.contains("jane.doe@example.com")); // never raw PII
    }
}
