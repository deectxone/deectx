use crate::span::{Action, Span};
use crate::detect::Detector;
use regex::Regex;
use std::collections::HashMap;

pub struct SecretsEntity {
    pub id: String,
    pub patterns: Vec<Regex>,
    pub entropy_min: Option<f64>,
    pub action: Action,
    pub alert: bool,
}

pub struct SecretsDetector {
    entities: Vec<(String, Vec<Regex>, Action, bool)>,
    entropy: Option<(String, Regex, f64, Action, bool)>,
}

impl SecretsDetector {
    pub fn from_entities(entities: Vec<SecretsEntity>) -> Self {
        let mut out = Self { entities: Vec::new(), entropy: None };
        for e in entities {
            if !e.patterns.is_empty() {
                out.entities.push((e.id.clone(), e.patterns, e.action, e.alert));
            }
            if e.entropy_min.is_some() && out.entropy.is_none() {
                out.entropy = Some((
                    e.id.clone(),
                    Regex::new(r"\b[A-Za-z0-9/+_=.-]{21,}\b").unwrap(),
                    e.entropy_min.unwrap(),
                    e.action,
                    e.alert,
                ));
            }
        }
        out
    }
}

pub(crate) fn shannon_entropy(s: &str) -> f64 {
    let mut freq: HashMap<char, usize> = HashMap::new();
    for c in s.chars() { *freq.entry(c).or_insert(0) += 1; }
    let len = s.len() as f64;
    freq.values().map(|&n| {
        let p = n as f64 / len;
        -p * p.log2()
    }).sum()
}

impl Detector for SecretsDetector {
    fn detect(&self, text: &str) -> Vec<Span> {
        let mut out = Vec::new();
        for (id, patterns, action, alert) in &self.entities {
            for pat in patterns {
                for m in pat.find_iter(text) {
                    out.push(Span { alert: *alert, ..Span::new(m.start(), m.end(), id, *action, m.as_str()) });
                }
            }
        }
        if let Some((id, re, min, action, alert)) = &self.entropy {
            for m in re.find_iter(text) {
                if shannon_entropy(m.as_str()) > *min {
                    out.push(Span { alert: *alert, ..Span::new(m.start(), m.end(), id, *action, m.as_str()) });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_detector() -> SecretsDetector {
        SecretsDetector::from_entities(vec![SecretsEntity {
            id: "api_key".into(),
            patterns: vec![
                Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
                Regex::new(r"ghp_[A-Za-z0-9]{36}").unwrap(),
                Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
            ],
            entropy_min: Some(4.5),
            action: Action::Redact,
            alert: false,
        }])
    }

    #[test]
    fn detects_aws_key_as_redact() {
        let spans = test_detector().detect("key is AKIAIOSFODNN7EXAMPLE ok");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].entity, "api_key");
        assert!(matches!(spans[0].action, Action::Redact));
    }

    #[test]
    fn entropy_gate() {
        assert!(shannon_entropy("AAAAAAAAAAAAAAAAAAAAAAAA") < 4.5);
        assert!(shannon_entropy("x9Kf2mQ8vLp3nR7sW1yZ4bN6") > 4.5);
    }

    #[test]
    fn ignores_low_entropy_long_words() {
        assert!(test_detector().detect("say AAAAAAAAAAAAAAAAAAAAAAAA aloud").is_empty());
    }

    #[test]
    fn entropy_scan_only_runs_when_entity_sets_entropy_min() {
        let d = SecretsDetector::from_entities(vec![SecretsEntity {
            id: "gh_token".into(),
            patterns: vec![Regex::new(r"ghp_[A-Za-z0-9]{36}").unwrap()],
            entropy_min: None,
            action: Action::Redact,
            alert: false,
        }]);
        // 21+ char bare token with high entropy is NOT flagged without entropy_min
        assert!(d.detect("token x9Kf2mQ8vLp3nR7sW1yZ4bN6cD").is_empty());
        // explicit pattern still fires
        assert_eq!(d.detect("ghp_abcdefghijklmnopqrstuvwxyz0123456789").len(), 1);
    }
}
