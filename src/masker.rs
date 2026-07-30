use crate::span::{Action, Span};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct SessionMap {
    by_original: HashMap<String, String>,
    counters: HashMap<String, usize>,
}

pub struct Masker {
    sessions: Mutex<HashMap<String, SessionMap>>,
}

impl Masker {
    pub fn new() -> Self {
        Self { sessions: Mutex::new(HashMap::new()) }
    }

    pub fn mask_text(&self, session: &str, text: &str, spans: &[Span]) -> String {
        let mut sessions = self.sessions.lock().unwrap();
        let map = sessions.entry(session.to_string()).or_default();
        let mut out = text.to_string();

        // First pass: collect replacements left-to-right for consistent counters
        let mut replacements: Vec<(usize, usize, String)> = Vec::new();
        let mut sorted = spans.to_vec();
        sorted.sort_by_key(|s| s.start);
        for span in &sorted {
            let replacement = match span.action {
                Action::Redact => "[REDACTED_SECRET]".to_string(),
                Action::Mask => {
                    if let Some(ph) = map.by_original.get(&span.text) {
                        ph.clone()
                    } else {
                        let label = span.entity.to_uppercase();
                        let n = map.counters.entry(label.clone()).or_insert(0);
                        *n += 1;
                        let ph = format!("[{}_{}]", label, n);
                        map.by_original.insert(span.text.clone(), ph.clone());
                        ph
                    }
                }
            };
            replacements.push((span.start, span.end, replacement));
        }

        // Second pass: apply right-to-left so byte offsets stay valid
        replacements.sort_by_key(|r| std::cmp::Reverse(r.0));
        for (start, end, replacement) in replacements {
            out.replace_range(start..end, &replacement);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::{Action, Span};

    #[test]
    fn masks_consistently_within_session() {
        let m = Masker::new();
        let t = "email a@x.com or again a@x.com";
        let spans = vec![
            Span::new(6, 13, "email", Action::Mask, "a@x.com"),
            Span::new(23, 30, "email", Action::Mask, "a@x.com"),
        ];
        let out = m.mask_text("s_1", t, &spans);
        assert_eq!(out, "email [EMAIL_1] or again [EMAIL_1]");
    }

    #[test]
    fn redact_stores_no_mapping_and_new_entities_increment() {
        let m = Masker::new();
        let t = "a@x.com and b@x.com";
        let spans = vec![
            Span::new(0, 7, "email", Action::Mask, "a@x.com"),
            Span::new(12, 19, "email", Action::Mask, "b@x.com"),
        ];
        assert_eq!(m.mask_text("s_1", t, &spans), "[EMAIL_1] and [EMAIL_2]");
        let s = "AKIAIOSFODNN7EXAMPLE";
        let spans = vec![Span::new(0, 20, "api_key", Action::Redact, s)];
        assert_eq!(m.mask_text("s_1", s, &spans), "[REDACTED_SECRET]");
    }

    #[test]
    fn sessions_are_isolated() {
        let m = Masker::new();
        let t = "a@x.com";
        let spans = vec![Span::new(0, 7, "email", Action::Mask, "a@x.com")];
        m.mask_text("s_1", t, &spans);
        assert_eq!(m.mask_text("s_2", t, &spans), "[EMAIL_1]");
    }
}
