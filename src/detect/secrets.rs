use crate::span::{Action, Span};
use crate::detect::Detector;
use regex::Regex;
use std::collections::HashMap;

pub struct SecretsDetector {
    patterns: Vec<Regex>,
    bare_token: Regex,
}

impl SecretsDetector {
    pub fn new() -> Self {
        Self {
            patterns: vec![
                Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
                Regex::new(r"ghp_[A-Za-z0-9]{36}").unwrap(),
                Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
            ],
            bare_token: Regex::new(r"\b[A-Za-z0-9/+_=.-]{21,}\b").unwrap(),
        }
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
        for pat in &self.patterns {
            for m in pat.find_iter(text) {
                out.push(Span::new(m.start(), m.end(), "api_key", Action::Redact, m.as_str()));
            }
        }
        for m in self.bare_token.find_iter(text) {
            if shannon_entropy(m.as_str()) > 4.5 {
                out.push(Span::new(m.start(), m.end(), "api_key", Action::Redact, m.as_str()));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_aws_key_as_redact() {
        let d = SecretsDetector::new();
        let spans = d.detect("key is AKIAIOSFODNN7EXAMPLE ok");
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
        let d = SecretsDetector::new();
        assert!(d.detect("say AAAAAAAAAAAAAAAAAAAAAAAA aloud").is_empty());
    }
}
