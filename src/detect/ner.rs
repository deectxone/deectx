use crate::chunk::chunk_text;
use crate::detect::Detector;
use crate::span::{Action, Span};
use std::path::PathBuf;

const ENT_TOKEN_ID: i64 = 250103;
const SEP_TOKEN_ID: i64 = 250104;
const CLS_ID: i64 = 1;
const EOS_SEP_ID: i64 = 2;
const MAX_WIDTH: usize = 12;
const THRESHOLD: f32 = 0.5;
const CHUNK_CHARS: usize = 512;
const CHUNK_OVERLAP: usize = 50;

pub struct NerDetector {
    session: Option<std::sync::Mutex<ort::session::Session>>,
    tokenizer: Option<tokenizers::Tokenizer>,
    labels: Vec<(String, Action, bool)>,
}

impl NerDetector {
    pub fn new(model_dir: PathBuf, labels: Vec<(String, Action, bool)>) -> Self {
        if labels.is_empty() {
            return Self { session: None, tokenizer: None, labels };
        }
        let model_path = model_dir.join("model.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");
        // Fail open before touching ort if the model files are absent, so we
        // never initialize onnxruntime (and poison its global mutex on a
        // missing-DLL panic) for a model dir that cannot work anyway.
        if !model_path.exists() || !tokenizer_path.exists() {
            tracing::warn!("NER model files missing; NER disabled (fail-open)");
            return Self { session: None, tokenizer: None, labels };
        }
        // ort lazy-inits onnxruntime on the first commit; a missing DLL panics
        // via an internal `.expect`, so the whole load is catch_unwind-guarded.
        let loaded = std::panic::catch_unwind(|| -> anyhow::Result<(ort::session::Session, tokenizers::Tokenizer)> {
            let session = ort::session::Session::builder()?.commit_from_file(&model_path)?;
            let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| anyhow::anyhow!("tokenizer load failed: {e}"))?;
            Ok((session, tokenizer))
        });
        match loaded {
            Ok(Ok((session, tokenizer))) => {
                tracing::info!("NER model loaded ({} labels)", labels.len());
                Self { session: Some(std::sync::Mutex::new(session)), tokenizer: Some(tokenizer), labels }
            }
            Ok(Err(e)) => {
                tracing::warn!("NER model failed to load; NER disabled (fail-open): {e}");
                Self { session: None, tokenizer: None, labels }
            }
            Err(_) => {
                tracing::warn!("NER model load panicked (onnxruntime DLL missing?); NER disabled (fail-open)");
                Self { session: None, tokenizer: None, labels }
            }
        }
    }
}

impl Detector for NerDetector {
    fn detect(&self, text: &str) -> Vec<Span> {
        let (Some(session), Some(tokenizer)) = (self.session.as_ref(), self.tokenizer.as_ref()) else {
            return Vec::new();
        };
        if self.labels.is_empty() {
            return Vec::new();
        }
        let mut spans = Vec::new();
        let Ok(mut session) = session.lock() else {
            tracing::warn!("NER session mutex poisoned; NER disabled (fail-open)");
            return Vec::new();
        };
        for chunk in chunk_text(text, CHUNK_CHARS, CHUNK_OVERLAP) {
            match run_chunk(&mut session, tokenizer, &self.labels, &chunk.text) {
                Ok(mut local) => {
                    for s in &mut local {
                        s.start += chunk.start;
                        s.end += chunk.start;
                    }
                    spans.extend(local);
                }
                Err(e) => tracing::warn!("NER chunk inference failed (fail-open): {e}"),
            }
        }
        spans
    }

    fn ready(&self) -> bool {
        match &self.session {
            Some(m) if m.is_poisoned() => false,
            Some(_) => self.tokenizer.is_some(),
            None => false,
        }
    }
}

fn run_chunk(
    session: &mut ort::session::Session,
    tokenizer: &tokenizers::Tokenizer,
    labels: &[(String, Action, bool)],
    text: &str,
) -> anyhow::Result<Vec<Span>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Ok(Vec::new());
    }
    let ranges = word_ranges(text);
    let label_names: Vec<&str> = labels.iter().map(|l| l.0.as_str()).collect();
    let (ids, words_mask) = build_prompt(tokenizer, &label_names, &words)?;
    let len = ids.len();
    let text_len = words.len();

    let ids_arr = ndarray::Array2::<i64>::from_shape_vec((1, len), ids)?;
    let attn_arr = ndarray::Array2::<i64>::ones((1, len));
    let words_arr = ndarray::Array2::<i64>::from_shape_vec((1, len), words_mask)?;
    let text_lengths = ndarray::Array2::<i64>::from_elem((1, 1), text_len as i64);

    let num_spans = text_len * MAX_WIDTH;
    let mut span_idx_flat = Vec::with_capacity(num_spans * 2);
    let mut span_mask = ndarray::Array2::<bool>::from_elem((1, num_spans), true);
    for s in 0..text_len {
        for w in 0..MAX_WIDTH {
            span_idx_flat.push(s as i64);
            span_idx_flat.push((s + w) as i64);
            if s + w >= text_len {
                span_mask[[0, s * MAX_WIDTH + w]] = false;
            }
        }
    }
    let span_idx_arr = ndarray::Array3::<i64>::from_shape_vec((1, num_spans, 2), span_idx_flat)?;

    let input_ids = ort::value::Tensor::from_array(ids_arr)?;
    let attention_mask = ort::value::Tensor::from_array(attn_arr)?;
    let words_mask_t = ort::value::Tensor::from_array(words_arr)?;
    let text_lengths_t = ort::value::Tensor::from_array(text_lengths)?;
    let span_idx_t = ort::value::Tensor::from_array(span_idx_arr)?;
    let span_mask_t = ort::value::Tensor::from_array(span_mask)?;

    let outputs = session.run(ort::inputs![
        "input_ids" => &input_ids,
        "attention_mask" => &attention_mask,
        "words_mask" => &words_mask_t,
        "text_lengths" => &text_lengths_t,
        "span_idx" => &span_idx_t,
        "span_mask" => &span_mask_t
    ])?;

    let mut logits_shape = Vec::new();
    let mut logits_data: Option<Vec<f32>> = None;
    for (name, val) in outputs.iter() {
        let shape: Vec<i64> = val.shape().to_vec();
        if name == "logits" {
            let (_s, data) = val.try_extract_tensor::<f32>()?;
            logits_data = Some(data.to_vec());
            logits_shape = shape;
        }
    }
    let Some(data) = logits_data else {
        return Ok(Vec::new());
    };
    Ok(decode_spans(&data, &logits_shape, &words, &ranges, labels))
}

fn build_prompt(
    tokenizer: &tokenizers::Tokenizer,
    labels: &[&str],
    text_words: &[&str],
) -> anyhow::Result<(Vec<i64>, Vec<i64>)> {
    let mut ids: Vec<i64> = vec![CLS_ID];
    let mut word_starts: Vec<usize> = Vec::new();
    let mut prompt_words: usize = 0;

    let push_word = |tok_ids: &[i64], ids: &mut Vec<i64>, starts: &mut Vec<usize>| {
        starts.push(ids.len());
        ids.extend_from_slice(tok_ids);
    };

    for label in labels {
        push_word(&[ENT_TOKEN_ID], &mut ids, &mut word_starts);
        prompt_words += 1;
        let enc = tokenizer.encode(*label, false).map_err(|e| anyhow::anyhow!("tokenizer encode failed: {e}"))?;
        let toks: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        push_word(&toks, &mut ids, &mut word_starts);
        prompt_words += 1;
    }
    push_word(&[SEP_TOKEN_ID], &mut ids, &mut word_starts);
    prompt_words += 1;

    for word in text_words {
        let enc = tokenizer.encode(*word, false).map_err(|e| anyhow::anyhow!("tokenizer encode failed: {e}"))?;
        let toks: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        push_word(&toks, &mut ids, &mut word_starts);
    }
    ids.push(EOS_SEP_ID);

    let len = ids.len();
    let mut words_mask = vec![0i64; len];
    let mut seen_at: usize = 0;
    for start in word_starts.iter().copied() {
        seen_at += 1;
        if seen_at > prompt_words {
            words_mask[start] = (seen_at - prompt_words) as i64;
        }
    }
    Ok((ids, words_mask))
}

fn word_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut word_start: Option<usize> = None;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = word_start.take() {
                ranges.push((s, idx));
            }
        } else if word_start.is_none() {
            word_start = Some(idx);
        }
    }
    if let Some(s) = word_start {
        ranges.push((s, text.len()));
    }
    ranges
}

fn decode_spans(
    logits: &[f32],
    shape: &[i64],
    text_words: &[&str],
    ranges: &[(usize, usize)],
    labels: &[(String, Action, bool)],
) -> Vec<Span> {
    let n_words = shape.get(1).copied().unwrap_or(0) as usize;
    let max_width = shape.get(2).copied().unwrap_or(0) as usize;
    let n_classes = shape.get(3).copied().unwrap_or(0) as usize;
    if n_words == 0 || max_width == 0 || n_classes == 0 || n_classes != labels.len() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    for c in 0..n_classes {
        let mut best: Option<(usize, usize, f32)> = None;
        for w in 0..n_words.min(text_words.len()) {
            for width in 0..max_width {
                if w + width >= n_words {
                    break;
                }
                let logit = logits[(w * max_width + width) * n_classes + c];
                let p = 1.0 / (1.0 + (-logit).exp());
                if p >= THRESHOLD && best.map_or(true, |(_, _, bp)| p > bp) {
                    best = Some((w, width, p));
                }
            }
        }
        if let Some((w, width, _)) = best {
            if let (Some(&(s, _)), Some(&(_, e))) = (ranges.get(w), ranges.get(w + width)) {
                let (label, action, alert) = &labels[c];
                spans.push(Span {
                    start: s,
                    end: e,
                    entity: label.clone(),
                    action: *action,
                    text: text_words[w..=w + width].join(" "),
                    alert: *alert,
                });
            }
        }
    }
    spans
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
        assert!(!d.ready());
    }

    #[test]
    fn no_labels_yields_no_spans() {
        let d = NerDetector::new(PathBuf::from("./definitely-missing-models"), vec![]);
        assert!(d.detect("anything at all").is_empty());
    }

    #[test]
    fn word_ranges_split_on_whitespace_with_byte_offsets() {
        let text = "John  Smith\nlives";
        assert_eq!(word_ranges(text), vec![(0, 4), (6, 11), (12, 17)]);
        let text2 = "héllo wörld";
        assert_eq!(word_ranges(text2), vec![(0, 6), (7, 13)]);
    }

    #[test]
    fn decode_spans_selects_best_confident_span_per_class() {
        let labels: Vec<(String, Action, bool)> = vec![
            ("person".into(), Action::Mask, false),
            ("location".into(), Action::Mask, true),
        ];
        let text_words = ["John", "Smith", "lives", "in", "Sydney"];
        let ranges = [(0usize, 4usize), (5, 10), (11, 16), (17, 19), (20, 26)];
        let n_words = 5;
        let max_width = 12;
        let n_classes = 2;
        let mut logits = vec![0f32; n_words * max_width * n_classes];
        let idx = |w: usize, width: usize, c: usize| (w * max_width + width) * n_classes + c;
        logits[idx(0, 1, 0)] = 5.0;
        logits[idx(4, 0, 1)] = 6.0;
        logits[idx(1, 0, 1)] = -3.0;

        let shape = vec![1i64, n_words as i64, max_width as i64, n_classes as i64];
        let spans = decode_spans(&logits, &shape, &text_words, &ranges, &labels);
        assert_eq!(spans.len(), 2, "one confident span per class: {spans:?}");
        let person = spans.iter().find(|s| s.entity == "person").unwrap();
        assert_eq!(person.start, 0);
        assert_eq!(person.end, 10);
        assert_eq!(person.text, "John Smith");
        assert!(!person.alert);
        let loc = spans.iter().find(|s| s.entity == "location").unwrap();
        assert_eq!(loc.text, "Sydney");
        assert_eq!(loc.start, 20);
        assert!(loc.alert);
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
        let chain = build_chain(&[pack], true, PathBuf::from("./definitely-missing-models"));
        assert!(!chain.ready(), "chain must report not-ready when the NER model is missing");
        assert!(chain.detect("John Smith lives in Sydney").is_empty());
    }
}