use crate::span::{Action, Span};
use crate::detect::Detector;
use regex::Regex;

pub struct RegexDetector {
    email: Regex,
    card: Regex,
}

impl RegexDetector {
    pub fn new() -> Self {
        Self {
            email: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(),
            card: Regex::new(r"\b(?:\d[ -]?){13,19}\b").unwrap(),
        }
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
        for m in self.email.find_iter(text) {
            out.push(Span::new(m.start(), m.end(), "email", Action::Mask, m.as_str()));
        }
        for m in self.card.find_iter(text) {
            let digits: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
            if luhn_valid(&digits) {
                out.push(Span::new(m.start(), m.end(), "credit_card", Action::Mask, m.as_str()));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_email() {
        let d = RegexDetector::new();
        let spans = d.detect("reach me at jane.doe@example.com please");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].entity, "email");
        assert_eq!(spans[0].text, "jane.doe@example.com");
    }

    #[test]
    fn detects_card_only_when_luhn_valid() {
        let d = RegexDetector::new();
        // 4111 1111 1111 1111 is the canonical Luhn-valid test card
        let hits = d.detect("card 4111 1111 1111 1111 exp 12/30");
        assert_eq!(hits.iter().filter(|s| s.entity == "credit_card").count(), 1);
        // same shape, invalid checksum
        let misses = d.detect("card 4111 1111 1111 1112 exp 12/30");
        assert_eq!(misses.iter().filter(|s| s.entity == "credit_card").count(), 0);
    }
}
