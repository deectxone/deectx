use crate::span::{Action, Span};
use crate::detect::Detector;
use regex::Regex;

pub struct RegexEntity {
    pub id: String,
    pub pattern: Regex,
    pub action: Action,
    pub luhn: bool,
}

pub struct RegexDetector {
    entities: Vec<RegexEntity>,
}

impl RegexDetector {
    pub fn from_entities(entities: Vec<RegexEntity>) -> Self {
        Self { entities }
    }
}

pub(crate) fn luhn_valid(digits: &str) -> bool {
    let digits: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&digits.len()) { return false; }
    let sum: u32 = digits.iter().rev().enumerate().map(|(i, d)| {
        if i % 2 == 1 { let x = d * 2; if x > 9 { x - 9 } else { x } } else { *d }
    }).sum();
    sum % 10 == 0
}

impl Detector for RegexDetector {
    fn detect(&self, text: &str) -> Vec<Span> {
        let mut out = Vec::new();
        for e in &self.entities {
            for m in e.pattern.find_iter(text) {
                if e.luhn {
                    let digits: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
                    if !luhn_valid(&digits) { continue; }
                }
                out.push(Span::new(m.start(), m.end(), &e.id, e.action, m.as_str()));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_detector() -> RegexDetector {
        RegexDetector::from_entities(vec![
            RegexEntity { id: "email".into(), pattern: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(), action: Action::Mask, luhn: false },
            RegexEntity { id: "credit_card".into(), pattern: Regex::new(r"\b(?:\d[ -]?){13,19}\b").unwrap(), action: Action::Mask, luhn: true },
        ])
    }

    #[test]
    fn detects_email() {
        let spans = test_detector().detect("reach me at jane.doe@example.com please");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].entity, "email");
        assert_eq!(spans[0].text, "jane.doe@example.com");
    }

    #[test]
    fn detects_card_only_when_luhn_valid() {
        let d = test_detector();
        let hits = d.detect("card 4111 1111 1111 1111 exp 12/30");
        assert_eq!(hits.iter().filter(|s| s.entity == "credit_card").count(), 1);
        let misses = d.detect("card 4111 1111 1111 1112 exp 12/30");
        assert_eq!(misses.iter().filter(|s| s.entity == "credit_card").count(), 0);
    }

    #[test]
    fn detects_multiple_entities_with_different_actions() {
        let d = RegexDetector::from_entities(vec![
            RegexEntity { id: "ip".into(), pattern: Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap(), action: Action::Redact, luhn: false },
        ]);
        let spans = d.detect("host 10.0.0.1 reachable");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].entity, "ip");
        assert!(matches!(spans[0].action, Action::Redact));
    }
}
