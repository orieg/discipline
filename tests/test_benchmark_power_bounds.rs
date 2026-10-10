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
}
