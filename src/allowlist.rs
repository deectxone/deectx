use crate::span::Span;

pub struct Allowlist {
    entries: Vec<String>,
}

impl Allowlist {
    pub fn new(entries: Vec<String>) -> Self {
        Self {
            entries: entries.into_iter()
                .map(|e| e.trim().to_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
        }
    }

    pub fn is_allowed(&self, text: &str) -> bool {
        self.entries.iter().any(|e| e == &text.to_lowercase())
    }

    pub fn filter(&self, spans: Vec<Span>) -> Vec<Span> {
        spans.into_iter().filter(|s| !self.is_allowed(&s.text)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Action;

    #[test]
    fn drops_spans_whose_text_is_allowed() {
        let allow = Allowlist::new(vec!["jane.doe@example.com".into()]);
        let spans = vec![
            Span::new(0, 20, "email", Action::Mask, "jane.doe@example.com"),
            Span::new(21, 41, "email", Action::Mask, "bob.smith@example.com"),
        ];
        let kept = allow.filter(spans);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "bob.smith@example.com");
    }

    #[test]
    fn matching_is_case_insensitive_and_trims_whitespace() {
        let allow = Allowlist::new(vec!["  JANE.Doe@Example.com  ".into()]);
        assert!(allow.is_allowed("jane.doe@example.com"));
        assert!(!allow.is_allowed("bob.smith@example.com"));
    }

    #[test]
    fn empty_entries_are_ignored() {
        let allow = Allowlist::new(vec![String::new(), "   ".into()]);
        assert!(!allow.is_allowed("anything"));
    }
}
