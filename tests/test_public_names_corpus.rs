//! The public-name extraction (#1086) measured on a corpus of files with known public
//! names: precision and recall per language, both 1.0. A pack that reports a private name
//! (precision) or misses a public one (recall) fails here with the names it got wrong.

use discipline::ast::default_registry;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Deserialize)]
struct Case {
    language: String,
    path: String,
    source: String,
    public: Vec<String>,
}

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Default)]
struct Tally {
    true_pos: usize,
    false_pos: usize,
    false_neg: usize,
    errors: Vec<String>,
}

#[test]
fn public_names_have_full_precision_and_recall_on_the_corpus() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/public_names_corpus.json");
    let corpus: Corpus = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let registry = default_registry();
    let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
    for case in &corpus.cases {
        let pack = registry
            .find_pack(&case.path)
            .unwrap_or_else(|| panic!("{}: no pack", case.path));
        assert_eq!(pack.id(), case.language, "{}", case.path);
        assert!(
            pack.supplies(discipline::ast::Fact::PublicNames),
            "{}: the {} pack does not supply public names",
            case.path,
            pack.id()
        );
        let got: BTreeSet<String> = pack
            .public_names(&case.path, &case.source)
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.path))
            .into_iter()
            .map(|n| n.name)
            .collect();
        let want: BTreeSet<String> = case.public.iter().cloned().collect();
        let t = tallies.entry(case.language.clone()).or_default();
        t.true_pos += got.intersection(&want).count();
        for extra in got.difference(&want) {
            t.false_pos += 1;
            t.errors
                .push(format!("{}: `{extra}` is not public", case.path));
        }
        for missed in want.difference(&got) {
            t.false_neg += 1;
            t.errors
                .push(format!("{}: `{missed}` was missed", case.path));
        }
    }
    let mut failures = Vec::new();
    println!("language     precision  recall  names");
    for (lang, t) in &tallies {
        let ratio = |num: usize, den: usize| {
            if den == 0 {
                1.0
            } else {
                num as f64 / den as f64
            }
        };
        let precision = ratio(t.true_pos, t.true_pos + t.false_pos);
        let recall = ratio(t.true_pos, t.true_pos + t.false_neg);
        println!(
            "{lang:<12} {precision:>9.3} {recall:>7.3} {:>6}",
            t.true_pos + t.false_neg
        );
        failures.extend(t.errors.iter().cloned());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
