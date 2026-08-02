use crate::span::{Action, Span};
use crate::detect::Detector;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checksum {
    Luhn,
    Mod97,
    AtoTfn,
}

pub struct RegexEntity {
    pub id: String,
    pub pattern: Regex,
    pub action: Action,
    pub checksum: Option<Checksum>,
    pub alert: bool,
}

pub struct RegexDetector {
    entities: Vec<RegexEntity>,
}

impl RegexDetector {
    pub fn from_entities(entities: Vec<RegexEntity>) -> Self {
        Self { entities }
    }
}

impl Detector for RegexDetector {
    fn detect(&self, text: &str) -> Vec<Span> {
        let mut out = Vec::new();
        for e in &self.entities {
            for m in e.pattern.find_iter(text) {
                let valid = match e.checksum {
                    None => true,
                    Some(Checksum::Luhn) => {
                        let digits: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
                        luhn_valid(&digits)
                    }
                    Some(Checksum::Mod97) => mod97_valid(m.as_str()),
                    Some(Checksum::AtoTfn) => ato_tfn_valid(m.as_str()),
                };
                if !valid {
                    continue;
                }
                out.push(Span { alert: e.alert, ..Span::new(m.start(), m.end(), &e.id, e.action, m.as_str()) });
            }
        }
        out
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

/// ISO 13616 IBAN mod-97 check: reorder (first 4 chars to the end), map
/// A=10..Z=35, then the resulting number must satisfy `% 97 == 1`.
pub(crate) fn mod97_valid(iban: &str) -> bool {
    let cleaned: String = iban
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if cleaned.len() < 5 {
        return false;
    }
    let rotated = format!("{}{}", &cleaned[4..], &cleaned[..4]);
    let mut rem: i64 = 0;
    for c in rotated.chars() {
        let v = c.to_digit(36).unwrap_or(0) as i64;
        if v > 9 {
            rem = (rem * 100 + v) % 97;
        } else {
            rem = (rem * 10 + v) % 97;
        }
    }
    rem == 1
}

/// ATO TFN check: 8 or 9 digits; the sum of digit·weight (weights from the
/// rightmost digit: 1,4,3,7,5,8,6,9,10) must be a multiple of 11.
pub(crate) fn ato_tfn_valid(tfn: &str) -> bool {
    let digits: Vec<u32> = tfn.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 8 && digits.len() != 9 {
        return false;
    }
    let weights = [1u32, 4, 3, 7, 5, 8, 6, 9, 10];
    let sum: u32 = digits.iter().rev().zip(weights.iter()).map(|(d, w)| d * w).sum();
    sum % 11 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_detector() -> RegexDetector {
        RegexDetector::from_entities(vec![
            RegexEntity { id: "email".into(), pattern: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(), action: Action::Mask, checksum: None, alert: false },
            RegexEntity { id: "credit_card".into(), pattern: Regex::new(r"\b(?:\d[ -]?){13,19}\b").unwrap(), action: Action::Mask, checksum: Some(Checksum::Luhn), alert: false },
            RegexEntity { id: "iban".into(), pattern: Regex::new(r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){3,7}[A-Z0-9]{1,3}\b").unwrap(), action: Action::Mask, checksum: Some(Checksum::Mod97), alert: false },
            RegexEntity { id: "tfn".into(), pattern: Regex::new(r"\b\d{8,9}\b").unwrap(), action: Action::Mask, checksum: Some(Checksum::AtoTfn), alert: false },
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
    fn detects_iban_only_when_mod97_valid() {
        let d = test_detector();
        // Canonical ISO 13616 example IBAN (Deutsche Bank) — mod 97 == 1.
        assert_eq!(d.detect("IBAN DE89370400440532013000").iter().filter(|s| s.entity == "iban").count(), 1);
        // Flip the last digit: mod 97 check must reject it.
        assert_eq!(d.detect("IBAN DE89370400440532013001").iter().filter(|s| s.entity == "iban").count(), 0);
    }

    #[test]
    fn detects_tfn_only_when_ato_checksum_valid() {
        let d = test_detector();
        // "12345678" satisfies the ATO TFN rule (weighted sum divisible by 11).
        assert_eq!(d.detect("TFN 12345678").iter().filter(|s| s.entity == "tfn").count(), 1);
        assert_eq!(d.detect("TFN 12345679").iter().filter(|s| s.entity == "tfn").count(), 0);
    }

    #[test]
    fn detects_multiple_entities_with_different_actions() {
        let d = RegexDetector::from_entities(vec![
            RegexEntity { id: "ip".into(), pattern: Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap(), action: Action::Redact, checksum: None, alert: false },
        ]);
        let spans = d.detect("host 10.0.0.1 reachable");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].entity, "ip");
        assert!(matches!(spans[0].action, Action::Redact));
    }

    #[test]
    fn mod97_validation_rejects_short_or_garbage_input() {
        assert!(!mod97_valid(""));
        assert!(!mod97_valid("DE89"));
        assert!(!mod97_valid("hello world 1234"));
    }

    #[test]
    fn ato_tfn_validation_rejects_wrong_lengths() {
        assert!(!ato_tfn_valid(""));
        assert!(!ato_tfn_valid("1234567"));
        assert!(!ato_tfn_valid("1234567890"));
    }

    #[test]
    fn alert_flag_propagates_to_spans() {
        let d = RegexDetector::from_entities(vec![
            RegexEntity { id: "iban".into(), pattern: Regex::new(r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){3,7}[A-Z0-9]{1,3}\b").unwrap(), action: Action::Mask, checksum: Some(Checksum::Mod97), alert: true },
            RegexEntity { id: "email".into(), pattern: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(), action: Action::Mask, checksum: None, alert: false },
        ]);
        let spans = d.detect("IBAN DE89370400440532013000 email a@x.com");
        assert!(spans.iter().any(|s| s.entity == "iban" && s.alert), "iban must alert: {spans:?}");
        assert!(spans.iter().any(|s| s.entity == "email" && !s.alert), "email must not alert: {spans:?}");
    }
}
