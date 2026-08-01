use crate::config::Config;
use crate::detect::{regex::{RegexDetector, RegexEntity}, secrets::{SecretsDetector, SecretsEntity}, Detector, DetectorChain};
use crate::span::Action;
use anyhow::Result;
use regex::Regex;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Pack {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub entities: Vec<PackEntity>,
    #[serde(default)]
    pub settings: PackSettings,
}

fn default_version() -> String { "0.1.0".into() }
fn default_detector() -> String { "regex".into() }

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PackEntity {
    pub id: String,
    #[serde(default = "default_detector")]
    pub detector: String, // "regex" | "secrets" | "ner"
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub checksum: Option<String>, // "luhn"
    #[serde(default)]
    pub entropy_min: Option<f64>,
    #[serde(default)]
    pub action: Action,
    #[serde(default)]
    pub alert: bool,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackSettings {
    #[serde(default)]
    pub fail_closed: bool,
    #[serde(default)]
    pub allow: Vec<String>,
}

impl Pack {
    pub fn builtin_default() -> Pack {
        serde_yaml::from_str(include_str!("default.yaml"))
            .expect("built-in default.yaml must parse")
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&text)?)
    }

    pub fn load_dir(dir: &Path) -> Result<Vec<Pack>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().map(|e| e == "yaml").unwrap_or(false) {
                match Pack::load(&path) {
                    Ok(p) => out.push(p),
                    Err(e) => tracing::warn!("skipping pack {}: {e}", path.display()),
                }
            }
        }
        Ok(out)
    }
}

impl Pack {
    pub fn regex_entities(&self) -> Vec<RegexEntity> {
        self.entities.iter()
            .filter(|e| e.detector == "regex")
            .filter_map(|e| {
                match Regex::new(e.pattern.as_deref().unwrap_or("")) {
                    Ok(pattern) => Some(RegexEntity {
                        id: e.id.clone(),
                        pattern,
                        action: e.action,
                        checksum: match e.checksum.as_deref() {
                            Some("luhn") => Some(crate::detect::regex::Checksum::Luhn),
                            Some("mod97") => Some(crate::detect::regex::Checksum::Mod97),
                            Some("ato_tfn") => Some(crate::detect::regex::Checksum::AtoTfn),
                            _ => None,
                        },
                    }),
                    Err(err) => {
                        tracing::warn!("pack entity {} has invalid regex: {err}", e.id);
                        None
                    }
                }
            })
            .collect()
    }

    pub fn secrets_entities(&self) -> Vec<SecretsEntity> {
        self.entities.iter()
            .filter(|e| e.detector == "secrets")
            .filter_map(|e| {
                let mut patterns = Vec::new();
                if let Some(p) = &e.pattern {
                    match Regex::new(p) {
                        Ok(r) => patterns.push(r),
                        Err(err) => tracing::warn!("pack entity {} has invalid regex: {err}", e.id),
                    }
                }
                for p in &e.patterns {
                    match Regex::new(p) {
                        Ok(r) => patterns.push(r),
                        Err(err) => tracing::warn!("pack entity {} has invalid regex: {err}", e.id),
                    }
                }
                Some(SecretsEntity {
                    id: e.id.clone(),
                    patterns,
                    entropy_min: e.entropy_min,
                    action: e.action,
                })
            })
            .collect()
    }
}

pub fn build_chain(packs: &[Pack], ner_enabled: bool, model_dir: PathBuf) -> DetectorChain {
    let regex: Vec<RegexEntity> = packs.iter().flat_map(|p| p.regex_entities()).collect();
    let secrets: Vec<SecretsEntity> = packs.iter().flat_map(|p| p.secrets_entities()).collect();
    let detectors: Vec<Box<dyn Detector>> = vec![
        Box::new(RegexDetector::from_entities(regex)),
        Box::new(SecretsDetector::from_entities(secrets)),
    ];
    #[cfg(feature = "ner")]
    let detectors = if ner_enabled {
        let mut d = detectors;
        d.push(Box::new(crate::detect::ner::NerDetector::new(
            model_dir,
            packs.iter().flat_map(ner_labels).collect(),
        )));
        d
    } else {
        detectors
    };
    #[cfg(not(feature = "ner"))]
    {
        let _ = model_dir;
        if ner_enabled {
            tracing::warn!("NER requested but the 'ner' feature is not compiled in; running regex+secrets only");
        }
    }
    DetectorChain::new(detectors)
}

#[cfg(feature = "ner")]
fn ner_labels(pack: &Pack) -> Vec<(String, Action)> {
    pack.entities.iter()
        .filter(|e| e.detector == "ner")
        .map(|e| (e.id.clone(), e.action))
        .collect()
}

pub fn load_active(cfg: &Config) -> Vec<Pack> {
    let mut packs = vec![Pack::builtin_default()];
    if let Some(dir) = &cfg.packs_dir {
        match Pack::load_dir(dir) {
            Ok(loaded) => {
                for p in loaded {
                    if cfg.active_packs.is_empty() || cfg.active_packs.contains(&p.name) {
                        packs.push(p);
                    }
                }
            }
            Err(e) => tracing::warn!("packs_dir {} unreadable, running default pack only: {e}", dir.display()),
        }
    }
    packs
}

pub fn allow_entries(cfg: &Config, packs: &[Pack]) -> Vec<String> {
    let mut out = cfg.allowlist.clone();
    for p in packs {
        out.extend(p.settings.allow.iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_builtin_default_pack() {
        let p = Pack::builtin_default();
        assert_eq!(p.name, "default");
        assert_eq!(p.entities.len(), 3);
        assert!(!p.settings.fail_closed);
    }

    #[test]
    fn load_round_trips_sample_yaml() {
        let dir = std::env::temp_dir().join(format!("deectx_pack_{}.yaml", std::process::id()));
        std::fs::write(&dir,
r#"name: sample
version: 0.1.0
entities:
  - id: phone
    detector: regex
    pattern: '\+?\d{10,15}'
    action: mask
  - id: medical
    detector: ner
    labels: [health, medical_condition]
    action: mask
    alert: true
settings:
  failClosed: true
  allow: ["info@example.com"]
"#).unwrap();
        let p = Pack::load(&dir).unwrap();
        assert_eq!(p.name, "sample");
        assert_eq!(p.entities.len(), 2);
        assert!(p.settings.fail_closed);
        assert_eq!(p.settings.allow, vec!["info@example.com"]);
        let med = &p.entities[1];
        assert!(med.alert);
        assert_eq!(med.labels, vec!["health", "medical_condition"]);
        assert!(med.pattern.is_none());
    }

    #[test]
    fn entities_default_to_mask_false_and_no_checksum() {
        let yaml = "name: minimal\nentities:\n  - id: x\n    detector: regex\n    pattern: 'x'\n";
        let p: Pack = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.entities[0].action, Action::Mask);
        assert!(!p.entities[0].alert);
        assert!(p.entities[0].labels.is_empty());
        assert!(p.entities[0].checksum.is_none());
        assert!(p.entities[0].patterns.is_empty());
    }
}
