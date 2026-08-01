use deectx::detect::Detector;
use deectx::packs::{build_chain, Pack};
use std::collections::HashMap;

const RECALL_GATE: f64 = 0.9;
const PRECISION_GATE: f64 = 0.8;

#[derive(serde::Deserialize)]
struct Fixture {
    name: String,
    text: String,
    expected: Vec<Expected>,
}

#[derive(serde::Deserialize)]
struct Expected {
    entity: String,
    text: String,
}

fn load_fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map(|e| e == "yaml").unwrap_or(false) {
            let text = std::fs::read_to_string(&path).unwrap();
            out.push(serde_yaml::from_str(&text).unwrap());
        }
    }
    out
}

#[test]
fn golden_set_precision_recall_gate() {
    let chain = build_chain(&[Pack::builtin_default()], false, std::path::PathBuf::from("./models"));
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_count = 0usize;
    let mut failures = Vec::new();

    for fx in load_fixtures() {
        let spans = chain.detect(&fx.text);
        let mut remaining: HashMap<(String, String), usize> = HashMap::new();
        for exp in &fx.expected {
            *remaining.entry((exp.entity.clone(), exp.text.trim().to_string())).or_insert(0) += 1;
        }
        let mut local_fp = 0usize;
        for s in &spans {
            let key = (s.entity.clone(), s.text.trim().to_string());
            if let Some(n) = remaining.get_mut(&key) {
                if *n > 0 { *n -= 1; tp += 1; } else { fp += 1; local_fp += 1; }
            } else {
                fp += 1;
                local_fp += 1;
            }
        }
        let local_fn: usize = remaining.values().sum();
        fn_count += local_fn;
        if local_fp > 0 || local_fn > 0 {
            failures.push(format!(
                "{}: missed={:?} false_positives={:?}",
                fx.name,
                remaining,
                spans.iter().map(|s| (s.entity.as_str(), s.text.trim())).collect::<Vec<_>>()
            ));
        }
    }

    let recall = tp as f64 / (tp + fn_count) as f64;
    let precision = tp as f64 / (tp + fp) as f64;
    for f in &failures {
        eprintln!("GOLDEN: {f}");
    }
    assert!(recall >= RECALL_GATE,
        "recall {recall:.2} < {RECALL_GATE}:\n{}", failures.join("\n"));
    assert!(precision >= PRECISION_GATE,
        "precision {precision:.2} < {PRECISION_GATE}:\n{}", failures.join("\n"));
}
