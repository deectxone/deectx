use chrono::{DateTime, NaiveDate, Utc};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LedgerEvent {
    pub entity: String,
    pub placeholder: Option<String>,
    pub ph_hash: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LedgerEntry {
    pub ts: DateTime<Utc>,
    pub tool: String,
    pub session: String,
    pub events: Vec<LedgerEvent>,
    pub latency_ms: u128,
    pub packs: Vec<String>,
}

pub struct Ledger {
    base: PathBuf,
    file: Mutex<Option<File>>,
    current_date: Mutex<NaiveDate>,
    retention_days: u64,
}

impl Ledger {
    pub fn new(path: PathBuf, retention_days: u64) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let ledger = Self {
            base: path,
            file: Mutex::new(Some(file)),
            current_date: Mutex::new(Utc::now().date_naive()),
            retention_days,
        };
        ledger.prune_old()?;
        Ok(ledger)
    }

    pub fn append(&self, entry: &LedgerEntry) -> io::Result<()> {
        let entry_date = entry.ts.date_naive();
        let needs_rotate = {
            let cur = self.current_date.lock().unwrap_or_else(|p| p.into_inner());
            *cur != entry_date
        };
        if needs_rotate {
            self.rotate_to(entry_date)?;
            *self.current_date.lock().unwrap_or_else(|p| p.into_inner()) = entry_date;
        }
        let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
        line.push('\n');
        let mut guard = self.file.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_mut() {
            Some(f) => f.write_all(line.as_bytes()),
            None => Ok(()),
        }
    }

    fn rotate_to(&self, new_date: NaiveDate) -> io::Result<()> {
        let old_date = *self.current_date.lock().unwrap_or_else(|p| p.into_inner());
        self.file.lock().unwrap_or_else(|p| p.into_inner()).take();
        if old_date != new_date {
            let rotated = rotated_path(&self.base, old_date);
            if self.base.exists() {
                std::fs::rename(&self.base, &rotated)?;
            }
            self.prune_old()?;
        }
        let f = OpenOptions::new().create(true).append(true).open(&self.base)?;
        *self.file.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
        Ok(())
    }

    fn prune_old(&self) -> io::Result<()> {
        let stem = self.base.file_stem().and_then(|s| s.to_str()).unwrap_or("ledger").to_string();
        let dir = self.base.parent().unwrap_or(Path::new("."));
        let today = Utc::now().date_naive();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(rest) = name.strip_prefix(&format!("{stem}-")) {
                if let Some(date_str) = rest.strip_suffix(".jsonl") {
                    if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                        let age = (today - date).num_days();
                        if age > self.retention_days as i64 {
                            tracing::info!("pruning expired ledger {name} (age {age}d)");
                            std::fs::remove_file(&path)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn read_all(path: &Path) -> io::Result<Vec<LedgerEntry>> {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("ledger").to_string();
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let p = entry?.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with(&stem) && name.ends_with(".jsonl") {
                files.push(p);
            }
        }
        files.sort();
        if !files.contains(&path.to_path_buf()) && path.exists() {
            files.push(path.to_path_buf());
        }
        let mut out = Vec::new();
        for f in files {
            if !f.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&f)?;
            for (i, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<LedgerEntry>(line) {
                    Ok(e) => out.push(e),
                    Err(e) => tracing::warn!("ledger {} line {} unparseable: {e}", f.display(), i + 1),
                }
            }
        }
        Ok(out)
    }
}

fn rotated_path(base: &Path, date: NaiveDate) -> PathBuf {
    let stem = base.file_stem().and_then(|s| s.to_str()).unwrap_or("ledger");
    let dir = base.parent().unwrap_or(Path::new("."));
    dir.join(format!("{stem}-{}.jsonl", date.format("%Y-%m-%d")))
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
        let ledger = Ledger::new(path.clone(), 90).unwrap();
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

    #[test]
    fn rotates_to_dated_file_and_read_all_spans_rotated_files() {
        let dir = std::env::temp_dir().join(format!("deectx_rot_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("ledger.jsonl");

        let ledger = Ledger::new(base.clone(), 90).unwrap();
        let yesterday = Utc::now() - chrono::Duration::days(1);
        ledger
            .append(&LedgerEntry {
                ts: yesterday,
                tool: "y".into(),
                session: "s_y".into(),
                events: vec![],
                latency_ms: 1,
                packs: vec![],
            })
            .unwrap();
        ledger
            .append(&LedgerEntry {
                ts: Utc::now(),
                tool: "t".into(),
                session: "s_t".into(),
                events: vec![],
                latency_ms: 2,
                packs: vec![],
            })
            .unwrap();

        let rotated = dir.join(format!("ledger-{}.jsonl", yesterday.format("%Y-%m-%d")));
        assert!(rotated.exists(), "yesterday entries must be rotated to a dated file");
        assert!(base.exists(), "today's file must remain at the base path");

        let all = Ledger::read_all(&base).unwrap();
        assert_eq!(all.len(), 2, "read_all must span base + rotated files: {all:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prunes_rotated_files_older_than_retention() {
        let dir = std::env::temp_dir().join(format!("deectx_prune_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("ledger.jsonl");
        std::fs::write(&base, "").unwrap();
        let old = dir.join("ledger-2020-01-01.jsonl");
        std::fs::write(&old, "").unwrap();
        let recent = dir.join(format!("ledger-{}.jsonl", Utc::now().date_naive().format("%Y-%m-%d")));
        std::fs::write(&recent, "").unwrap();

        let _ = Ledger::new(base.clone(), 90).unwrap();
        assert!(!old.exists(), "old rotated file must be pruned");
        assert!(recent.exists(), "recent rotated file must be kept");
        assert!(base.exists(), "base file must be kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
