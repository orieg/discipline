use discipline::guards::perf::bounds::{
    minimum_detectable_difference, sample_size_for_interval, standard_normal_quantile,
    wilson_score_interval,
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
    status: String,
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
    assert_eq!(doc.study.id, "agent-pr-precision-v1");
    assert_eq!(doc.study.status, "locked-pre-data");
    assert_eq!(doc.study.novelty_tier_claimed, "Characterisation");
    assert!(!doc.study.tier_justification.trim().is_empty());
    assert!(!doc.study.discipline_version.is_empty());
    assert!(!doc.study.configuration_mode.is_empty());
    assert!(!doc.study.primary_unit_of_analysis.is_empty());
    assert!(!doc.study.secondary_unit_of_analysis.is_empty());
    assert!(!doc.study.recall_claimed);

    // 2. Disjoint pilot and evaluation bands, public only
    assert_eq!(doc.population.visibility, "public-only");
    assert!(doc.population.exclude_private_repositories);
    assert!(doc.population.exclude_named_consumer_repositories);
    assert_eq!(doc.population.pull_request_state, "merged-only");
    assert_eq!(doc.population.band_separation, "disjoint-by-repository");
    assert!(!doc.population.pilot_repos.is_empty());
    assert!(!doc.population.evaluation_repos.is_empty());
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
