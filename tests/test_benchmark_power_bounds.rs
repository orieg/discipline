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

#[derive(serde::Deserialize)]
struct SummaryDoc {
    study_id: String,
    confidence_level: f64,
    cohens_kappa: f64,
    pull_request_block_rates: BlockRatesSection,
    class_results: Vec<ClassResultEntry>,
    twin_baseline_result: TwinResultEntry,
    per_gate_results: Vec<PerGateResultEntry>,
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
        ("summary.json", summary.cohens_kappa),
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
    adj: AdjudicationDoc,
    findings: FindingsDoc,
    gate_class: std::collections::BTreeMap<String, String>,
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
        adj: serde_json::from_str(&read("docs/benchmarks/results/adjudication.json"))
            .expect("adjudication.json must parse cleanly"),
        findings: serde_json::from_str(&read("docs/benchmarks/results/findings.json"))
            .expect("findings.json must parse cleanly"),
        gate_class,
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

    let BenchmarkInputs { summary, adj, .. } = inputs;

    assert_eq!(summary.study_id, "agent-pr-precision-v1");
    assert_eq!(adj.study_id, "agent-pr-precision-v1");
    assert!(adj.blind_to_source_tool);
    assert!(adj.hash_contributor_and_rater_identities);
    assert_eq!(adj.adjudications.len(), adj.total_items);
    assert_eq!(
        adj.exact_agreement_count + adj.disagreement_count,
        adj.total_items
    );
    assert!(adj.cohens_kappa >= adj.min_acceptable_kappa);
    assert!(adj.kappa_passed);
    assert!((summary.cohens_kappa - adj.cohens_kappa).abs() < 1e-6);

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
