//! Detection-efficacy benchmark for the built-in `default` pack.
//!
//! Reproduce:  `cargo run --example benchmark`
//!
//! Loads every labeled fixture from `tests/golden/` and `benchmark/corpus/`,
//! runs the default detector chain (regex + Luhn checksum + secrets/entropy;
//! NER disabled so the result is deterministic and needs no ONNX model), and
//! reports precision / recall / false-negative rate per entity and overall.
//! A fixture's `expected` list is ground truth; any detected span not in it is
//! a false positive, and any expected span not detected is a false negative.

use deectx::packs::{build_chain, Pack};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(serde::Deserialize)]
struct Fixture {
    name: String,
    text: String,
    #[serde(default)]
    expected: Vec<Expected>,
}

#[derive(serde::Deserialize)]
struct Expected {
    entity: String,
    text: String,
}

#[derive(Default)]
struct Counts {
    tp: usize,
    fp: usize,
    fn_: usize,
}

fn load_dir(dir: &Path, out: &mut Vec<Fixture>) {
    if !dir.exists() {
        return;
    }
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().map(|e| e == "yaml").unwrap_or(false) {
            let text = std::fs::read_to_string(&path).unwrap();
            match serde_yaml::from_str::<Fixture>(&text) {
                Ok(fx) => out.push(fx),
                Err(e) => eprintln!("skip {}: {e}", path.display()),
            }
        }
    }
}

fn precision(c: &Counts) -> f64 {
    if c.tp + c.fp == 0 {
        f64::NAN
    } else {
        c.tp as f64 / (c.tp + c.fp) as f64
    }
}

fn recall(c: &Counts) -> f64 {
    if c.tp + c.fn_ == 0 {
        f64::NAN
    } else {
        c.tp as f64 / (c.tp + c.fn_) as f64
    }
}

fn fn_rate(c: &Counts) -> f64 {
    if c.tp + c.fn_ == 0 {
        f64::NAN
    } else {
        c.fn_ as f64 / (c.tp + c.fn_) as f64
    }
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut fixtures = Vec::new();
    load_dir(&root.join("tests/golden"), &mut fixtures);
    load_dir(&root.join("benchmark/corpus"), &mut fixtures);
    fixtures.sort_by(|a, b| a.name.cmp(&b.name));

    let chain = build_chain(&[Pack::builtin_default()], false, PathBuf::from("./models"));

    let mut per: BTreeMap<String, Counts> = BTreeMap::new();
    let mut overall = Counts::default();
    let mut misses: Vec<String> = Vec::new();
    let mut positives = 0usize;
    let mut negatives = 0usize;

    for fx in &fixtures {
        if fx.expected.is_empty() {
            negatives += 1;
        } else {
            positives += 1;
        }
        let spans = chain.detect(&fx.text);
        let mut remaining: BTreeMap<(String, String), usize> = BTreeMap::new();
        for e in &fx.expected {
            *remaining
                .entry((e.entity.clone(), e.text.trim().to_string()))
                .or_insert(0) += 1;
        }
        for s in &spans {
            let key = (s.entity.clone(), s.text.trim().to_string());
            if let Some(n) = remaining.get_mut(&key) {
                if *n > 0 {
                    *n -= 1;
                    per.entry(s.entity.clone()).or_default().tp += 1;
                    overall.tp += 1;
                    continue;
                }
            }
            per.entry(s.entity.clone()).or_default().fp += 1;
            overall.fp += 1;
            misses.push(format!(
                "  FP  {:<12} {:?}  in `{}`",
                s.entity,
                s.text.trim(),
                fx.name
            ));
        }
        for ((ent, txt), n) in &remaining {
            for _ in 0..*n {
                per.entry(ent.clone()).or_default().fn_ += 1;
                overall.fn_ += 1;
                misses.push(format!("  FN  {ent:<12} {txt:?}  in `{}`", fx.name));
            }
        }
    }

    println!("# deeCtx detection benchmark — `default` pack (NER disabled)");
    println!();
    println!(
        "Fixtures: {} ({} positive, {} negative)",
        fixtures.len(),
        positives,
        negatives
    );
    println!();
    println!("| Entity | TP | FP | FN | Precision | Recall | FN-rate |");
    println!("|--------|---:|---:|---:|----------:|-------:|--------:|");
    for (ent, c) in &per {
        println!(
            "| {ent} | {} | {} | {} | {:.3} | {:.3} | {:.3} |",
            c.tp,
            c.fp,
            c.fn_,
            precision(c),
            recall(c),
            fn_rate(c)
        );
    }
    println!(
        "| **overall** | {} | {} | {} | {:.3} | {:.3} | {:.3} |",
        overall.tp,
        overall.fp,
        overall.fn_,
        precision(&overall),
        recall(&overall),
        fn_rate(&overall)
    );
    println!();
    if misses.is_empty() {
        println!("_No false positives or false negatives on this corpus._");
    } else {
        println!("## Misclassifications");
        println!();
        for m in &misses {
            println!("{m}");
        }
    }
}
