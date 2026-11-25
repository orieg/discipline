//! Step-0 bounds of the base-tests study (`docs/benchmarks/base-tests-preregistration.toml`,
//! #1052): the Wilson ceiling functions in `src/guards/perf/bounds.rs` return their pinned
//! values, every sample size the pre-registration states is what they return, the Python mirror
//! in `scripts/benchmark_power_audit.py` agrees, and every population and compute count is the
//! one the committed #838 files give.

mod common;

use discipline::guards::perf::bounds::{
    sample_size_for_wilson_lower_bound_power, sample_size_for_wilson_upper_bound_power,
    wilson_pass_probability, wilson_score_interval, wilson_upper_pass_probability,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

const PREREG: &str = "docs/benchmarks/base-tests-preregistration.toml";
const STRATA: &str = "docs/benchmarks/results-recall-confirmatory/strata.json";
const PILOT: &str = "docs/benchmarks/results-base-tests/pilot";

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn prereg() -> toml::Value {
    let raw = std::fs::read_to_string(root().join(PREREG)).expect("pre-registration exists");
    toml::from_str(&raw).expect("pre-registration parses")
}

fn strata() -> Value {
    let raw = std::fs::read_to_string(root().join(STRATA)).expect("strata.json exists");
    serde_json::from_str(&raw).expect("strata.json parses")
}

fn int(v: &toml::Value) -> usize {
    v.as_integer().expect("an integer") as usize
}

fn float(v: &toml::Value) -> f64 {
    v.as_float().expect("a float")
}

/// The repository owner of a SWE-bench instance id (`django__django-11099` -> `django`).
fn owner(instance_id: &str) -> &str {
    instance_id.split("__").next().expect("an owner prefix")
}

fn ids(path: &str) -> Vec<String> {
    std::fs::read_to_string(root().join(path))
        .expect("id file exists")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn counts_table(t: &toml::Value) -> BTreeMap<String, usize> {
    t.as_table()
        .expect("a table")
        .iter()
        .map(|(k, v)| (k.clone(), int(v)))
        .collect()
}

#[test]
fn wilson_upper_pass_probability_matches_the_exact_binomial_tail() {
    // Cross-checked with exact rational arithmetic (`fractions.Fraction`, `math.comb`).
    for (n, p, ceiling, expected) in [
        (83, 0.05, 0.15, 0.878_481_673_269_529_2),
        (82, 0.05, 0.15, 0.773_089_972_210_432_3),
        (272, 0.10, 0.15, 0.686_459_287_674_456_3),
        (272, 0.05, 0.10, 0.860_161_816_618_945_6),
    ] {
        let got = wilson_upper_pass_probability(n, p, ceiling, 0.95).unwrap();
        assert!(
            (got - expected).abs() < 1e-10,
            "n={n}, p={p}, ceiling={ceiling}: {got} != {expected}"
        );
    }
    // Even k = 0 has a Wilson upper bound above 0.10 at n = 5.
    assert_eq!(
        wilson_upper_pass_probability(5, 0.01, 0.10, 0.95).unwrap(),
        0.0
    );
}

/// The ceiling tail is the lower-bound tail reflected: the score interval of `(n - k) / n` is
/// `(1 - upper, 1 - lower)`.
#[test]
fn the_ceiling_bound_is_the_floor_bound_reflected() {
    for n in [1usize, 10, 37, 83, 112, 272, 1000] {
        for (p, c) in [(0.02, 0.15), (0.05, 0.15), (0.10, 0.20), (0.30, 0.45)] {
            let up = wilson_upper_pass_probability(n, p, c, 0.95).unwrap();
            let low = wilson_pass_probability(n, 1.0 - p, 1.0 - c, 0.95).unwrap();
            assert!(
                (up - low).abs() < 1e-9,
                "n={n}, p={p}, c={c}: {up} vs {low}"
            );
        }
    }
    assert_eq!(
        sample_size_for_wilson_upper_bound_power(0.10, 0.20, 0.95, 0.80, 2000).unwrap(),
        sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 2000).unwrap()
    );
}

/// The monotonicity choice: 74-78 meet the power, 79-82 fall below it again, so the
/// requirement is 83, the start of the last unbroken run.
#[test]
fn the_ceiling_requirement_is_the_last_unbroken_run() {
    let meets = |n: usize| wilson_upper_pass_probability(n, 0.05, 0.15, 0.95).unwrap() >= 0.80;
    assert!((74..=78).all(meets));
    assert!(!(79..=82).any(meets));
    assert_eq!(
        sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 2000).unwrap(),
        83
    );
    assert_eq!(
        sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 500).unwrap(),
        83
    );
}

#[test]
fn ceiling_bounds_reject_invalid_inputs() {
    assert!(wilson_upper_pass_probability(0, 0.05, 0.15, 0.95).is_err());
    assert!(wilson_upper_pass_probability(10, 0.0, 0.15, 0.95).is_err());
    assert!(wilson_upper_pass_probability(10, 0.05, 1.0, 0.95).is_err());
    assert!(wilson_upper_pass_probability(10, 0.05, 0.15, f64::NAN).is_err());
    // The ceiling must sit above the planning rate, and n_max must reach the power.
    // A ceiling at the planning rate is refused by its own guard, not by the power search.
    let err = sample_size_for_wilson_upper_bound_power(0.15, 0.15, 0.95, 0.80, 2000)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("must be above the expected proportion"),
        "{err}"
    );
    assert!(sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 50).is_err());
    assert!(sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 0.80, 0).is_err());
    assert!(sample_size_for_wilson_upper_bound_power(0.05, 0.15, 0.95, 1.0, 2000).is_err());
}

#[test]
fn the_study_is_a_draft_until_its_pilot_values_are_written() {
    let p = prereg();
    let s = &p["study"];
    assert_eq!(s["id"].as_str(), Some("base-tests-v1"));
    assert_eq!(s["issue"].as_integer(), Some(1052));
    let pilot = &p["pilot"];
    match s["status"].as_str().unwrap() {
        "draft" => {
            assert_eq!(pilot["observed_friction"].as_str(), Some("PILOT"));
            assert_eq!(pilot["observed_eligible_share"].as_str(), Some("PILOT"));
            assert_eq!(p["binary"]["sha256"].as_str(), Some("LOCK"));
        }
        "locked-pre-data" => {
            assert!(pilot["observed_friction"].as_float().is_some());
            assert!(pilot["observed_eligible_share"].as_float().is_some());
            for (k, v) in p["constants"].as_table().unwrap() {
                assert_ne!(v.as_str(), Some("LOCK"), "constant {k} unset at the lock");
            }
        }
        other => panic!("unexpected status {other}"),
    }
}

#[test]
fn the_population_counts_are_the_committed_id_files() {
    let p = prereg();
    let pop = &p["population"];
    let confirmatory = ids(pop["confirmatory_ids"].as_str().unwrap());
    let pilot = ids(pop["pilot_ids"].as_str().unwrap());
    assert_eq!(int(&pop["confirmatory_instances"]), confirmatory.len());
    assert_eq!(int(&pop["pilot_instances"]), pilot.len());
    let mut per_repo = BTreeMap::new();
    for id in &confirmatory {
        *per_repo.entry(owner(id).to_string()).or_insert(0usize) += 1;
    }
    assert_eq!(counts_table(&pop["confirmatory_per_repository"]), per_repo);
    assert_eq!(
        int(&p["power_and_bounds"]["q2_available"]),
        confirmatory.len()
    );
}

#[test]
fn the_q1_counts_are_the_committed_counterfactual_strata() {
    let p = prereg();
    let q1 = &p["q1_feasibility"];
    let s = strata();
    let mut per_repo = BTreeMap::new();
    for pair in s["counterfactual"]["pairs"].as_array().unwrap() {
        if matches!(pair["stratum"].as_str(), Some("mixed" | "test-side")) {
            let id = pair["instance_id"].as_str().unwrap();
            *per_repo.entry(owner(id).to_string()).or_insert(0usize) += 1;
        }
    }
    assert_eq!(counts_table(&q1["eligible_pairs_per_repository"]), per_repo);
    assert_eq!(int(&q1["eligible_pairs"]), per_repo.values().sum::<usize>());
    assert_eq!(
        int(&q1["sampled_pairs"]),
        per_repo.values().map(|&c| c.min(2)).sum::<usize>()
    );
    let salt = q1["q1_salt"].as_str().unwrap();
    assert_eq!(salt.len(), 32);
    assert!(salt.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn the_q2_sample_sizes_match_rust_bounds() {
    let pb = &prereg()["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["upper_bound_n_max"]);
    let available = int(&pb["q2_available"]);
    let (rate, ceiling) = (float(&pb["q2_planning_rate"]), float(&pb["q2_ceiling"]));
    let required =
        sample_size_for_wilson_upper_bound_power(rate, ceiling, conf, power, n_max).unwrap();
    assert_eq!(int(&pb["q2_required_gold_prs"]), required);
    let at_available = wilson_upper_pass_probability(available, rate, ceiling, conf).unwrap();
    assert!(
        (float(&pb["q2_power_at_available"]) - at_available).abs() < 5e-6,
        "{at_available}"
    );
    assert!(
        (float(&pb["q2_min_eligible_share"]) - required as f64 / available as f64).abs() < 5e-4
    );
    let verdict = |n: usize| {
        if n <= available {
            "PROCEED"
        } else {
            "REJECT_UNDETECTABLE"
        }
    };
    assert_eq!(pb["q2_step0_verdict"].as_str(), Some(verdict(required)));
    let rows = pb["q2_sensitivity"].as_array().unwrap();
    assert!(rows
        .iter()
        .any(|r| { float(&r["planning_rate"]) == rate && float(&r["ceiling"]) == ceiling }));
    for row in rows {
        let (r, c) = (float(&row["planning_rate"]), float(&row["ceiling"]));
        let n = sample_size_for_wilson_upper_bound_power(r, c, conf, power, n_max).unwrap();
        assert_eq!(int(&row["required_gold_prs"]), n, "({r}, {c})");
        assert_eq!(row["verdict"].as_str(), Some(verdict(n)), "({r}, {c})");
    }
}

#[test]
fn the_compute_figures_are_the_counterfactual_runner_records() {
    let c = &prereg()["compute"];
    let s = strata();
    let mut seconds: Vec<u64> = Vec::new();
    let mut timeouts = 0usize;
    for pair in s["counterfactual"]["pairs"].as_array().unwrap() {
        for run in pair["runs"].as_array().unwrap() {
            seconds.push(run["seconds"].as_u64().unwrap());
            timeouts += usize::from(run["timed_out"].as_bool().unwrap());
        }
    }
    seconds.sort_unstable();
    let n = seconds.len();
    let median = if n % 2 == 1 {
        seconds[n / 2] as f64
    } else {
        (seconds[n / 2 - 1] + seconds[n / 2]) as f64 / 2.0
    };
    assert_eq!(int(&c["counterfactual_runs"]), n);
    assert_eq!(
        c["counterfactual_median_seconds"].as_integer(),
        Some(median as i64)
    );
    assert_eq!(median.fract(), 0.0);
    assert_eq!(
        int(&c["counterfactual_max_seconds"]) as u64,
        *seconds.last().unwrap()
    );
    assert_eq!(int(&c["counterfactual_timeouts"]), timeouts);
}

/// `--base-tests-bounds-json` prints the values the pre-registration records, and the Python
/// ceiling probability agrees with the Rust one.
#[cfg(unix)]
#[test]
fn python_base_tests_bounds_json_matches_the_preregistration() {
    let out = common::script_command("python3")
        .args([
            "-I",
            "scripts/benchmark_power_audit.py",
            "--base-tests-bounds-json",
        ])
        .current_dir(root())
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let pb = &prereg()["power_and_bounds"];
    let q2 = &json["q2_primary"];
    assert_eq!(
        q2["planning_rate"].as_f64(),
        pb["q2_planning_rate"].as_float()
    );
    assert_eq!(q2["ceiling"].as_f64(), pb["q2_ceiling"].as_float());
    assert_eq!(
        q2["available"].as_u64(),
        Some(int(&pb["q2_available"]) as u64)
    );
    assert_eq!(
        q2["n_max"].as_u64(),
        Some(int(&pb["upper_bound_n_max"]) as u64)
    );
    assert_eq!(
        q2["required_gold_prs"].as_u64(),
        Some(int(&pb["q2_required_gold_prs"]) as u64)
    );
    assert_eq!(q2["verdict"].as_str(), pb["q2_step0_verdict"].as_str());
    let rust = wilson_upper_pass_probability(272, 0.05, 0.15, 0.95).unwrap();
    assert!((q2["power_at_available"].as_f64().unwrap() - rust).abs() < 1e-9);
    let grid = json["q2_grid"].as_array().unwrap();
    let rows = pb["q2_sensitivity"].as_array().unwrap();
    assert_eq!(grid.len(), rows.len());
    for (g, r) in grid.iter().zip(rows) {
        assert_eq!(g["planning_rate"].as_f64(), r["planning_rate"].as_float());
        assert_eq!(g["ceiling"].as_f64(), r["ceiling"].as_float());
        assert_eq!(
            g["required_gold_prs"].as_u64(),
            Some(int(&r["required_gold_prs"]) as u64)
        );
        let (p, c) = (float(&r["planning_rate"]), float(&r["ceiling"]));
        let rust = wilson_upper_pass_probability(272, p, c, 0.95).unwrap();
        assert!(
            (g["power_at_available"].as_f64().unwrap() - rust).abs() < 1e-9,
            "({p}, {c})"
        );
    }
}

fn pilot_json(name: &str) -> Value {
    let raw = std::fs::read_to_string(root().join(PILOT).join(name)).expect("pilot file exists");
    serde_json::from_str(&raw).expect("pilot file parses")
}

fn close(got: &Value, want: f64, what: &str) {
    let got = got.as_f64().unwrap_or_else(|| panic!("{what} is a number"));
    assert!((got - want).abs() < 1e-9, "{what}: {got} != {want}");
}

/// `[pilot] writes_at_lock`: the eligible share and the friction of the pilot's Part 2 runs,
/// counted here from `runs.json`, with their Wilson intervals from `bounds.rs`, and the Part 2
/// requirement re-derived at the Wilson upper bound of the friction, with its Step-0 verdict
/// against the eligible gold pull requests the observed share leaves of the 272.
#[test]
fn the_pilot_values_are_recomputed_from_the_pilot_runs() {
    let p = prereg();
    let pb = &p["power_and_bounds"];
    let (conf, power) = (
        float(&pb["confidence_level"]),
        float(&pb["statistical_power"]),
    );
    let (n_max, available) = (int(&pb["upper_bound_n_max"]), int(&pb["q2_available"]));
    let ceiling = float(&pb["q2_ceiling"]);
    let runs = pilot_json("runs.json");
    let rows: Vec<&Value> = runs["part2"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["population"] == "pilot")
        .collect();
    let n = rows.len();
    assert_eq!(n, int(&p["population"]["pilot_instances"]));
    let flag = |r: &&Value, key: &str| r["outcome"][key].as_bool() == Some(true);
    let eligible = rows.iter().filter(|r| flag(r, "eligible")).count();
    let blocked = rows.iter().filter(|r| flag(r, "blocked")).count();
    assert!(rows
        .iter()
        .filter(|r| flag(r, "blocked"))
        .all(|r| flag(r, "eligible")));

    let v = pilot_json("pilot-values.json");
    let share = &v["observed_eligible_share"];
    assert_eq!(share["k"].as_u64(), Some(eligible as u64));
    assert_eq!(share["n"].as_u64(), Some(n as u64));
    let (lo, hi) = wilson_score_interval(eligible, n, conf).unwrap();
    close(&share["wilson_95"][0], lo, "eligible share lower bound");
    close(&share["wilson_95"][1], hi, "eligible share upper bound");
    let friction = &v["observed_friction"];
    assert_eq!(friction["k"].as_u64(), Some(blocked as u64));
    assert_eq!(friction["n"].as_u64(), Some(eligible as u64));
    let (flo, fhi) = wilson_score_interval(blocked, eligible, conf).unwrap();
    close(&friction["wilson_95"][0], flo, "friction lower bound");
    close(&friction["wilson_95"][1], fhi, "friction upper bound");
    close(
        &v["q2_planning_rate_at_lock"],
        fhi,
        "planning rate at the lock",
    );

    let expected_eligible = eligible * available / n;
    assert_eq!(
        int_json(&v["q2_expected_eligible_at_observed_share"]),
        expected_eligible
    );
    let required = if fhi < ceiling {
        sample_size_for_wilson_upper_bound_power(fhi, ceiling, conf, power, n_max).ok()
    } else {
        None
    };
    assert_eq!(
        v["q2_required_gold_prs"].as_u64().map(|x| x as usize),
        required
    );
    let verdict = match required {
        Some(r) if r <= expected_eligible => "PROCEED",
        _ => "REJECT_UNDETECTABLE",
    };
    assert_eq!(v["q2_step0_verdict"].as_str(), Some(verdict));
}

fn int_json(v: &Value) -> usize {
    v.as_u64().expect("an integer") as usize
}

/// The candidate constants the pilot ran with leave no `LOCK` and configure every repository
/// owner of the confirmatory population, so `--constants` can overlay all of them.
#[test]
fn the_pilot_constants_cover_every_confirmatory_repository() {
    let p = prereg();
    let c = pilot_json("constants.json");
    let keys: Vec<&String> = p["constants"].as_table().unwrap().keys().collect();
    let mut got: Vec<&String> = c.as_object().unwrap().keys().collect();
    got.sort();
    let mut want = keys.clone();
    want.sort();
    assert_eq!(
        got, want,
        "constants.json holds exactly the [constants] keys"
    );
    for (k, val) in c.as_object().unwrap() {
        assert_ne!(val.as_str(), Some("LOCK"), "{k}");
    }
    let owners = counts_table(&p["population"]["confirmatory_per_repository"]);
    for key in [
        "runner_command_per_repository",
        "junit_options_per_repository",
        "tests_paths_globs_per_repository",
    ] {
        let table = c[key].as_object().unwrap();
        for owner in owners.keys() {
            assert!(table.contains_key(owner), "{key} has no entry for {owner}");
        }
    }
}

/// The lock commit writes the pilot's values unchanged: every `[constants]` value is the one in
/// `constants.json` the pilot ran with, `[binary]` is the binary `provenance.json` records, the
/// `[pilot]` figures are `pilot-values.json`'s, every confirmatory instance has a pinned image,
/// and Part 2 is labelled estimate-only exactly when the Step-0 verdict at the pilot rate is
/// `REJECT_UNDETECTABLE` (`[lock]`), with the 0.15 ceiling left where it was.
#[test]
fn the_lock_writes_the_pilot_values_and_the_estimate_only_label() {
    let p = prereg();
    assert_eq!(p["study"]["status"].as_str(), Some("locked-pre-data"));
    let constants = serde_json::to_value(&p["constants"]).expect("constants convert");
    assert_eq!(constants, pilot_json("constants.json"));

    let prov = &pilot_json("provenance.json")["binary"];
    for key in ["release", "sha256", "tarball_sha256"] {
        assert_eq!(
            p["binary"][key].as_str(),
            prov[key].as_str(),
            "binary.{key}"
        );
    }

    let v = pilot_json("pilot-values.json");
    let pilot = &p["pilot"];
    for key in ["observed_eligible_share", "observed_friction"] {
        close(&v[key]["point"], float(&pilot[key]), key);
        let ci = pilot[format!("{key}_wilson_95").as_str()]
            .as_array()
            .expect("an interval");
        close(&v[key]["wilson_95"][0], float(&ci[0]), key);
        close(&v[key]["wilson_95"][1], float(&ci[1]), key);
    }
    close(
        &v["q2_planning_rate_at_lock"],
        float(&pilot["q2_planning_rate_at_lock"]),
        "planning rate at the lock",
    );
    let verdict = pilot["q2_step0_verdict_at_lock"].as_str();
    assert_eq!(verdict, v["q2_step0_verdict"].as_str());
    let label = p["q2_friction"].get("label").and_then(toml::Value::as_str);
    match verdict {
        Some("REJECT_UNDETECTABLE") => assert_eq!(label, Some("estimate-only")),
        Some("PROCEED") => assert_eq!(label, None),
        other => panic!("unexpected Step-0 verdict {other:?}"),
    }
    assert!((float(&p["power_and_bounds"]["q2_ceiling"]) - 0.15).abs() < 1e-12);

    let digests = p["constants"]["image_digests"].as_table().expect("a table");
    let strata = strata();
    let pinned: std::collections::BTreeSet<&str> = strata["counterfactual"]["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["image_digest"].as_str().is_some_and(|d| !d.is_empty()))
        .filter_map(|r| r["instance_id"].as_str())
        .collect();
    let ids =
        std::fs::read_to_string(root().join(p["population"]["confirmatory_ids"].as_str().unwrap()))
            .expect("confirmatory ids exist");
    for id in ids.lines().map(str::trim).filter(|l| !l.is_empty()) {
        assert!(
            pinned.contains(id) || digests.contains_key(id),
            "{id} has no pinned image"
        );
    }
}

const CONFIRMATORY: &str = "docs/benchmarks/results-base-tests";

fn confirmatory_json(name: &str) -> Value {
    let path = root().join(CONFIRMATORY).join(name);
    let file = std::fs::File::open(&path).expect("confirmatory file exists");
    if name.ends_with(".gz") {
        serde_json::from_reader(flate2::read::GzDecoder::new(file))
            .expect("confirmatory file parses")
    } else {
        serde_json::from_reader(file).expect("confirmatory file parses")
    }
}

/// The confirmatory run (`[verdict] q2_rule`, estimate-only): every result is a confirmatory run
/// of the locked binary on the image the lock pins; the Part 2 instances are the confirmatory id
/// file and the Part 1 pairs the committed sample's; and `summary.json`'s Part 2 counts, Wilson
/// intervals, per-repository table and verdict are the ones `runs.json.gz` gives through
/// `bounds.rs`, with every block listed in `blocks.json`.
#[test]
fn the_confirmatory_summary_is_recomputed_from_the_confirmatory_runs() {
    let p = prereg();
    let conf = float(&p["power_and_bounds"]["confidence_level"]);
    let runs = confirmatory_json("runs.json.gz");
    let part2 = runs["part2"].as_array().expect("part2 rows");
    let part1 = runs["part1"].as_array().expect("part1 rows");
    let sha = p["binary"]["sha256"].as_str().unwrap();
    for r in part2.iter().chain(part1) {
        assert_eq!(r["mode"].as_str(), Some("confirmatory"));
        assert_eq!(
            r["binary_sha256"].as_str(),
            Some(sha),
            "{}",
            r["instance_id"]
        );
    }

    let mut got: Vec<&str> = part2
        .iter()
        .map(|r| r["instance_id"].as_str().unwrap())
        .collect();
    got.sort_unstable();
    let mut want = ids(p["population"]["confirmatory_ids"].as_str().unwrap());
    want.sort_unstable();
    assert_eq!(got, want, "Part 2 ran exactly the confirmatory instances");
    assert!(part2.iter().all(|r| r["population"] == "confirmatory"));
    assert_eq!(
        part1.len(),
        int(&p["q1_feasibility"]["sampled_pairs"]),
        "Part 1 ran every sampled pair"
    );

    // The image each instance ran on is the one the lock pins (`image_for` in the runner).
    let strata = strata();
    let digests = p["constants"]["image_digests"].as_table().unwrap();
    let prov = confirmatory_json("provenance.json");
    for r in part2 {
        let id = r["instance_id"].as_str().unwrap();
        let pinned = strata["counterfactual"]["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["instance_id"] == id && c["image_digest"].as_str().is_some_and(|d| !d.is_empty())
            })
            .and_then(|c| c["image_digest"].as_str())
            .or_else(|| digests.get(id).and_then(toml::Value::as_str))
            .unwrap_or_else(|| panic!("{id} has no pinned image"));
        assert_eq!(r["image"].as_str(), Some(pinned), "{id}");
        assert_eq!(
            prov["images"][id].as_str(),
            Some(pinned),
            "provenance image of {id}"
        );
    }
    assert_eq!(prov["binary"]["sha256"].as_str(), Some(sha));

    let flag = |r: &Value, key: &str| r["outcome"][key].as_bool() == Some(true);
    let eligible = part2.iter().filter(|r| flag(r, "eligible")).count();
    let blocked: Vec<&str> = part2
        .iter()
        .filter(|r| flag(r, "blocked"))
        .map(|r| r["instance_id"].as_str().unwrap())
        .collect();
    assert!(part2
        .iter()
        .filter(|r| flag(r, "blocked"))
        .all(|r| flag(r, "eligible")));

    let s = &confirmatory_json("summary.json")["part2"]["confirmatory"];
    assert_eq!(int_json(&s["instances"]), part2.len());
    assert_eq!(int_json(&s["eligible"]), eligible);
    assert_eq!(int_json(&s["friction"]["k"]), blocked.len());
    assert_eq!(int_json(&s["friction"]["n"]), eligible);
    let (lo, hi) = wilson_score_interval(eligible, part2.len(), conf).unwrap();
    close(
        &s["eligible_share"]["wilson_95"][0],
        lo,
        "eligible share lower bound",
    );
    close(
        &s["eligible_share"]["wilson_95"][1],
        hi,
        "eligible share upper bound",
    );
    let (flo, fhi) = wilson_score_interval(blocked.len(), eligible, conf).unwrap();
    close(&s["friction"]["wilson_95"][0], flo, "friction lower bound");
    close(&s["friction"]["wilson_95"][1], fhi, "friction upper bound");
    assert_eq!(s["verdict"].as_str(), Some("ESTIMATE_ONLY"));

    let mut per_repo: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    for r in part2 {
        let t = per_repo
            .entry(owner(r["instance_id"].as_str().unwrap()).to_string())
            .or_default();
        t[0] += 1;
        t[1] += usize::from(flag(r, "eligible"));
        t[2] += usize::from(flag(r, "blocked"));
    }
    let table = s["per_repository"].as_object().unwrap();
    assert_eq!(table.len(), per_repo.len());
    for (o, [n, e, b]) in &per_repo {
        let row = &table[o.as_str()];
        assert_eq!(
            [
                int_json(&row["instances"]),
                int_json(&row["eligible"]),
                int_json(&row["blocked"])
            ],
            [*n, *e, *b],
            "{o}"
        );
    }

    let blocks = confirmatory_json("blocks.json");
    let mut listed: Vec<&str> = blocks["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["instance_id"].as_str().unwrap())
        .collect();
    listed.sort_unstable();
    let mut blocked = blocked;
    blocked.sort_unstable();
    assert_eq!(
        listed, blocked,
        "blocks.json lists every blocked gold pull request"
    );

    // `[q2_friction] classification`: every block carries one pre-registered class and a
    // reason, the rater is disclosed, and summary.json's class counts are blocks.json's.
    let classes: Vec<String> = p["q2_friction"]["block_classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().split(':').next().unwrap().to_string())
        .collect();
    assert!(blocks["rater"].as_str().is_some_and(|r| r.contains("AI")));
    let mut by_class: BTreeMap<String, usize> = classes.iter().map(|c| (c.clone(), 0)).collect();
    for b in blocks["blocks"].as_array().unwrap() {
        let class = b["class"]
            .as_str()
            .unwrap_or_else(|| panic!("{} has no class", b["instance_id"]));
        assert!(
            classes.iter().any(|c| c == class),
            "{class} is not pre-registered"
        );
        assert!(
            b["reason"].as_str().is_some_and(|r| r.len() > 20),
            "{} has no reason",
            b["instance_id"]
        );
        *by_class.get_mut(class).unwrap() += 1;
    }
    for (class, n) in &by_class {
        assert_eq!(int_json(&s["block_classes"][class.as_str()]), *n, "{class}");
    }
    assert_eq!(int_json(&s["unclassified_blocks"]), 0);
}

/// One Part 3 run's outcome from its `discipline check` JSON report, as `part3_outcome` reads
/// it: `(ran, kind)`, where a kind is a block. A failed base test names its id; a
/// `base-test-failed` finding without one is the suite failing.
fn part3_kind(result: &Value) -> (bool, Option<&'static str>) {
    if result.get("error").is_some_and(|e| !e.is_null())
        || result["timed_out"].as_bool() == Some(true)
        || result["report"].is_null()
    {
        return (false, None);
    }
    let (mut failed, mut suite, mut none) = (false, false, false);
    for o in result["report"]["outcomes"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for v in o["violations"].as_array().into_iter().flatten() {
            match v["code"].as_str() {
                Some("command/base-test-failed") => {
                    if v["message"]
                        .as_str()
                        .is_some_and(|m| m.starts_with("Base test `"))
                    {
                        failed = true;
                    } else {
                        suite = true;
                    }
                }
                Some("command/base-tests-no-cases") => none = true,
                _ => {}
            }
        }
    }
    let kind = if failed {
        Some("failing-test")
    } else if suite {
        Some("suite-failed")
    } else if none {
        Some("no-cases")
    } else {
        None
    };
    (true, kind)
}

/// Part 3 (`[q3_added_value]`, `PASS_categorical_by_design`): the runs are exactly #838's 212
/// confirmatory gaming runs, each with the shape and diff-gate verdict `scripts/recall_shapes.py`
/// computes from the committed #838 files, run by the locked binary on the image its instance's
/// Part 2 run used; and `summary.json`'s per-shape and overall counts, Wilson intervals and block
/// kinds are the ones the runs' reports give through `bounds.rs`.
#[cfg(unix)]
#[test]
fn the_part3_summary_is_recomputed_from_the_part3_runs() {
    let p = prereg();
    let conf = float(&p["power_and_bounds"]["confidence_level"]);
    let sha = p["binary"]["sha256"].as_str().unwrap();
    let runs = confirmatory_json("runs.json.gz");
    let part3 = runs["part3"].as_array().expect("part3 rows");
    let prov = confirmatory_json("provenance.json");

    let out = common::script_command("python3")
        .args(["-I", "scripts/recall_shapes.py", "--runs"])
        .current_dir(root())
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "recall_shapes.py failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let shapes: Value = serde_json::from_slice(&out.stdout).expect("recall_shapes.py prints JSON");
    let classified: BTreeMap<&str, &Value> = shapes["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["pair_id"].as_str().unwrap(), r))
        .collect();
    assert_eq!(part3.len(), classified.len(), "Part 3 ran every gaming run");
    let mut seen = std::collections::BTreeSet::new();
    for r in part3 {
        let id = r["pair_id"].as_str().unwrap();
        assert!(seen.insert(id), "{id} ran twice");
        let c = classified
            .get(id)
            .unwrap_or_else(|| panic!("{id} is not a gaming run"));
        assert_eq!(r["shape"], c["shape"], "{id} shape");
        assert_eq!(r["diff_blocked"], c["blocked"], "{id} diff-gate verdict");
        assert_eq!(r["replay"], shapes["replay"], "{id} replay");
        assert_eq!(r["mode"].as_str(), Some("confirmatory"), "{id}");
        assert_eq!(r["binary_sha256"].as_str(), Some(sha), "{id}");
        let iid = r["instance_id"].as_str().unwrap();
        assert!(prov["images"][iid].is_string(), "{iid} has no Part 2 image");
        assert_eq!(r["result"]["image"], prov["images"][iid], "{id} image");
    }

    let s = &confirmatory_json("summary.json")["part3"];
    assert_eq!(s["label"].as_str(), Some("PASS_categorical_by_design"));
    assert_eq!(s["replay"], shapes["replay"]);
    let check = |rows: &[&Value], cell: &Value, what: &str| {
        let mut not_run = 0;
        let (mut diff, mut base, mut union, mut misses, mut caught) = (0, 0, 0, 0, 0);
        let mut kinds: BTreeMap<&str, usize> = ["failing-test", "suite-failed", "no-cases"]
            .into_iter()
            .map(|k| (k, 0))
            .collect();
        let ran: Vec<(&Value, Option<&str>)> = rows
            .iter()
            .filter_map(|r| match part3_kind(&r["result"]) {
                (true, kind) => Some((*r, kind)),
                (false, _) => {
                    not_run += 1;
                    None
                }
            })
            .collect();
        for (r, kind) in &ran {
            let d = r["diff_blocked"].as_bool() == Some(true);
            diff += usize::from(d);
            base += usize::from(kind.is_some());
            union += usize::from(d || kind.is_some());
            if !d {
                misses += 1;
                caught += usize::from(kind.is_some());
            }
            if let Some(k) = kind {
                *kinds.get_mut(k).unwrap() += 1;
            }
        }
        let n = ran.len();
        assert_eq!(int_json(&cell["runs"]), rows.len(), "{what} runs");
        assert_eq!(int_json(&cell["not_run"]), not_run, "{what} not run");
        for (key, k, n) in [
            ("diff_blocked", diff, n),
            ("base_tests_blocked", base, n),
            ("union_blocked", union, n),
            ("diff_misses_blocked_by_base_tests", caught, misses),
        ] {
            assert_eq!(int_json(&cell[key]["k"]), k, "{what} {key} k");
            assert_eq!(int_json(&cell[key]["n"]), n, "{what} {key} n");
            if n > 0 {
                let (lo, hi) = wilson_score_interval(k, n, conf).unwrap();
                close(
                    &cell[key]["wilson_95"][0],
                    lo,
                    &format!("{what} {key} lower"),
                );
                close(
                    &cell[key]["wilson_95"][1],
                    hi,
                    &format!("{what} {key} upper"),
                );
            }
        }
        for (k, v) in &kinds {
            assert_eq!(int_json(&cell["base_tests_kinds"][*k]), *v, "{what} {k}");
        }
    };
    let all: Vec<&Value> = part3.iter().collect();
    check(&all, &s["all"], "all");
    let table = s["shapes"].as_object().unwrap();
    assert_eq!(table.len(), shapes["shapes"].as_object().unwrap().len());
    for (shape, cell) in table {
        let rows: Vec<&Value> = part3.iter().filter(|r| r["shape"] == *shape).collect();
        assert_eq!(
            rows.len() as u64,
            shapes["shapes"][shape]["runs"].as_u64().unwrap(),
            "{shape}"
        );
        check(&rows, cell, shape);
    }
}
