use discipline::guards::perf::bounds::{
    design_effect_inflated_n, implied_design_effect, kappa_sample_size,
    minimum_detectable_difference, poisson_at_least, pull_requests_for_yield,
    sample_size_for_interval, sample_size_for_wilson_lower_bound,
    sample_size_for_wilson_lower_bound_power, standard_normal_quantile, trajectories_required,
    wilson_pass_probability, wilson_score_interval, zero_event_upper_bound,
};

#[test]
fn test_standard_normal_quantile_two_sided_critical_values() {
    // Pinned reference values for critical values:
    // alpha = 0.05 (95% two-sided CI) -> z = 1.959964...
    let z_95 = standard_normal_quantile(0.975).unwrap();
    assert!((z_95 - 1.959_963_984_540_054).abs() < 1e-9);

    // alpha = 0.01 (99% two-sided CI) -> z = 2.575829...
    let z_99 = standard_normal_quantile(0.995).unwrap();
    assert!((z_99 - 2.575_829_303_548_900_4).abs() < 1e-9);

    // alpha = 0.10 (90% two-sided CI) -> z = 1.644854...
    let z_90 = standard_normal_quantile(0.95).unwrap();
    assert!((z_90 - 1.644_853_626_951_472_2).abs() < 1e-9);

    // Power critical values (one-sided):
    // power = 0.80 -> z = 0.841621...
    let z_p80 = standard_normal_quantile(0.80).unwrap();
    assert!((z_p80 - 0.841_621_233_572_914_3).abs() < 1e-9);

    // power = 0.90 -> z = 1.281552...
    let z_p90 = standard_normal_quantile(0.90).unwrap();
    assert!((z_p90 - 1.281_551_565_544_600_4).abs() < 1e-9);

    // Complementary symmetry:
    let z_low = standard_normal_quantile(0.025).unwrap();
    assert_eq!(z_low, -z_95);
}

#[test]
fn test_sample_size_inversion_matches_wilson_score_interval_margin() {
    let z = standard_normal_quantile(0.975).unwrap();
    let continuous_margin = |p: f64, size: usize| -> f64 {
        let s = size as f64;
        let denom = 1.0 + z * z / s;
        let rad = p * (1.0 - p) / s + (z * z) / (4.0 * s * s);
        (z / denom) * rad.sqrt()
    };

    for &(p, w) in &[
        (0.90, 0.05),
        (0.80, 0.05),
        (0.70, 0.05),
        (0.50, 0.05),
        (0.85, 0.03),
        (0.95, 0.02),
    ] {
        let n = sample_size_for_interval(p, w, 0.95).unwrap();
        assert!(n >= 1);

        // Continuous Wilson margin: at n <= w, at n-1 > w
        assert!(
            continuous_margin(p, n) <= w,
            "failed at p={p}, w={w}: margin at n={n} was {} > {w}",
            continuous_margin(p, n)
        );
        if n > 1 {
            assert!(
                continuous_margin(p, n - 1) > w,
                "failed at p={p}, w={w}: margin at n-1={} was {} <= {w}",
                n - 1,
                continuous_margin(p, n - 1)
            );
        }

        // At sample size n with discrete wilson_score_interval:
        let k_n = (n as f64 * p).round() as usize;
        let (low_n, up_n) = wilson_score_interval(k_n, n, 0.95).unwrap();
        let margin_n = (up_n - low_n) / 2.0;
        assert!(
            margin_n <= w + 1e-4,
            "failed at p={p}, w={w}: discrete margin at n={n} was {margin_n} > {w}"
        );
    }
}

#[test]
fn test_minimum_detectable_difference_power_equation_consistency() {
    // Test that the solved delta satisfies the two-sample power equation:
    // delta == (z_alpha/2 + z_beta) * sqrt( p1*(1-p1)/n1 + (p1+delta)*(1-p1-delta)/n2 )
    for &(n1, n2, p1) in &[
        (100, 100, 0.60),
        (150, 150, 0.70),
        (80, 120, 0.55),
        (200, 300, 0.65),
    ] {
        let delta = minimum_detectable_difference(n1, n2, p1, 0.95, 0.80).unwrap();
        assert!(delta > 0.0);

        let p2 = p1 + delta;
        let z_sum =
            standard_normal_quantile(0.975).unwrap() + standard_normal_quantile(0.80).unwrap();
        let se = ((p1 * (1.0 - p1) / (n1 as f64)) + (p2 * (1.0 - p2) / (n2 as f64))).sqrt();
        let expected_delta = z_sum * se;

        assert!(
            (delta - expected_delta).abs() < 1e-6,
            "consistency failed for n1={n1}, n2={n2}, p1={p1}: delta={delta}, expected={expected_delta}"
        );
    }
}

#[test]
fn test_sample_size_and_mdd_invalid_inputs_fail_closed() {
    // Zero sample sizes
    assert!(minimum_detectable_difference(0, 100, 0.6, 0.95, 0.8).is_err());
    assert!(minimum_detectable_difference(100, 0, 0.6, 0.95, 0.8).is_err());

    // Non-finite and boundary probabilities
    assert!(sample_size_for_interval(0.0, 0.05, 0.95).is_err());
    assert!(sample_size_for_interval(1.0, 0.05, 0.95).is_err());
    assert!(sample_size_for_interval(f64::NAN, 0.05, 0.95).is_err());

    assert!(sample_size_for_interval(0.9, 0.0, 0.95).is_err());
    assert!(sample_size_for_interval(0.9, 0.5, 0.95).is_err());
    assert!(sample_size_for_interval(0.9, -0.05, 0.95).is_err());

    assert!(minimum_detectable_difference(100, 100, 0.0, 0.95, 0.8).is_err());
    assert!(minimum_detectable_difference(100, 100, 1.0, 0.95, 0.8).is_err());
    assert!(minimum_detectable_difference(100, 100, 0.6, 0.0, 0.8).is_err());
    assert!(minimum_detectable_difference(100, 100, 0.6, 1.0, 0.8).is_err());
    assert!(minimum_detectable_difference(100, 100, 0.6, 0.95, 0.0).is_err());
    assert!(minimum_detectable_difference(100, 100, 0.6, 0.95, 1.0).is_err());

    // Power unachievable when p1 + delta > 1.0
    assert!(minimum_detectable_difference(10, 10, 0.98, 0.95, 0.99).is_err());
}

// Recall study Step 0 (#838, `docs/benchmarks/recall-preregistration.toml`): the Rust
// mirrors of the recall functions in `scripts/benchmark_power_audit.py`, pinned to the same
// values. `tests/test_benchmark_power_bounds_recall.rs` pins the Python side and compares
// the two on a grid.

#[test]
fn recall_wilson_lower_bound_sample_sizes_match_the_pinned_values() {
    // (assumed recall, floor) -> (point-estimate n, power-0.80 n), confidence 0.95.
    for (expected, floor, point, power) in [
        (0.90, 0.80, 68, 112),
        (0.90, 0.75, 37, 58),
        (0.85, 0.70, 39, 69),
    ] {
        assert_eq!(
            sample_size_for_wilson_lower_bound(expected, floor, 0.95, 2000).unwrap(),
            point,
            "point n at ({expected}, {floor})"
        );
        assert_eq!(
            sample_size_for_wilson_lower_bound_power(expected, floor, 0.95, 0.80, 2000).unwrap(),
            power,
            "power n at ({expected}, {floor})"
        );
    }
    // Independent of n_max once n_max is past the last gap.
    assert_eq!(
        sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 500).unwrap(),
        68
    );
    assert_eq!(
        sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 500).unwrap(),
        112
    );
}

#[test]
fn recall_wilson_pass_probability_matches_the_exact_binomial_tail() {
    // The first value is the Python function's float (`--recall-bounds-json` audit row
    // n = 112); the others are the Python test's exact-arithmetic references.
    for (n, p, floor, expected) in [
        (112, 0.90, 0.80, 0.850_602_542_593_940_6),
        (111, 0.90, 0.80, 0.781_664_997_762_1),
        (100, 0.90, 0.80, 0.801_821_112_640_4),
        (58, 0.90, 0.75, 0.878_528_651_867_7),
        (69, 0.85, 0.70, 0.855_412_199_075_6),
    ] {
        let got = wilson_pass_probability(n, p, floor, 0.95).unwrap();
        assert!(
            (got - expected).abs() < 1e-9,
            "pass probability at n={n}, p={p}, floor={floor}: {got} != {expected}"
        );
    }
    // No count can clear a floor of 0.99 at n = 5.
    assert_eq!(wilson_pass_probability(5, 0.90, 0.99, 0.95).unwrap(), 0.0);
}

#[test]
fn recall_wilson_pass_probability_stays_finite_at_large_n() {
    // ln C(2000, k) is near 1380 at the mode: a direct product would overflow.
    for (p, floor) in [(0.90, 0.80), (0.70, 0.65), (0.50, 0.55)] {
        let got = wilson_pass_probability(2000, p, floor, 0.95).unwrap();
        assert!(
            got.is_finite() && (0.0..=1.0).contains(&got),
            "p={p}, floor={floor}: {got}"
        );
    }
    assert!(wilson_pass_probability(2000, 0.90, 0.80, 0.95).unwrap() > 0.999_999);
    assert!(wilson_pass_probability(2000, 0.50, 0.55, 0.95).unwrap() < 1e-6);
}

/// The monotonicity rule: each requirement is the start of the last unbroken run of
/// qualifying sizes up to n_max, not the first size that qualifies.
#[test]
fn recall_requirement_is_the_last_unbroken_run_not_the_first_crossing() {
    let clears = |n: usize| {
        let k = (0.90 * n as f64 + 0.5).floor() as usize;
        wilson_score_interval(k, n, 0.95).unwrap().0 >= 0.80
    };
    let first_clearing = (1..=2000).find(|&n| clears(n)).unwrap();
    assert_eq!(first_clearing, 54);
    let gaps: Vec<usize> = (54..69).filter(|&n| !clears(n)).collect();
    assert_eq!(gaps, [56, 57, 58, 59, 60, 66, 67]);
    assert_eq!(
        sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 2000).unwrap(),
        68
    );

    let meets = |n: usize| wilson_pass_probability(n, 0.90, 0.80, 0.95).unwrap() >= 0.80;
    let first_meeting = (1..=2000).find(|&n| meets(n)).unwrap();
    assert_eq!(first_meeting, 100);
    let below: Vec<usize> = (100..113).filter(|&n| !meets(n)).collect();
    assert_eq!(below, [101, 102, 103, 104, 105, 110, 111]);
    assert_eq!(
        sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 2000).unwrap(),
        112
    );
}

#[test]
fn recall_trajectories_required_rounds_up_but_not_on_float_noise() {
    assert_eq!(trajectories_required(112, 0.30, 0.50, 0.90).unwrap(), 830);
    // 100 / 0.25 and 10 / 0.1 are not exact in binary; neither may add a run.
    assert_eq!(trajectories_required(100, 0.50, 0.50, 1.0).unwrap(), 400);
    assert_eq!(trajectories_required(10, 0.10, 1.0, 1.0).unwrap(), 100);
    assert_eq!(trajectories_required(1, 1.0, 1.0, 1.0).unwrap(), 1);
}

#[test]
fn recall_bounds_invalid_inputs_fail_closed() {
    assert!(wilson_pass_probability(0, 0.9, 0.8, 0.95).is_err());
    assert!(wilson_pass_probability(10, 1.0, 0.8, 0.95).is_err());
    assert!(wilson_pass_probability(10, 0.9, 0.0, 0.95).is_err());
    assert!(wilson_pass_probability(10, 0.9, 0.8, 1.0).is_err());
    assert!(wilson_pass_probability(10, f64::NAN, 0.8, 0.95).is_err());

    // Floor not below the expected proportion; n_max that does not qualify; n_max of 0.
    assert!(sample_size_for_wilson_lower_bound(0.80, 0.80, 0.95, 2000).is_err());
    assert!(sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 10).is_err());
    assert!(sample_size_for_wilson_lower_bound(0.90, 0.80, 0.95, 0).is_err());
    assert!(sample_size_for_wilson_lower_bound(0.90, 0.80, f64::NAN, 2000).is_err());

    assert!(sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 1.0, 2000).is_err());
    assert!(sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.0, 2000).is_err());
    assert!(sample_size_for_wilson_lower_bound_power(0.90, 0.80, 0.95, 0.80, 50).is_err());
    assert!(sample_size_for_wilson_lower_bound_power(0.70, 0.80, 0.95, 0.80, 2000).is_err());

    assert!(trajectories_required(0, 0.5, 0.5, 0.5).is_err());
    assert!(trajectories_required(10, 0.0, 0.5, 0.5).is_err());
    assert!(trajectories_required(10, 0.5, 1.5, 0.5).is_err());
    assert!(trajectories_required(10, 0.5, 0.5, f64::NAN).is_err());
    assert!(trajectories_required(10, 1e-300, 1e-300, 1.0).is_err());
}

/// The recall pre-registration's required sizes are what the Rust functions return for
/// its own inputs.
#[test]
fn recall_preregistration_required_n_matches_rust_bounds() {
    let raw = std::fs::read_to_string("docs/benchmarks/recall-preregistration.toml")
        .expect("docs/benchmarks/recall-preregistration.toml must exist");
    let doc: toml::Value = toml::from_str(&raw).expect("recall pre-registration must parse");
    let pb = &doc["power_and_bounds"];
    let f = |key: &str| pb[key].as_float().unwrap_or_else(|| panic!("{key} float"));
    let i = |key: &str| {
        usize::try_from(
            pb[key]
                .as_integer()
                .unwrap_or_else(|| panic!("{key} integer")),
        )
        .unwrap()
    };
    let (expected, floor) = (f("assumed_recall"), f("recall_floor"));
    let (confidence, power, n_max) = (f("confidence_level"), f("statistical_power"), i("n_max"));
    assert_eq!(
        sample_size_for_wilson_lower_bound(expected, floor, confidence, n_max).unwrap(),
        i("required_n_point_estimate")
    );
    assert_eq!(
        sample_size_for_wilson_lower_bound_power(expected, floor, confidence, power, n_max)
            .unwrap(),
        i("required_n_before_design_effect")
    );
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreregistrationDoc {
    study: StudySection,
    population: PopulationSection,
    adjudication: AdjudicationSection,
    twin_baseline: TwinBaselineSection,
    power_and_bounds: PowerAndBoundsSection,
    gate_floors: Vec<GateFloorGroup>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StudySection {
    id: String,
    previous_study_id: String,
    status: String,
    locked_date: String,
    novelty_tier_claimed: String,
    tier_justification: String,
    discipline_version: String,
    configuration_mode: String,
    primary_unit_of_analysis: String,
    secondary_unit_of_analysis: String,
    recall_claimed: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PopulationSection {
    visibility: String,
    exclude_private_repositories: bool,
    exclude_named_consumer_repositories: bool,
    pull_request_state: String,
    pr_number_rule: String,
    head_base_definition: String,
    pilot_git_log_window: usize,
    evaluation_git_log_window: usize,
    max_coding_agent_commits_per_repo: usize,
    max_dependency_bot_commits_per_repo: usize,
    band_separation: String,
    pilot_repos: Vec<String>,
    evaluation_repos: Vec<String>,
    agent_bot_logins: Vec<String>,
    agent_coauthor_trailers: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AdjudicationSection {
    independent_raters: usize,
    agreement_metric: String,
    min_acceptable_kappa: f64,
    blind_to_source_tool: bool,
    hash_contributor_and_rater_identities: bool,
    committed_rater_packet: String,
    rater_kind_disclosure: String,
    disagreement_resolution_rule: String,
    labels: Vec<AdjudicationLabel>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AdjudicationLabel {
    id: String,
    counts_as_success: bool,
    counts_in_denominator: bool,
    definition: String,
    worked_example: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TwinBaselineSection {
    id: String,
    sampled_flags: usize,
    sample_seed: u64,
    packet_shuffle_seed: u64,
    rule: String,
    winning_regime: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PowerAndBoundsSection {
    confidence_level: f64,
    statistical_power: f64,
    target_half_width: f64,
    insufficient_n_verdict: String,
    amendment_verdict: String,
    structural_assumed_precision: f64,
    structural_assumed_precision_n: usize,
    structural_required_n: usize,
    heuristic_assumed_precision: f64,
    heuristic_assumed_precision_n: usize,
    heuristic_required_n: usize,
    twin_assumed_baseline_precision: f64,
    mdd_structural_vs_twin: f64,
    mdd_heuristic_vs_twin: f64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GateFloorGroup {
    class: String,
    precision_floor: f64,
    assumed_precision: f64,
    required_n: usize,
    rationale: String,
    gates: Vec<String>,
}

#[test]
fn test_preregistration_audit_bounds_match_committed_toml() {
    let raw = std::fs::read_to_string("docs/benchmarks/precision-preregistration.toml")
        .expect("docs/benchmarks/precision-preregistration.toml must exist");
    let doc: PreregistrationDoc =
        toml::from_str(&raw).expect("precision-preregistration.toml must parse strictly");

    // 1. Study metadata & novelty tier
    assert_eq!(doc.study.id, "agent-pr-precision-v2");
    assert_eq!(doc.study.previous_study_id, "agent-pr-precision-v1");
    assert_eq!(doc.study.status, "locked-pre-data");
    assert!(!doc.study.locked_date.is_empty());
    assert_eq!(doc.study.novelty_tier_claimed, "Characterisation");
    assert!(!doc.study.tier_justification.trim().is_empty());
    assert_eq!(doc.study.discipline_version, "0.23.0");
    assert!(!doc.study.configuration_mode.is_empty());
    assert!(!doc.study.primary_unit_of_analysis.is_empty());
    assert!(!doc.study.secondary_unit_of_analysis.is_empty());
    assert!(!doc.study.recall_claimed);

    // 2. Disjoint pilot and evaluation bands, public only
    assert_eq!(doc.population.visibility, "public-only");
    assert!(doc.population.exclude_private_repositories);
    assert!(doc.population.exclude_named_consumer_repositories);
    assert_eq!(doc.population.pull_request_state, "merged-only");
    assert!(!doc.population.pr_number_rule.trim().is_empty());
    assert!(!doc.population.head_base_definition.trim().is_empty());
    assert!(doc.population.pilot_git_log_window >= 1000);
    assert!(doc.population.evaluation_git_log_window >= 1000);
    assert_eq!(doc.population.max_coding_agent_commits_per_repo, 45);
    assert_eq!(doc.population.max_dependency_bot_commits_per_repo, 15);
    assert_eq!(doc.population.band_separation, "disjoint-by-repository");
    assert!(!doc.population.pilot_repos.is_empty());
    assert!(!doc.population.evaluation_repos.is_empty());
    assert!(doc
        .population
        .evaluation_repos
        .contains(&"astral-sh/ruff".to_string()));
    assert!(!doc
        .population
        .evaluation_repos
        .contains(&"ruff-sh/ruff".to_string()));
    for repo in &doc.population.pilot_repos {
        assert!(
            !doc.population.evaluation_repos.contains(repo),
            "pilot repo {repo} must not appear in evaluation_repos"
        );
    }
    assert!(!doc.population.agent_bot_logins.is_empty());
    assert!(!doc.population.agent_coauthor_trailers.is_empty());

    // 3. Adjudication protocol: 2 raters, Cohen's kappa, 3 labels with worked examples
    assert_eq!(doc.adjudication.independent_raters, 2);
    assert_eq!(doc.adjudication.agreement_metric, "cohens_kappa");
    assert!(doc.adjudication.min_acceptable_kappa >= 0.70);
    assert!(doc.adjudication.blind_to_source_tool);
    assert!(doc.adjudication.hash_contributor_and_rater_identities);
    assert_eq!(
        doc.adjudication.committed_rater_packet,
        "docs/benchmarks/results/rater-packet.json"
    );
    assert!(!doc.adjudication.rater_kind_disclosure.trim().is_empty());
    assert!(!doc
        .adjudication
        .disagreement_resolution_rule
        .trim()
        .is_empty());
    let label_ids: Vec<&str> = doc
        .adjudication
        .labels
        .iter()
        .map(|l| l.id.as_str())
        .collect();
    assert_eq!(
        label_ids,
        vec!["true-positive", "false-positive", "cannot-tell"]
    );
    for label in &doc.adjudication.labels {
        assert!(!label.definition.trim().is_empty());
        assert!(!label.worked_example.trim().is_empty());
        assert!(label.counts_in_denominator);
    }
    assert!(doc.adjudication.labels[0].counts_as_success);
    assert!(!doc.adjudication.labels[1].counts_as_success);
    assert!(!doc.adjudication.labels[2].counts_as_success);

    // 4. Twin baseline specification with winning regime
    assert!(!doc.twin_baseline.id.is_empty());
    assert_eq!(doc.twin_baseline.sampled_flags, 150);
    assert_eq!(doc.twin_baseline.sample_seed, 482);
    assert_eq!(doc.twin_baseline.packet_shuffle_seed, 48201);
    assert!(!doc.twin_baseline.rule.trim().is_empty());
    assert!(!doc.twin_baseline.winning_regime.trim().is_empty());

    // 5. Mathematical audit: verify every sample size and MDD in the pre-registration
    // matches `sample_size_for_interval` and `minimum_detectable_difference`.
    let pb = &doc.power_and_bounds;
    assert_eq!(pb.insufficient_n_verdict, "insufficient-n");
    assert_eq!(
        pb.amendment_verdict,
        "INTERMEDIATE_amended_threshold_redesign"
    );

    let calc_structural_assumed_n = sample_size_for_interval(
        pb.structural_assumed_precision,
        pb.target_half_width,
        pb.confidence_level,
    )
    .unwrap();
    assert_eq!(pb.structural_assumed_precision_n, calc_structural_assumed_n);

    let calc_structural_req_n =
        sample_size_for_interval(0.90, pb.target_half_width, pb.confidence_level).unwrap();
    assert_eq!(pb.structural_required_n, calc_structural_req_n);

    let calc_heuristic_assumed_n = sample_size_for_interval(
        pb.heuristic_assumed_precision,
        pb.target_half_width,
        pb.confidence_level,
    )
    .unwrap();
    assert_eq!(pb.heuristic_assumed_precision_n, calc_heuristic_assumed_n);

    let calc_heuristic_req_n =
        sample_size_for_interval(0.80, pb.target_half_width, pb.confidence_level).unwrap();
    assert_eq!(pb.heuristic_required_n, calc_heuristic_req_n);

    let calc_mdd_structural = minimum_detectable_difference(
        pb.structural_required_n,
        pb.structural_required_n,
        pb.twin_assumed_baseline_precision,
        pb.confidence_level,
        pb.statistical_power,
    )
    .unwrap();
    assert!(
        (pb.mdd_structural_vs_twin - calc_mdd_structural).abs() < 1e-12,
        "structural MDD mismatch: toml={}, computed={}",
        pb.mdd_structural_vs_twin,
        calc_mdd_structural
    );

    let calc_mdd_heuristic = minimum_detectable_difference(
        pb.heuristic_required_n,
        pb.heuristic_required_n,
        pb.twin_assumed_baseline_precision,
        pb.confidence_level,
        pb.statistical_power,
    )
    .unwrap();
    assert!(
        (pb.mdd_heuristic_vs_twin - calc_mdd_heuristic).abs() < 1e-12,
        "heuristic MDD mismatch: toml={}, computed={}",
        pb.mdd_heuristic_vs_twin,
        calc_mdd_heuristic
    );

    // 6. Verify per-gate floor groups reference valid registered gates and that at
    // `required_n` with `assumed_precision`, the Wilson 95% lower bound strictly clears `precision_floor`.
    assert_eq!(doc.gate_floors.len(), 2);
    for group in &doc.gate_floors {
        assert!(!group.class.is_empty());
        assert!(!group.rationale.trim().is_empty());
        assert!(!group.gates.is_empty());
        let expected_n = sample_size_for_interval(
            group.precision_floor,
            pb.target_half_width,
            pb.confidence_level,
        )
        .unwrap();
        assert_eq!(
            group.required_n, expected_n,
            "required_n mismatch for group {}",
            group.class
        );
        let k_assumed = (group.required_n as f64 * group.assumed_precision).round() as usize;
        let (low, high) =
            wilson_score_interval(k_assumed, group.required_n, pb.confidence_level).unwrap();
        assert!(
            (high - low) / 2.0 <= pb.target_half_width,
            "half-width at assumed precision must be <= target_half_width"
        );
        assert!(
            low >= group.precision_floor,
            "at assumed precision {}, Wilson lower bound {} must strictly clear floor {}",
            group.assumed_precision,
            low,
            group.precision_floor
        );
        for gate_id in &group.gates {
            assert!(
                discipline::config::gate_info(gate_id).is_some(),
                "unknown gate_id {gate_id} in precision-preregistration.toml"
            );
        }
    }
}

#[derive(serde::Deserialize)]
struct SummaryDoc {
    study_id: String,
    discipline_version: String,
    evaluated_commit: String,
    confidence_level: f64,
    /// `null` when a re-run has no fresh items to compute it on (v5).
    cohens_kappa: Option<f64>,
    rater_kind: String,
    synthetic_rater_disclosure: String,
    pull_request_block_rates: BlockRatesSection,
    class_results: Vec<ClassResultEntry>,
    twin_baseline_result: TwinResultEntry,
    per_gate_results: Vec<PerGateResultEntry>,
}

#[derive(serde::Deserialize)]
struct ManifestDoc {
    study_id: String,
    discipline_version: String,
    evaluated_commit: String,
    total_pull_requests: usize,
    discipline_blocked_pull_requests: usize,
    twin_blocked_pull_requests: usize,
    pull_requests: Vec<ManifestPullRequest>,
}

#[derive(serde::Deserialize)]
struct ManifestPullRequest {
    repo: String,
    pr_number: u64,
    base_sha: String,
    head_sha: String,
}

#[derive(serde::Deserialize)]
struct RaterPacketDoc {
    study_id: String,
    blind_to_source_tool: bool,
    total_items: usize,
    items: Vec<RaterPacketItem>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RaterPacketItem {
    item_id: String,
    repo: String,
    commit_sha: String,
    commit_subject: String,
    file: Option<String>,
    line: Option<usize>,
    rule_summary: String,
    diff_excerpt: String,
    line_context: Option<String>,
}

#[derive(serde::Deserialize)]
struct BlockRatesSection {
    total_pull_requests: usize,
    discipline: BlockRateArm,
    naive_sensitive_path_filter: BlockRateArm,
}

#[derive(serde::Deserialize)]
struct BlockRateArm {
    blocked: usize,
    n: usize,
    point_estimate: f64,
    wilson_95_ci: [f64; 2],
}

#[derive(serde::Deserialize)]
struct ClassResultEntry {
    class: String,
    true_positives: usize,
    false_positives: usize,
    cannot_tell: usize,
    k: usize,
    n: usize,
    required_n: usize,
    precision_floor: f64,
    point_estimate: f64,
    wilson_95_ci: [f64; 2],
    verdict: String,
}

#[derive(serde::Deserialize)]
struct TwinResultEntry {
    id: String,
    true_positives: usize,
    false_positives: usize,
    cannot_tell: usize,
    k: usize,
    n: usize,
    required_n: usize,
    precision_floor: f64,
    point_estimate: f64,
    wilson_95_ci: [f64; 2],
    verdict: String,
    observed_mdd_vs_structural: f64,
    observed_precision_delta_structural_vs_twin: f64,
}

#[derive(serde::Deserialize)]
struct PerGateResultEntry {
    gate: String,
    class: String,
    true_positives: usize,
    false_positives: usize,
    cannot_tell: usize,
    k: usize,
    n: usize,
    required_n: usize,
    precision_floor: f64,
    point_estimate: Option<f64>,
    wilson_95_ci: Option<[f64; 2]>,
    verdict: String,
}

#[derive(serde::Deserialize)]
struct AdjudicationDoc {
    study_id: String,
    blind_to_source_tool: bool,
    hash_contributor_and_rater_identities: bool,
    rater_1_kind: String,
    rater_2_kind: String,
    tie_breaker_kind: String,
    synthetic_rater_disclosure: String,
    committed_rater_packet: String,
    total_items: usize,
    exact_agreement_count: usize,
    disagreement_count: usize,
    cohens_kappa: f64,
    min_acceptable_kappa: f64,
    kappa_passed: bool,
    adjudications: Vec<AdjudicationItem>,
}

#[derive(serde::Deserialize, Clone)]
struct AdjudicationItem {
    item_id: String,
    rater_1_label: String,
    rater_2_label: String,
    agreed: bool,
    tie_breaker_label: Option<String>,
    final_label: String,
}

#[derive(serde::Deserialize)]
struct FindingsDoc {
    items: Vec<FindingItem>,
}

#[derive(serde::Deserialize)]
struct FindingItem {
    item_id: String,
    source_tool: String,
    gate: String,
}

/// Final-label counts for one gate, class or the twin.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct LabelTally {
    true_positives: usize,
    false_positives: usize,
    cannot_tell: usize,
}

impl LabelTally {
    fn add(&mut self, label: &str) -> Result<(), String> {
        match label {
            "true-positive" => self.true_positives += 1,
            "false-positive" => self.false_positives += 1,
            "cannot-tell" => self.cannot_tell += 1,
            other => return Err(format!("unknown final label {other}")),
        }
        Ok(())
    }
}

/// Everything summary.json reports that follows from the item-level labels.
struct RecomputedLabels {
    per_gate: std::collections::BTreeMap<String, LabelTally>,
    per_class: std::collections::BTreeMap<String, LabelTally>,
    twin: LabelTally,
    exact_agreement: usize,
    cohens_kappa: f64,
}

/// Unweighted Cohen's kappa over nominal labels (Cohen 1960), the same definition as
/// `cohens_kappa` in `scripts/benchmark_precision.py`.
fn cohens_kappa(pairs: &[(&str, &str)]) -> f64 {
    use std::collections::BTreeMap;
    let n = pairs.len() as f64;
    let mut a: BTreeMap<&str, f64> = BTreeMap::new();
    let mut b: BTreeMap<&str, f64> = BTreeMap::new();
    let mut agreed = 0.0;
    for (x, y) in pairs {
        *a.entry(x).or_default() += 1.0;
        *b.entry(y).or_default() += 1.0;
        if x == y {
            agreed += 1.0;
        }
    }
    let p_o = agreed / n;
    let p_e: f64 = a
        .iter()
        .map(|(c, ca)| (ca / n) * (b.get(c).copied().unwrap_or(0.0) / n))
        .sum();
    (p_o - p_e) / (1.0 - p_e)
}

/// Joins each finding to its adjudication and tallies the final labels per gate, per
/// gate class (from the locked pre-registration) and for the twin, and recomputes the
/// raters' agreement from their individual labels.
fn recompute_from_labels(
    findings: &[FindingItem],
    adjudications: &[AdjudicationItem],
    gate_class: &std::collections::BTreeMap<String, String>,
) -> Result<RecomputedLabels, String> {
    let by_id: std::collections::BTreeMap<&str, &AdjudicationItem> = adjudications
        .iter()
        .map(|a| (a.item_id.as_str(), a))
        .collect();
    if by_id.len() != findings.len() || adjudications.len() != findings.len() {
        return Err(format!(
            "{} findings vs {} adjudications ({} distinct ids)",
            findings.len(),
            adjudications.len(),
            by_id.len()
        ));
    }
    let mut out = RecomputedLabels {
        per_gate: Default::default(),
        per_class: Default::default(),
        twin: LabelTally::default(),
        exact_agreement: 0,
        cohens_kappa: 0.0,
    };
    for f in findings {
        let adj = by_id
            .get(f.item_id.as_str())
            .ok_or_else(|| format!("{} has no adjudication", f.item_id))?;
        match f.source_tool.as_str() {
            "discipline" => {
                let class = gate_class
                    .get(&f.gate)
                    .ok_or_else(|| format!("{}: gate {} has no class", f.item_id, f.gate))?;
                out.per_gate
                    .entry(f.gate.clone())
                    .or_default()
                    .add(&adj.final_label)?;
                out.per_class
                    .entry(class.clone())
                    .or_default()
                    .add(&adj.final_label)?;
            }
            "naive-sensitive-path-filter" => out.twin.add(&adj.final_label)?,
            other => return Err(format!("{}: unknown source_tool {other}", f.item_id)),
        }
    }
    let pairs: Vec<(&str, &str)> = adjudications
        .iter()
        .map(|a| (a.rater_1_label.as_str(), a.rater_2_label.as_str()))
        .collect();
    out.exact_agreement = pairs.iter().filter(|(x, y)| x == y).count();
    out.cohens_kappa = cohens_kappa(&pairs);
    Ok(out)
}

fn tally_matches(
    what: &str,
    got: LabelTally,
    tp: usize,
    fp: usize,
    ct: usize,
) -> Result<(), String> {
    let want = LabelTally {
        true_positives: tp,
        false_positives: fp,
        cannot_tell: ct,
    };
    if got == want {
        Ok(())
    } else {
        Err(format!(
            "{what}: labels give {got:?}, summary.json says {want:?}"
        ))
    }
}

/// Compares summary.json's counts and kappa with the ones the item labels give.
fn check_summary_against_labels(
    summary: &SummaryDoc,
    adj: &AdjudicationDoc,
    r: &RecomputedLabels,
) -> Result<(), String> {
    for cr in &summary.class_results {
        let got = r.per_class.get(&cr.class).copied().unwrap_or_default();
        tally_matches(
            &format!("class {}", cr.class),
            got,
            cr.true_positives,
            cr.false_positives,
            cr.cannot_tell,
        )?;
    }
    for class in r.per_class.keys() {
        if !summary.class_results.iter().any(|c| &c.class == class) {
            return Err(format!("class {class} has labels but no summary row"));
        }
    }
    for gr in &summary.per_gate_results {
        let got = r.per_gate.get(&gr.gate).copied().unwrap_or_default();
        tally_matches(
            &format!("gate {}", gr.gate),
            got,
            gr.true_positives,
            gr.false_positives,
            gr.cannot_tell,
        )?;
    }
    for gate in r.per_gate.keys() {
        if !summary.per_gate_results.iter().any(|g| &g.gate == gate) {
            return Err(format!("gate {gate} has labels but no summary row"));
        }
    }
    let tr = &summary.twin_baseline_result;
    tally_matches(
        "twin",
        r.twin,
        tr.true_positives,
        tr.false_positives,
        tr.cannot_tell,
    )?;
    if r.exact_agreement != adj.exact_agreement_count {
        return Err(format!(
            "rater labels agree on {} items, adjudication.json says {}",
            r.exact_agreement, adj.exact_agreement_count
        ));
    }
    for (name, reported) in [
        (
            "summary.json",
            summary.cohens_kappa.ok_or("summary.json has no kappa")?,
        ),
        ("adjudication.json", adj.cohens_kappa),
    ] {
        if (reported - r.cohens_kappa).abs() >= 1e-6 {
            return Err(format!(
                "{name} kappa {reported} vs {} from the rater labels",
                r.cohens_kappa
            ));
        }
    }
    Ok(())
}

struct BenchmarkInputs {
    summary: SummaryDoc,
    manifest: ManifestDoc,
    packet: RaterPacketDoc,
    adj: AdjudicationDoc,
    findings: FindingsDoc,
    gate_class: std::collections::BTreeMap<String, String>,
    evaluation_repos: Vec<String>,
}

fn load_benchmark_inputs() -> BenchmarkInputs {
    let read = |path: &str| {
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path} must exist: {e}"))
    };
    let prereg: PreregistrationDoc =
        toml::from_str(&read("docs/benchmarks/precision-preregistration.toml"))
            .expect("precision-preregistration.toml must parse strictly");
    let gate_class = prereg
        .gate_floors
        .iter()
        .flat_map(|g| g.gates.iter().map(|gate| (gate.clone(), g.class.clone())))
        .collect();
    BenchmarkInputs {
        summary: serde_json::from_str(&read("docs/benchmarks/results/summary.json"))
            .expect("summary.json must parse cleanly"),
        manifest: serde_json::from_str(&read("docs/benchmarks/results/manifest.json"))
            .expect("manifest.json must parse cleanly"),
        packet: serde_json::from_str(&read("docs/benchmarks/results/rater-packet.json"))
            .expect("rater-packet.json must parse cleanly"),
        adj: serde_json::from_str(&read("docs/benchmarks/results/adjudication.json"))
            .expect("adjudication.json must parse cleanly"),
        findings: serde_json::from_str(&read("docs/benchmarks/results/findings.json"))
            .expect("findings.json must parse cleanly"),
        gate_class,
        evaluation_repos: prereg.population.evaluation_repos,
    }
}

#[test]
fn flipping_one_adjudication_label_breaks_the_summary_cross_check() {
    let inputs = load_benchmark_inputs();
    let recheck = |adjudications: &[AdjudicationItem]| {
        let r = recompute_from_labels(&inputs.findings.items, adjudications, &inputs.gate_class)
            .unwrap();
        check_summary_against_labels(&inputs.summary, &inputs.adj, &r)
    };
    // A final label flipped on one discipline finding moves its gate and class tally.
    let discipline_ids: std::collections::BTreeSet<&str> = inputs
        .findings
        .items
        .iter()
        .filter(|f| f.source_tool == "discipline")
        .map(|f| f.item_id.as_str())
        .collect();
    let mut flipped = inputs.adj.adjudications.clone();
    let tp = flipped
        .iter_mut()
        .find(|a| a.final_label == "true-positive" && discipline_ids.contains(a.item_id.as_str()))
        .expect("a discipline true positive");
    tp.final_label = "false-positive".into();
    let err = recheck(&flipped).unwrap_err();
    assert!(err.contains("class") || err.contains("gate"), "{err}");

    // A final label flipped on one twin flag moves the twin tally.
    let mut flipped = inputs.adj.adjudications.clone();
    let twin = flipped
        .iter_mut()
        .find(|a| a.final_label == "false-positive" && !discipline_ids.contains(a.item_id.as_str()))
        .expect("a twin false positive");
    twin.final_label = "true-positive".into();
    assert!(recheck(&flipped).unwrap_err().starts_with("twin"));

    // One rater label flipped changes the agreement count and kappa.
    let mut flipped = inputs.adj.adjudications.clone();
    flipped[0].rater_1_label = if flipped[0].rater_1_label == "true-positive" {
        "false-positive".into()
    } else {
        "true-positive".into()
    };
    assert!(recheck(&flipped).unwrap_err().contains("agree"));
}

fn expected_verdict(
    k: usize,
    n: usize,
    required_n: usize,
    floor: f64,
    ci_low: f64,
) -> &'static str {
    if n < required_n {
        "insufficient-n"
    } else if ci_low >= floor {
        "PASS"
    } else if (k as f64 / n as f64) >= floor {
        "INTERMEDIATE_floor_within_ci"
    } else {
        "REJECTED"
    }
}

#[test]
fn test_benchmark_summary_results_match_rust_bounds() {
    let inputs = load_benchmark_inputs();

    // The counts and kappa in summary.json must follow from the item-level labels:
    // findings.json joined to adjudication.json's final labels, per gate, per class
    // (from the locked pre-registration) and for the twin; kappa from both raters.
    let recomputed = recompute_from_labels(
        &inputs.findings.items,
        &inputs.adj.adjudications,
        &inputs.gate_class,
    )
    .expect("every finding joins to one adjudication");
    check_summary_against_labels(&inputs.summary, &inputs.adj, &recomputed).unwrap();
    for gr in &inputs.summary.per_gate_results {
        assert_eq!(
            inputs.gate_class.get(&gr.gate),
            Some(&gr.class),
            "gate {} class differs from the pre-registration",
            gr.gate
        );
    }

    let BenchmarkInputs {
        summary,
        manifest,
        packet,
        adj,
        evaluation_repos,
        ..
    } = inputs;

    assert_eq!(summary.study_id, "agent-pr-precision-v2");
    assert_eq!(manifest.study_id, "agent-pr-precision-v2");
    assert_eq!(packet.study_id, "agent-pr-precision-v2");
    assert_eq!(adj.study_id, "agent-pr-precision-v2");
    assert_eq!(summary.discipline_version, "0.23.0");
    assert_eq!(manifest.discipline_version, "0.23.0");
    assert_eq!(summary.evaluated_commit.len(), 40);
    assert_eq!(summary.evaluated_commit, manifest.evaluated_commit);
    assert!(adj.blind_to_source_tool);
    assert!(packet.blind_to_source_tool);
    assert!(adj.hash_contributor_and_rater_identities);
    assert_eq!(
        adj.committed_rater_packet,
        "docs/benchmarks/results/rater-packet.json"
    );
    assert!(!adj.rater_1_kind.is_empty());
    assert!(!adj.rater_2_kind.is_empty());
    assert!(!adj.tie_breaker_kind.is_empty());
    assert!(!adj.synthetic_rater_disclosure.trim().is_empty());
    assert_eq!(summary.rater_kind, adj.rater_1_kind);
    assert_eq!(
        summary.synthetic_rater_disclosure,
        adj.synthetic_rater_disclosure
    );
    assert_eq!(packet.total_items, adj.total_items);
    assert_eq!(packet.items.len(), adj.total_items);
    for (p_item, a_item) in packet.items.iter().zip(adj.adjudications.iter()) {
        assert_eq!(p_item.item_id, a_item.item_id);
        assert!(evaluation_repos.contains(&p_item.repo));
        assert_eq!(p_item.commit_sha.len(), 12);
        assert!(!p_item.commit_subject.trim().is_empty());
        assert!(!p_item.rule_summary.trim().is_empty());
        assert!(!p_item.diff_excerpt.trim().is_empty());
        if p_item.line.is_some() {
            assert!(p_item.file.is_some());
            assert!(p_item.line_context.is_some());
        }
    }
    assert_eq!(manifest.pull_requests.len(), manifest.total_pull_requests);
    assert_eq!(
        manifest.discipline_blocked_pull_requests,
        summary.pull_request_block_rates.discipline.blocked
    );
    assert_eq!(
        manifest.twin_blocked_pull_requests,
        summary
            .pull_request_block_rates
            .naive_sensitive_path_filter
            .blocked
    );
    for pr in &manifest.pull_requests {
        assert!(
            evaluation_repos.contains(&pr.repo),
            "unknown repo {} in manifest.json",
            pr.repo
        );
        assert!(pr.pr_number > 0);
        assert_eq!(pr.base_sha.len(), 40);
        assert_eq!(pr.head_sha.len(), 40);
    }
    assert_eq!(adj.adjudications.len(), adj.total_items);
    assert_eq!(
        adj.exact_agreement_count + adj.disagreement_count,
        adj.total_items
    );
    assert!(adj.cohens_kappa >= adj.min_acceptable_kappa);
    assert!(adj.kappa_passed);
    assert!((summary.cohens_kappa.unwrap() - adj.cohens_kappa).abs() < 1e-6);

    for item in &adj.adjudications {
        assert!(!item.item_id.is_empty());
        if item.agreed {
            assert_eq!(item.rater_1_label, item.rater_2_label);
            assert_eq!(item.final_label, item.rater_1_label);
            assert!(item.tie_breaker_label.is_none());
        } else {
            assert_ne!(item.rater_1_label, item.rater_2_label);
            assert_eq!(
                item.tie_breaker_label.as_deref(),
                Some(item.final_label.as_str())
            );
        }
    }

    // Verify PR block rate Wilson intervals
    for arm in [
        &summary.pull_request_block_rates.discipline,
        &summary.pull_request_block_rates.naive_sensitive_path_filter,
    ] {
        assert_eq!(arm.n, summary.pull_request_block_rates.total_pull_requests);
        let (low, high) =
            wilson_score_interval(arm.blocked, arm.n, summary.confidence_level).unwrap();
        assert!((arm.point_estimate - (arm.blocked as f64 / arm.n as f64)).abs() < 1e-6);
        assert!((arm.wilson_95_ci[0] - low).abs() < 1e-6);
        assert!((arm.wilson_95_ci[1] - high).abs() < 1e-6);
    }

    // Verify class-level Wilson intervals and verdicts
    assert_eq!(summary.class_results.len(), 2);
    for cr in &summary.class_results {
        assert_eq!(cr.k, cr.true_positives);
        assert_eq!(
            cr.n,
            cr.true_positives + cr.false_positives + cr.cannot_tell
        );
        let (low, high) = wilson_score_interval(cr.k, cr.n, summary.confidence_level).unwrap();
        assert!((cr.point_estimate - (cr.k as f64 / cr.n as f64)).abs() < 1e-6);
        assert!(
            (cr.wilson_95_ci[0] - low).abs() < 1e-6,
            "class {} low mismatch: {} vs {}",
            cr.class,
            cr.wilson_95_ci[0],
            low
        );
        assert!(
            (cr.wilson_95_ci[1] - high).abs() < 1e-6,
            "class {} high mismatch: {} vs {}",
            cr.class,
            cr.wilson_95_ci[1],
            high
        );
        let exp_v = expected_verdict(cr.k, cr.n, cr.required_n, cr.precision_floor, low);
        assert_eq!(cr.verdict, exp_v, "class {} verdict mismatch", cr.class);
    }

    // Verify twin baseline Wilson interval, MDD, and verdict
    let tr = &summary.twin_baseline_result;
    assert_eq!(tr.id, "naive-sensitive-path-filter");
    assert_eq!(tr.k, tr.true_positives);
    assert_eq!(
        tr.n,
        tr.true_positives + tr.false_positives + tr.cannot_tell
    );
    let (t_low, t_high) = wilson_score_interval(tr.k, tr.n, summary.confidence_level).unwrap();
    assert!((tr.point_estimate - (tr.k as f64 / tr.n as f64)).abs() < 1e-6);
    assert!((tr.wilson_95_ci[0] - t_low).abs() < 1e-6);
    assert!((tr.wilson_95_ci[1] - t_high).abs() < 1e-6);
    assert_eq!(
        tr.verdict,
        expected_verdict(tr.k, tr.n, tr.required_n, tr.precision_floor, t_low)
    );

    let structural = &summary.class_results[0];
    let obs_mdd =
        minimum_detectable_difference(structural.n, tr.n, 0.55, summary.confidence_level, 0.80)
            .unwrap();
    assert!((tr.observed_mdd_vs_structural - obs_mdd).abs() < 1e-6);
    assert!(
        (tr.observed_precision_delta_structural_vs_twin
            - (structural.point_estimate - tr.point_estimate))
            .abs()
            < 1e-6
    );

    // Verify per-gate Wilson intervals and verdicts
    for gr in &summary.per_gate_results {
        assert!(
            discipline::config::gate_info(&gr.gate).is_some(),
            "unknown gate {} in summary.json",
            gr.gate
        );
        assert_eq!(gr.k, gr.true_positives);
        assert_eq!(
            gr.n,
            gr.true_positives + gr.false_positives + gr.cannot_tell
        );
        if gr.n == 0 {
            assert!(gr.point_estimate.is_none());
            assert!(gr.wilson_95_ci.is_none());
            assert_eq!(gr.verdict, "insufficient-n");
        } else {
            let (low, high) = wilson_score_interval(gr.k, gr.n, summary.confidence_level).unwrap();
            let pt = gr
                .point_estimate
                .expect("point_estimate present when n > 0");
            let ci = gr.wilson_95_ci.expect("wilson_95_ci present when n > 0");
            assert!((pt - (gr.k as f64 / gr.n as f64)).abs() < 1e-6);
            assert!(
                (ci[0] - low).abs() < 1e-6,
                "gate {} low mismatch: {} vs {}",
                gr.gate,
                ci[0],
                low
            );
            assert!(
                (ci[1] - high).abs() < 1e-6,
                "gate {} high mismatch: {} vs {}",
                gr.gate,
                ci[1],
                high
            );
            assert_eq!(
                gr.verdict,
                expected_verdict(gr.k, gr.n, gr.required_n, gr.precision_floor, low)
            );
        }
    }
}

// agent-pr-precision-v3, -v4 and -v5: the v2 corpus re-run with discipline 0.25.3, 0.26.0
// and 0.27.0, labels inherited from the previous study where a finding is unchanged
// (docs/benchmarks/precision-preregistration-v3.toml, -v4.toml, -v5.toml). -v6 re-runs it
// with 0.28.1 and labels every item fresh (-v6.toml).

const V3_PREREG: &str = "docs/benchmarks/precision-preregistration-v3.toml";
const V3_RESULTS: &str = "docs/benchmarks/results-v3";
const V4_PREREG: &str = "docs/benchmarks/precision-preregistration-v4.toml";
const V4_RESULTS: &str = "docs/benchmarks/results-v4";
const V5_PREREG: &str = "docs/benchmarks/precision-preregistration-v5.toml";
const V5_RESULTS: &str = "docs/benchmarks/results-v5";
const V6_PREREG: &str = "docs/benchmarks/precision-preregistration-v6.toml";
const V6_RESULTS: &str = "docs/benchmarks/results-v6";

/// One re-run and the study whose labels it inherits.
struct Rerun {
    prereg: &'static str,
    results: &'static str,
    previous: &'static str,
    previous_results: &'static str,
}

const V3: Rerun = Rerun {
    prereg: V3_PREREG,
    results: V3_RESULTS,
    previous: "v2",
    previous_results: "docs/benchmarks/results",
};

const V4: Rerun = Rerun {
    prereg: V4_PREREG,
    results: V4_RESULTS,
    previous: "v3",
    previous_results: V3_RESULTS,
};

const V5: Rerun = Rerun {
    prereg: V5_PREREG,
    results: V5_RESULTS,
    previous: "v4",
    previous_results: V4_RESULTS,
};

/// v6 inherits no label; `previous` only names the study it is matched against.
const V6: Rerun = Rerun {
    prereg: V6_PREREG,
    results: V6_RESULTS,
    previous: "v5",
    previous_results: V5_RESULTS,
};

fn read_toml(path: &str) -> toml::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path} must exist: {e}"));
    toml::from_str(&raw).unwrap_or_else(|e| panic!("{path} must parse: {e}"))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> T {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path} must exist: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path} must parse: {e}"))
}

#[test]
fn v3_preregistration_keeps_v2_floors_population_and_labels() {
    let v2 = read_toml("docs/benchmarks/precision-preregistration.toml");
    let v3 = read_toml(V3_PREREG);
    let s = &v3["study"];
    assert_eq!(s["id"].as_str(), Some("agent-pr-precision-v3"));
    assert_eq!(s["previous_study_id"].as_str(), v2["study"]["id"].as_str());
    assert_eq!(s["status"].as_str(), Some("locked-pre-data"));
    assert_eq!(s["discipline_version"].as_str(), Some("0.25.3"));
    assert_eq!(s["recall_claimed"].as_bool(), Some(false));
    assert!(s["corpus_prior_inspection"]
        .as_str()
        .is_some_and(|t| t.contains("already") || t.contains("inspected")));

    // Floors, required n, the pinned bounds, the label rubric and the twin are v2's.
    for (key, value) in v2["power_and_bounds"].as_table().unwrap() {
        assert_eq!(
            v3["power_and_bounds"].get(key),
            Some(value),
            "power_and_bounds.{key} differs from v2"
        );
    }
    let floors = |doc: &toml::Value| -> Vec<(String, f64, f64, i64, Vec<String>)> {
        doc["gate_floors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                (
                    g["class"].as_str().unwrap().to_string(),
                    g["precision_floor"].as_float().unwrap(),
                    g["assumed_precision"].as_float().unwrap(),
                    g["required_n"].as_integer().unwrap(),
                    g["gates"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect(),
                )
            })
            .collect()
    };
    assert_eq!(floors(&v3), floors(&v2));
    assert_eq!(v3["twin_baseline"], v2["twin_baseline"]);
    assert_eq!(v3["adjudication"]["labels"], v2["adjudication"]["labels"]);
    for key in [
        "independent_raters",
        "agreement_metric",
        "min_acceptable_kappa",
        "disagreement_resolution_rule",
    ] {
        assert_eq!(
            v3["adjudication"][key], v2["adjudication"][key],
            "adjudication.{key}"
        );
    }

    // The population is v2's manifest, not a re-harvest.
    let p = &v3["population"];
    assert_eq!(p["reharvest"].as_bool(), Some(false));
    assert_eq!(
        p["manifest_path"].as_str(),
        Some("docs/benchmarks/results/manifest.json")
    );
    assert_eq!(p["evaluation_repos"], v2["population"]["evaluation_repos"]);
    let v2_manifest: ManifestDoc = read_json("docs/benchmarks/results/manifest.json");
    assert_eq!(
        p["pull_request_count"].as_integer(),
        Some(v2_manifest.pull_requests.len() as i64)
    );
}

#[test]
fn v4_preregistration_keeps_v3_floors_and_pins_the_v3_manifest() {
    let v2 = read_toml("docs/benchmarks/precision-preregistration.toml");
    let v3 = read_toml(V3_PREREG);
    let v4 = read_toml(V4_PREREG);
    let s = &v4["study"];
    assert_eq!(s["id"].as_str(), Some("agent-pr-precision-v4"));
    assert_eq!(s["previous_study_id"].as_str(), v3["study"]["id"].as_str());
    assert_eq!(s["status"].as_str(), Some("locked-pre-data"));
    assert_eq!(s["discipline_version"].as_str(), Some("0.26.0"));
    assert_eq!(s["recall_claimed"].as_bool(), Some(false));
    assert!(s["corpus_prior_inspection"].as_str().is_some_and(|t| t
        .contains("agent-pr-precision-v2")
        && t.contains("agent-pr-precision-v3")));

    // Floors, required n, pinned bounds, the label rubric and the twin are v3's (so v2's).
    assert_eq!(v4["power_and_bounds"], v3["power_and_bounds"]);
    for (key, value) in v2["power_and_bounds"].as_table().unwrap() {
        assert_eq!(
            v4["power_and_bounds"].get(key),
            Some(value),
            "power_and_bounds.{key}"
        );
    }
    let floors = |doc: &toml::Value| -> Vec<toml::Value> {
        doc["gate_floors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                let mut g = g.as_table().unwrap().clone();
                // Prose only: the rationale and the registry note name the versions.
                g.remove("rationale");
                g.remove("unlisted_gate_rule");
                toml::Value::Table(g)
            })
            .collect()
    };
    assert_eq!(floors(&v4), floors(&v3));
    assert_eq!(v4["twin_baseline"], v3["twin_baseline"]);
    assert_eq!(v4["adjudication"]["labels"], v3["adjudication"]["labels"]);
    for key in [
        "independent_raters",
        "agreement_metric",
        "min_acceptable_kappa",
        "blind_to_source_tool",
        "blind_to_gate_name",
        "blind_to_previous_labels",
        "anchor_items",
        "kappa_scope",
        "disagreement_resolution_rule",
    ] {
        assert_eq!(
            v4["adjudication"][key], v3["adjudication"][key],
            "adjudication.{key}"
        );
    }
    assert_eq!(
        v4["adjudication"]["committed_rater_packet"].as_str(),
        Some("docs/benchmarks/results-v4/rater-packet.json")
    );

    // The population is v3's manifest, pinned by hash, not a re-harvest.
    let p = &v4["population"];
    assert_eq!(p["reharvest"].as_bool(), Some(false));
    assert_eq!(
        p["manifest_path"].as_str(),
        Some("docs/benchmarks/results-v3/manifest.json")
    );
    assert_eq!(p["evaluation_repos"], v3["population"]["evaluation_repos"]);
    let raw = std::fs::read("docs/benchmarks/results-v3/manifest.json").unwrap();
    assert_eq!(
        p["manifest_sha256"].as_str(),
        Some(discipline::report::gitlab::sha256_hex(&raw).as_str()),
        "results-v3/manifest.json changed after the v4 pre-registration"
    );
    let v3_manifest: ManifestDoc = read_json("docs/benchmarks/results-v3/manifest.json");
    assert_eq!(
        p["pull_request_count"].as_integer(),
        Some(v3_manifest.pull_requests.len() as i64)
    );

    // The label-reuse key adds the message key, written before the data.
    let reuse = &v4["label_reuse"];
    assert_eq!(
        reuse["identity_key"],
        toml::Value::Array(
            [
                "repo",
                "head_sha",
                "gate",
                "code",
                "file",
                "line",
                "message_key"
            ]
            .map(|k| toml::Value::String(k.into()))
            .to_vec()
        )
    );
    assert!(reuse["message_key"]
        .as_str()
        .unwrap()
        .contains("commit:<replay-commit>"));
}

#[test]
fn v5_preregistration_keeps_v4_floors_and_pins_the_v4_manifest() {
    let v4 = read_toml(V4_PREREG);
    let v5 = read_toml(V5_PREREG);
    let s = &v5["study"];
    assert_eq!(s["id"].as_str(), Some("agent-pr-precision-v5"));
    assert_eq!(s["previous_study_id"].as_str(), v4["study"]["id"].as_str());
    assert_eq!(s["status"].as_str(), Some("locked-pre-data"));
    assert_eq!(s["discipline_version"].as_str(), Some("0.27.0"));
    assert_eq!(s["recall_claimed"].as_bool(), Some(false));
    assert!(s["corpus_prior_inspection"].as_str().is_some_and(|t| t
        .contains("agent-pr-precision-v3")
        && t.contains("agent-pr-precision-v4")));

    // Floors, required n, pinned bounds, the label rubric, the twin and the reuse key are v4's.
    assert_eq!(v5["power_and_bounds"], v4["power_and_bounds"]);
    let floors = |doc: &toml::Value| -> Vec<toml::Value> {
        doc["gate_floors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                let mut g = g.as_table().unwrap().clone();
                // Prose only: the rationale and the registry note name the versions.
                g.remove("rationale");
                g.remove("unlisted_gate_rule");
                toml::Value::Table(g)
            })
            .collect()
    };
    assert_eq!(floors(&v5), floors(&v4));
    assert_eq!(v5["twin_baseline"], v4["twin_baseline"]);
    assert_eq!(v5["adjudication"]["labels"], v4["adjudication"]["labels"]);
    for key in [
        "independent_raters",
        "agreement_metric",
        "min_acceptable_kappa",
        "blind_to_source_tool",
        "blind_to_gate_name",
        "blind_to_previous_labels",
        "packet_scope",
        "anchor_items",
        "kappa_scope",
        "disagreement_resolution_rule",
    ] {
        assert_eq!(
            v5["adjudication"][key], v4["adjudication"][key],
            "adjudication.{key}"
        );
    }
    assert_eq!(
        v5["label_reuse"]["identity_key"],
        v4["label_reuse"]["identity_key"]
    );
    assert_eq!(
        v5["adjudication"]["committed_rater_packet"].as_str(),
        Some("docs/benchmarks/results-v5/rater-packet.json")
    );
    // The degeneracy rule replaces v4's report-only diagnostic.
    assert!(v5["adjudication"].get("rater_label_diagnostic").is_none());
    let rule = v5["adjudication"]["rater_degeneracy_rule"]
        .as_str()
        .unwrap();
    assert!(rule.contains("failed-degenerate-raters") && rule.contains("replaced once"));

    // The population is v4's manifest, pinned by hash, not a re-harvest.
    let p = &v5["population"];
    assert_eq!(p["reharvest"].as_bool(), Some(false));
    assert_eq!(
        p["manifest_path"].as_str(),
        Some("docs/benchmarks/results-v4/manifest.json")
    );
    assert_eq!(p["evaluation_repos"], v4["population"]["evaluation_repos"]);
    let raw = std::fs::read("docs/benchmarks/results-v4/manifest.json").unwrap();
    assert_eq!(
        p["manifest_sha256"].as_str(),
        Some(discipline::report::gitlab::sha256_hex(&raw).as_str()),
        "results-v4/manifest.json changed after the v5 pre-registration"
    );
}

#[test]
fn v6_preregistration_keeps_v5_floors_pins_the_v5_manifest_and_inherits_nothing() {
    let v5 = read_toml(V5_PREREG);
    let v6 = read_toml(V6_PREREG);
    let s = &v6["study"];
    assert_eq!(s["id"].as_str(), Some("agent-pr-precision-v6"));
    assert_eq!(s["previous_study_id"].as_str(), v5["study"]["id"].as_str());
    assert_eq!(s["status"].as_str(), Some("locked-pre-data"));
    assert_eq!(s["discipline_version"].as_str(), Some("0.28.1"));
    assert_eq!(s["recall_claimed"].as_bool(), Some(false));
    assert!(s["corpus_prior_inspection"]
        .as_str()
        .is_some_and(|t| t.contains("agent-pr-precision-v2") && t.contains("`-v5`")));

    // Floors, required n, pinned bounds, the label rubric and the twin are v5's.
    assert_eq!(v6["power_and_bounds"], v5["power_and_bounds"]);
    let floors = |doc: &toml::Value| -> Vec<toml::Value> {
        doc["gate_floors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                let mut g = g.as_table().unwrap().clone();
                g.remove("rationale");
                g.remove("unlisted_gate_rule");
                toml::Value::Table(g)
            })
            .collect()
    };
    assert_eq!(floors(&v6), floors(&v5));
    assert_eq!(v6["twin_baseline"], v5["twin_baseline"]);
    assert_eq!(v6["adjudication"]["labels"], v5["adjudication"]["labels"]);
    for key in [
        "independent_raters",
        "agreement_metric",
        "min_acceptable_kappa",
        "blind_to_source_tool",
        "blind_to_gate_name",
        "blind_to_previous_labels",
        "anchor_items",
        "disagreement_resolution_rule",
    ] {
        assert_eq!(
            v6["adjudication"][key], v5["adjudication"][key],
            "adjudication.{key}"
        );
    }
    // Nothing is inherited: [label_reuse] is replaced by a record-only [v5_match].
    assert!(v6.get("label_reuse").is_none());
    assert_eq!(
        v6["v5_match"]["identity_key"],
        v5["label_reuse"]["identity_key"]
    );
    assert!(v6["v5_match"]["rule"]
        .as_str()
        .unwrap()
        .contains("No v6 label is inherited"));
    let adj = &v6["adjudication"];
    assert_eq!(
        adj["committed_rater_packet"].as_str(),
        Some("docs/benchmarks/results-v6/rater-packet.json")
    );
    assert_ne!(
        adj["anchor_sample_seed"],
        v5["adjudication"]["anchor_sample_seed"]
    );
    assert_ne!(
        adj["packet_shuffle_seed"],
        v5["adjudication"]["packet_shuffle_seed"]
    );
    assert_ne!(
        adj["rater_models"]["rater_1"],
        adj["rater_models"]["rater_2"]
    );
    assert!(adj["rater_prompt"]
        .as_str()
        .unwrap()
        .contains("{BATCH_FILE}"));
    let rule = adj["rater_degeneracy_rule"].as_str().unwrap();
    assert!(rule.contains("failed-degenerate-raters") && rule.contains("replaced once"));

    // The in-sample disclosure names the three pull requests holding the fixed v5 items.
    let disclosure = &v6["in_sample_disclosure"];
    assert_eq!(
        disclosure["in_sample_pull_requests"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    for id in [
        "ITEM-0015",
        "ITEM-0016",
        "ITEM-0044",
        "ITEM-0126",
        "ITEM-0127",
    ] {
        assert!(disclosure["fixed_items"].as_str().unwrap().contains(id));
    }

    // The population is v5's manifest, pinned by hash, not a re-harvest.
    let p = &v6["population"];
    assert_eq!(p["reharvest"].as_bool(), Some(false));
    assert_eq!(
        p["manifest_path"].as_str(),
        Some("docs/benchmarks/results-v5/manifest.json")
    );
    assert_eq!(p["evaluation_repos"], v5["population"]["evaluation_repos"]);
    let raw = std::fs::read("docs/benchmarks/results-v5/manifest.json").unwrap();
    assert_eq!(
        p["manifest_sha256"].as_str(),
        Some(discipline::report::gitlab::sha256_hex(&raw).as_str()),
        "results-v5/manifest.json changed after the v6 pre-registration"
    );
}

#[derive(serde::Deserialize)]
struct RerunAdjudicationDoc {
    study_id: String,
    cohens_kappa: Option<f64>,
    cohens_kappa_scope: String,
    inherited_label_count: usize,
    fresh_label_count: usize,
    fresh_exact_agreement_count: usize,
    total_items: usize,
    committed_rater_packet: String,
    synthetic_rater_disclosure: String,
    rater_1_kind: String,
    adjudications: Vec<RerunAdjudicationItem>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RerunAdjudicationItem {
    item_id: String,
    label_source: String,
    rater_1_label: String,
    rater_2_label: String,
    agreed: bool,
    tie_breaker_label: Option<String>,
    final_label: String,
    #[serde(default)]
    v2_item_id: Option<String>,
    #[serde(default)]
    v3_item_id: Option<String>,
    #[serde(default)]
    v4_item_id: Option<String>,
    #[serde(default)]
    packet_item_id: Option<String>,
    /// v5 onward: set when the rater-degeneracy rule made a fresh final label `cannot-tell`.
    #[serde(default)]
    final_label_rule: Option<String>,
}

impl RerunAdjudicationItem {
    fn previous_item_id(&self, previous: &str) -> Option<&str> {
        match previous {
            "v2" => self.v2_item_id.as_deref(),
            "v3" => self.v3_item_id.as_deref(),
            "v4" => self.v4_item_id.as_deref(),
            _ => None,
        }
    }
}

/// Checks a re-run's artifacts against their own labels, the previous study's labels they
/// inherit and the Rust bounds. Returns the first inconsistency.
fn check_rerun_results(
    study: &Rerun,
    summary: &SummaryDoc,
    adj: &RerunAdjudicationDoc,
    findings: &FindingsDoc,
) -> Result<(), String> {
    let prev = study.previous;
    // Only the labels: the previous study's kappa can be null (v5), which a v6 check must accept.
    #[derive(serde::Deserialize)]
    struct PreviousLabels {
        adjudications: Vec<AdjudicationItem>,
    }
    let prev_adj: PreviousLabels =
        read_json(&format!("{}/adjudication.json", study.previous_results));
    let prev_by_id: std::collections::BTreeMap<&str, &AdjudicationItem> = prev_adj
        .adjudications
        .iter()
        .map(|a| (a.item_id.as_str(), a))
        .collect();
    let packet: RaterPacketDoc = read_json(&format!("{}/rater-packet.json", study.results));
    let packet_ids: std::collections::BTreeSet<&str> =
        packet.items.iter().map(|i| i.item_id.as_str()).collect();

    let mut inherited = 0;
    let mut fresh_pairs = Vec::new();
    for a in &adj.adjudications {
        if a.agreed != (a.rater_1_label == a.rater_2_label) {
            return Err(format!(
                "{}: agreed flag disagrees with the labels",
                a.item_id
            ));
        }
        let expected_final = match a.final_label_rule.as_deref() {
            Some("rater-degeneracy-cannot-tell") if a.label_source == "fresh" => {
                Some("cannot-tell")
            }
            Some(other) => return Err(format!("{}: final_label_rule {other}", a.item_id)),
            None if a.agreed => Some(a.rater_1_label.as_str()),
            None => a.tie_breaker_label.as_deref(),
        };
        if expected_final != Some(a.final_label.as_str()) {
            return Err(format!(
                "{}: final label does not follow the rule",
                a.item_id
            ));
        }
        match a.label_source.as_str() {
            s if s == format!("inherited-{prev}") => {
                inherited += 1;
                let id = a
                    .previous_item_id(prev)
                    .ok_or_else(|| format!("inherited item without {prev}_item_id"))?;
                let p = prev_by_id
                    .get(id)
                    .ok_or_else(|| format!("unknown {prev} item {id}"))?;
                if (&p.rater_1_label, &p.rater_2_label, &p.final_label)
                    != (&a.rater_1_label, &a.rater_2_label, &a.final_label)
                    || p.tie_breaker_label != a.tie_breaker_label
                {
                    return Err(format!("{}: labels differ from {prev} {id}", a.item_id));
                }
            }
            "fresh" => {
                let id = a
                    .packet_item_id
                    .as_deref()
                    .ok_or("fresh item without packet_item_id")?;
                if !packet_ids.contains(id) {
                    return Err(format!(
                        "{}: packet item {id} is not in the packet",
                        a.item_id
                    ));
                }
                fresh_pairs.push((a.rater_1_label.as_str(), a.rater_2_label.as_str()));
            }
            other => return Err(format!("{}: unknown label_source {other}", a.item_id)),
        }
    }
    if (inherited, fresh_pairs.len()) != (adj.inherited_label_count, adj.fresh_label_count) {
        return Err(format!(
            "labels give {inherited} inherited / {} fresh, adjudication.json says {} / {}",
            fresh_pairs.len(),
            adj.inherited_label_count,
            adj.fresh_label_count
        ));
    }
    let fresh_agreed = fresh_pairs.iter().filter(|(x, y)| x == y).count();
    if fresh_agreed != adj.fresh_exact_agreement_count {
        return Err("fresh agreement count differs from the labels".into());
    }
    let kappa = if fresh_pairs.is_empty() {
        // No fresh items: kappa is undefined and reported as null.
        None
    } else if fresh_pairs.iter().all(|(x, y)| x == y)
        && fresh_pairs.iter().all(|(x, _)| *x == fresh_pairs[0].0)
    {
        // One category for both raters: kappa is defined as 1 on full agreement.
        Some(1.0)
    } else {
        Some(cohens_kappa(&fresh_pairs))
    };
    for (name, reported) in [
        ("summary.json", summary.cohens_kappa),
        ("adjudication.json", adj.cohens_kappa),
    ] {
        let same = match (reported, kappa) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-6,
            (a, b) => a.is_none() && b.is_none(),
        };
        if !same {
            return Err(format!(
                "{name} kappa {reported:?} vs {kappa:?} from the fresh labels"
            ));
        }
    }

    // Tallies per gate, class and twin follow from the final labels.
    let prereg = read_toml(study.prereg);
    let gate_class: std::collections::BTreeMap<String, String> = prereg["gate_floors"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| {
            let class = g["class"].as_str().unwrap().to_string();
            g["gates"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |x| (x.as_str().unwrap().to_string(), class.clone()))
        })
        .collect();
    let as_v2: Vec<AdjudicationItem> = adj
        .adjudications
        .iter()
        .map(|a| AdjudicationItem {
            item_id: a.item_id.clone(),
            rater_1_label: a.rater_1_label.clone(),
            rater_2_label: a.rater_2_label.clone(),
            agreed: a.agreed,
            tie_breaker_label: a.tie_breaker_label.clone(),
            final_label: a.final_label.clone(),
        })
        .collect();
    let r = recompute_from_labels(&findings.items, &as_v2, &gate_class)?;
    for cr in &summary.class_results {
        let got = r.per_class.get(&cr.class).copied().unwrap_or_default();
        tally_matches(
            &cr.class,
            got,
            cr.true_positives,
            cr.false_positives,
            cr.cannot_tell,
        )?;
        check_interval_and_verdict(
            &cr.class,
            (cr.k, cr.n, cr.required_n, cr.precision_floor),
            (cr.point_estimate, cr.wilson_95_ci, &cr.verdict),
        )?;
    }
    for gr in &summary.per_gate_results {
        let got = r.per_gate.get(&gr.gate).copied().unwrap_or_default();
        tally_matches(
            &gr.gate,
            got,
            gr.true_positives,
            gr.false_positives,
            gr.cannot_tell,
        )?;
        if gr.n > 0 {
            check_interval_and_verdict(
                &gr.gate,
                (gr.k, gr.n, gr.required_n, gr.precision_floor),
                (
                    gr.point_estimate.ok_or("missing point estimate")?,
                    gr.wilson_95_ci.ok_or("missing interval")?,
                    &gr.verdict,
                ),
            )?;
        }
    }
    let tr = &summary.twin_baseline_result;
    tally_matches(
        "twin",
        r.twin,
        tr.true_positives,
        tr.false_positives,
        tr.cannot_tell,
    )?;
    check_interval_and_verdict(
        "twin",
        (tr.k, tr.n, tr.required_n, tr.precision_floor),
        (tr.point_estimate, tr.wilson_95_ci, &tr.verdict),
    )?;
    let br = &summary.pull_request_block_rates;
    for (name, arm) in [
        ("discipline", &br.discipline),
        ("twin", &br.naive_sensitive_path_filter),
    ] {
        let (low, high) = wilson_score_interval(arm.blocked, arm.n, 0.95).unwrap();
        if (arm.wilson_95_ci[0] - low).abs() >= 1e-6 || (arm.wilson_95_ci[1] - high).abs() >= 1e-6 {
            return Err(format!(
                "{name} block-rate interval differs from the bounds"
            ));
        }
    }
    Ok(())
}

/// `(k, n, required_n, floor)` as measured and locked; `(point, ci, verdict)` as reported.
fn check_interval_and_verdict(
    what: &str,
    (k, n, required_n, floor): (usize, usize, usize, f64),
    (point, ci, verdict): (f64, [f64; 2], &str),
) -> Result<(), String> {
    let (low, high) = wilson_score_interval(k, n, 0.95).map_err(|e| format!("{what}: {e:?}"))?;
    if (point - k as f64 / n as f64).abs() >= 1e-6
        || (ci[0] - low).abs() >= 1e-6
        || (ci[1] - high).abs() >= 1e-6
    {
        return Err(format!(
            "{what}: point estimate or interval differs from {k}/{n}"
        ));
    }
    let want = expected_verdict(k, n, required_n, floor, low);
    if verdict != want {
        return Err(format!(
            "{what}: verdict {verdict}, the locked rule gives {want}"
        ));
    }
    Ok(())
}

/// The identity, rater and population checks of a re-run after v3, then the label cross-check.
fn check_rerun_artifacts(study: &Rerun, study_id: &str, version: &str) {
    let summary: SummaryDoc = read_json(&format!("{}/summary.json", study.results));
    let adj: RerunAdjudicationDoc = read_json(&format!("{}/adjudication.json", study.results));
    let findings: FindingsDoc = read_json(&format!("{}/findings.json", study.results));
    let manifest: ManifestDoc = read_json(&format!("{}/manifest.json", study.results));
    let prev_manifest: ManifestDoc =
        read_json(&format!("{}/manifest.json", study.previous_results));

    assert_eq!(summary.study_id, study_id);
    assert_eq!(manifest.study_id, study_id);
    assert_eq!(adj.study_id, study_id);
    assert_eq!(summary.discipline_version, version);
    assert_eq!(manifest.discipline_version, version);
    assert_eq!(summary.evaluated_commit, manifest.evaluated_commit);
    assert_eq!(summary.evaluated_commit.len(), 40);
    assert_eq!(adj.cohens_kappa_scope, "fresh-items");
    assert_eq!(adj.total_items, adj.adjudications.len());
    assert_eq!(
        adj.committed_rater_packet,
        format!("{}/rater-packet.json", study.results)
    );
    assert_eq!(adj.rater_1_kind, "llm-subagent");
    assert_eq!(summary.rater_kind, "llm-subagent");
    assert!(adj.synthetic_rater_disclosure.contains("not human"));

    // Exactly the previous study's pairs, in its order.
    let pairs = |m: &ManifestDoc| -> Vec<(String, String, String, u64)> {
        m.pull_requests
            .iter()
            .map(|p| {
                (
                    p.repo.clone(),
                    p.base_sha.clone(),
                    p.head_sha.clone(),
                    p.pr_number,
                )
            })
            .collect()
    };
    assert_eq!(pairs(&manifest), pairs(&prev_manifest));
    assert_eq!(
        manifest.total_pull_requests,
        prev_manifest.total_pull_requests
    );
    assert_eq!(
        manifest.discipline_blocked_pull_requests,
        summary.pull_request_block_rates.discipline.blocked
    );

    check_rerun_results(study, &summary, &adj, &findings).unwrap();
}

#[test]
fn v3_results_follow_from_labels_and_inherit_v2_labels() {
    let summary: SummaryDoc = read_json(&format!("{V3_RESULTS}/summary.json"));
    let adj: RerunAdjudicationDoc = read_json(&format!("{V3_RESULTS}/adjudication.json"));
    let findings: FindingsDoc = read_json(&format!("{V3_RESULTS}/findings.json"));
    let manifest: ManifestDoc = read_json(&format!("{V3_RESULTS}/manifest.json"));
    let v2_manifest: ManifestDoc = read_json("docs/benchmarks/results/manifest.json");

    assert_eq!(summary.study_id, "agent-pr-precision-v3");
    assert_eq!(manifest.study_id, "agent-pr-precision-v3");
    assert_eq!(adj.study_id, "agent-pr-precision-v3");
    assert_eq!(summary.discipline_version, "0.25.3");
    assert_eq!(manifest.discipline_version, "0.25.3");
    assert_eq!(summary.evaluated_commit, manifest.evaluated_commit);
    assert_eq!(summary.evaluated_commit.len(), 40);
    assert_eq!(adj.cohens_kappa_scope, "fresh-items");
    assert_eq!(adj.total_items, adj.adjudications.len());
    assert_eq!(
        adj.committed_rater_packet,
        format!("{V3_RESULTS}/rater-packet.json")
    );
    assert_eq!(adj.rater_1_kind, "llm-subagent");
    assert_eq!(summary.rater_kind, "llm-subagent");
    assert!(adj.synthetic_rater_disclosure.contains("not human"));

    // Exactly v2's pairs, in v2's order.
    let pairs = |m: &ManifestDoc| -> Vec<(String, String, String, u64)> {
        m.pull_requests
            .iter()
            .map(|p| {
                (
                    p.repo.clone(),
                    p.base_sha.clone(),
                    p.head_sha.clone(),
                    p.pr_number,
                )
            })
            .collect()
    };
    assert_eq!(pairs(&manifest), pairs(&v2_manifest));
    assert_eq!(
        manifest.total_pull_requests,
        v2_manifest.total_pull_requests
    );
    assert_eq!(
        manifest.discipline_blocked_pull_requests,
        summary.pull_request_block_rates.discipline.blocked
    );

    check_rerun_results(&V3, &summary, &adj, &findings).unwrap();
}

#[test]
fn v4_results_follow_from_labels_and_inherit_v3_labels() {
    check_rerun_artifacts(&V4, "agent-pr-precision-v4", "0.26.0");
}

#[test]
fn v5_results_follow_from_labels_and_inherit_v4_labels() {
    check_rerun_artifacts(&V5, "agent-pr-precision-v5", "0.27.0");
}

/// Three single-label corruptions that the cross-check must each reject (v4 onward).
fn flipping_one_label_breaks_the_cross_check(study: &Rerun) {
    let summary: SummaryDoc = read_json(&format!("{}/summary.json", study.results));
    let findings: FindingsDoc = read_json(&format!("{}/findings.json", study.results));
    let fresh =
        || -> RerunAdjudicationDoc { read_json(&format!("{}/adjudication.json", study.results)) };
    let inherited = format!("inherited-{}", study.previous);

    // An inherited final label that no longer matches the previous study.
    let mut adj = fresh();
    let item = adj
        .adjudications
        .iter_mut()
        .find(|a| a.label_source == inherited && a.agreed)
        .expect("an agreed inherited item");
    let flipped = if item.final_label == "true-positive" {
        "false-positive"
    } else {
        "true-positive"
    };
    item.rater_1_label = flipped.into();
    item.rater_2_label = flipped.into();
    item.final_label = flipped.into();
    assert!(check_rerun_results(study, &summary, &adj, &findings)
        .unwrap_err()
        .contains(&format!("differ from {}", study.previous)));

    // A final label that does not follow the agreement rule.
    let mut adj = fresh();
    adj.adjudications[0].final_label = "cannot-tell".into();
    if adj.adjudications[0].rater_1_label == "cannot-tell" {
        adj.adjudications[0].final_label = "true-positive".into();
    }
    assert!(check_rerun_results(study, &summary, &adj, &findings)
        .unwrap_err()
        .contains("does not follow"));

    // An inherited item relabelled fresh moves the inherited / fresh counts.
    let mut adj = fresh();
    let item = adj
        .adjudications
        .iter_mut()
        .find(|a| a.label_source == inherited)
        .unwrap();
    item.label_source = "fresh".into();
    item.packet_item_id = Some("PKT-0001".into());
    assert!(check_rerun_results(study, &summary, &adj, &findings)
        .unwrap_err()
        .contains("inherited"));
}

#[test]
fn flipping_one_v3_label_breaks_the_v3_cross_check() {
    let summary: SummaryDoc = read_json(&format!("{V3_RESULTS}/summary.json"));
    let findings: FindingsDoc = read_json(&format!("{V3_RESULTS}/findings.json"));
    let fresh =
        || -> RerunAdjudicationDoc { read_json(&format!("{V3_RESULTS}/adjudication.json")) };

    // An inherited final label that no longer matches v2.
    let mut adj = fresh();
    let item = adj
        .adjudications
        .iter_mut()
        .find(|a| a.label_source == "inherited-v2" && a.agreed)
        .expect("an agreed inherited item");
    let flipped = if item.final_label == "true-positive" {
        "false-positive"
    } else {
        "true-positive"
    };
    item.rater_1_label = flipped.into();
    item.rater_2_label = flipped.into();
    item.final_label = flipped.into();
    assert!(check_rerun_results(&V3, &summary, &adj, &findings)
        .unwrap_err()
        .contains("differ from v2"));

    // A final label that does not follow the agreement rule.
    let mut adj = fresh();
    adj.adjudications[0].final_label = "cannot-tell".into();
    if adj.adjudications[0].rater_1_label == "cannot-tell" {
        adj.adjudications[0].final_label = "true-positive".into();
    }
    assert!(check_rerun_results(&V3, &summary, &adj, &findings)
        .unwrap_err()
        .contains("does not follow"));

    // An inherited item relabelled fresh moves the inherited / fresh counts.
    let mut adj = fresh();
    let item = adj
        .adjudications
        .iter_mut()
        .find(|a| a.label_source == "inherited-v2")
        .unwrap();
    item.label_source = "fresh".into();
    item.packet_item_id = Some("PKT-0001".into());
    assert!(check_rerun_results(&V3, &summary, &adj, &findings)
        .unwrap_err()
        .contains("inherited"));
}

#[test]
fn flipping_one_v4_label_breaks_the_v4_cross_check() {
    flipping_one_label_breaks_the_cross_check(&V4);
}

#[test]
fn flipping_one_v5_label_breaks_the_v5_cross_check() {
    flipping_one_label_breaks_the_cross_check(&V5);
}

#[test]
fn v6_results_follow_from_labels_with_every_label_fresh() {
    check_rerun_artifacts(&V6, "agent-pr-precision-v6", "0.28.1");
    let adj: RerunAdjudicationDoc = read_json(&format!("{V6_RESULTS}/adjudication.json"));
    assert_eq!(adj.inherited_label_count, 0);
    assert!(adj
        .adjudications
        .iter()
        .all(|a| a.label_source == "fresh" && a.packet_item_id.is_some()));
}

#[test]
fn flipping_one_v6_label_breaks_the_v6_cross_check() {
    let summary: SummaryDoc = read_json(&format!("{V6_RESULTS}/summary.json"));
    let findings: FindingsDoc = read_json(&format!("{V6_RESULTS}/findings.json"));
    let fresh =
        || -> RerunAdjudicationDoc { read_json(&format!("{V6_RESULTS}/adjudication.json")) };

    // An agreed label flipped on both raters moves a tally away from summary.json.
    let mut adj = fresh();
    let item = adj
        .adjudications
        .iter_mut()
        .find(|a| a.agreed && a.final_label_rule.is_none())
        .expect("an agreed item");
    let flipped = if item.final_label == "true-positive" {
        "false-positive"
    } else {
        "true-positive"
    };
    item.rater_1_label = flipped.into();
    item.rater_2_label = flipped.into();
    item.final_label = flipped.into();
    assert!(check_rerun_results(&V6, &summary, &adj, &findings).is_err());

    // A final label that does not follow the agreement rule.
    let mut adj = fresh();
    adj.adjudications[0].final_label = "cannot-tell".into();
    if adj.adjudications[0].rater_1_label == "cannot-tell" {
        adj.adjudications[0].final_label = "true-positive".into();
    }
    assert!(check_rerun_results(&V6, &summary, &adj, &findings)
        .unwrap_err()
        .contains("does not follow"));

    // A fresh item relabelled inherited moves the inherited / fresh counts.
    let mut adj = fresh();
    adj.adjudications[0].label_source = "inherited-v5".into();
    assert!(check_rerun_results(&V6, &summary, &adj, &findings).is_err());
}

// ---- agent-pr-precision-v7 (stratified AIDev design) -----------------------------------

const V7_PREREG: &str = "docs/benchmarks/precision-preregistration-v7.toml";

#[test]
fn v7_zero_event_upper_bound_is_the_exact_clopper_pearson_bound() {
    // 1 - 0.05^(1/232): v6's 232 pull requests bound a gate that never fired below 1.283%.
    let v6 = zero_event_upper_bound(232, 0.95).unwrap();
    assert!((v6 - 0.012_829_628_664_530_968).abs() < 1e-15, "{v6}");
    // One trial at 95%: the bound is the confidence level itself.
    assert!((zero_event_upper_bound(1, 0.95).unwrap() - 0.95).abs() < 1e-15);
    // The rule of three overstates the exact bound, and the two converge as n grows.
    for n in [30, 232, 1533, 100_000] {
        let exact = zero_event_upper_bound(n, 0.95).unwrap();
        assert!(exact < 3.0 / n as f64, "n = {n}");
        assert!(
            (exact * n as f64 - 20f64.ln()).abs() < 0.05 * 20f64.ln(),
            "n = {n}"
        );
    }
    assert!(zero_event_upper_bound(0, 0.95).is_err());
    assert!(zero_event_upper_bound(10, 1.0).is_err());
    assert!(zero_event_upper_bound(10, f64::NAN).is_err());
}

#[test]
fn v7_poisson_at_least_matches_closed_forms() {
    // P(X >= 1 | mu = 1) = 1 - e^-1; P(X >= 3 | mu = 2) = 1 - 5 e^-2.
    let one = poisson_at_least(1, 1.0).unwrap();
    assert!((one - (1.0 - (-1.0f64).exp())).abs() < 1e-15, "{one}");
    let three = poisson_at_least(3, 2.0).unwrap();
    assert!(
        (three - (1.0 - 5.0 * (-2.0f64).exp())).abs() < 1e-15,
        "{three}"
    );
    assert_eq!(poisson_at_least(0, 7.0).unwrap(), 1.0);
    assert_eq!(poisson_at_least(4, 0.0).unwrap(), 0.0);
    // Large means stay finite where e^-mu alone underflows.
    let big = poisson_at_least(1000, 2000.0).unwrap();
    assert!(big.is_finite() && big > 1.0 - 1e-12, "{big}");
    let small = poisson_at_least(2000, 1000.0).unwrap();
    assert!((0.0..1e-12).contains(&small), "{small}");
    assert!(poisson_at_least(3, -1.0).is_err());
    assert!(poisson_at_least(3, f64::INFINITY).is_err());
}

#[test]
fn v7_pull_requests_for_yield_is_the_smallest_meeting_size() {
    // One unit at 0.1 per pull request: 1 - e^(-0.1 N) >= 0.5 first holds at N = 7 (ln 2 / 0.1 = 6.93).
    assert_eq!(pull_requests_for_yield(1, 0.1, 0.5).unwrap(), 7);
    // One unit at one per pull request: 1 - e^-1 = 0.63 >= 0.5 at N = 1.
    assert_eq!(pull_requests_for_yield(1, 1.0, 0.5).unwrap(), 1);
    // The v7 sizes are the boundary: the size meets the assurance and the one below does not.
    for (units, rate, assurance, expected) in [
        (254, 11.0 / 149.0, 0.80, 3621),
        (307, 29.0 / 149.0, 0.80, 1653),
        (184, 3.0 / 149.0, 0.80, 9701),
        (141, 20.0 / 232.0, 0.50, 1632),
    ] {
        let n = pull_requests_for_yield(units, rate, assurance).unwrap();
        assert_eq!(n, expected, "{units} units at {rate}");
        assert!(poisson_at_least(units, n as f64 * rate).unwrap() >= assurance);
        assert!(poisson_at_least(units, (n - 1) as f64 * rate).unwrap() < assurance);
    }
    // At assurance 0.5 the size sits just under the mean-yield size units / rate.
    let median = pull_requests_for_yield(141, 20.0 / 232.0, 0.5).unwrap();
    assert!(median <= (141.0 * 232.0 / 20.0f64).ceil() as usize);
    assert!(pull_requests_for_yield(0, 0.1, 0.5).is_err());
    assert!(pull_requests_for_yield(10, 0.0, 0.5).is_err());
    assert!(pull_requests_for_yield(10, f64::NAN, 0.5).is_err());
    assert!(pull_requests_for_yield(10, 0.1, 1.0).is_err());
    assert!(pull_requests_for_yield(10, 1e-12, 0.5).is_err());
}

#[test]
fn v7_design_effect_inflated_n_is_kish_and_rounds_up_but_not_on_float_noise() {
    // v6: 29 structural units held by 24 pull requests, worst-case correlation 1.
    assert_eq!(
        design_effect_inflated_n(254, 29.0 / 24.0, 1.0).unwrap(),
        307
    );
    // One unit per cluster, or no correlation: no inflation.
    assert_eq!(design_effect_inflated_n(254, 1.0, 1.0).unwrap(), 254);
    assert_eq!(design_effect_inflated_n(100, 3.0, 0.0).unwrap(), 100);
    // 100 * (1 + 1 * 0.5) = 150 exactly; 0.1-steps in binary may not add a unit.
    assert_eq!(design_effect_inflated_n(100, 2.0, 0.5).unwrap(), 150);
    assert_eq!(design_effect_inflated_n(10, 1.1, 1.0).unwrap(), 11);
    assert!(design_effect_inflated_n(0, 1.0, 1.0).is_err());
    assert!(design_effect_inflated_n(10, 0.9, 1.0).is_err());
    assert!(design_effect_inflated_n(10, 2.0, 1.5).is_err());
    assert!(design_effect_inflated_n(10, 2.0, -0.1).is_err());
    assert!(design_effect_inflated_n(10, f64::NAN, 0.5).is_err());
}

#[test]
fn v7_implied_design_effect_reads_v6_intervals() {
    // v6 on 119/125: Wilson [0.899, 0.978] against the PR-clustered bootstrap [0.841, 0.994].
    let (lo, hi) = wilson_score_interval(119, 125, 0.95).unwrap();
    assert!(
        (lo - 0.899).abs() < 5e-4 && (hi - 0.978).abs() < 5e-4,
        "{lo} {hi}"
    );
    let deff = implied_design_effect(0.994 - 0.841, 0.978 - 0.899).unwrap();
    assert!((deff - 3.750_841_211_344_340_6).abs() < 1e-12, "{deff}");
    assert_eq!(implied_design_effect(0.2, 0.2).unwrap(), 1.0);
    assert!(implied_design_effect(0.0, 0.1).is_err());
    assert!(implied_design_effect(0.1, -0.1).is_err());
    assert!(implied_design_effect(f64::INFINITY, 0.1).is_err());
}

#[test]
fn v7_kappa_sample_size_matches_cohens_standard_error() {
    // Balanced label, kappa 0.80: p_e = 0.5, p_o = 0.9,
    // n = ceil(1.959964^2 * 0.09 / (0.01 * 0.25)) = ceil(138.29) = 139.
    assert_eq!(kappa_sample_size(0.80, 0.5, 0.10, 0.95).unwrap(), 139);
    // Majority label on 80% of items: p_e = 0.68, p_o = 0.936 -> 225.
    assert_eq!(kappa_sample_size(0.80, 0.80, 0.10, 0.95).unwrap(), 225);
    // The requirement grows as the label skews, and shrinks as the half-width widens.
    let skewed = kappa_sample_size(0.80, 0.95, 0.10, 0.95).unwrap();
    assert!(skewed > 3 * 225, "{skewed}");
    assert!(kappa_sample_size(0.80, 0.80, 0.20, 0.95).unwrap() < 225);
    // The size it returns gives a half-width at or under the target; one fewer does not.
    let half_width = |n: usize, kappa: f64, share: f64| -> f64 {
        let z = standard_normal_quantile(0.975).unwrap();
        let p_e = share * share + (1.0 - share) * (1.0 - share);
        let p_o = p_e + kappa * (1.0 - p_e);
        z * (p_o * (1.0 - p_o) / (n as f64 * (1.0 - p_e).powi(2))).sqrt()
    };
    assert!(half_width(225, 0.80, 0.80) <= 0.10);
    assert!(half_width(224, 0.80, 0.80) > 0.10);
    assert!(kappa_sample_size(0.0, 0.8, 0.1, 0.95).is_err());
    assert!(kappa_sample_size(0.8, 0.4, 0.1, 0.95).is_err());
    assert!(kappa_sample_size(0.8, 1.0, 0.1, 0.95).is_err());
    assert!(kappa_sample_size(0.8, 0.8, 0.0, 0.95).is_err());
    assert!(kappa_sample_size(0.8, 0.8, 0.1, f64::NAN).is_err());
}

fn ratio(v: &toml::Value) -> f64 {
    let a = v.as_array().expect("a [numerator, denominator] pair");
    a[0].as_integer().unwrap() as f64 / a[1].as_integer().unwrap() as f64
}

fn width(v: &toml::Value) -> f64 {
    let a = v.as_array().expect("an [upper, lower] pair");
    a[0].as_float().unwrap() - a[1].as_float().unwrap()
}

fn int(v: &toml::Value) -> usize {
    v.as_integer().expect("an integer") as usize
}

fn float(v: &toml::Value) -> f64 {
    v.as_float().expect("a float")
}

/// Every sample size the v7 pre-registration states is what the Rust function returns for
/// its stated inputs.
#[test]
fn v7_preregistration_sample_sizes_match_rust_bounds() {
    let v7 = read_toml(V7_PREREG);
    let s = &v7["study"];
    assert_eq!(s["id"].as_str(), Some("agent-pr-precision-v7"));
    assert_eq!(
        s["previous_study_id"].as_str(),
        Some("agent-pr-precision-v6")
    );
    assert_eq!(s["status"].as_str(), Some("locked-pre-data"));
    assert_eq!(s["recall_claimed"].as_bool(), Some(false));

    let pb = &v7["power_and_bounds"];
    let conf = float(&pb["confidence_level"]);
    let power = float(&pb["statistical_power"]);
    let n_max = int(&pb["wilson_n_max"]);
    let (sp, sf) = (
        float(&pb["structural_assumed_precision"]),
        float(&pb["structural_floor"]),
    );
    let (hp, hf) = (
        float(&pb["heuristic_assumed_precision"]),
        float(&pb["heuristic_floor"]),
    );
    assert_eq!(
        int(&pb["structural_point_estimate_n"]),
        sample_size_for_wilson_lower_bound(sp, sf, conf, n_max).unwrap()
    );
    let first_clearing = (1..n_max)
        .find(|&m| {
            let k = (sp * m as f64 + 0.5).floor() as usize;
            wilson_score_interval(k, m, conf).unwrap().0 >= sf
        })
        .unwrap();
    assert_eq!(int(&pb["structural_first_clearing_n"]), first_clearing);
    // The non-clearing sizes the rationale names between the first clearing size and the
    // point-estimate requirement.
    let point_estimate = int(&pb["structural_point_estimate_n"]);
    let non_clearing: Vec<usize> = (first_clearing..point_estimate)
        .filter(|&m| {
            let k = (sp * m as f64 + 0.5).floor() as usize;
            wilson_score_interval(k, m, conf).unwrap().0 < sf
        })
        .collect();
    let named: Vec<usize> = (131..=139).chain(151..=152).collect();
    assert_eq!(non_clearing, named);
    assert!(pb["required_n_rationale"]
        .as_str()
        .unwrap()
        .contains("131 to 139 and 151 to 152 do not clear"));
    let per_gate = sample_size_for_wilson_lower_bound_power(sp, sf, conf, power, n_max).unwrap();
    assert_eq!(int(&pb["structural_required_n_per_gate"]), per_gate);
    assert_eq!(
        int(&pb["v6_required_n"]),
        sample_size_for_interval(0.90, 0.05, conf).unwrap()
    );
    let v6_power = wilson_pass_probability(int(&pb["v6_required_n"]), sp, sf, conf).unwrap();
    assert!((float(&pb["v6_required_n_pass_probability"]) - v6_power).abs() < 1e-9);
    let structural_class = design_effect_inflated_n(
        per_gate,
        ratio(&pb["structural_units_per_firing_pull_request"]),
        1.0,
    )
    .unwrap();
    assert_eq!(int(&pb["structural_class_required_n"]), structural_class);
    let test_code = design_effect_inflated_n(
        per_gate,
        ratio(&pb["test_code_units_per_firing_pull_request"]),
        1.0,
    )
    .unwrap();
    assert_eq!(int(&pb["test_code_required_n"]), test_code);
    assert_eq!(
        int(&pb["heuristic_point_estimate_n"]),
        sample_size_for_wilson_lower_bound(hp, hf, conf, n_max).unwrap()
    );
    let heuristic = sample_size_for_wilson_lower_bound_power(hp, hf, conf, power, n_max).unwrap();
    assert_eq!(int(&pb["heuristic_required_n"]), heuristic);
    let heuristic_class = design_effect_inflated_n(
        heuristic,
        ratio(&pb["heuristic_units_per_firing_pull_request"]),
        1.0,
    )
    .unwrap();
    assert_eq!(int(&pb["heuristic_class_required_n"]), heuristic_class);

    // The gate-floor tables repeat the same requirements.
    let floors = v7["gate_floors"].as_array().unwrap();
    assert_eq!(int(&floors[0]["required_n_per_gate"]), per_gate);
    assert_eq!(int(&floors[0]["class_required_n"]), structural_class);
    assert_eq!(float(&floors[0]["precision_floor"]), sf);
    assert_eq!(float(&floors[0]["assumed_precision"]), sp);
    assert_eq!(int(&floors[1]["required_n_per_gate"]), heuristic);
    assert_eq!(float(&floors[1]["assumed_precision"]), hp);
    assert_eq!(int(&floors[1]["class_required_n"]), heuristic_class);
    assert_eq!(float(&floors[1]["precision_floor"]), hf);
    // The class lists are v6's.
    let v6 = read_toml(V6_PREREG);
    for (i, floor) in floors.iter().enumerate() {
        assert_eq!(floor["gates"], v6["gate_floors"][i]["gates"]);
    }

    // Stage 1: the block-rate prefix, the zero-event bounds and the yield sizes.
    let s1 = &v7["stage_1"];
    let prefix = sample_size_for_interval(
        float(&s1["block_rate_planning_proportion"]),
        float(&s1["block_rate_target_half_width"]),
        conf,
    )
    .unwrap();
    assert_eq!(int(&s1["rate_prefix_n"]), prefix);
    let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
    assert!(close(
        float(&s1["zero_event_upper_bound_at_rate_prefix"]),
        zero_event_upper_bound(prefix, conf).unwrap()
    ));
    assert!(close(
        float(&s1["v6_zero_event_upper_bound"]),
        zero_event_upper_bound(int(&s1["v6_pull_requests"]), conf).unwrap()
    ));
    let assurance = float(&s1["yield_assurance"]);
    let test_rate = ratio(&s1["test_code_units_per_pull_request"]);
    assert_eq!(
        int(&s1["test_code_pull_requests_for_yield"]),
        pull_requests_for_yield(test_code, test_rate, assurance).unwrap()
    );
    assert_eq!(
        int(&s1["structural_pull_requests_for_yield"]),
        pull_requests_for_yield(
            structural_class,
            ratio(&s1["structural_units_per_pull_request"]),
            assurance
        )
        .unwrap()
    );
    assert_eq!(
        int(&s1["heuristic_pull_requests_for_yield"]),
        pull_requests_for_yield(
            heuristic_class,
            ratio(&s1["heuristic_units_per_pull_request"]),
            assurance
        )
        .unwrap()
    );
    assert_eq!(
        int(&s1["v6_code_gate_pull_requests_for_141"]),
        pull_requests_for_yield(
            141,
            ratio(&s1["v6_code_gate_findings_per_pull_request"]),
            float(&s1["v6_code_gate_assurance"])
        )
        .unwrap()
    );
    let tc = s1["test_code_units_per_pull_request"].as_array().unwrap();
    let lower = wilson_score_interval(
        tc[0].as_integer().unwrap() as usize,
        tc[1].as_integer().unwrap() as usize,
        conf,
    )
    .unwrap()
    .0;
    assert!(close(
        float(&s1["test_code_rate_wilson_lower_bound"]),
        lower
    ));
    let max_n = pull_requests_for_yield(test_code, lower, assurance).unwrap();
    assert_eq!(int(&s1["max_n"]), max_n);
    // The heuristic class cannot be reached within the cap, as the stopping rule states.
    assert!(int(&s1["heuristic_pull_requests_for_yield"]) > max_n);
    assert!(int(&s1["test_code_pull_requests_for_yield"]) > prefix);

    // Stage 2: the v6 design effect and the targeted candidate cap.
    let s2 = &v7["stage_2"];
    let deff = implied_design_effect(
        width(&s2["v6_bootstrap_width"]),
        width(&s2["v6_wilson_width"]),
    )
    .unwrap();
    assert!(close(float(&s2["v6_implied_design_effect"]), deff));
    assert_eq!(int(&s2["targeted_candidate_cap"]), prefix);

    // Twin: minimum detectable differences against the class and gate requirements.
    let twin = &v7["twin_baseline"];
    let flags = int(&twin["sampled_flags"]);
    let p_twin = float(&twin["assumed_baseline_precision"]);
    assert!(close(
        float(&twin["mdd_structural_class_vs_twin"]),
        minimum_detectable_difference(flags, structural_class, p_twin, conf, power).unwrap()
    ));
    assert!(close(
        float(&twin["mdd_structural_gate_vs_twin"]),
        minimum_detectable_difference(flags, per_gate, p_twin, conf, power).unwrap()
    ));
    assert_eq!(
        twin["rule"].as_str().map(|r| r.contains("v6's rule")),
        Some(true)
    );

    // Human subset for an informative kappa, and the reliability verdict.
    let human = &v7["adjudication"]["human"];
    assert_eq!(
        int(&human["human_subset_n"]),
        kappa_sample_size(
            float(&human["planning_kappa"]),
            float(&human["planning_majority_share"]),
            float(&human["kappa_target_half_width"]),
            conf
        )
        .unwrap()
    );
    assert_eq!(int(&human["raters"]), 2);
    let rel = &v7["reliability"];
    assert_eq!(float(&rel["min_acceptable_kappa"]), 0.75);
    assert_eq!(
        rel["unreliable_labels_verdict"].as_str(),
        Some("INTERMEDIATE_unreliable_labels")
    );

    // Batches below v6's 40; both rubric questions are scored.
    let adj = &v7["adjudication"];
    assert!(int(&adj["batch_size"]) < int(&v6["adjudication"]["batch_size"]));
    assert_ne!(adj["llm_raters"]["rater_1"], adj["llm_raters"]["rater_2"]);
    let prompt = adj["rater_prompt"].as_str().unwrap();
    assert!(prompt.contains("{BATCH_FILE}"));
    assert!(prompt.contains("rule_fidelity") && prompt.contains("actionability"));
}
