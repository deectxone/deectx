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
    #[serde(default)]
    pub active_packs: Vec<String>,
    #[serde(default)]
    pub packs_dir: Option<PathBuf>,
    #[serde(default)]
    pub allowlist: Vec<String>,
}

fn default_listen() -> String { "127.0.0.1:8787".into() }
fn default_upstream() -> String { "https://api.openai.com".into() }
fn default_ledger() -> PathBuf { PathBuf::from("./ledger.jsonl") }

impl Default for Config {
    fn default() -> Self {
        Self { listen: default_listen(), upstream: default_upstream(), ledger_path: default_ledger(), active_packs: Vec::new(), packs_dir: None, allowlist: Vec::new() }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }
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
}
