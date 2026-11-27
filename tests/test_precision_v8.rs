//! `scripts/precision_v8.py` builds and scores `agent-pr-precision-v8`
//! (`docs/benchmarks/precision-preregistration-v8.toml`). Its self-checks run here, so a
//! change that breaks how packet items keep their v6 ids fails `cargo test`.

mod common;

use discipline::guards::perf::bounds::wilson_score_interval;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

#[test]
fn the_v8_script_self_checks_pass() {
    let out = common::script_command("python3")
        .args(["-I", "scripts/precision_v8.py", "--test"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

const V8: &str = "docs/benchmarks/results-v8";
const R3: &str = "r3-gemini-4-neon";

fn v8_json(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(V8).join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("v8 file exists"))
        .expect("v8 file parses")
}

/// A rater slot: `{item_id: {question: label}}`, from the `batch-NN.json` files of a directory;
/// `nested` slots keep their labels under `labels`.
fn slot(dir: &str, nested: bool) -> BTreeMap<String, Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(V8).join(dir);
    let mut files: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            // `batch-NN.json` only, as `load_slot`'s `batch-??.json`: a re-asked batch's
            // invalid first answer (`batch-06-invalid-1.json`) is kept but is not a label.
            let name = p.file_name().unwrap().to_str().unwrap();
            name.len() == 13
                && name.starts_with("batch-")
                && name.ends_with(".json")
                && name[6..8].bytes().all(|b| b.is_ascii_digit())
        })
        .collect();
    files.sort();
    let mut out = BTreeMap::new();
    for f in files {
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        let labels = if nested { &doc["labels"] } else { &doc };
        for (k, v) in labels.as_object().unwrap() {
            assert!(
                out.insert(k.clone(), v.clone()).is_none(),
                "{k} twice in {dir}"
            );
        }
    }
    out
}

/// Unweighted Cohen's kappa (Cohen 1960), as `scripts/benchmark_precision.py::cohens_kappa`.
fn kappa(a: &[&str], b: &[&str]) -> f64 {
    let n = a.len() as f64;
    let agreed = a.iter().zip(b).filter(|(x, y)| x == y).count() as f64;
    let mut cats: Vec<&str> = a.iter().chain(b).copied().collect();
    cats.sort_unstable();
    cats.dedup();
    let pe: f64 = cats
        .iter()
        .map(|c| {
            (a.iter().filter(|x| *x == c).count() as f64 / n)
                * (b.iter().filter(|x| *x == c).count() as f64 / n)
        })
        .sum();
    if (1.0 - pe).abs() < 1e-12 {
        return if agreed == n { 1.0 } else { 0.0 };
    }
    (agreed / n - pe) / (1.0 - pe)
}

fn close(got: &Value, want: f64, what: &str) {
    let g = got
        .as_f64()
        .unwrap_or_else(|| panic!("{what} is not a number"));
    assert!((g - want).abs() < 1e-9, "{what}: {g} != {want}");
}

fn n(v: &Value) -> usize {
    usize::try_from(v.as_u64().expect("count")).unwrap()
}

/// #1076, exploratory: the third AI rater's agreement file is the one its batches give. The
/// slot labels every packet item once, in the r2 slot's batches, with one valid pair of labels
/// per item; the disclosure names it an AI model; and every count, kappa, per-class table and
/// Wilson interval in `r3-gemini-4-neon-agreement.json` is recomputed from the batches, the two
/// v8 slots, `adjudication.json` and `findings.json`.
#[test]
fn the_third_rater_agreement_is_recomputed_from_its_batches() {
    let r = v8_json("r3-gemini-4-neon-agreement.json");
    assert!(r["label"].as_str().unwrap().starts_with("exploratory"));
    assert!(r["label"].as_str().unwrap().contains("not human"));
    assert!(r["synthetic_rater_disclosure"]
        .as_str()
        .unwrap()
        .contains("No human labelled any item"));
    let r3 = slot(R3, true);
    let r2 = slot("r2", true);
    let r1 = slot("r1", false);
    let packet = v8_json("rater-packet.json");
    let mut ids: Vec<String> = packet["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["item_id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    assert_eq!(
        r3.keys().cloned().collect::<Vec<_>>(),
        ids,
        "every packet item, once"
    );
    let qs: [(&str, [&str; 3]); 2] = [
        (
            "rule_fidelity",
            ["true-positive", "false-positive", "cannot-tell"],
        ),
        (
            "actionability",
            ["would-block", "would-not-block", "cannot-tell"],
        ),
    ];
    for (id, v) in &r3 {
        assert_eq!(v.as_object().unwrap().len(), 2, "{id}");
        for (q, labels) in &qs {
            assert!(labels.contains(&v[*q].as_str().unwrap()), "{id} {q}");
        }
    }
    assert_eq!(n(&r["items"]), ids.len());
    assert_eq!(n(&r["batches"]), 14);

    let final_labels: BTreeMap<String, Value> = v8_json("adjudication.json")["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["item_id"].as_str().unwrap().to_string(),
                i["final"].clone(),
            )
        })
        .collect();
    for (q, labels) in &qs {
        for l in labels {
            let k = r3.values().filter(|v| v[*q] == *l).count();
            assert_eq!(n(&r["label_counts"][*q][*l]), k, "{q} {l}");
        }
    }
    for (name, other) in [("r1", &r1), ("r2", &r2), ("final", &final_labels)] {
        for (q, _) in &qs {
            let a: Vec<&str> = ids.iter().map(|i| r3[i][*q].as_str().unwrap()).collect();
            let b: Vec<&str> = ids.iter().map(|i| other[i][*q].as_str().unwrap()).collect();
            let cell = &r["agreement"][name][*q];
            assert_eq!(n(&cell["n"]), ids.len());
            let agree = a.iter().zip(&b).filter(|(x, y)| x == y).count();
            assert_eq!(n(&cell["agree"]), agree, "{name} {q} agree");
            close(&cell["kappa"], kappa(&a, &b), &format!("{name} {q} kappa"));
        }
    }

    let kind: BTreeMap<String, String> = v8_json("findings.json")["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            let k = if i["source_tool"] == "discipline" {
                i["gate_class"].as_str().unwrap()
            } else {
                "twin"
            };
            (i["item_id"].as_str().unwrap().to_string(), k.to_string())
        })
        .collect();
    let table = r["by_class"].as_object().unwrap();
    let mut classes: Vec<&String> = kind.values().collect();
    classes.sort();
    classes.dedup();
    assert_eq!(table.len(), classes.len());
    for c in classes {
        let rows: Vec<&String> = ids.iter().filter(|i| kind[*i] == *c).collect();
        let cell = &table[c.as_str()];
        assert_eq!(n(&cell["items"]), rows.len(), "{c}");
        let mut cross: BTreeMap<String, usize> = BTreeMap::new();
        for i in &rows {
            let key = format!(
                "{} -> {}",
                final_labels[*i]["rule_fidelity"].as_str().unwrap(),
                r3[*i]["rule_fidelity"].as_str().unwrap()
            );
            *cross.entry(key).or_default() += 1;
        }
        let got: BTreeMap<String, usize> = cell["final_vs_slot"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), n(v)))
            .collect();
        assert_eq!(got, cross, "{c} final vs slot");
        for (who, labels) in [("final", &final_labels), ("slot", &r3)] {
            let k = rows
                .iter()
                .filter(|i| labels[**i]["rule_fidelity"] == "true-positive")
                .count();
            let s = &cell["rule_fidelity_true_positive"][who];
            assert_eq!([n(&s["k"]), n(&s["n"])], [k, rows.len()], "{c} {who}");
            let (lo, hi) = wilson_score_interval(k, rows.len(), 0.95).unwrap();
            close(&s["wilson_95"][0], lo, &format!("{c} {who} lower"));
            close(&s["wilson_95"][1], hi, &format!("{c} {who} upper"));
        }
    }
}
