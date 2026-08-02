use crate::detect::Detector;
use crate::span::{Action, Span};
use std::path::PathBuf;

pub struct NerDetector {
    session: Option<ort::session::Session>,
    labels: Vec<(String, Action, bool)>,
}

impl NerDetector {
    pub fn new(model_dir: PathBuf, labels: Vec<(String, Action, bool)>) -> Self {
        if !labels.is_empty() {
            tracing::warn!("NerDetector is not yet wired to load a model (model_dir={:?}); NER detection is a no-op", model_dir);
        }
        Self { session: None, labels }
    }
}

impl Detector for NerDetector {
    fn detect(&self, _text: &str) -> Vec<Span> {
        if self.session.is_none() || self.labels.is_empty() {
            return Vec::new();
        }
        Vec::new()
    }

    fn ready(&self) -> bool {
        self.session.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::{build_chain, Pack, PackEntity};

    #[test]
    fn missing_model_fails_open_to_empty_spans() {
        let d = NerDetector::new(
            PathBuf::from("./definitely-missing-models"),
            vec![("person".into(), Action::Mask, false)],
        );
        assert!(d.detect("John Smith lives in Sydney").is_empty());
    }

    #[test]
    fn no_labels_yields_no_spans() {
        let d = NerDetector::new(PathBuf::from("./definitely-missing-models"), vec![]);
        assert!(d.detect("anything at all").is_empty());
    }

    #[test]
    fn ner_enabled_build_chain_wires_fail_open() {
        let pack = Pack {
            name: "ner_test".into(),
            version: "0.1.0".into(),
            entities: vec![PackEntity {
                id: "person".into(),
                detector: "ner".into(),
                labels: vec!["person".into()],
                pattern: None,
                patterns: Vec::new(),
                checksum: None,
                entropy_min: None,
                action: Action::Mask,
                alert: false,
            }],
            settings: Default::default(),
        };
        // NerDetector is constructed from the "ner" entity but has no session,
        // so the chain must still run and return no spans (fail-open).
        let chain = build_chain(&[pack], true, PathBuf::from("./definitely-missing-models"));
        assert!(!chain.ready(), "chain must report not-ready when the NER model is missing");
        assert!(chain.detect("John Smith lives in Sydney").is_empty());
    }
}
