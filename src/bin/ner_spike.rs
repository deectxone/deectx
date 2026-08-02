use std::path::Path;

const ENT_TOKEN_ID: i64 = 250103;
const SEP_TOKEN_ID: i64 = 250104;
const CLS_ID: i64 = 1;
const EOS_SEP_ID: i64 = 2;
const MAX_WIDTH: usize = 12;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let dir = std::env::var("DEECTX_MODEL_DIR").unwrap_or_else(|_| "./models".into());
    let model_path = Path::new(&dir).join("model.onnx");
    let tok_path = Path::new(&dir).join("tokenizer.json");
    tracing::info!("loading model from {:?}", model_path);
    assert!(model_path.exists(), "model.onnx missing in {dir}");
    assert!(tok_path.exists(), "tokenizer.json missing in {dir}");

    let mut session = ort::session::Session::builder()?.commit_from_file(model_path)?;
    tracing::info!("session loaded");
    for i in session.inputs() {
        let shape: Vec<i64> = i
            .dtype()
            .tensor_shape()
            .map(|s| s.to_vec())
            .unwrap_or_default();
        tracing::info!(
            "input {}: type={:?} shape={shape:?}",
            i.name(),
            i.dtype().tensor_type()
        );
    }
    for o in session.outputs() {
        let shape: Vec<i64> = o
            .dtype()
            .tensor_shape()
            .map(|s| s.to_vec())
            .unwrap_or_default();
        tracing::info!(
            "output {}: type={:?} shape={shape:?}",
            o.name(),
            o.dtype().tensor_type()
        );
    }

    let tokenizer = tokenizers::Tokenizer::from_file(tok_path)
        .map_err(|e| anyhow::anyhow!("tokenizer load failed: {e}"))?;

    let text = "John Smith lives in Sydney";
    let labels = ["person", "location", "organization", "date"];
    let text_words: Vec<&str> = text.split_whitespace().collect();
    tracing::info!("text words: {text_words:?}; labels: {labels:?}");

    let mut ids: Vec<i64> = vec![CLS_ID];
    let mut word_starts: Vec<usize> = Vec::new();
    let mut seen_words: usize = 0;
    let mut prompt_words: usize = 0;

    let mut push_word = |tok_ids: &[i64], ids: &mut Vec<i64>, starts: &mut Vec<usize>| {
        starts.push(ids.len());
        ids.extend_from_slice(tok_ids);
    };

    for label in labels.iter() {
        push_word(&[ENT_TOKEN_ID], &mut ids, &mut word_starts);
        seen_words += 1;
        prompt_words += 1;
        let enc = tokenizer
            .encode(*label, false)
            .map_err(|e| anyhow::anyhow!("encode {label}: {e}"))?;
        let toks: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        push_word(&toks, &mut ids, &mut word_starts);
        seen_words += 1;
        prompt_words += 1;
    }
    push_word(&[SEP_TOKEN_ID], &mut ids, &mut word_starts);
    seen_words += 1;
    prompt_words += 1;

    for word in text_words.iter() {
        let enc = tokenizer
            .encode(*word, false)
            .map_err(|e| anyhow::anyhow!("encode {word}: {e}"))?;
        let toks: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        push_word(&toks, &mut ids, &mut word_starts);
        seen_words += 1;
    }
    ids.push(EOS_SEP_ID);

    let len = ids.len();
    let text_len = text_words.len();
    let mut words_mask = vec![0i64; len];
    let mut seen_at: usize = 0;
    for start in word_starts.iter().copied() {
        seen_at += 1;
        if seen_at > prompt_words {
            words_mask[start] = (seen_at - prompt_words) as i64;
        }
    }

    tracing::info!(
        "prompt_words={prompt_words}; text_words={text_len}; seq_len={len}; total word starts={}",
        word_starts.len()
    );
    tracing::info!("input_ids={ids:?}");
    tracing::info!("words_mask={words_mask:?}");

    let ids_arr = ndarray::Array2::<i64>::from_shape_vec((1, len), ids.clone())?;
    let attn_arr = ndarray::Array2::<i64>::ones((1, len));
    let words_arr = ndarray::Array2::<i64>::from_shape_vec((1, len), words_mask.clone())?;
    let text_lengths = ndarray::Array2::<i64>::from_elem((1, 1), text_len as i64);

    let mut span_idx_flat = Vec::with_capacity(text_len * MAX_WIDTH * 2);
    let mut span_mask = ndarray::Array2::<bool>::from_elem((1, text_len * MAX_WIDTH), true);
    for s in 0..text_len {
        for w in 0..MAX_WIDTH {
            span_idx_flat.push(s as i64);
            span_idx_flat.push((s + w) as i64);
            if s + w >= text_len {
                span_mask[[0, s * MAX_WIDTH + w]] = false;
            }
        }
    }
    let num_spans = text_len * MAX_WIDTH;
    let span_idx_arr = ndarray::Array3::<i64>::from_shape_vec((1, num_spans, 2), span_idx_flat)?;

    tracing::info!("num_spans={num_spans}; text_lengths={text_len}");

    let input_ids = ort::value::Tensor::from_array(ids_arr)?;
    let attention_mask = ort::value::Tensor::from_array(attn_arr)?;
    let words_mask = ort::value::Tensor::from_array(words_arr)?;
    let text_lengths = ort::value::Tensor::from_array(text_lengths)?;
    let span_idx = ort::value::Tensor::from_array(span_idx_arr)?;
    let span_mask = ort::value::Tensor::from_array(span_mask)?;

    let outputs = session.run(ort::inputs![
        "input_ids" => &input_ids,
        "attention_mask" => &attention_mask,
        "words_mask" => &words_mask,
        "text_lengths" => &text_lengths,
        "span_idx" => &span_idx,
        "span_mask" => &span_mask
    ])?;
    tracing::info!("forward pass OK: {} outputs", outputs.len());
    let mut logits_shape = Vec::new();
    let mut logits_data: Option<Vec<f32>> = None;
    for (name, val) in outputs.iter() {
        let shape: Vec<i64> = val.shape().to_vec();
        tracing::info!("output {name}: shape {shape:?}");
        if name == "logits" {
            let (_s, data) = val.try_extract_tensor::<f32>()?;
            logits_data = Some(data.to_vec());
            logits_shape = shape;
        }
    }
    if let Some(data) = logits_data {
        let n_words = logits_shape[1] as usize;
        let max_width = logits_shape[2] as usize;
        let n_classes = logits_shape[3] as usize;
        let label_names = ["person", "location", "organization", "date"];
        for w in 0..n_words {
            let mut scores = Vec::with_capacity(n_classes);
            for c in 0..n_classes {
                let logit = data[(w * max_width + 0) * n_classes + c];
                let p = 1.0 / (1.0 + (-logit).exp());
                scores.push((label_names[c], p));
            }
            scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            tracing::info!(
                "word {w} ({}) top: {:?}",
                text_words.get(w).copied().unwrap_or("?"),
                &scores[..n_classes.min(3)]
            );
        }
        for c in 0..n_classes {
            let mut best = (0usize, 0usize, f32::MIN);
            for w in 0..n_words {
                for width in 0..max_width {
                    if w + width >= n_words {
                        break;
                    }
                    let logit = data[(w * max_width + width) * n_classes + c];
                    let p = 1.0 / (1.0 + (-logit).exp());
                    if p > best.2 {
                        best = (w, width, p);
                    }
                }
            }
            let span = &text_words[best.0..=best.0 + best.1];
            tracing::info!(
                "best {:>12} span: words {:?} = \"{}\" (p={:.4})",
                label_names[c],
                &text_words[best.0..=best.0 + best.1],
                span.join(" "),
                best.2
            );
        }
    }
    Ok(())
}
