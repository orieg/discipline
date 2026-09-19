//! Claim hygiene and provenance tags sentinel (`provenance-tags`).
//!
//! Enforces:
//! - Markdown tables with unit-bearing numbers require a provenance tag:
//!   `(measured: host, commit)`, `(target)`, or `(projected)`.
//! - Mechanism claims (e.g. `memory-latency-bound`, `branch misprediction`)
//!   require hardware counter citations or explicit `hypothesis` / `unmeasured` qualifiers.
//! - Published wall-clock ratios (e.g. `2.9x faster`) require confidence intervals
//!   `[lo, hi]` or provisional qualifiers.
//! - Paired figures (e.g. `11.9 ns vs 108.9 ns`, cross-metric statements)
//!   require shared workload IDs (`(workload: id)`) or documented differentiation markers.

use super::{exempt_filter, Context, GateOutcome, PathFilter};
use crate::config::GateSettings;
use crate::tokens;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

pub const GATE: &str = "provenance-tags";

static UNIT_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b\d+(?:[.,]\d+)?\s*(?:ns|µs|us|ms|ops/s|Mops/s|M ops/s|M/s|B/key|B/k|bytes/key|GB|MB|KiB|MiB)\b|\d+(?:\.\d+)?\s*[×x]\b").unwrap()
});

static PROVENANCE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\((?:measured|target|projected|unverified|retracted|pending)").unwrap()
});

static MECHANISM_TERMS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:memory[- ]latency[- ]bound|latency[- ]bound|bandwidth[- ]bound|cache[- ]miss[- ]bound|miss[- ]bound|branch[- ]misprediction|mispredict(?:s|ed|ion)?[- ]bound|TLB[- ]bound|page[- ]walk[- ]bound|memory[- ]level parallelism|MLP[- ]bound|fill[- ]buffer[- ]bound|MSHR[- ]bound|front[- ]end bound|back[- ]end bound|stall(?:ed|ing)? on (?:L[123]|DRAM|memory))\b").unwrap()
});

static MECHANISM_NEGATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:cannot see|can(?:'|’)?t see|does not (?:see|measure|model|capture)|blind to|ignores?|invisible to|no counter|unable to (?:see|measure))").unwrap()
});

static MECHANISM_EVIDENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:perf stat|perf_counters|point_lookup_counters|hardware counter|counters? (?:on|show|locate|say)|branch-misses|branch_misses|L1-dcache|LLC-load|dTLB|cycle_activity|mem_load_retired|cpu_core/|cpu_atom/|--cache-sim|callgrind.*(?:LL|RAM) |results/baseline_|\bunmeasured\b|\bnot measured\b|\bhypothesis\b|\bunverified\b|\bretracted\b|\bcause unknown\b|\bconjecture\b)").unwrap()
});

static WALLCLOCK_RATIO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b\d+(?:\.\d+)?\s*(?:x\b|×(?:\s|$|[^\w]))").unwrap());

static WALLCLOCK_CONTEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\bns\b|µs|\bus\b|\bms\b|ops/s|Mops|M/s|latency|throughput|faster|slower|speedup|wall.?clock)").unwrap()
});

static DETERMINISTIC_METRIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:instruction|callgrind|\bIr\b|B/key|B/k|bytes/key|byte accounting|symbol|deterministic|inst\b|density|memory|footprint|\bKB/|\bMB/|\bB/|resident|allocat|\bRAM\b|heap)").unwrap()
});

static INTERVAL_EVIDENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\[\s*[-+]?\d+(?:\.\d+)?[%x×]?\s*,\s*[-+]?\d+(?:\.\d+)?[%x×]?\s*\]|\bBCa\b|confidence interval|\bCI\b|bca_bootstrap|results/baseline_|\bno interval\b|\bunsourced\b|\bsuperseded\b|\bretracted\b|\bindicative\b|\bunmeasured\b|\bprovisional\b|pending re-measurement)").unwrap()
});

static PAIRED_VS_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\b\d+(?:\.\d+)?\s*(?:ns|µs|us|ms|s|B/key|B/k|bytes/key|B/docID|bits/docID|B/tok|B/entry|Mops/s|M ops/s|M/s|Minst|M inst|inst|tps|M\b)\b(?:\*\*)?\s*(?:vs\.?|vs|against)\s*(?:\*\*)?\d+(?:\.\d+)?|\b\d+(?:\.\d+)?\b(?:\*\*)?\s*(?:vs\.?|vs|against)\s*(?:\*\*)?\d+(?:\.\d+)?\s*(?:ns|µs|us|ms|s|B/key|B/k|bytes/key|B/docID|bits/docID|B/tok|B/entry|Mops/s|M ops/s|M/s|Minst|M inst|inst|tps|M\b)\b)").unwrap()
});

static INSTRUCTION_METRIC_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\b\d+(?:\.\d+)?\s*[x×]\s*(?:the\s+)?instructions?\b|\b\d+(?:\.\d+)?\s*(?:M\s*inst|Minst|inst|instructions?|instruction-retired|Ir)\b|\b\d+(?:\.\d+)?%\s*(?:fewer|more)?\s*instructions?\b)").unwrap()
});

static TIME_METRIC_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\b\d+(?:\.\d+)?\s*(?:ns|µs|us|ms|s)\b|\b\d+(?:\.\d+)?\s*[x×]\s*(?:faster|slower|speedup)?\s*(?:in\s+)?wall[- ]?clock\b|wall[- ]?clock(?:\s+latency)?\s*(?:of\s*)?\d+(?:\.\d+)?\s*(?:ns|µs|us|ms|s|x|×)|\b\d+(?:\.\d+)?\s*[x×]\s*(?:faster|slower)\b)").unwrap()
});

static THROUGHPUT_METRIC_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\b\d+(?:\.\d+)?\s*(?:Mops/s|M ops/s|Mops|tps|M/s|ops/s|items/sec|inserts/sec)\b)",
    )
    .unwrap()
});

static MEMORY_METRIC_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\b\d+(?:\.\d+)?\s*(?:B/key|B/k|bytes/key|B/docID|bits/docID|B/tok|B/entry|B/state)\b|\b\d+(?:\.\d+)?\s*[x×]\s*(?:lower|higher|less|more)?\s*(?:RAM|memory|heap|footprint)\b|\b\d+(?:\.\d+)?\s*(?:MB|MiB|GB|GiB|KB|KiB)\s*(?:RAM|heap|memory|live heap)\b)").unwrap()
});

static WORKLOAD_TAG_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:[\(\[]|;\s*|\b)workload:\s*`?([a-zA-Z0-9_-]+)`?\b").unwrap()
});

static WORKLOAD_DIFF_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:[\(\[]|;\s*|\b)workloads\s+differ:\s*`?([a-zA-Z0-9_-]+)`?\s+vs\s+`?([a-zA-Z0-9_-]+)`?\b").unwrap()
});

static PAIRED_FALLBACK_PAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:different\s+experiment|different\s+workload|not\s+comparable|retracted|superseded|neither\s+half\s+describes|two\s+halves\s+are\s+different|historical\s+record|pre-#\d+|unmeasured|unverified|definitional|no\s+arm\s+on\s+which\s+both\s+were\s+observed|strawman|target|\(target\)|until\s+measured|pending\s+(?:fair-baseline\s+)?re-run)\b").unwrap()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HygieneFinding {
    pub line: usize,
    pub title: &'static str,
    pub message: String,
    pub remediation: &'static str,
    pub is_warning: bool,
}

pub fn strip_fences(lines: &[&str]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut in_harness_audit = false;

    for (idx, &line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }

        if line.contains("<!-- BEGIN HARNESS AUDIT TABLE") {
            in_harness_audit = true;
            continue;
        }
        if line.contains("<!-- END HARNESS AUDIT TABLE") {
            in_harness_audit = false;
            continue;
        }
        if in_harness_audit {
            continue;
        }

        out.push((line_num, line.to_string()));
    }
    out
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();

    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c == '|' {
            if !current.trim().is_empty() {
                sentences.push(std::mem::take(&mut current));
            }
            i += 1;
            continue;
        }
        current.push(c);
        if c == '.' || c == '!' || c == '?' {
            let is_decimal = c == '.'
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && i + 1 < n
                && chars[i + 1].is_ascii_digit();
            if !is_decimal {
                let next_is_space_or_end = i + 1 >= n || chars[i + 1].is_whitespace();
                if next_is_space_or_end && !current.trim().is_empty() {
                    sentences.push(std::mem::take(&mut current));
                }
            }
        }
        i += 1;
    }
    if !current.trim().is_empty() {
        sentences.push(current);
    }
    sentences
}

pub fn scan_markdown_text(
    text: &str,
    path_label: &str,
    check_tables: bool,
    check_mechanisms: bool,
    check_intervals: bool,
    check_paired_figures: bool,
) -> Vec<HygieneFinding> {
    if path_label.ends_with("AGENTS.md")
        || path_label.ends_with("CLAUDE.md")
        || path_label.ends_with("GEMINI.md")
    {
        return Vec::new();
    }

    let raw_lines: Vec<&str> = text.lines().collect();
    let stripped = strip_fences(&raw_lines);
    let mut findings = Vec::new();

    // 1. Table Provenance check
    if check_tables {
        let has_provenance = raw_lines.iter().any(|l| PROVENANCE_TAG.is_match(l));
        if !has_provenance {
            let mut i = 0;
            while i < stripped.len() {
                if stripped[i].1.trim_start().starts_with('|') {
                    let start_line = stripped[i].0;
                    let mut has_unit = false;
                    while i < stripped.len() && stripped[i].1.trim_start().starts_with('|') {
                        if UNIT_TOKEN.is_match(&stripped[i].1) {
                            has_unit = true;
                        }
                        i += 1;
                    }
                    if has_unit {
                        findings.push(HygieneFinding {
                            line: start_line,
                            title: "Unprovenanced Table Numerics",
                            message: "Markdown table contains unit-bearing numbers without a provenance tag (measured: host, commit), (target), or (projected).".to_string(),
                            remediation: "Add a provenance tag to the table caption or header, e.g. *(measured: host, commit)* or *(target)*.",
                            is_warning: true,
                        });
                    }
                } else {
                    i += 1;
                }
            }
        }
    }

    // Group into paragraphs
    let mut paras: Vec<Vec<(usize, String)>> = Vec::new();
    let mut current_para: Vec<(usize, String)> = Vec::new();

    for (num, line) in stripped {
        if line.trim().is_empty() {
            if !current_para.is_empty() {
                paras.push(std::mem::take(&mut current_para));
            }
        } else {
            current_para.push((num, line));
        }
    }
    if !current_para.is_empty() {
        paras.push(current_para);
    }

    for (para_idx, para) in paras.iter().enumerate() {
        let para_text = para
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        // Check for line or paragraph allow markers
        if para_text.contains("discipline:allow(provenance-tags)")
            || para_text.contains("allow-provenance")
            || para_text.contains("allow-unpaired-figures")
            || para_text.contains("docs-lint: allow")
            || para_text.contains("docs-lint:allow")
        {
            continue;
        }

        let next_para_text = if para_idx + 1 < paras.len() {
            paras[para_idx + 1]
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            String::new()
        };
        let window_text = format!("{para_text} {next_para_text}");

        // 2. Mechanism Claims
        if check_mechanisms {
            let has_mech_evidence = MECHANISM_EVIDENCE.is_match(&window_text)
                || MECHANISM_NEGATION.is_match(&window_text);
            if !has_mech_evidence {
                for (line_num, text) in para {
                    if let Some(m) = MECHANISM_TERMS.find(text) {
                        findings.push(HygieneFinding {
                            line: *line_num,
                            title: "Mechanism Claim Without Evidence",
                            message: format!(
                                "Paragraph asserts mechanism `{}` without citing hardware counter evidence or an explicit hypothesis qualifier.",
                                m.as_str()
                            ),
                            remediation: "Cite hardware counters (perf stat, cycle_activity, etc.), an artifact, or mark as hypothesis / unmeasured.",
                            is_warning: false,
                        });
                        break;
                    }
                }
            }
        }

        // 3. Wall-Clock Intervals
        if check_intervals {
            let has_interval_evidence = INTERVAL_EVIDENCE.is_match(&window_text);
            if !has_interval_evidence {
                for (line_num, text) in para {
                    if WALLCLOCK_RATIO.is_match(text)
                        && WALLCLOCK_CONTEXT.is_match(text)
                        && !DETERMINISTIC_METRIC.is_match(text)
                    {
                        findings.push(HygieneFinding {
                            line: *line_num,
                            title: "Bare Wall-Clock Ratio Without Interval",
                            message: "Published wall-clock speedup or slowdown ratio lacks confidence interval [lo, hi] or explicit qualifier.".to_string(),
                            remediation: "Add confidence interval [lo, hi] (e.g. BCa 95% CI) or qualify as provisional / unmeasured / unsourced.",
                            is_warning: false,
                        });
                        break;
                    }
                }
            }
        }

        // 4. Paired Figures
        if check_paired_figures {
            let has_workload_tag = WORKLOAD_TAG_PAT.is_match(&para_text)
                || WORKLOAD_DIFF_PAT.is_match(&para_text)
                || PAIRED_FALLBACK_PAT.is_match(&para_text);

            for (line_num, text) in para {
                if has_workload_tag {
                    continue;
                }

                // Check 4a: Direct "A vs B" comparison
                let mut hit_vs = false;
                if let Some(m) = PAIRED_VS_PAT.find(text) {
                    let start = m.start().saturating_sub(50);
                    let end = (m.end() + 100).min(text.len());
                    let ctx = &text[start..end];
                    if !WORKLOAD_TAG_PAT.is_match(ctx)
                        && !WORKLOAD_DIFF_PAT.is_match(ctx)
                        && !PAIRED_FALLBACK_PAT.is_match(ctx)
                    {
                        findings.push(HygieneFinding {
                            line: *line_num,
                            title: "Paired Figures Without Workload Tag",
                            message: format!(
                                "Paired figures `{}` lack a shared workload tag (workload: id) or differentiation marker.",
                                m.as_str().trim()
                            ),
                            remediation: "Add (workload: id), (workloads differ: a vs b), or an explicit retraction marker.",
                            is_warning: false,
                        });
                        hit_vs = true;
                    }
                }

                if hit_vs {
                    continue;
                }

                // Check 4b: Multi-metric cross-metric pairing in one sentence
                let sentences = split_sentences(text);

                for s in &sentences {
                    if WORKLOAD_TAG_PAT.is_match(s)
                        || WORKLOAD_DIFF_PAT.is_match(s)
                        || PAIRED_FALLBACK_PAT.is_match(s)
                    {
                        continue;
                    }
                    let has_inst = INSTRUCTION_METRIC_PAT.is_match(s);
                    let has_time = TIME_METRIC_PAT.is_match(s);
                    let has_tput = THROUGHPUT_METRIC_PAT.is_match(s);
                    let has_mem = MEMORY_METRIC_PAT.is_match(s);

                    let classes_count = has_inst as usize
                        + has_time as usize
                        + has_tput as usize
                        + has_mem as usize;
                    if classes_count >= 2 {
                        findings.push(HygieneFinding {
                            line: *line_num,
                            title: "Cross-Metric Figures Without Workload Tag",
                            message: "Cross-metric figures in the same sentence lack a shared workload tag (workload: id) or differentiation marker.".to_string(),
                            remediation: "Add (workload: id), (workloads differ: a vs b), or an explicit retraction marker.",
                            is_warning: false,
                        });
                        break;
                    }
                }
            }
        }
    }

    findings
}

pub fn evaluate_provenance_tags(ctx: &Context) -> Result<GateOutcome> {
    let settings = &ctx.config.gates.provenance_tags;
    let mut out = GateOutcome::new(GATE);

    if !settings.enabled() {
        out.enabled = false;
        return Ok(out);
    }

    let exempt = exempt_filter(settings)?;
    let patterns = vec!["**/*.md".to_string(), "**/*.markdown".to_string()];
    let doc_filter = PathFilter::new(&patterns)?;

    // Collect candidate markdown files: changed files if diff-scoped, or tracked files
    let changed_files = ctx.git.changed_files()?;
    let candidate_paths: Vec<String> = changed_files
        .iter()
        .map(|f| f.path.clone())
        .filter(|p| doc_filter.matches(p) && !exempt.matches(p))
        .collect();

    let mut scanned_count = 0;

    for path in &candidate_paths {
        let content = match ctx.git.head_content(path)? {
            Some(c) => c,
            None => continue,
        };

        scanned_count += 1;
        let findings = scan_markdown_text(
            &content,
            path,
            settings.check_tables,
            settings.check_mechanisms,
            settings.check_intervals,
            settings.check_paired_figures,
        );

        for f in findings {
            let override_rec = ctx
                .find_override(GATE, tokens::ALLOW_PROVENANCE, path)
                .or_else(|| {
                    ctx.find_gate_or_subject_override(GATE, tokens::ALLOW_PROVENANCE, path)
                });

            if let Some(ov) = override_rec {
                out.overrides.push(ov);
            } else {
                let severity = if f.is_warning {
                    crate::config::Severity::Warning
                } else {
                    settings.severity()
                };
                out.push(
                    severity,
                    f.title,
                    Some(path),
                    Some(f.line),
                    f.message,
                    f.remediation,
                );
            }
        }
    }

    // Also scan PR body if provided
    if let Some(ref body) = ctx.pr_body {
        scanned_count += 1;
        let findings = scan_markdown_text(
            body,
            "PR body",
            settings.check_tables,
            settings.check_mechanisms,
            settings.check_intervals,
            settings.check_paired_figures,
        );

        for f in findings {
            let severity = if f.is_warning {
                crate::config::Severity::Warning
            } else {
                settings.severity()
            };
            out.push(
                severity,
                f.title,
                Some("PR body"),
                Some(f.line),
                f.message,
                f.remediation,
            );
        }
    }

    out.examined = scanned_count;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_provenance_check() {
        let untagged = "| arm | ns |\n|---|---|\n| a | 35.8 ns |\n";
        let findings = scan_markdown_text(untagged, "t.md", true, false, false, false);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Unprovenanced Table Numerics");

        let tagged = "*(measured: host, commit)*\n| arm | ns |\n|---|---|\n| a | 35.8 ns |\n";
        let findings = scan_markdown_text(tagged, "t.md", true, false, false, false);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_mechanism_claims_check() {
        let claim =
            "The arm is memory-latency-bound, so the work removed is off the critical path.\n";
        let findings = scan_markdown_text(claim, "t.md", false, true, false, false);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Mechanism Claim Without Evidence");

        let evidence = "The arm is memory-latency-bound according to perf stat counters.\n";
        let findings = scan_markdown_text(evidence, "t.md", false, true, false, false);
        assert!(findings.is_empty());

        let hypothesis = "Hypothesis: the arm is memory-latency-bound.\n";
        let findings = scan_markdown_text(hypothesis, "t.md", false, true, false, false);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_wallclock_interval_check() {
        let bare = "Point lookups are 2.9x faster than BTreeMap at 1M keys.\n";
        let findings = scan_markdown_text(bare, "t.md", false, false, true, false);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Bare Wall-Clock Ratio Without Interval");

        let interval = "Random 1M get is 1.031x, BCa 95% CI [1.024, 1.038].\n";
        let findings = scan_markdown_text(interval, "t.md", false, false, true, false);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_paired_figures_check() {
        let bare = "Sequential lookup is 11.9 ns vs 108.9 ns at 1M keys.\n";
        let findings = scan_markdown_text(bare, "t.md", false, false, false, true);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "Paired Figures Without Workload Tag");

        let tagged =
            "Sequential lookup is 11.9 ns vs 108.9 ns at 1M keys (workload: core_compare).\n";
        let findings = scan_markdown_text(tagged, "t.md", false, false, false, true);
        assert!(findings.is_empty());

        let retracted_diff = "Retracted (workloads differ: a vs b): 11.9 ns vs 108.9 ns.\n";
        let findings = scan_markdown_text(retracted_diff, "t.md", false, false, false, true);
        assert!(findings.is_empty());

        let retracted_exp =
            "Retracted: the two halves are different experiments (11.9 ns vs 108.9 ns).\n";
        let findings = scan_markdown_text(retracted_exp, "t.md", false, false, false, true);
        assert!(findings.is_empty());

        let cross_ascii = "libexpanse retires 0.55x the instructions of stock libjudy on random 1M lookup and is 1.11x slower in wall clock.\n";
        let findings = scan_markdown_text(cross_ascii, "t.md", false, false, false, true);
        assert_eq!(findings.len(), 1);

        let cross_unicode = "libexpanse retires 0.55× the instructions of stock libjudy on random 1M lookup and is 1.11× slower in wall clock.\n";
        let findings = scan_markdown_text(cross_unicode, "t.md", false, false, false, true);
        assert_eq!(findings.len(), 1);

        let cross_ram = "Achieves 2.66x lower RAM at 1.9M ops/s.\n";
        let findings = scan_markdown_text(cross_ram, "t.md", false, false, false, true);
        assert_eq!(findings.len(), 1);

        let cross_ram_tagged =
            "Achieves 2.66x lower RAM at 1.9M ops/s (workload: domain_grammar_masks).\n";
        let findings = scan_markdown_text(cross_ram_tagged, "t.md", false, false, false, true);
        assert!(findings.is_empty());
    }
}
