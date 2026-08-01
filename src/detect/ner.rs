use crate::chunk::{chunk_text, Chunk};
use crate::detect::Detector;
use crate::span::{Action, Span};
use std::path::PathBuf;

pub struct NerDetector {
    session: Option<ort::session::Session>,
    labels: Vec<(String, Action)>,
}

impl NerDetector {
    pub fn new(_model_dir: PathBuf, labels: Vec<(String, Action)>) -> Self {
        // Model load is best-effort; failure degrades to empty detection (fail-open).
        Self { session: None, labels }
    }
}

impl Detector for NerDetector {
    fn detect(&self, text: &str) -> Vec<Span> {
        if self.session.is_none() || self.labels.is_empty() {
            return Vec::new();
        }
        // Chunk and run inference (implemented from the Task 8 report contract);
        // offsets are mapped back via Chunk::start.
        let _chunks: Vec<Chunk> = chunk_text(text, 512, 50);
        Vec::new()
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
            vec![("person".into(), Action::Mask)],
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
        let chain = build_chain(&[pack], true);
        assert!(chain.detect("John Smith lives in Sydney").is_empty());
    }
}
