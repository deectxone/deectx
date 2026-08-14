use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_upstream")]
    pub upstream: String,
    #[serde(default = "default_ledger")]
    pub ledger_path: PathBuf,
    #[serde(default = "default_retention_days")]
    pub ledger_retention_days: u64,
    #[serde(default)]
    pub active_packs: Vec<String>,
    #[serde(default)]
    pub packs_dir: Option<PathBuf>,
    #[serde(default)]
    pub model_dir: Option<PathBuf>,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub ner: bool,
    #[serde(default)]
    pub upstream_anthropic: Option<String>,
    #[serde(default = "default_stats_enabled")]
    pub stats_enabled: bool,
    #[serde(default = "default_upstream_responses")]
    pub upstream_responses: String,
}

fn default_listen() -> String {
    "127.0.0.1:8787".into()
}
fn default_upstream() -> String {
    "https://api.openai.com".into()
}
fn default_ledger() -> PathBuf {
    crate::home::ledger_path()
}
fn default_retention_days() -> u64 {
    90
}
fn default_stats_enabled() -> bool {
    true
}
fn default_upstream_responses() -> String {
    "https://api.openai.com/v1/responses".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            upstream: default_upstream(),
            ledger_path: default_ledger(),
            ledger_retention_days: default_retention_days(),
            active_packs: Vec::new(),
            packs_dir: None,
            model_dir: None,
            allowlist: Vec::new(),
            ner: false,
            upstream_anthropic: None,
            stats_enabled: default_stats_enabled(),
            upstream_responses: default_upstream_responses(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }
}

/// Rewrite (or append) the `active_packs` line in `path`'s TOML, line by
/// line, leaving every other line — including comments and formatting —
/// untouched. Used by `POST /packs` so toggling packs from the dashboard
/// doesn't clobber the rest of a hand-edited config.toml. Creates the file
/// (with just the new line) if it doesn't exist yet.
pub fn write_active_packs(path: &Path, packs: &[String]) -> Result<()> {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let list = packs
        .iter()
        .map(|p| format!("{p:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let new_line = format!("active_packs = [{list}]");

    let mut found = false;
    let mut out: Vec<String> = original
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("active_packs") {
                found = true;
                new_line.clone()
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        out.push(new_line);
    }
    let mut text = out.join("\n");
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_partial_toml_with_defaults() {
        let dir = std::env::temp_dir().join("deectx_cfg_test.toml");
        std::fs::write(&dir, "upstream = \"https://example.com\"\n").unwrap();
        let cfg = Config::load(&dir).unwrap();
        assert_eq!(cfg.upstream, "https://example.com");
        assert_eq!(cfg.listen, "127.0.0.1:8787");
    }

    #[test]
    fn write_active_packs_replaces_existing_line_in_place() {
        let path = std::env::temp_dir().join(format!(
            "deectx_cfg_active_packs_{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "listen = \"127.0.0.1:9\"\nactive_packs = [\"gdpr\"]\nner = true\n",
        )
        .unwrap();
        write_active_packs(&path, &["gdpr".into(), "cdr-au".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("active_packs = [\"gdpr\", \"cdr-au\"]"));
        assert!(
            text.contains("listen = \"127.0.0.1:9\""),
            "other keys must survive: {text}"
        );
        assert!(
            text.contains("ner = true"),
            "keys after active_packs must survive: {text}"
        );
        assert_eq!(
            text.matches("active_packs").count(),
            1,
            "must not duplicate the line"
        );
        let cfg = Config::load(&path).unwrap();
        assert_eq!(
            cfg.active_packs,
            vec!["gdpr".to_string(), "cdr-au".to_string()]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn write_active_packs_appends_when_missing() {
        let path = std::env::temp_dir().join(format!(
            "deectx_cfg_active_packs_append_{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, "# deeCtx config\n").unwrap();
        write_active_packs(&path, &["gdpr".into()]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# deeCtx config"));
        assert!(text.contains("active_packs = [\"gdpr\"]"));
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg.active_packs, vec!["gdpr".to_string()]);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn default_ledger_is_under_home() {
        let _g = crate::home::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        std::env::set_var("DEECTX_HOME", "/tmp/deectx-cfg-home");
        assert_eq!(
            Config::default().ledger_path,
            std::path::PathBuf::from("/tmp/deectx-cfg-home").join("ledger.jsonl")
        );
        std::env::remove_var("DEECTX_HOME");
    }
}
